//! `init` — make this host (ADR-013 §5).
//!
//! One verb for the first install and every reconfigure. With no store it
//! creates one from a registry entry or clones an existing store. With a store
//! it asks one question — keep (migrate), clean start, or rebase — and each
//! answer is one git operation. Every path is idempotent and honours
//! `--what-if`; only clean start is destructive, and it backs up first.

use crate::Cli;
use crate::commands::{git, git_clone, git_stdout};
use crate::store::{Merge, RegistryClone, fetch_split, merge_entry, print_registry};
use clap::{Args, ValueEnum};
use dotfiles_core::binding::{
    HostBinding, StoreBinding, UNREGISTERED, contract_home, expand_home, valid_config_id,
};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Files a pre-ADR-013 store carried that belong to the tool, removed by migrate.
const ENGINE_FILES: &[&str] = &["bootstrap.sh", "install.sh"];
/// Machine-local markers folded into the binding by migrate.
const LEGACY_PROFILE: &str = ".dotfiles-profile";
const LEGACY_LOCAL: &str = ".dotfiles-local.toml";
const DEFAULT_CONFIG: &str = "sh.dotarchy.starter";

#[derive(Args)]
pub struct InitArgs {
    /// Registry entry to create the store from, or to rebase onto.
    #[arg(long)]
    config: Option<String>,
    /// Adopt an existing store by cloning this URL instead of using the registry.
    #[arg(long, conflicts_with = "config")]
    existing: Option<String>,
    /// Store path (default: the binding's, else ~/.dotfiles).
    #[arg(long)]
    store: Option<PathBuf>,
    /// Create a remote for a new store.
    #[arg(long, value_enum)]
    remote: Option<Remote>,
    /// Repository name when --remote creates one.
    #[arg(long, default_value = "dotfiles-store")]
    repo_name: String,
    /// Active profile to record (default: hostname, resolved at run time).
    #[arg(long)]
    profile: Option<String>,
    /// What to do when a store already exists.
    #[arg(long, value_enum)]
    mode: Option<Mode>,
    /// Show what would happen and change nothing.
    #[arg(long)]
    what_if: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Remote {
    None,
    Github,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
enum Mode {
    /// Keep the store; strip engine files and fold legacy markers into the binding.
    Migrate,
    /// Back the store up and start over from a registry entry.
    Clean,
    /// Keep the store; change which registry entry its base content descends from.
    Rebase,
}

/// Everything `init` needs to decide, resolved once.
struct Plan {
    home: PathBuf,
    binding_path: PathBuf,
    binding: Option<HostBinding>,
    store: PathBuf,
    what_if: bool,
}

pub fn run(cli: &Cli, args: &InitArgs) -> anyhow::Result<()> {
    let home = crate::home_dir(cli)?;
    let binding_path = HostBinding::default_path(&home);
    let binding = crate::load_binding(&binding_path);
    // `--store` first, then the same resolution every other verb uses.
    let store = match &args.store {
        Some(p) => crate::absolute(&expand_home(&p.to_string_lossy(), &home)),
        None => crate::resolve_store(cli, &home, binding.as_ref()).0,
    };
    let plan = Plan {
        home,
        binding_path,
        binding,
        store,
        what_if: args.what_if,
    };

    if let Some(id) = &args.config
        && !valid_config_id(id)
    {
        anyhow::bail!("'{id}' is not a configuration id (reverse-DNS, e.g. com.example.dotfiles)");
    }

    let has_store = plan.store.join(".git").exists();
    if !has_store {
        if plan.store.exists() && std::fs::read_dir(&plan.store)?.next().is_some() {
            anyhow::bail!(
                "{} exists and is not a git repo; choose another --store or move it aside",
                plan.store.display()
            );
        }
        return first_install(&plan, args);
    }
    reconfigure(&plan, args)
}

// ---------------------------------------------------------------- first install

fn first_install(plan: &Plan, args: &InitArgs) -> anyhow::Result<()> {
    println!("=== dotfiles init — first install ===");
    println!("store: {}", contract_home(&plan.store, &plan.home));

    let source = match (&args.existing, &args.config) {
        (Some(url), _) => Source::Existing(url.clone()),
        (None, Some(id)) => Source::Registry(id.clone()),
        (None, None) => choose_source(plan)?,
    };
    if plan.what_if {
        println!("would create the store from {source}");
        return Ok(());
    }

    let mut legacy_profile = None;
    let config = match &source {
        Source::Existing(url) => {
            clone_store(url, &plan.store)?;
            legacy_profile = read_legacy_profile(&plan.store);
            // An adopted store may predate ADR-013; bring it across in the same run.
            if legacy_markers(&plan.store).is_empty() {
                plan.binding
                    .as_ref()
                    .and_then(|b| b.store.as_ref())
                    .map(|s| s.config.clone())
                    .unwrap_or(UNREGISTERED.into())
            } else {
                migrate_store(plan, args, None)?
            }
        }
        Source::Registry(id) => {
            create_store_from_registry(plan, id)?;
            id.clone()
        }
    };

    let profile = args.profile.clone().or(legacy_profile);
    let added = ensure_packages_dir(&plan.store, profile.as_deref())?;
    commit_if_dirty(
        &plan.store,
        "store: Initialize host packages directory",
        &added,
    )?;
    offer_remote(plan, args)?;
    write_binding(plan, &config, profile)?;
    println!(
        "\ndone — `dotfiles status` to see what would deploy, `dotfiles deploy --dry-run` to preview."
    );
    Ok(())
}

enum Source {
    Registry(String),
    Existing(String),
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Registry(id) => write!(f, "registry entry {id}"),
            Source::Existing(url) => write!(f, "existing store {url}"),
        }
    }
}

