//! `dotfiles` — the sole surface (ADR-001 #4 as amended by ADR-007, ADR-004).
//!
//! Scriptable, structured output; a drop-in replacement for the reference bash
//! `dotfiles` tool, reading the rich self-documenting TOML manifest. Verbs:
//! `status`, `deploy`, `enable`, `disable`, `add`, `remove`, `list`, `push`.

mod banner;
mod claude;
mod commands;
mod diff_view;
mod pkg;
mod profile;
mod show;
mod table;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use dotfiles_core::{DeployStatus, Manifest, Mode, State};
use std::path::{Path, PathBuf};
use table::{Align, Table, cell};

#[derive(Parser)]
#[command(name = "dotfiles", version, about = "Self-documenting dotfiles management")]
struct Cli {
    /// Path to the TOML manifest (default: `<store>/.dotfiles-manifest.toml`).
    #[arg(long, global = true)]
    manifest: Option<PathBuf>,
    /// Repo root that source paths resolve against (default: manifest's dir).
    #[arg(long, global = true)]
    repo_root: Option<PathBuf>,
    /// Home dir that target paths resolve against (default: $HOME).
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    /// Active profile (default: $DOTFILES_PROFILE, the `.dotfiles-profile` file,
    /// a `[profiles]` match against the hostname, then the hostname).
    #[arg(long, global = true)]
    profile: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show the deploy status of every managed dotfile.
    Status {
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Human)]
        format: Format,
    },
    /// Create symlinks (or copies) for every enabled dotfile.
    Deploy {
        /// Show what would change without touching the filesystem.
        #[arg(long)]
        dry_run: bool,
        /// Back up and overwrite an existing target instead of skipping it.
        #[arg(long, short)]
        force: bool,
    },
    /// Enable a dotfile (sets `enabled = true`).
    Enable { app: String },
    /// Disable a dotfile (sets `enabled = false`) and remove its symlink.
    Disable { app: String },
    /// Add a new dotfile to the manifest.
    Add {
        /// Stable handle, e.g. `zsh`.
        app: String,
        /// Deploy target relative to `$HOME`, e.g. `.zshrc`.
        system_path: String,
        /// Source path in the repo (default: `<app>/<basename of system_path>`).
        repo_path: Option<String>,
        /// Deployment mode.
        #[arg(long, value_enum, default_value_t = ModeArg::Symlink)]
        mode: ModeArg,
        /// The durable rationale for this entry (ADR-002).
        #[arg(long)]
        why: Option<String>,
    },
    /// Remove a dotfile from the manifest (does not touch deployed files).
    Remove { app: String },
    /// List every managed dotfile.
    List,
    /// Show one dotfile in full: rationale, structured spec, and deploy state.
    Show { app: String },
    /// Commit and push the dotfiles repo to origin.
    Push {
        /// Commit message / rationale. Required when there are uncommitted changes.
        #[arg(long, short)]
        message: Option<String>,
        /// Branch to push (must be the current branch).
        #[arg(long, short, default_value = "main")]
        branch: String,
    },
    /// Fast-forward pull from origin.
    Pull {
        /// Branch to pull (must be the current branch).
        #[arg(long, short, default_value = "main")]
        branch: String,
    },
    /// Preview local state vs origin (uncommitted + ahead/behind).
    Diff {
        /// Branch to compare against.
        #[arg(long, short, default_value = "main")]
        branch: String,
        /// Show a friendly, line-numbered colored diff instead of a stat summary.
        #[arg(long, short)]
        details: bool,
        /// With --details, render the raw `git diff` instead of the friendly view.
        #[arg(long, short)]
        git: bool,
    },
    /// Track explicitly-installed packages per host (pacman / AUR / flatpak).
    Pkg(pkg::PkgArgs),
    /// Manage profiles — named scopes over dotfiles + packages (per machine/role).
    Profile(profile::ProfileArgs),
    /// Read and write Claude Code user-scope settings (~/.claude/settings.json).
    Claude(claude::ClaudeArgs),
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Human,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Symlink,
    Copy,
}

impl From<ModeArg> for Mode {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Symlink => Mode::Symlink,
            ModeArg::Copy => Mode::Copy,
        }
    }
}

/// Resolved paths the verbs operate against, gated on being inside a git repo.
struct Ctx {
    manifest: PathBuf,
    repo_root: PathBuf,
    home: PathBuf,
    /// The active profile (resolved once at startup).
    profile: String,
}

