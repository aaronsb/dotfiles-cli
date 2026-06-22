//! The `profile` verb — manage profiles, named scopes over dotfiles + packages
//! (one per machine or role). Backed by the `[profiles]` registry and per-entry
//! `profiles` tags in the manifest, plus per-profile `packages/<profile>/` dirs.

use crate::Ctx;
use crate::table::{self, Align, Table, cell};
use dotfiles_core::{Manifest, edit};
use std::path::Path;

/// `profile [list|add|remove|copy|use] …`. A missing action defaults to `list`.
#[derive(clap::Args)]
pub struct ProfileArgs {
    #[command(subcommand)]
    action: Option<ProfileAction>,
}

/// The `profile` sub-actions, as real clap subcommands (ADR-101).
#[derive(clap::Subcommand)]
enum ProfileAction {
    /// List declared profiles, with the active one marked.
    List,
    /// Declare a profile and create its package dir.
    Add {
        /// Profile name to declare.
        name: String,
        /// Human description of the profile.
        #[arg(long)]
        desc: Option<String>,
        /// Hostname match glob (e.g. `vm-*`) for fleet resolution.
        #[arg(long = "match")]
        match_pattern: Option<String>,
    },
    /// Drop the registry entry and strip it from entry tags.
    #[command(alias = "rm")]
    Remove {
        /// Profile name to remove.
        name: String,
        /// Also delete the profile's `packages/<name>/` lists (destructive).
        #[arg(long)]
        purge: bool,
    },
    /// Copy memberships and/or package lists from one profile to another.
    #[command(alias = "cp")]
    Copy {
        /// Source profile to copy from.
        src: String,
        /// Destination profile (must already be declared).
        dst: String,
        /// Copy only this entry's membership to the destination.
        #[arg(long)]
        only: Option<String>,
        /// Copy dotfile memberships (entries tagged with the source).
        #[arg(long)]
        dotfiles: bool,
        /// Copy package lists; optionally one source (native|aur|flatpak).
        #[arg(long, num_args = 0..=1, default_missing_value = "all")]
        pkg: Option<String>,
    },
    /// Record the active profile in `.dotfiles-profile`.
    Use {
        /// Profile name to activate (must be declared).
        name: String,
    },
}

/// Dispatch the `profile` verb. A missing sub-action defaults to `list`.
pub fn run(ctx: &Ctx, args: &ProfileArgs) -> anyhow::Result<()> {
    match args.action.as_ref() {
        None | Some(ProfileAction::List) => list(ctx),
        Some(ProfileAction::Add { name, desc, match_pattern }) => {
            add(ctx, name, desc.as_deref(), match_pattern.as_deref())
        }
        Some(ProfileAction::Remove { name, purge }) => remove(ctx, name, *purge),
        Some(ProfileAction::Copy { src, dst, only, dotfiles, pkg }) => {
            copy(ctx, src, dst, only.as_deref(), *dotfiles, pkg.as_deref())
        }
        Some(ProfileAction::Use { name }) => use_profile(ctx, name),
    }
}

fn read_src(ctx: &Ctx) -> anyhow::Result<String> {
    std::fs::read_to_string(&ctx.manifest)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", ctx.manifest.display()))
}

/// `profile list` — declared profiles with the active one marked.
fn list(ctx: &Ctx) -> anyhow::Result<()> {
    let manifest = ctx.load()?;
    if manifest.profiles.is_empty() {
        let p = &ctx.profile;
        println!("No profiles declared yet.");
        println!(
            "'{p}' is active implicitly (derived from the hostname). To declare it, run `dotfiles profile add {p}`."
        );
        return Ok(());
    }
    let mut t = Table::new()
        .title("Profiles")
        .column("NAME", Align::Left)
        .column("MATCH", Align::Left)
        .column("DESCRIPTION", Align::Left);
    for (name, p) in &manifest.profiles {
        let name_cell = if *name == ctx.profile {
            cell(format!("● {name}")).fg(table::GREEN)
        } else {
            cell(format!("  {name}"))
        };
        t.row(vec![
            name_cell,
            cell(p.match_pattern.clone().unwrap_or_default()),
            cell(p.description.clone().unwrap_or_default()),
        ]);
    }
    t.print();
    println!("\nActive profile: {}", table::paint(&ctx.profile, table::GREEN));
    if !manifest.profiles.contains_key(&ctx.profile) {
        println!("(not a declared profile — used implicitly; `dotfiles profile add {}` to declare it)", ctx.profile);
    }
    Ok(())
}

