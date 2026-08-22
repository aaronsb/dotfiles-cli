//! `store` — the store this host is bound to (ADR-013): show the binding,
//! list the registry, and pull the store's registry upstream.
//!
//! Also the registry plumbing `init` shares: a cached clone of the public
//! registry under `$XDG_CACHE_HOME/dotfiles/registry`, and `git subtree split`
//! to turn `configs/<id>/` into a root-relative history a store can merge.

use crate::Ctx;
use crate::commands::{git, git_clone, git_stdout};
use clap::{Args, Subcommand};
use dotfiles_core::binding::{
    HostBinding, Registry, RegistryEntry, UNREGISTERED, cli_satisfies, contract_home,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether this process has already refreshed the registry cache.
static FETCHED: AtomicBool = AtomicBool::new(false);

/// Where the public registry lives unless `$DOTFILES_REGISTRY` says otherwise.
pub const DEFAULT_REGISTRY: &str = "https://github.com/dotarchy/dotfiles.git";

#[derive(Args)]
pub struct StoreArgs {
    #[command(subcommand)]
    action: Option<StoreAction>,
}

#[derive(Subcommand)]
enum StoreAction {
    /// Print the host binding and the store's git state (default).
    Show,
    /// List the registry's published configurations.
    Registry,
    /// The registry entry this store descends from.
    Upstream {
        #[command(subcommand)]
        action: UpstreamAction,
    },
}

#[derive(Subcommand)]
enum UpstreamAction {
    /// Merge the latest registry entry into the store.
    Pull {
        /// List what would change without merging.
        #[arg(long)]
        what_if: bool,
    },
}

/// `registry` needs no store, so the context is resolved only for the verbs
/// that read one.
pub fn run(cli: &crate::Cli, args: &StoreArgs) -> anyhow::Result<()> {
    match args.action.as_ref().unwrap_or(&StoreAction::Show) {
        StoreAction::Show => show(&Ctx::resolve(cli)?),
        StoreAction::Registry => {
            let home = crate::home_dir(cli)?;
            let reg = RegistryClone::open(&home)?;
            print_registry(&reg.load()?);
            Ok(())
        }
        StoreAction::Upstream {
            action: UpstreamAction::Pull { what_if },
        } => upstream_pull(&Ctx::resolve(cli)?, *what_if),
    }
}

/// The binding, one field per line.
pub fn show_binding(b: &HostBinding, path: &Path) -> anyhow::Result<()> {
    println!("binding   {}", path.display());
    if let Some(s) = &b.store {
        println!("store     {}", s.path);
        println!("config    {}", s.config);
        println!(
            "profile   {}",
            s.profile.as_deref().unwrap_or("— (hostname at run time)")
        );
    }
    Ok(())
}

fn show(ctx: &Ctx) -> anyhow::Result<()> {
    match &ctx.binding {
        Some(b) if b.store.is_some() => show_binding(b, &ctx.binding_path)?,
        _ => {
            println!(
                "binding   {} (absent — run `dotfiles init`)",
                ctx.binding_path.display()
            );
            println!("store     {}", contract_home(&ctx.repo_root, &ctx.home));
        }
    }
    println!("active    {}", ctx.profile);
    let origin = git_stdout(&ctx.repo_root, &["remote", "get-url", "origin"])?;
    let origin = origin.trim();
    println!(
        "origin    {}",
        if origin.is_empty() {
            "— (local only)"
        } else {
            origin
        }
    );
    let dirty = git_stdout(&ctx.repo_root, &["status", "--porcelain"])?
        .lines()
        .count();
    println!(
        "worktree  {}",
        if dirty == 0 {
            "clean".to_string()
        } else {
            format!("{dirty} change(s) uncommitted")
        }
    );
    Ok(())
}

pub fn print_registry(reg: &Registry) {
    if reg.configs.is_empty() {
        println!("registry has no configurations.");
        return;
    }
    let w = reg.configs.keys().map(String::len).max().unwrap_or(0);
    for (id, e) in &reg.configs {
        println!(
            "{id:<w$}  {:<8}  {}",
            e.cli.as_deref().unwrap_or(""),
            e.description.as_deref().unwrap_or("")
        );
    }
}

fn upstream_pull(ctx: &Ctx, what_if: bool) -> anyhow::Result<()> {
    let Some(s) = ctx.binding.as_ref().and_then(|b| b.store.as_ref()) else {
        anyhow::bail!("no host binding — run `dotfiles init` first");
    };
    if s.config == UNREGISTERED {
        anyhow::bail!(
            "this store descends from no registry entry; `dotfiles init --mode rebase --config <id>` to adopt one"
        );
    }
    let reg = RegistryClone::open(&ctx.home)?;
    let msg = format!("store: Pull upstream {}", s.config);
    match merge_entry(&ctx.repo_root, &reg, &s.config, what_if, false, &msg)? {
        Merge::Unrelated => anyhow::bail!(
            "the store's history does not include {} — use `dotfiles init --mode rebase --config {}`",
            s.config,
            s.config
        ),
        Merge::UpToDate => println!("already up to date with {}", s.config),
        Merge::Previewed | Merge::Merged => {}
    }
    Ok(())
}

/// What [`merge_entry`] did.
pub enum Merge {
    /// The store already contains the entry's history.
    UpToDate,
    /// No common ancestor and `allow_unrelated` was false; nothing done.
    Unrelated,
    /// `what_if`: printed the preview, changed nothing.
    Previewed,
    Merged,
}

/// Fetch the entry's split history and merge it into the store. Conflicts stop
/// the merge in the working tree and are reported as an error. `init --mode
/// rebase` and `store upstream pull` are this with different `allow_unrelated`.
pub fn merge_entry(
    store: &Path,
    reg: &RegistryClone,
    id: &str,
    what_if: bool,
    allow_unrelated: bool,
    msg: &str,
) -> anyhow::Result<Merge> {
    let split = reg.split(id)?;
    fetch_split(store, &reg.path, &split)?;

    let related = git(store, &["merge-base", "HEAD", "FETCH_HEAD"])?
        .status
        .success();
    let incoming = git_stdout(store, &["rev-list", "--count", "HEAD..FETCH_HEAD"])?;
    if related && incoming.trim() == "0" {
        return Ok(Merge::UpToDate);
    }
    if !related && !allow_unrelated {
        return Ok(Merge::Unrelated);
    }
    if what_if {
        if related {
            println!("would merge {} commit(s) from {id}:", incoming.trim());
            print!(
                "{}",
                git_stdout(store, &["diff", "--stat", "HEAD...FETCH_HEAD"])?
            );
        } else {
            // With no common ancestor, every file both sides carry with
            // different content conflicts.
            let both = git_stdout(
                store,
                &[
                    "diff",
                    "--name-only",
                    "--diff-filter=M",
                    "HEAD",
                    "FETCH_HEAD",
                ],
            )?;
            println!(
                "would merge {id} ({}); {} file(s) would conflict:",
                &split[..7],
                both.lines().count()
            );
            print!("{both}");
        }
        return Ok(Merge::Previewed);
    }
    let dirty = git_stdout(store, &["status", "--porcelain", "--untracked-files=no"])?;
    if !dirty.trim().is_empty() {
        anyhow::bail!("the store has uncommitted changes; commit or stash them, then re-run");
    }
    let mut argv = vec!["merge", "--no-edit", "-m", msg];
    if !related {
        argv.push("--allow-unrelated-histories");
    }
    argv.push("FETCH_HEAD");
    let out = git(store, &argv)?;
    if !out.status.success() {
        report_conflicts(store)?;
        anyhow::bail!("merge stopped on conflicts — resolve, commit, then re-run");
    }
    println!(
        "merged {} from {id}:",
        if related {
            format!("{} commit(s)", incoming.trim())
        } else {
            "base content".into()
        }
    );
    print!(
        "{}",
        git_stdout(store, &["diff", "--stat", "HEAD~1..HEAD"])?
    );
    Ok(Merge::Merged)
}

/// Print the files a stopped merge left conflicted.
pub fn report_conflicts(store: &Path) -> anyhow::Result<()> {
    let files = git_stdout(store, &["diff", "--name-only", "--diff-filter=U"])?;
    eprintln!("conflicts in:");
    for f in files.lines() {
        eprintln!("  {f}");
    }
    Ok(())
}

/// `git fetch <registry-clone> <split-sha>` into the store.
pub fn fetch_split(store: &Path, registry: &Path, split: &str) -> anyhow::Result<()> {
    let reg = registry.to_string_lossy();
    let out = git(store, &["fetch", "--quiet", &reg, split])?;
    if !out.status.success() {
        anyhow::bail!(
            "fetching {split} from the registry clone: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// A cached clone of the public registry.
pub struct RegistryClone {
    pub path: PathBuf,
    pub url: String,
}

impl RegistryClone {
    pub fn url() -> String {
        std::env::var("DOTFILES_REGISTRY")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_REGISTRY.to_string())
    }

    fn cache_dir(home: &Path) -> PathBuf {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".cache"))
            .join("dotfiles")
            .join("registry")
    }

    /// Clone the registry if absent, otherwise fetch and reset to its head.
    pub fn open(home: &Path) -> anyhow::Result<Self> {
        let url = Self::url();
        let path = Self::cache_dir(home);
        if path.join(".git").exists() {
            let current = git_stdout(&path, &["remote", "get-url", "origin"])?;
            if current.trim() != url {
                std::fs::remove_dir_all(&path)?;
            }
        }
        if !path.join(".git").exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            git_clone(&url, &path).map_err(|e| anyhow::anyhow!("cloning registry: {e}"))?;
        } else if !FETCHED.swap(true, Ordering::SeqCst) {
            // Once per process: interactive paths open the registry to list it
            // and again to use it.
            let out = git(&path, &["fetch", "--quiet", "origin"])?;
            if !out.status.success() {
                eprintln!(
                    "warning: registry fetch failed, using the cached copy: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            } else {
                git(&path, &["reset", "--quiet", "--hard", "FETCH_HEAD"])?;
            }
        }
        Ok(Self { path, url })
    }

    pub fn load(&self) -> anyhow::Result<Registry> {
        let p = self.path.join("registry.toml");
        let src = std::fs::read_to_string(&p).map_err(|e| {
            anyhow::anyhow!(
                "reading {}: {e} (is {} a dotfiles registry?)",
                p.display(),
                self.url
            )
        })?;
        Ok(Registry::from_toml(&src)?)
    }

    /// The registry row for `id`, after checking the running CLI satisfies it.
    pub fn entry(&self, id: &str) -> anyhow::Result<RegistryEntry> {
        let reg = self.load()?;
        let Some(e) = reg.configs.get(id) else {
            anyhow::bail!(
                "'{id}' is not in the registry ({}); `dotfiles store registry` lists what is",
                self.url
            );
        };
        if let Some(req) = &e.cli {
            let running = env!("CARGO_PKG_VERSION");
            match cli_satisfies(req, running) {
                Ok(true) => {}
                Ok(false) => anyhow::bail!(
                    "'{id}' needs dotfiles {req}; this is v{running} — `dotfiles pull` or reinstall first"
                ),
                Err(why) => eprintln!("warning: ignoring '{id}' cli requirement: {why}"),
            }
        }
        if !self.path.join(Registry::entry_dir(id)).is_dir() {
            anyhow::bail!("'{id}' is indexed but configs/{id}/ is missing from the registry");
        }
        Ok(e.clone())
    }

    /// `git subtree split` the entry into a root-relative history; returns its sha.
    pub fn split(&self, id: &str) -> anyhow::Result<String> {
        self.entry(id)?;
        let prefix = format!("--prefix={}", Registry::entry_dir(id));
        let out = git(&self.path, &["subtree", "split", &prefix])?;
        if !out.status.success() {
            anyhow::bail!(
                "git subtree split failed (is git-subtree installed?): {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if sha.len() < 7 {
            anyhow::bail!("git subtree split returned no commit for configs/{id}");
        }
        Ok(sha)
    }
}