impl Ctx {
    fn resolve(cli: &Cli) -> anyhow::Result<Self> {
        let home = cli
            .home
            .clone()
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .ok_or_else(|| anyhow::anyhow!("no --home and $HOME unset"))?;

        // Locate the dotfiles store: explicit --repo-root, else $DOTFILES_DIR,
        // else ~/.dotfiles. This lets `dotfiles` run from any directory.
        let store = cli
            .repo_root
            .clone()
            .or_else(|| std::env::var_os("DOTFILES_DIR").map(PathBuf::from))
            .unwrap_or_else(|| home.join(".dotfiles"));
        let manifest = cli
            .manifest
            .clone()
            .unwrap_or_else(|| store.join(".dotfiles-manifest.toml"));
        let repo_root = cli
            .repo_root
            .clone()
            .or_else(|| manifest.parent().map(Path::to_path_buf))
            .unwrap_or(store);

        // First-run gate (ADR-001 #7): operate only inside a git repo.
        if let Err(msg) = dotfiles_core::first_run_gate(&repo_root) {
            eprintln!("dotfiles: {msg}");
            std::process::exit(2);
        }
        let profile = resolve_active_profile(cli, &repo_root, &manifest);
        Ok(Ctx { manifest, repo_root, home, profile })
    }

    /// Read and parse the manifest into the typed catalog.
    fn load(&self) -> anyhow::Result<Manifest> {
        let src = std::fs::read_to_string(&self.manifest)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", self.manifest.display()))?;
        Ok(Manifest::from_toml(&src)?)
    }
}

/// Resolve the active profile. An explicit choice — `--profile`,
/// `$DOTFILES_PROFILE`, or the `.dotfiles-profile` file — wins; otherwise a
/// `[profiles]` `match` glob against the hostname, then the hostname itself.
fn resolve_active_profile(cli: &Cli, repo_root: &Path, manifest_path: &Path) -> String {
    let explicit = cli
        .profile
        .clone()
        .or_else(|| std::env::var("DOTFILES_PROFILE").ok())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::fs::read_to_string(repo_root.join(".dotfiles-profile"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });
    if let Some(name) = explicit {
        return name;
    }
    // Pattern resolution needs the manifest; tolerate a missing/unparseable one.
    let manifest = std::fs::read_to_string(manifest_path)
        .ok()
        .and_then(|s| Manifest::from_toml(&s).ok())
        .unwrap_or_default();
    dotfiles_core::resolve_profile(&manifest, None, &pkg::short_hostname())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // No subcommand: show the banner and the command list.
    let Some(command) = &cli.command else {
        banner::print();
        Cli::command().print_help()?;
        println!();
        return Ok(());
    };

    match command {
        Command::Status { format } => {
            let ctx = Ctx::resolve(&cli)?;
            status(&ctx, *format)?;
        }
        Command::List => {
            let ctx = Ctx::resolve(&cli)?;
            list(&ctx)?;
        }
        Command::Show { app } => {
            let ctx = Ctx::resolve(&cli)?;
            show::run(&ctx, app)?;
        }
        Command::Deploy { dry_run, force } => {
            let ctx = Ctx::resolve(&cli)?;
            commands::deploy(&ctx, *dry_run, *force)?;
        }
        Command::Enable { app } => {
            let ctx = Ctx::resolve(&cli)?;
            commands::set_enabled(&ctx, app, true)?;
        }
        Command::Disable { app } => {
            let ctx = Ctx::resolve(&cli)?;
            commands::set_enabled(&ctx, app, false)?;
        }
        Command::Add { app, system_path, repo_path, mode, why } => {
            let ctx = Ctx::resolve(&cli)?;
            commands::add(&ctx, app, system_path, repo_path.as_deref(), (*mode).into(), why.as_deref())?;
        }
        Command::Remove { app } => {
            let ctx = Ctx::resolve(&cli)?;
            commands::remove(&ctx, app)?;
        }
        Command::Push { message, branch } => {
            let ctx = Ctx::resolve(&cli)?;
            commands::push(&ctx, message.as_deref(), branch)?;
        }
        Command::Pull { branch } => {
            let ctx = Ctx::resolve(&cli)?;
            commands::pull(&ctx, branch)?;
        }
        Command::Diff { branch, details, git } => {
            let ctx = Ctx::resolve(&cli)?;
            commands::diff(&ctx, branch, *details, *git)?;
        }
        Command::Pkg(args) => {
            let ctx = Ctx::resolve(&cli)?;
            pkg::run(&ctx, args)?;
        }
        Command::Profile(args) => {
            let ctx = Ctx::resolve(&cli)?;
            profile::run(&ctx, args)?;
        }
        Command::Claude(args) => {
            let ctx = Ctx::resolve(&cli)?;
            claude::run(&ctx, args)?;
        }
    }
    Ok(())
}

