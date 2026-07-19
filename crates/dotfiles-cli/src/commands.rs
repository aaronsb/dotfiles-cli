//! The mutating verbs: `deploy` (filesystem) and `enable`/`disable`/`add`/
//! `remove` (manifest edits via `toml_edit`), plus `push` (git).
//!
//! Manifest writes go through `dotfiles_core::edit` so `why`/`spec`/comments
//! survive untouched; deploy delegates to `dotfiles_core::deploy`.

use crate::Ctx;
use crate::diff_view;
use dotfiles_core::deploy::{DeployOptions, DeployOutcome, deploy_entry};
use dotfiles_core::edit::{self, NewEntry};
use dotfiles_core::{DeployStatus, Manifest, Mode, deploy_status};
use std::path::Path;
use std::process::Command;

/// `deploy` — symlink (or copy) every enabled entry into place.
pub fn deploy(ctx: &Ctx, dry_run: bool, force: bool) -> anyhow::Result<()> {
    let manifest = ctx.load()?;
    let opts = DeployOptions { dry_run, force };
    if dry_run {
        println!("=== Deploying Dotfiles (dry run) ===\n");
    } else {
        println!("=== Deploying Dotfiles ===\n");
    }

    for e in &manifest.entries {
        // Skip entries that don't belong to the active profile.
        if !e.active_in(&ctx.profile) {
            continue;
        }
        let verb = if dry_run { "would" } else { "" };
        match deploy_entry(e, &ctx.repo_root, &ctx.home, opts) {
            DeployOutcome::AlreadyDeployed => println!("{:<22} already deployed", e.name),
            DeployOutcome::Disabled => {} // disabled entries are silently skipped
            DeployOutcome::Deployed { backed_up } => {
                if let Some(p) = backed_up {
                    println!("{:<22} {verb} backed up existing -> {}", e.name, p.display());
                }
                println!("{:<22} {verb} deploy -> ~/{}", e.name, e.target);
            }
            DeployOutcome::Skipped { reason } => println!("{:<22} skipped: {reason}", e.name),
            DeployOutcome::SourceMissing => {
                eprintln!("{:<22} source missing at {}", e.name, e.path);
            }
            DeployOutcome::Error { message } => {
                eprintln!("{:<22} error: {message}", e.name);
            }
        }
    }

    // Orchestrated projection of Claude user-scope settings (ADR-010) — a no-op
    // unless the repo carries a claude/settings.d store. Non-fatal to the deploy.
    if let Err(e) = crate::claude::deploy_step(ctx, dry_run) {
        eprintln!("claude settings: {e}");
    }
    Ok(())
}

/// `enable` / `disable` — flip one entry's flag; disabling also removes a live
/// symlink (mirroring the bash tool).
pub fn set_enabled(ctx: &Ctx, app: &str, enabled: bool) -> anyhow::Result<()> {
    let src = std::fs::read_to_string(&ctx.manifest)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", ctx.manifest.display()))?;
    let mut doc = edit::parse(&src)?;

    if !edit::set_enabled(&mut doc, app, enabled) {
        anyhow::bail!("entry '{app}' not found in the manifest");
    }
    std::fs::write(&ctx.manifest, doc.to_string())?;

    if enabled {
        println!("enabled {app}");
        return Ok(());
    }

    // Disabling: remove the deployed symlink if it points at our source.
    let manifest = Manifest::from_toml(&src)?;
    if let Some(entry) = manifest.entries.iter().find(|e| e.name == app)
        && deploy_status(entry, &ctx.repo_root, &ctx.home) == DeployStatus::Linked
    {
        let target = ctx.home.join(&entry.target);
        std::fs::remove_file(&target)
            .map_err(|e| anyhow::anyhow!("removing {}: {e}", target.display()))?;
        println!("disabled {app} and removed its symlink");
        return Ok(());
    }
    println!("disabled {app}");
    Ok(())
}

/// `add` — append a new entry to the manifest.
pub fn add(
    ctx: &Ctx,
    app: &str,
    system_path: &str,
    repo_path: Option<&str>,
    mode: Mode,
    why: Option<&str>,
) -> anyhow::Result<()> {
    // Normalize the target: relative to $HOME, leading `~/` or absolute home stripped.
    let target = normalize_target(system_path, &ctx.home);
    let basename = Path::new(&target)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| target.clone());
    let default_repo = format!("{app}/{basename}");
    let path = repo_path.unwrap_or(&default_repo);

    let src = std::fs::read_to_string(&ctx.manifest)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", ctx.manifest.display()))?;
    let mut doc = edit::parse(&src)?;
    edit::add_entry(&mut doc, NewEntry { name: app, path, target: &target, mode, why })
        .map_err(|e| anyhow::anyhow!(e))?;
    std::fs::write(&ctx.manifest, doc.to_string())?;

    println!("added {app} ({path} -> ~/{target}, mode: {mode})");
    if why.is_none() {
        println!("note: no `why` recorded — add one to the entry to keep the manifest self-documenting.");
    }
    Ok(())
}

