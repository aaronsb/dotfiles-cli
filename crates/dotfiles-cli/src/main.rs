//! `dotfiles` — the sole surface (ADR-001 #4 as amended by ADR-007, ADR-004).
//!
//! Scriptable, structured output; a drop-in replacement for the reference bash
//! `dotfiles` tool, reading the rich self-documenting TOML manifest. Verbs:
//! `status`, `deploy`, `enable`, `disable`, `add`, `remove`, `list`, `push`.

mod commands;
mod pkg;

use clap::{Parser, Subcommand, ValueEnum};
use dotfiles_core::{DeployStatus, Manifest, Mode, State};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "dotfiles", version, about = "Self-documenting dotfiles management")]
struct Cli {
    /// Path to the TOML manifest.
    #[arg(long, global = true, default_value = ".dotfiles-manifest.toml")]
    manifest: PathBuf,
    /// Repo root that source paths resolve against (default: manifest's dir).
    #[arg(long, global = true)]
    repo_root: Option<PathBuf>,
    /// Home dir that target paths resolve against (default: $HOME).
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
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
        /// Show full colored diffs, not just a stat summary.
        #[arg(long, short)]
        details: bool,
    },
    /// Track explicitly-installed packages per host (pacman / AUR / flatpak).
    Pkg(pkg::PkgArgs),
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
}

impl Ctx {
    fn resolve(cli: &Cli) -> anyhow::Result<Self> {
        let manifest = cli.manifest.clone();
        let repo_root = cli
            .repo_root
            .clone()
            .or_else(|| manifest.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let home = cli
            .home
            .clone()
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .ok_or_else(|| anyhow::anyhow!("no --home and $HOME unset"))?;

        // First-run gate (ADR-001 #7): operate only inside a git repo.
        if let Err(msg) = dotfiles_core::first_run_gate(&repo_root) {
            eprintln!("dotfiles: {msg}");
            std::process::exit(2);
        }
        Ok(Ctx { manifest, repo_root, home })
    }

    /// Read and parse the manifest into the typed catalog.
    fn load(&self) -> anyhow::Result<Manifest> {
        let src = std::fs::read_to_string(&self.manifest)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", self.manifest.display()))?;
        Ok(Manifest::from_toml(&src)?)
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Command::Status { format } => {
            let ctx = Ctx::resolve(&cli)?;
            status(&ctx, *format)?;
        }
        Command::List => {
            let ctx = Ctx::resolve(&cli)?;
            list(&ctx)?;
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
        Command::Diff { branch, details } => {
            let ctx = Ctx::resolve(&cli)?;
            commands::diff(&ctx, branch, *details)?;
        }
        Command::Pkg(args) => {
            let ctx = Ctx::resolve(&cli)?;
            pkg::run(&ctx, args)?;
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
            println!("=== Dotfiles Status ===\n");
            let mut issues = 0;
            for es in &state.entries {
                let label = status_label(&es.status, es.entry.enabled);
                if matches!(es.status, DeployStatus::WrongTarget { .. } | DeployStatus::Conflict | DeployStatus::Broken | DeployStatus::Missing) && es.entry.enabled {
                    issues += 1;
                }
                println!("{:<22} {:<30} {label}", es.entry.name, es.entry.target);
            }
            println!();
            if issues > 0 {
                println!("{issues} dotfile(s) need attention — run `dotfiles deploy`.");
            } else {
                println!("All enabled dotfiles are deployed.");
            }
        }
    }
    Ok(())
}

/// Presentation label for a deploy status (human format only).
fn status_label(s: &DeployStatus, enabled: bool) -> &'static str {
    if !enabled {
        return "disabled";
    }
    match s {
        DeployStatus::Linked => "deployed",
        DeployStatus::Present => "deployed (copy)",
        DeployStatus::Missing => "not deployed",
        DeployStatus::Conflict => "exists (unmanaged)",
        DeployStatus::Broken => "broken (dangling link)",
        DeployStatus::WrongTarget { .. } => "wrong symlink",
        DeployStatus::Error { .. } => "error",
    }
}

/// `list` — the managed catalog as a table.
fn list(ctx: &Ctx) -> anyhow::Result<()> {
    let manifest = ctx.load()?;
    println!("=== Managed Dotfiles ===\n");
    println!("{:<22} {:<28} {:<28} {:<8} MODE", "APP", "SYSTEM PATH", "REPO PATH", "ENABLED");
    for e in &manifest.entries {
        println!(
            "{:<22} {:<28} {:<28} {:<8} {}",
            e.name,
            format!("~/{}", e.target),
            e.path,
            e.enabled,
            e.mode,
        );
    }
    Ok(())
}