/// `profile add <name>` — declare a profile and create its package dir.
fn add(ctx: &Ctx, name: &str, desc: Option<&str>, match_pattern: Option<&str>) -> anyhow::Result<()> {
    let mut doc = edit::parse(&read_src(ctx)?)?;
    edit::add_profile(&mut doc, name, desc, match_pattern)
        .map_err(|e| anyhow::anyhow!(e))?;
    std::fs::write(&ctx.manifest, doc.to_string())?;
    std::fs::create_dir_all(ctx.repo_root.join("packages").join(name))?;
    println!("added profile '{name}'");
    Ok(())
}

/// `profile remove <name>` — drop the registry entry and strip it from entry
/// tags. Keeps the profile's `packages/<name>/` lists unless `--purge`. Deployed
/// files are always left intact.
fn remove(ctx: &Ctx, name: &str, purge: bool) -> anyhow::Result<()> {
    let src = read_src(ctx)?;
    let manifest = Manifest::from_toml(&src)?;
    let pkg_dir = ctx.repo_root.join("packages").join(name);
    if !manifest.profiles.contains_key(name) && !pkg_dir.is_dir() {
        anyhow::bail!("profile '{name}' not found");
    }
    let mut doc = edit::parse(&src)?;
    edit::remove_profile(&mut doc, name);
    std::fs::write(&ctx.manifest, doc.to_string())?;

    let note = if pkg_dir.is_dir() {
        if purge {
            std::fs::remove_dir_all(&pkg_dir)?;
            " (+ its package lists, purged)"
        } else {
            " — its packages/ lists were kept (use --purge to delete)"
        }
    } else {
        ""
    };
    println!("removed profile '{name}'{note}; entry tags stripped, deployed files left intact.");
    Ok(())
}

/// `profile copy <src> <dst> [--only E|--dotfiles|--pkg [source]]` — copy
/// memberships and/or package lists from one profile to another. With no flags,
/// copies everything. The destination must already be declared.
fn copy(ctx: &Ctx, src: &str, dst: &str, only: Option<&str>, dotfiles: bool, pkg: Option<&str>) -> anyhow::Result<()> {
    if src == dst {
        anyhow::bail!("source and destination profiles are the same ('{src}')");
    }
    let text = read_src(ctx)?;
    let manifest = Manifest::from_toml(&text)?;
    if !manifest.profiles.contains_key(dst) {
        anyhow::bail!("destination profile '{dst}' is not declared — add it first");
    }

    let all = only.is_none() && !dotfiles && pkg.is_none();
    let do_dotfiles = all || dotfiles || only.is_some();
    let do_pkg = all || pkg.is_some();

    let mut tagged = 0;
    if do_dotfiles {
        let mut doc = edit::parse(&text)?;
        if let Some(entry) = only {
            if !edit::add_entry_profile(&mut doc, entry, dst) {
                anyhow::bail!("entry '{entry}' not found in the manifest");
            }
            tagged = 1;
        } else {
            // Copy explicit memberships only (universal entries stay universal).
            for e in &manifest.entries {
                if e.profiles.iter().any(|p| p == src)
                    && edit::add_entry_profile(&mut doc, &e.name, dst)
                {
                    tagged += 1;
                }
            }
        }
        std::fs::write(&ctx.manifest, doc.to_string())?;
    }

    let mut copied_pkgs = 0;
    if do_pkg {
        let from = ctx.repo_root.join("packages").join(src);
        let to = ctx.repo_root.join("packages").join(dst);
        std::fs::create_dir_all(&to)?;
        for s in pkg_sources(pkg) {
            let f = from.join(format!("{s}.txt"));
            if f.is_file() {
                std::fs::copy(&f, to.join(format!("{s}.txt")))?;
                copied_pkgs += 1;
            }
        }
    }

    println!("copied {src} -> {dst}: {tagged} dotfile membership(s), {copied_pkgs} package list(s).");
    Ok(())
}

/// Which package sources a `--pkg [source]` value selects.
fn pkg_sources(arg: Option<&str>) -> Vec<&'static str> {
    match arg {
        Some("native") => vec!["native"],
        Some("aur") => vec!["aur"],
        Some("flatpak") => vec!["flatpak"],
        _ => vec!["native", "aur", "flatpak"],
    }
}

/// `profile use <name>` — record the active profile in `.dotfiles-profile`.
fn use_profile(ctx: &Ctx, name: &str) -> anyhow::Result<()> {
    let manifest = ctx.load()?;
    if !manifest.profiles.contains_key(name) {
        anyhow::bail!("profile '{name}' is not declared — add it first with `profile add {name}`");
    }
    let path: &Path = &ctx.repo_root;
    std::fs::write(path.join(".dotfiles-profile"), format!("{name}\n"))?;
    println!("active profile set to '{name}' (wrote .dotfiles-profile).");
    Ok(())
}