/// `remove` — drop an entry from the manifest (deployed files are left intact).
pub fn remove(ctx: &Ctx, app: &str) -> anyhow::Result<()> {
    let src = std::fs::read_to_string(&ctx.manifest)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", ctx.manifest.display()))?;
    let mut doc = edit::parse(&src)?;
    if !edit::remove_entry(&mut doc, app) {
        anyhow::bail!("entry '{app}' not found in the manifest");
    }
    std::fs::write(&ctx.manifest, doc.to_string())?;
    println!("removed {app} from the manifest (deployed files left intact)");
    Ok(())
}

/// Strip a leading `~/` or `$HOME/` from a path so it is relative to `$HOME`.
fn normalize_target(system_path: &str, home: &Path) -> String {
    if let Some(rest) = system_path.strip_prefix("~/") {
        return rest.to_string();
    }
    if let Ok(rest) = Path::new(system_path).strip_prefix(home) {
        return rest.to_string_lossy().into_owned();
    }
    system_path.to_string()
}

/// `push` — commit (requiring a message when dirty) and push to origin.
///
/// The bash tool prompts interactively for the "what changed?" message; this
/// agent-facing surface requires `-m <why>` instead, keeping the same invariant
/// (no empty-message commits) without a prompt.
pub fn push(ctx: &Ctx, message: Option<&str>, branch: &str) -> anyhow::Result<()> {
    let repo = &ctx.repo_root;
    ensure_git_remote(repo)?;
    ensure_on_branch(repo, branch)?;

    let dirty = git_stdout(repo, &["status", "--porcelain"])?;
    if !dirty.trim().is_empty() {
        let Some(msg) = message else {
            anyhow::bail!(
                "uncommitted changes present — pass -m <message> describing what changed (empty messages are refused)"
            );
        };
        if msg.trim().is_empty() {
            anyhow::bail!("empty commit message refused");
        }
        if !git(repo, &["add", "-A"])?.status.success() {
            anyhow::bail!("git add failed");
        }
        if !git(repo, &["commit", "-m", msg])?.status.success() {
            anyhow::bail!("git commit failed");
        }
        println!("committed: {msg}");
    }

    // Push; set upstream if origin/<branch> does not exist yet.
    let upstream_exists = git(repo, &["rev-parse", "--verify", &format!("origin/{branch}")])?
        .status
        .success();
    let push_args: Vec<&str> = if upstream_exists {
        vec!["push", "origin", branch]
    } else {
        vec!["push", "-u", "origin", branch]
    };
    let out = git(repo, &push_args)?;
    if out.status.success() {
        println!("pushed {branch} to origin");
        Ok(())
    } else {
        anyhow::bail!("git push failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
}

/// `pull` — fast-forward-only pull of `origin/<branch>` into the current branch.
pub fn pull(ctx: &Ctx, branch: &str) -> anyhow::Result<()> {
    let repo = &ctx.repo_root;
    ensure_git_remote(repo)?;
    ensure_on_branch(repo, branch)?;

    if !git(repo, &["fetch", "origin", branch, "--quiet"])?.status.success() {
        anyhow::bail!("fetch failed (does origin/{branch} exist?)");
    }
    let remote_ref = format!("origin/{branch}");
    let behind = count(repo, &format!("HEAD..{remote_ref}"))?;
    if behind == 0 {
        println!("already up to date with {remote_ref}");
        return Ok(());
    }

    let old_head = git_stdout(repo, &["rev-parse", "HEAD"])?.trim().to_string();
    if !git(repo, &["merge", "--ff-only", &remote_ref, "--quiet"])?.status.success() {
        anyhow::bail!("pull failed (likely diverged) — resolve manually");
    }
    println!("pulled {behind} commit(s) from {remote_ref}:");
    print!("{}", git_stdout(repo, &["log", "--oneline", &format!("{old_head}..HEAD")])?);
    print!("{}", git_stdout(repo, &["diff", "--stat", &format!("{old_head}..HEAD")])?);
    Ok(())
}

/// `diff` — preview local state vs `origin/<branch>` before a pull/push.
///
/// `details` selects a detailed body instead of the stat summary; `raw` (the
/// `--git` flag) renders that body as native `git diff` output rather than the
/// friendly line-numbered view. `raw` implies `details`.
pub fn diff(ctx: &Ctx, branch: &str, details: bool, raw: bool) -> anyhow::Result<()> {
    let repo = &ctx.repo_root;
    ensure_git_remote(repo)?;
    if !git(repo, &["fetch", "origin", branch, "--quiet"])?.status.success() {
        anyhow::bail!("fetch failed (does origin/{branch} exist?)");
    }
    let remote_ref = format!("origin/{branch}");
    let detailed = details || raw;

    let dirty = git_stdout(repo, &["status", "--porcelain"])?;
    if !dirty.trim().is_empty() {
        println!("Uncommitted changes:");
        for line in dirty.lines() {
            println!("    {line}");
        }
        if detailed {
            // Working tree + staged, rendered together.
            print!("{}", show_diff(repo, &[vec![], vec!["--cached"]], raw)?);
        }
        println!();
    }

    if !git(repo, &["rev-parse", "--verify", &remote_ref])?.status.success() {
        println!("{remote_ref} does not exist on origin yet.");
        return Ok(());
    }

    let ahead = count(repo, &format!("{remote_ref}..HEAD"))?;
    let behind = count(repo, &format!("HEAD..{remote_ref}"))?;
    if ahead == 0 && behind == 0 && dirty.trim().is_empty() {
        println!("HEAD is in sync with {remote_ref}.");
        return Ok(());
    }
    if ahead > 0 {
        println!("Local is {ahead} commit(s) ahead of {remote_ref} (would push):");
        print!("{}", git_stdout(repo, &["log", "--oneline", &format!("{remote_ref}..HEAD")])?);
        let range = format!("{remote_ref}..HEAD");
        if detailed {
            print!("{}", show_diff(repo, &[vec![range.as_str()]], raw)?);
        } else {
            print!("{}", git_stdout(repo, &["diff", "--stat", &range])?);
        }
        println!();
    }
    if behind > 0 {
        println!("Remote is {behind} commit(s) ahead (would pull):");
        print!("{}", git_stdout(repo, &["log", "--oneline", &format!("HEAD..{remote_ref}")])?);
        let range = format!("HEAD..{remote_ref}");
        if detailed {
            print!("{}", show_diff(repo, &[vec![range.as_str()]], raw)?);
        } else {
            print!("{}", git_stdout(repo, &["diff", "--stat", &range])?);
        }
        println!();
    }
    Ok(())
}

/// Render one or more `git diff` invocations (each `args` set appended after
/// `diff`) as either native colored output (`raw`) or the friendly view.
fn show_diff(repo: &Path, arg_sets: &[Vec<&str>], raw: bool) -> anyhow::Result<String> {
    let mut body = String::new();
    for args in arg_sets {
        let mut a = vec!["diff"];
        if raw {
            a.push("--color=always");
        }
        a.extend_from_slice(args);
        body.push_str(&git_stdout(repo, &a)?);
    }
    Ok(if raw { body } else { diff_view::render(&body) })
}

/// Require a git repo with an `origin` remote.
fn ensure_git_remote(repo: &Path) -> anyhow::Result<()> {
    if !git(repo, &["rev-parse", "--git-dir"])?.status.success() {
        anyhow::bail!("{} is not a git repository", repo.display());
    }
    if !git(repo, &["remote", "get-url", "origin"])?.status.success() {
        anyhow::bail!("no 'origin' remote configured in {}", repo.display());
    }
    Ok(())
}

/// Require the working tree to be on `branch`.
fn ensure_on_branch(repo: &Path, branch: &str) -> anyhow::Result<()> {
    let current = git_stdout(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let current = current.trim();
    if current != branch {
        anyhow::bail!("currently on '{current}', not '{branch}' (use --branch {current})");
    }
    Ok(())
}

/// `git rev-list --count <range>` as a number.
fn count(repo: &Path, range: &str) -> anyhow::Result<u64> {
    let out = git_stdout(repo, &["rev-list", "--count", range])?;
    Ok(out.trim().parse().unwrap_or(0))
}

fn git(repo: &Path, args: &[&str]) -> anyhow::Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("running git {args:?}: {e}"))
}

fn git_stdout(repo: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = git(repo, args)?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