fn choose_source(plan: &Plan) -> anyhow::Result<Source> {
    require_tty("--config <id> or --existing <url>")?;
    println!("\nWhere should this host's store come from?");
    println!("  1) a registry configuration (default: {DEFAULT_CONFIG})");
    println!("  2) an existing store you already push to (git URL)");
    match ask("choice", Some("1"))?.as_str() {
        "2" => Ok(Source::Existing(ask("git URL", None)?)),
        _ => {
            let reg = RegistryClone::open(&plan.home)?;
            println!();
            print_registry(&reg.load()?);
            Ok(Source::Registry(ask(
                "configuration",
                Some(DEFAULT_CONFIG),
            )?))
        }
    }
}

fn clone_store(url: &str, store: &Path) -> anyhow::Result<()> {
    println!("cloning {url} …");
    git_clone(url, store)
}

/// `git init` + fetch the entry's split history + reset onto it: the store
/// starts with the configuration's own history, so later pulls are related.
fn create_store_from_registry(plan: &Plan, id: &str) -> anyhow::Result<()> {
    let reg = RegistryClone::open(&plan.home)?;
    let split = reg.split(id)?;
    create_store_from_split(plan, id, &reg, &split)
}

fn create_store_from_split(
    plan: &Plan,
    id: &str,
    reg: &RegistryClone,
    split: &str,
) -> anyhow::Result<()> {
    println!("creating store from {id} ({})", &split[..7]);
    std::fs::create_dir_all(&plan.store)?;
    let out = git(&plan.store, &["init", "--quiet", "-b", "main"])?;
    if !out.status.success() {
        anyhow::bail!(
            "git init failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    fetch_split(&plan.store, &reg.path, split)?;
    git(&plan.store, &["reset", "--quiet", "--hard", "FETCH_HEAD"])?;
    Ok(())
}

// ---------------------------------------------------------------- reconfigure

fn reconfigure(plan: &Plan, args: &InitArgs) -> anyhow::Result<()> {
    let bound = plan.binding.as_ref().and_then(|b| b.store.as_ref());
    let markers = legacy_markers(&plan.store);
    // `--store` naming a different repo than the binding is a rebinding.
    let bound_here = plan
        .binding
        .as_ref()
        .and_then(|b| b.store_path(&plan.home))
        .is_some_and(|p| crate::same_dir(&p, &plan.store));
    // A requested config this store does not already descend from.
    let wants_rebase = args
        .config
        .as_ref()
        .is_some_and(|id| !(bound_here && bound.is_some_and(|s| s.config == *id)));

    // Already current: bound to this store, nothing legacy remains, no change asked.
    if bound_here
        && markers.is_empty()
        && args.mode.is_none()
        && !wants_rebase
        && args.profile.is_none()
    {
        println!("this host is already set up:");
        return crate::store::show_binding(plan.binding.as_ref().unwrap(), &plan.binding_path);
    }

    let mode = match args.mode {
        Some(m) => m,
        // A legacy store migrates first; the Migrate arm rebases afterwards.
        None if wants_rebase => {
            if markers.is_empty() {
                Mode::Rebase
            } else {
                Mode::Migrate
            }
        }
        None if markers.is_empty() => Mode::Migrate, // only the binding changes
        None if plan.what_if => {
            println!("(no --mode given; previewing migrate)");
            Mode::Migrate
        }
        None => choose_mode(&markers)?,
    };
    println!("=== dotfiles init — {mode:?} ===");
    println!("store: {}", contract_home(&plan.store, &plan.home));

    match mode {
        Mode::Migrate => {
            let profile = args
                .profile
                .clone()
                .or_else(|| bound.and_then(|s| s.profile.clone()))
                .or_else(|| read_legacy_profile(&plan.store));
            let config = migrate_store(plan, args, bound.map(|s| s.config.clone()))?;
            if !plan.what_if {
                let added = ensure_packages_dir(&plan.store, profile.as_deref())?;
                commit_if_dirty(
                    &plan.store,
                    "store: Migrate to the ADR-013 host binding",
                    &added,
                )?;
            }
            // `--config` asked for ancestry, not a label: rebase after migrating.
            if wants_rebase {
                rebase_store(plan, &config)?;
            }
            write_binding(plan, &config, profile)
        }
        Mode::Clean => {
            let id = match &args.config {
                Some(id) => id.clone(),
                None => {
                    require_tty("--config <id>")?;
                    ask("configuration to start from", Some(DEFAULT_CONFIG))?
                }
            };
            let backup = backup_dir(&plan.home)?;
            if plan.what_if {
                println!(
                    "would move {} to {} and create a new store from {id}",
                    plan.store.display(),
                    backup.display()
                );
                return Ok(());
            }
            // Everything that can fail on the registry side happens before the
            // live store moves.
            let reg = RegistryClone::open(&plan.home)?;
            let split = reg.split(&id)?;
            println!("moving {} -> {}", plan.store.display(), backup.display());
            std::fs::create_dir_all(backup.parent().unwrap())?;
            std::fs::rename(&plan.store, &backup).map_err(|e| {
                anyhow::anyhow!(
                    "moving the store aside: {e} (a backup on another filesystem needs a manual move)"
                )
            })?;
            create_store_from_split(plan, &id, &reg, &split)?;
            let profile = args.profile.clone();
            let added = ensure_packages_dir(&plan.store, profile.as_deref())?;
            commit_if_dirty(
                &plan.store,
                "store: Initialize host packages directory",
                &added,
            )?;
            offer_remote(plan, args)?;
            write_binding(plan, &id, profile)
        }
        Mode::Rebase => {
            let id = match &args.config {
                Some(id) => id.clone(),
                None => {
                    require_tty("--config <id>")?;
                    let reg = RegistryClone::open(&plan.home)?;
                    print_registry(&reg.load()?);
                    ask("configuration to rebase onto", None)?
                }
            };
            rebase_store(plan, &id)?;
            let profile = args
                .profile
                .clone()
                .or_else(|| bound.and_then(|s| s.profile.clone()));
            write_binding(plan, &id, profile)
        }
    }
}

fn choose_mode(markers: &[String]) -> anyhow::Result<Mode> {
    require_tty("--mode migrate|clean|rebase")?;
    println!("\nA store already exists here.");
    if !markers.is_empty() {
        println!(
            "It predates the host binding; these would be folded in or removed: {}",
            markers.join(", ")
        );
    }
    println!("  1) keep it — migrate onto the host binding");
    println!("  2) clean start — back it up and create a new store");
    println!("  3) rebase — keep it, change which registry entry it descends from");
    Ok(match ask("choice", Some("1"))?.as_str() {
        "2" => Mode::Clean,
        "3" => Mode::Rebase,
        _ => Mode::Migrate,
    })
}

/// Legacy files present at the store root.
fn legacy_markers(store: &Path) -> Vec<String> {
    ENGINE_FILES
        .iter()
        .chain([LEGACY_PROFILE, LEGACY_LOCAL].iter())
        .filter(|f| store.join(f).exists())
        .map(|f| f.to_string())
        .collect()
}

fn read_legacy_profile(store: &Path) -> Option<String> {
    std::fs::read_to_string(store.join(LEGACY_PROFILE))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Strip engine files and legacy markers; returns the `config` to record.
/// Refuses on a dirty worktree, because the migration commit must be only
/// the migration.
fn migrate_store(plan: &Plan, args: &InitArgs, current: Option<String>) -> anyhow::Result<String> {
    let markers = legacy_markers(&plan.store);
    let config = args
        .config
        .clone()
        .or(current)
        .unwrap_or_else(|| UNREGISTERED.to_string());
    if markers.is_empty() {
        return Ok(config);
    }
    if plan.what_if {
        println!("would remove: {}", markers.join(", "));
        if let Some(p) = read_legacy_profile(&plan.store) {
            println!("would record profile '{p}' in the binding");
        }
        return Ok(config);
    }
    let dirty = git_stdout(
        &plan.store,
        &["status", "--porcelain", "--untracked-files=no"],
    )?;
    if !dirty.trim().is_empty() {
        anyhow::bail!("the store has uncommitted changes; commit or stash them, then re-run");
    }
    if plan.store.join(LEGACY_LOCAL).exists() {
        // Untracked and gitignored (ADR-012), so git keeps no copy: back it up.
        let backup = backup_dir(&plan.home)?;
        std::fs::create_dir_all(&backup)?;
        std::fs::copy(plan.store.join(LEGACY_LOCAL), backup.join(LEGACY_LOCAL))?;
        println!(
            "backed up {LEGACY_LOCAL} -> {}",
            backup.join(LEGACY_LOCAL).display()
        );
        let src = std::fs::read_to_string(plan.store.join(LEGACY_LOCAL)).unwrap_or_default();
        if src.contains("[packages]") {
            eprintln!(
                "note: {LEGACY_LOCAL} pointed packages elsewhere; packages now live in <store>/packages/ (ADR-013 §4). Move them back by hand if you kept any there."
            );
        }
    }
    for f in &markers {
        let tracked = git(&plan.store, &["ls-files", "--error-unmatch", f])?
            .status
            .success();
        if tracked {
            git(&plan.store, &["rm", "--quiet", "-r", f])?;
        } else {
            let p = plan.store.join(f);
            if p.is_dir() {
                std::fs::remove_dir_all(&p)?
            } else {
                std::fs::remove_file(&p)?
            }
        }
        println!("removed {f}");
    }
    Ok(config)
}

/// Merge the entry's split history over the store's base content.
fn rebase_store(plan: &Plan, id: &str) -> anyhow::Result<()> {
    let reg = RegistryClone::open(&plan.home)?;
    let msg = format!("store: Rebase on {id}");
    match merge_entry(&plan.store, &reg, id, plan.what_if, true, &msg) {
        Ok(Merge::UpToDate) => println!("the store already contains {id}"),
        Ok(Merge::Merged) => println!("rebased on {id}"),
        Ok(Merge::Previewed | Merge::Unrelated) => {}
        Err(e) => anyhow::bail!("{e} `dotfiles init --mode rebase --config {id}`"),
    }
    Ok(())
}

// ---------------------------------------------------------------- shared steps

/// Create `packages/<profile>/` if absent; returns the paths it created so the
/// caller can stage exactly those.
fn ensure_packages_dir(store: &Path, profile: Option<&str>) -> anyhow::Result<Vec<String>> {
    let name = profile
        .map(str::to_string)
        .unwrap_or_else(crate::pkg::short_hostname);
    let dir = store.join("packages").join(&name);
    if dir.exists() {
        return Ok(vec![]);
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(".gitkeep"), "")?;
    println!("created packages/{name}/");
    Ok(vec![format!("packages/{name}/.gitkeep")])
}

/// Commit tracked changes plus `new_paths`. Untracked files stay untracked:
/// a migration commit is only the migration.
fn commit_if_dirty(store: &Path, msg: &str, new_paths: &[String]) -> anyhow::Result<()> {
    git(store, &["add", "-u"])?;
    for p in new_paths {
        git(store, &["add", "--", p])?;
    }
    let staged = git_stdout(store, &["diff", "--cached", "--name-only"])?;
    if staged.trim().is_empty() {
        return Ok(());
    }
    let out = git(store, &["commit", "--quiet", "-m", msg])?;
    if !out.status.success() {
        anyhow::bail!(
            "git commit failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    println!("committed: {msg}");
    Ok(())
}

/// Offer (or, with --remote, perform) remote creation. Only `gh` is wired.
fn offer_remote(plan: &Plan, args: &InitArgs) -> anyhow::Result<()> {
    let has_origin = git(&plan.store, &["remote", "get-url", "origin"])?
        .status
        .success();
    if has_origin {
        return Ok(());
    }
    let gh_ok = || {
        Command::new("gh")
            .args(["auth", "status"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    let choice = match args.remote {
        Some(r) => r,
        None if !std::io::stdin().is_terminal() || !gh_ok() => Remote::None,
        None => {
            println!("\nThe store is a local git repo. Create a private GitHub remote for it?");
            if ask(
                &format!("create github.com/<you>/{} [y/N]", args.repo_name),
                Some("n"),
            )?
            .to_lowercase()
            .starts_with('y')
            {
                Remote::Github
            } else {
                Remote::None
            }
        }
    };
    match choice {
        Remote::None => {
            println!(
                "store is local-only; `git -C {} remote add origin <url>` when you want one.",
                contract_home(&plan.store, &plan.home)
            );
        }
        Remote::Github => {
            if !gh_ok() {
                anyhow::bail!(
                    "--remote github needs `gh` installed and authenticated (`gh auth login`)"
                );
            }
            let out = Command::new("gh")
                .args([
                    "repo",
                    "create",
                    &args.repo_name,
                    "--private",
                    "--remote=origin",
                    "--push",
                    "--source",
                ])
                .arg(&plan.store)
                .output()?;
            if !out.status.success() {
                anyhow::bail!(
                    "gh repo create failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            println!(
                "created and pushed: {}",
                String::from_utf8_lossy(&out.stdout).trim()
            );
        }
    }
    Ok(())
}

fn write_binding(plan: &Plan, config: &str, profile: Option<String>) -> anyhow::Result<()> {
    let b = HostBinding {
        store: Some(StoreBinding {
            path: contract_home(&plan.store, &plan.home),
            config: config.to_string(),
            profile,
        }),
    };
    if plan.what_if {
        println!("would write {}:", plan.binding_path.display());
        return crate::store::show_binding(&b, &plan.binding_path);
    }
    b.save(&plan.binding_path).map_err(|e| anyhow::anyhow!(e))?;
    println!("wrote {}", plan.binding_path.display());
    crate::store::show_binding(&b, &plan.binding_path)
}

fn backup_dir(home: &Path) -> anyhow::Result<PathBuf> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    Ok(dotfiles_core::deploy::backup_dir(home).join(format!("store-{secs}")))
}

// ---------------------------------------------------------------- prompting

fn require_tty(flags: &str) -> anyhow::Result<()> {
    if std::io::stdin().is_terminal() {
        Ok(())
    } else {
        anyhow::bail!("not interactive — pass {flags}")
    }
}

fn ask(question: &str, default: Option<&str>) -> anyhow::Result<String> {
    match default {
        Some(d) => print!("{question} [{d}]: "),
        None => print!("{question}: "),
    }
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let line = line.trim();
    if line.is_empty() {
        return default
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("a value is required"));
    }
    Ok(line.to_string())
}