/// `status` — derived deploy state, human table or JSON.
fn status(ctx: &Ctx, format: Format) -> anyhow::Result<()> {
    let manifest = ctx.load()?;
    let state = State::derive(&manifest, &ctx.repo_root, &ctx.home);
    match format {
        Format::Json => println!("{}", serde_json::to_string_pretty(&state)?),
        Format::Human => {
            // Provenance: a declared `[profiles.<name>]` scope, or an implicit
            // hostname fallback. Shares `profile list`'s vocabulary (ADR-102).
            let declared = manifest.profiles.contains_key(&ctx.profile);
            let provenance = if declared { "declared" } else { "implicit" };
            let mut t = Table::new()
                .title(format!("Dotfiles Status — profile: {} ({provenance})", ctx.profile))
                .column("APP", Align::Left)
                .column("TARGET", Align::Left)
                .column("STATUS", Align::Left);
            let mut issues = 0;
            // Scope breakdown so a profile-filtered report announces its basis.
            let (mut universal, mut scoped, mut other) = (0u32, 0u32, 0u32);
            for es in &state.entries {
                // Entries not in the active profile are intentionally absent here.
                if !es.entry.active_in(&ctx.profile) {
                    other += 1;
                    t.row(vec![
                        cell(&es.entry.name),
                        cell(&es.entry.target),
                        cell("other-profile").fg(table::DIM),
                    ]);
                    continue;
                }
                if es.entry.profiles.is_empty() {
                    universal += 1;
                } else {
                    scoped += 1;
                }
                let (label, color) = status_view(&es.status, es.entry.enabled);
                if es.entry.enabled
                    && matches!(
                        es.status,
                        DeployStatus::WrongTarget { .. }
                            | DeployStatus::Conflict
                            | DeployStatus::Broken
                            | DeployStatus::Missing
                    )
                {
                    issues += 1;
                }
                t.row(vec![
                    cell(&es.entry.name),
                    cell(&es.entry.target),
                    cell(label).fg(color),
                ]);
            }
            t.print();
            println!();
            // Scope line only when profiles are in use — a profile-indifferent
            // repo stays uncluttered (ADR-102).
            if !manifest.profiles.is_empty() {
                let mut scope = format!(
                    "Scope ({}): {universal} universal · {scoped} scoped here",
                    ctx.profile
                );
                if other > 0 {
                    scope.push_str(&format!(" · {other} in other profiles"));
                }
                println!("{}", table::paint(&scope, table::DIM));
            }
            if issues > 0 {
                println!("{issues} dotfile(s) need attention — run `dotfiles deploy`.");
            } else {
                println!("All enabled dotfiles are deployed.");
            }
        }
    }
    Ok(())
}

/// Presentation label + color for a deploy status (human format only).
pub(crate) fn status_view(s: &DeployStatus, enabled: bool) -> (&'static str, &'static str) {
    if !enabled {
        return ("disabled", table::DIM);
    }
    match s {
        DeployStatus::Linked => ("deployed", table::GREEN),
        DeployStatus::Present => ("deployed (copy)", table::GREEN),
        DeployStatus::Missing => ("not deployed", table::YELLOW),
        DeployStatus::Conflict => ("exists (unmanaged)", table::YELLOW),
        DeployStatus::Broken => ("broken (dangling link)", table::RED),
        DeployStatus::WrongTarget { .. } => ("wrong symlink", table::RED),
        DeployStatus::Error { .. } => ("error", table::RED),
    }
}

/// `list` — the managed catalog as a table.
fn list(ctx: &Ctx) -> anyhow::Result<()> {
    let manifest = ctx.load()?;
    let mut t = Table::new()
        .title("Managed Dotfiles")
        .column("APP", Align::Left)
        .column("SYSTEM PATH", Align::Left)
        .column("REPO PATH", Align::Left)
        .column("ENABLED", Align::Left)
        .column("MODE", Align::Left);
    for e in &manifest.entries {
        let enabled = if e.enabled {
            cell("yes").fg(table::GREEN)
        } else {
            cell("no").fg(table::DIM)
        };
        t.row(vec![
            cell(&e.name),
            cell(format!("~/{}", e.target)),
            cell(&e.path),
            enabled,
            cell(e.mode.to_string()),
        ]);
    }
    t.print();
    Ok(())
}
