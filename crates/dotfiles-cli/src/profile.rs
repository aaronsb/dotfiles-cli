//! The `profile` verb — manage profiles, named scopes over dotfiles + packages
//! (one per machine or role). Backed by the `[profiles]` registry and per-entry
//! `profiles` tags in the manifest, plus per-profile `packages/<profile>/` dirs.

use crate::Ctx;
use crate::table::{self, Align, Table, cell};
use dotfiles_core::{Manifest, edit};
use std::path::Path;

/// `profile [list|add|remove|copy|use] [name...] [flags]`. Default action `list`.
#[derive(clap::Args)]
pub struct ProfileArgs {
    /// Action: `list`, `add`, `remove`, `copy`, `use`.
    #[arg(default_value = "list")]
    action: String,
    /// Positional names: `<name>` for add/remove/use, `<src> <dst>` for copy.
    #[arg(trailing_var_arg = true)]
    names: Vec<String>,
    /// `add`: human description of the profile.
    #[arg(long)]
    desc: Option<String>,
    /// `add`: hostname match glob (e.g. `vm-*`) for fleet resolution.
    #[arg(long = "match")]
    match_pattern: Option<String>,
    /// `copy`: copy only this entry's membership to the destination.
    #[arg(long)]
    only: Option<String>,
    /// `copy`: copy dotfile memberships (entries tagged with the source).
    #[arg(long)]
    dotfiles: bool,
    /// `copy`: copy package lists; optionally one source (native|aur|flatpak).
    #[arg(long, num_args = 0..=1, default_missing_value = "all")]
    pkg: Option<String>,
}

/// Dispatch the `profile` verb.
pub fn run(ctx: &Ctx, args: &ProfileArgs) -> anyhow::Result<()> {
    match args.action.as_str() {
        "list" => list(ctx),
        "add" => add(ctx, args),
        "remove" | "rm" => remove(ctx, name_at(args, 0)?),
        "copy" | "cp" => copy(ctx, args),
        "use" => use_profile(ctx, name_at(args, 0)?),
        other => anyhow::bail!(
            "unknown profile subcommand '{other}' — try: list, add, remove, copy, use"
        ),
    }
}

fn name_at(args: &ProfileArgs, i: usize) -> anyhow::Result<&str> {
    args.names
        .get(i)
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!("expected a profile name"))
}

fn read_src(ctx: &Ctx) -> anyhow::Result<String> {
    std::fs::read_to_string(&ctx.manifest)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", ctx.manifest.display()))
}

/// `profile list` — declared profiles with the active one marked.
fn list(ctx: &Ctx) -> anyhow::Result<()> {
    let manifest = ctx.load()?;
    if manifest.profiles.is_empty() {
        println!("No profiles declared. Active: '{}' (implicit, from hostname).", ctx.profile);
        println!("Declare one with `dotfiles profile add <name>`.");
        return Ok(());
    }
    let mut t = Table::new()
        .title("Profiles")
        .column("", Align::Left)
        .column("NAME", Align::Left)
        .column("MATCH", Align::Left)
        .column("DESCRIPTION", Align::Left);
    for (name, p) in &manifest.profiles {
        let marker = if *name == ctx.profile {
            cell("●").fg(table::GREEN)
        } else {
            cell(" ")
        };
        t.row(vec![
            marker,
            cell(name),
            cell(p.match_pattern.clone().unwrap_or_default()),
            cell(p.description.clone().unwrap_or_default()),
        ]);
    }
    t.print();
    println!("\nActive profile: {}", table::paint(&ctx.profile, table::GREEN));
    Ok(())
}

/// `profile add <name>` — declare a profile and create its package dir.
fn add(ctx: &Ctx, args: &ProfileArgs) -> anyhow::Result<()> {
    let name = name_at(args, 0)?;
    let mut doc = edit::parse(&read_src(ctx)?)?;
    edit::add_profile(&mut doc, name, args.desc.as_deref(), args.match_pattern.as_deref())
        .map_err(|e| anyhow::anyhow!(e))?;
    std::fs::write(&ctx.manifest, doc.to_string())?;
    std::fs::create_dir_all(ctx.repo_root.join("packages").join(name))?;
    println!("added profile '{name}'");
    Ok(())
}

/// `profile remove <name>` — drop the registry entry, strip it from entry tags,
/// and remove its package dir. Deployed files are left intact.
fn remove(ctx: &Ctx, name: &str) -> anyhow::Result<()> {
    let src = read_src(ctx)?;
    let manifest = Manifest::from_toml(&src)?;
    let pkg_dir = ctx.repo_root.join("packages").join(name);
    if !manifest.profiles.contains_key(name) && !pkg_dir.is_dir() {
        anyhow::bail!("profile '{name}' not found");
    }
    let mut doc = edit::parse(&src)?;
    edit::remove_profile(&mut doc, name);
    std::fs::write(&ctx.manifest, doc.to_string())?;

    let mut note = "";
    if pkg_dir.is_dir() {
        std::fs::remove_dir_all(&pkg_dir)?;
        note = " (+ its package lists)";
    }
    println!("removed profile '{name}'{note} — entry tags stripped; deployed files left intact.");
    Ok(())
}

/// `profile copy <src> <dst> [--only E|--dotfiles|--pkg [source]]` — copy
/// memberships and/or package lists from one profile to another. With no flags,
/// copies everything. The destination must already be declared.
fn copy(ctx: &Ctx, args: &ProfileArgs) -> anyhow::Result<()> {
    let src = name_at(args, 0)?;
    let dst = name_at(args, 1)?;
    if src == dst {
        anyhow::bail!("source and destination profiles are the same ('{src}')");
    }
    let text = read_src(ctx)?;
    let manifest = Manifest::from_toml(&text)?;
    if !manifest.profiles.contains_key(dst) {
        anyhow::bail!("destination profile '{dst}' is not declared — add it first");
    }

    let all = args.only.is_none() && !args.dotfiles && args.pkg.is_none();
    let do_dotfiles = all || args.dotfiles || args.only.is_some();
    let do_pkg = all || args.pkg.is_some();

    let mut tagged = 0;
    if do_dotfiles {
        let mut doc = edit::parse(&text)?;
        if let Some(entry) = &args.only {
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
        for s in pkg_sources(args.pkg.as_deref()) {
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
