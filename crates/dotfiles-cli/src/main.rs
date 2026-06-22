//! `dotfiles-cli` — the non-interactive JSON surface (ADR-001 #4, ADR-004).
//!
//! The agent-facing front-end: fully scriptable, structured output. v0.1:
//! `status` derives the dotfiles state (catalog + deploy status, ADR-005) and
//! prints it as JSON.

use clap::{Parser, Subcommand, ValueEnum};
use dotfiles_core::{Manifest, State};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "dotfiles-cli", version, about = "Agent-facing surface for dotfiles-tui")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Derive and print the managed-dotfiles state (catalog + deploy status).
    Status {
        /// Path to the TOML manifest.
        #[arg(long, default_value = ".dotfiles-manifest.toml")]
        manifest: PathBuf,
        /// Repo root that source paths resolve against (default: manifest's dir).
        #[arg(long)]
        repo_root: Option<PathBuf>,
        /// Home dir that target paths resolve against (default: $HOME).
        #[arg(long)]
        home: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Json)]
        format: Format,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Json,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Status { manifest, repo_root, home, format } => {
            let src = std::fs::read_to_string(&manifest)
                .map_err(|e| anyhow::anyhow!("reading {}: {e}", manifest.display()))?;
            let m = Manifest::from_toml(&src)?;

            let repo_root = repo_root
                .or_else(|| manifest.parent().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| PathBuf::from("."));
            let home = home
                .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
                .ok_or_else(|| anyhow::anyhow!("no --home and $HOME unset"))?;

            let state = State::derive(&m, &repo_root, &home);
            match format {
                Format::Json => println!("{}", serde_json::to_string_pretty(&state)?),
            }
        }
    }
    Ok(())
}
