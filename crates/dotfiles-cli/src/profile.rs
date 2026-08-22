//! The `profile` verb — manage profiles, named scopes over dotfiles + packages
//! (one per machine or role). Backed by the `[profiles]` registry and per-entry
//! `profiles` tags in the manifest, per-entry `paths.<profile>` content variants
//! (ADR-011), plus per-profile `packages/<profile>/` dirs.
//!
//! Every operation addresses a **ref** — `<profile>` for the whole scope or
//! `<profile>/<entry>` for one item — so compare (`diff`), write (`push`/
//! `pull`), and delete (`remove`) share one grammar.

use crate::diff_view;
use crate::table::{self, Align, Table, cell};
use crate::{Ctx, commands};
use dotfiles_core::{Entry, Manifest, deploy, edit};
use std::path::Path;

/// `profile [list|add|remove|diff|push|pull|copy|use] …`. A missing action
/// defaults to `list`.
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
    /// Drop a profile, or one entry's variant: `<profile>[/<entry>]`.
    #[command(alias = "rm")]
    Remove {
        /// What to remove: a profile, or `<profile>/<entry>`.
        target: String,
        /// Also delete the files it owns (package lists / variant content).
        #[arg(long)]
        purge: bool,
    },
    /// Compare profiles: no args = all, 1 = active vs it, 2 = A vs B.
    Diff {
        /// Refs to compare; qualify one (`slab/nvim`) to narrow to an entry.
        #[arg(num_args = 0..=2)]
        refs: Vec<String>,
        /// Render the content diff of every entry that differs.
        #[arg(long, short)]
        details: bool,
    },
    /// Copy content/scope from one ref onto another: `push <src> <dst>`.
    Push {
        /// Source ref: `<profile>` or `<profile>/<entry>`.
        src: String,
        /// Destination ref (must already be a declared profile).
        dst: String,
        #[command(flatten)]
        opts: TransferOpts,
    },
    /// The inverse of push — `pull <src> [<dst>]`, dst defaulting to active.
    Pull {
        /// Source ref: `<profile>` or `<profile>/<entry>`.
        src: String,
        /// Destination ref (default: the active profile).
        dst: Option<String>,
        #[command(flatten)]
        opts: TransferOpts,
    },
    /// Copy memberships and/or package lists — whole-profile `push` (ADR-011 §6).
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
    /// Record the active profile in the host binding.
    Use {
        /// Profile name to activate (must be declared).
        name: String,
    },
}

/// Flags shared by `push` and `pull` — the two directions of one transfer.
#[derive(clap::Args, Clone)]
struct TransferOpts {
    /// Overwrite the destination's existing variant (destructive).
    #[arg(long, short)]
    force: bool,
    /// Store a newly created variant at this repo path instead of the default.
    #[arg(long = "as")]
    as_path: Option<String>,
    /// Whole-profile only: also copy package lists (native|aur|flatpak).
    #[arg(long, num_args = 0..=1, default_missing_value = "all")]
    pkg: Option<String>,
}

/// Dispatch the `profile` verb. A missing sub-action defaults to `list`.
pub fn run(ctx: &Ctx, args: &ProfileArgs) -> anyhow::Result<()> {
    match args.action.as_ref() {
        None | Some(ProfileAction::List) => list(ctx),
        Some(ProfileAction::Add {
            name,
            desc,
            match_pattern,
        }) => add(ctx, name, desc.as_deref(), match_pattern.as_deref()),
        Some(ProfileAction::Remove { target, purge }) => remove(ctx, target, *purge),
        Some(ProfileAction::Diff { refs, details }) => diff(ctx, refs, *details),
        Some(ProfileAction::Push { src, dst, opts }) => transfer(ctx, src, dst, opts),
        Some(ProfileAction::Pull { src, dst, opts }) => {
            let dst = dst.clone().unwrap_or_else(|| ctx.profile.clone());
            transfer(ctx, src, &dst, opts)
        }
        Some(ProfileAction::Copy {
            src,
            dst,
            only,
            dotfiles,
            pkg,
        }) => copy(ctx, src, dst, only.as_deref(), *dotfiles, pkg.as_deref()),
        Some(ProfileAction::Use { name }) => use_profile(ctx, name),
    }
}

// --- refs -----------------------------------------------------------------

/// A profile ref: a whole profile, or one entry within it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ref {
    profile: String,
    entry: Option<String>,
}

impl std::fmt::Display for Ref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.entry {
            Some(e) => write!(f, "{}/{e}", self.profile),
            None => f.write_str(&self.profile),
        }
    }
}

/// Parse `<profile>` or `<profile>/<entry>`. A trailing slash is the bare form.
fn parse_ref(s: &str) -> anyhow::Result<Ref> {
    let (profile, entry) = match s.split_once('/') {
        Some((p, e)) => (p.trim(), e.trim()),
        None => (s.trim(), ""),
    };
    if profile.is_empty() {
        anyhow::bail!("'{s}' names no profile — expected <profile> or <profile>/<entry>");
    }
    if entry.contains('/') {
        anyhow::bail!("'{s}' has too many parts — a ref is <profile> or <profile>/<entry>");
    }
    Ok(Ref {
        profile: profile.to_string(),
        entry: (!entry.is_empty()).then(|| entry.to_string()),
    })
}

fn read_src(ctx: &Ctx) -> anyhow::Result<String> {
    std::fs::read_to_string(&ctx.manifest)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", ctx.manifest.display()))
}

/// Look an entry up by name, or fail with a pointer to `list`.
fn find<'a>(manifest: &'a Manifest, name: &str) -> anyhow::Result<&'a Entry> {
    manifest
        .entries
        .iter()
        .find(|e| e.name == name)
        .ok_or_else(|| anyhow::anyhow!("no managed dotfile named '{name}' — try `dotfiles list`"))
}

/// `profile list` — declared profiles with the active one marked.
fn list(ctx: &Ctx) -> anyhow::Result<()> {
    let manifest = ctx.load_raw()?;
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
        .column("ENTRIES", Align::Right)
        .column("VARIANTS", Align::Right)
        .column("DESCRIPTION", Align::Left);
    for (name, p) in &manifest.profiles {
        let name_cell = if *name == ctx.profile {
            cell(format!("● {name}")).fg(table::GREEN)
        } else {
            cell(format!("  {name}"))
        };
        let entries = manifest
            .entries
            .iter()
            .filter(|e| e.active_in(name))
            .count();
        let variants = manifest
            .entries
            .iter()
            .filter(|e| e.has_variant(name))
            .count();
        t.row(vec![
            name_cell,
            cell(p.match_pattern.clone().unwrap_or_default()),
            cell(entries.to_string()),
            cell(if variants == 0 {
                String::new()
            } else {
                variants.to_string()
            }),
            cell(p.description.clone().unwrap_or_default()),
        ]);
    }
    t.print();
    println!(
        "\nActive profile: {}",
        table::paint(&ctx.profile, table::GREEN)
    );
    if !manifest.profiles.contains_key(&ctx.profile) {
        println!(
            "(not a declared profile — used implicitly; `dotfiles profile add {}` to declare it)",
            ctx.profile
        );
    }
    Ok(())
}

/// `profile add <name>` — declare a profile and create its package dir.
fn add(
    ctx: &Ctx,
    name: &str,
    desc: Option<&str>,
    match_pattern: Option<&str>,
) -> anyhow::Result<()> {
    let mut doc = edit::parse(&read_src(ctx)?)?;
    edit::add_profile(&mut doc, name, desc, match_pattern).map_err(|e| anyhow::anyhow!(e))?;
    std::fs::write(&ctx.manifest, doc.to_string())?;
    std::fs::create_dir_all(ctx.repo_root.join("packages").join(name))?;
    println!("added profile '{name}'");
    Ok(())
}

/// `profile remove <ref> [--purge]`.
///
/// A bare profile drops the registry entry, its per-entry tags, and its variant
/// declarations. A qualified ref drops just that entry's variant for the profile
/// (falling back to base) — or, when there is no variant, its membership tag.
/// Deployed files are always left intact.
fn remove(ctx: &Ctx, target: &str, purge: bool) -> anyhow::Result<()> {
    let r = parse_ref(target)?;
    match &r.entry {
        Some(entry) => remove_item(ctx, &r.profile, entry, purge),
        None => remove_profile(ctx, &r.profile, purge),
    }
}

fn remove_profile(ctx: &Ctx, name: &str, purge: bool) -> anyhow::Result<()> {
    let src = read_src(ctx)?;
    let manifest = Manifest::from_toml(&src)?;
    let pkg_dir = ctx.repo_root.join("packages").join(name);
    if !manifest.profiles.contains_key(name) && !pkg_dir.is_dir() {
        anyhow::bail!("profile '{name}' not found");
    }
    // Variants keyed to a profile that is going away can never resolve again;
    // name them, since --purge is about to decide their content's fate.
    let variants: Vec<(String, String)> = manifest
        .entries
        .iter()
        .filter_map(|e| e.paths.get(name).map(|p| (e.name.clone(), p.clone())))
        .collect();

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
    if !variants.is_empty() {
        println!("{} variant declaration(s) dropped:", variants.len());
        for (entry, path) in &variants {
            let fate = if purge {
                match purge_path(&ctx.repo_root.join(path)) {
                    Ok(()) => "purged",
                    Err(_) => "could not delete",
                }
            } else {
                "content kept"
            };
            println!("    {entry}: {path} ({fate})");
        }
    }
    Ok(())
}

/// `profile remove <profile>/<entry>` — drop the variant, else the membership.
fn remove_item(ctx: &Ctx, profile: &str, entry: &str, purge: bool) -> anyhow::Result<()> {
    let src = read_src(ctx)?;
    let manifest = Manifest::from_toml(&src)?;
    let e = find(&manifest, entry)?;
    let mut doc = edit::parse(&src)?;

    if let Some(variant) = e.paths.get(profile).cloned() {
        edit::remove_entry_path(&mut doc, entry, profile);
        std::fs::write(&ctx.manifest, doc.to_string())?;
        println!(
            "'{entry}' in '{profile}' now falls back to the base path '{}'.",
            e.path
        );
        if purge {
            // Only ever delete a path that nothing else still points at.
            if variant == e.path || e.paths.iter().any(|(p, v)| p != profile && *v == variant) {
                println!("kept {variant} — another profile still resolves to it.");
            } else {
                purge_path(&ctx.repo_root.join(&variant))?;
                println!("purged {variant}.");
            }
        } else {
            println!("kept {variant} (use --purge to delete it).");
        }
        return Ok(());
    }

    if edit::remove_entry_profile(&mut doc, entry, profile) {
        std::fs::write(&ctx.manifest, doc.to_string())?;
        let m = Manifest::from_toml(&doc.to_string())?;
        let now = find(&m, entry)?;
        let scope = if now.profiles.is_empty() {
            " — it has no profile tags left, so it is universal again".to_string()
        } else {
            format!(" — still in: {}", now.profiles.join(", "))
        };
        println!("untagged '{entry}' from profile '{profile}'{scope}. Deployed files left intact.");
        return Ok(());
    }

    anyhow::bail!(
        "'{profile}' has neither a variant nor an explicit membership for '{entry}' — nothing to remove \
         (it is {})",
        if e.active_in(profile) {
            "universal, so active everywhere"
        } else {
            "not in that profile"
        }
    )
}

/// Delete a file or directory tree under the store.
fn purge_path(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

// --- diff -----------------------------------------------------------------

/// Where one profile's copy of an entry lives, and whether it has one at all.
enum Side {
    /// Not in this profile's scope.
    Absent,
    /// The entry's base path.
    Base(String),
    /// A `paths.<profile>` override.
    Variant(String),
}

impl Side {
    fn of(e: &Entry, profile: &str) -> Self {
        if !e.active_in(profile) {
            Side::Absent
        } else if e.has_variant(profile) {
            Side::Variant(e.path_for(profile).to_string())
        } else {
            Side::Base(e.path.clone())
        }
    }

    fn path(&self) -> Option<&str> {
        match self {
            Side::Absent => None,
            Side::Base(p) | Side::Variant(p) => Some(p),
        }
    }

    fn label(&self) -> (&'static str, &'static str) {
        match self {
            Side::Absent => ("—", table::DIM),
            Side::Base(_) => ("base", table::DIM),
            Side::Variant(_) => ("variant", table::CYAN),
        }
    }
}

/// `profile diff [<a>] [<b>] [--details]`.
fn diff(ctx: &Ctx, refs: &[String], details: bool) -> anyhow::Result<()> {
    let manifest = ctx.load_raw()?;
    let parsed: Vec<Ref> = refs
        .iter()
        .map(|s| parse_ref(s))
        .collect::<Result<_, _>>()?;
    match parsed.len() {
        0 => {
            if details {
                anyhow::bail!(
                    "--details needs two profiles to compare — e.g. `profile diff slab --details`"
                );
            }
            matrix(ctx, &manifest)
        }
        1 => pair(
            ctx,
            &manifest,
            &Ref {
                profile: ctx.profile.clone(),
                entry: None,
            },
            &parsed[0],
            details,
        ),
        2 => pair(ctx, &manifest, &parsed[0], &parsed[1], details),
        n => anyhow::bail!("expected at most two refs to compare, got {n}"),
    }
}

/// No-arg `diff` — one row per entry, one column per declared profile, so the
/// whole fleet's shape is visible at once.
fn matrix(ctx: &Ctx, manifest: &Manifest) -> anyhow::Result<()> {
    let mut names: Vec<String> = manifest.profiles.keys().cloned().collect();
    if names.is_empty() {
        anyhow::bail!("no profiles declared — `dotfiles profile add <name>` first");
    }
    if !names.contains(&ctx.profile) {
        names.push(ctx.profile.clone());
    }
    let mut t = Table::new()
        .title("Profiles — entry coverage")
        .column("ENTRY", Align::Left);
    for n in &names {
        let header = if *n == ctx.profile {
            format!("● {n}")
        } else {
            n.clone()
        };
        t = t.column(header, Align::Left);
    }
    for e in &manifest.entries {
        let mut row = vec![if e.enabled {
            cell(&e.name)
        } else {
            cell(format!("{} (disabled)", e.name)).fg(table::DIM)
        }];
        for n in &names {
            let (label, color) = Side::of(e, n).label();
            row.push(cell(label).fg(color));
        }
        t.row(row);
    }
    t.print();
    println!(
        "\n{}",
        table::paint(
            "base = the entry's shared path · variant = its own copy · — = not in that profile",
            table::DIM
        )
    );
    println!("Compare two directly: `dotfiles profile diff <a> <b> [--details]`.");
    Ok(())
}

/// Two-profile comparison, optionally narrowed to one entry by a qualified ref.
fn pair(ctx: &Ctx, manifest: &Manifest, a: &Ref, b: &Ref, details: bool) -> anyhow::Result<()> {
    if a.profile == b.profile {
        anyhow::bail!("'{}' compared against itself", a.profile);
    }
    let only = match (&a.entry, &b.entry) {
        (Some(x), Some(y)) if x != y => {
            anyhow::bail!(
                "refs name different entries ('{x}' vs '{y}') — a diff compares one entry across profiles"
            )
        }
        (Some(x), _) | (_, Some(x)) => Some(x.clone()),
        (None, None) => None,
    };
    if let Some(name) = &only {
        find(manifest, name)?;
    }

    let mut t = Table::new()
        .title(format!("{} ↔ {}", a.profile, b.profile))
        .column("ENTRY", Align::Left)
        .column(a.profile.clone(), Align::Left)
        .column(b.profile.clone(), Align::Left)
        .column("STATE", Align::Left);

    let mut differing: Vec<(String, String, String)> = Vec::new();
    let (mut same, mut skipped) = (0u32, 0u32);

    for e in &manifest.entries {
        if only.as_deref().is_some_and(|n| n != e.name) {
            continue;
        }
        let (sa, sb) = (Side::of(e, &a.profile), Side::of(e, &b.profile));
        let (pa, pb) = (sa.path(), sb.path());
        if pa.is_none() && pb.is_none() {
            skipped += 1;
            continue;
        }
        let (state, color) = match (pa, pb) {
            (Some(_), None) => (format!("only in {}", a.profile), table::YELLOW),
            (None, Some(_)) => (format!("only in {}", b.profile), table::YELLOW),
            (Some(x), Some(y)) if x == y => {
                same += 1;
                ("same (shared path)".to_string(), table::GREEN)
            }
            (Some(x), Some(y)) => {
                let (xa, ya) = (ctx.repo_root.join(x), ctx.repo_root.join(y));
                match (xa.exists(), ya.exists()) {
                    (false, _) => (format!("missing: {x}"), table::RED),
                    (_, false) => (format!("missing: {y}"), table::RED),
                    _ => match numstat(&ctx.repo_root, x, y)? {
                        None => {
                            same += 1;
                            ("same".to_string(), table::GREEN)
                        }
                        Some((add, del)) => {
                            differing.push((e.name.clone(), x.to_string(), y.to_string()));
                            (format!("differs (+{add} -{del})"), table::YELLOW)
                        }
                    },
                }
            }
            (None, None) => unreachable!("handled above"),
        };
        let (la, ca) = sa.label();
        let (lb, cb) = sb.label();
        t.row(vec![
            cell(&e.name),
            cell(la).fg(ca),
            cell(lb).fg(cb),
            cell(state).fg(color),
        ]);
    }
    t.print();

    let mut summary = format!("{same} identical · {} differing", differing.len());
    if skipped > 0 {
        summary.push_str(&format!(" · {skipped} in neither profile"));
    }
    println!("\n{}", table::paint(&summary, table::DIM));
    println!(
        "{}",
        table::paint(
            "Package lists are compared separately: `dotfiles pkg diff <a> <b>`.",
            table::DIM
        )
    );

    if details {
        for (name, x, y) in &differing {
            println!(
                "\n{}",
                table::paint(
                    &format!("── {name}: {} → {} ──", a.profile, b.profile),
                    table::BOLD
                )
            );
            print!("{}", diff_view::render(&raw_diff(&ctx.repo_root, x, y)?));
        }
    } else if !differing.is_empty() {
        println!("Run again with --details to see what differs.");
    }
    Ok(())
}

/// Added/removed line counts between two store paths, or `None` when identical.
/// Binary or unmergeable content reports as a difference with zero counts.
fn numstat(repo: &Path, a: &str, b: &str) -> anyhow::Result<Option<(u64, u64)>> {
    let out = commands::git_stdout(repo, &["diff", "--no-index", "--numstat", "--", a, b])?;
    if out.trim().is_empty() {
        return Ok(None);
    }
    let (mut add, mut del) = (0u64, 0u64);
    for line in out.lines() {
        let mut cols = line.split('\t');
        add += cols.next().and_then(|c| c.parse::<u64>().ok()).unwrap_or(0);
        del += cols.next().and_then(|c| c.parse::<u64>().ok()).unwrap_or(0);
    }
    Ok(Some((add, del)))
}

/// The unified diff between two store paths, for the friendly renderer.
fn raw_diff(repo: &Path, a: &str, b: &str) -> anyhow::Result<String> {
    commands::git_stdout(repo, &["diff", "--no-index", "--", a, b])
}

// --- push / pull ----------------------------------------------------------

/// `push <src> <dst>` (and `pull`, which is the same with the ends swapped).
/// Bare refs move the whole scope; qualified refs move one entry's content.
fn transfer(ctx: &Ctx, src: &str, dst: &str, opts: &TransferOpts) -> anyhow::Result<()> {
    let (s, d) = (parse_ref(src)?, parse_ref(dst)?);
    if s.profile == d.profile && s.entry.is_none() {
        anyhow::bail!(
            "source and destination profiles are the same ('{}')",
            s.profile
        );
    }
    let manifest = ctx.load_raw()?;
    if !manifest.profiles.contains_key(&d.profile) {
        anyhow::bail!(
            "destination profile '{}' is not declared — add it first",
            d.profile
        );
    }
    match (&s.entry, &d.entry) {
        (Some(e), Some(f)) if e != f => anyhow::bail!(
            "cannot push '{e}' onto '{f}' — an entry's variants all belong to the same catalog row; \
             push '{e}' to '{}' instead",
            d.profile
        ),
        (Some(entry), _) => push_item(ctx, &s.profile, &d.profile, entry, opts),
        (None, _) => push_profile(ctx, &s.profile, &d.profile, opts),
    }
}

/// Move one entry's content from `src` to `dst`, creating or overwriting the
/// destination's variant (ADR-011 §4).
fn push_item(
    ctx: &Ctx,
    src: &str,
    dst: &str,
    entry: &str,
    opts: &TransferOpts,
) -> anyhow::Result<()> {
    let text = read_src(ctx)?;
    let manifest = Manifest::from_toml(&text)?;
    let e = find(&manifest, entry)?;

    let src_rel = e.path_for(src).to_string();
    let src_abs = ctx.repo_root.join(&src_rel);
    if !src_abs.exists() {
        anyhow::bail!("'{src}' resolves '{entry}' to {src_rel}, which is not in the store");
    }

    let existing = e.paths.get(dst).cloned();
    // Both ends on the shared base with nothing to fork: the two profiles are
    // the same bytes by construction, so a transfer would be theater. Drift a
    // machine has in its working tree is git's business, not the manifest's.
    if existing.is_none() && src != dst && src_rel == e.path {
        println!(
            "'{src}' and '{dst}' both resolve '{entry}' to the base path '{}' — same content, nothing to transfer.",
            e.path
        );
        println!(
            "{}",
            table::paint(
                "If this machine has drifted, that is an uncommitted edit — see `dotfiles diff --details`.",
                table::DIM
            )
        );
        return Ok(());
    }

    let dst_rel = opts
        .as_path
        .clone()
        .or(existing.clone())
        .unwrap_or_else(|| format!("{}-{dst}", e.path));
    guard_destination(&manifest, e, dst, &dst_rel)?;
    let dst_abs = ctx.repo_root.join(&dst_rel);

    if dst_abs.exists() {
        match numstat(&ctx.repo_root, &src_rel, &dst_rel)? {
            None => {
                println!(
                    "'{dst}' already has '{entry}' identical to '{src}' at {dst_rel} — nothing to do."
                );
                return Ok(());
            }
            Some((add, del)) if !opts.force => {
                println!(
                    "'{dst}' already has a variant of '{entry}' at {dst_rel} (+{add} -{del} vs '{src}'):\n"
                );
                print!(
                    "{}",
                    diff_view::render(&raw_diff(&ctx.repo_root, &dst_rel, &src_rel)?)
                );
                anyhow::bail!("refusing to overwrite without --force");
            }
            Some(_) => purge_path(&dst_abs)?,
        }
    }

    deploy::copy_tree(&src_abs, &dst_abs)
        .map_err(|err| anyhow::anyhow!("copying {src_rel} → {dst_rel}: {err}"))?;

    let mut doc = edit::parse(&text)?;
    edit::set_entry_path(&mut doc, entry, dst, &dst_rel);
    // Content in a profile that would not deploy it is never what was meant.
    let tagged = if e.profiles.is_empty() || e.profiles.iter().any(|p| p == dst) {
        false
    } else {
        edit::add_entry_profile(&mut doc, entry, dst)
    };
    std::fs::write(&ctx.manifest, doc.to_string())?;

    let verb = if existing.is_some() {
        "overwrote"
    } else {
        "created"
    };
    println!("{verb} '{dst}' variant of '{entry}': {dst_rel} (from '{src}' at {src_rel})");
    if tagged {
        println!("tagged '{entry}' into profile '{dst}' so it deploys there.");
    }
    if src == dst {
        let note = format!(
            "'{dst}' now has its own copy — the base path is free to change without following it."
        );
        println!("{}", table::paint(&note, table::DIM));
    }
    println!("Run `dotfiles deploy` on '{dst}' to pick it up.");
    Ok(())
}

/// Refuse destinations that would quietly rewrite content another profile is
/// still resolving to — the base path, or someone else's variant.
fn guard_destination(
    manifest: &Manifest,
    e: &Entry,
    dst: &str,
    dst_rel: &str,
) -> anyhow::Result<()> {
    if dst_rel.trim().is_empty() || Path::new(dst_rel).is_absolute() {
        anyhow::bail!("variant path '{dst_rel}' must be a relative path inside the store");
    }
    if dst_rel.split('/').any(|c| c == "..") {
        anyhow::bail!("variant path '{dst_rel}' must stay inside the store");
    }
    if dst_rel == e.path {
        anyhow::bail!(
            "'{dst_rel}' is the base path of entry '{}' — writing there would change every profile that \
             resolves to it. Edit the base directly and commit, or pick another path with --as.",
            e.name
        );
    }
    if let Some((other, _)) = e
        .paths
        .iter()
        .find(|(p, v)| p.as_str() != dst && v.as_str() == dst_rel)
    {
        anyhow::bail!(
            "'{dst_rel}' is already '{other}'s variant of '{}' — pick another path with --as",
            e.name
        );
    }
    if let Some(other) = manifest
        .entries
        .iter()
        .find(|o| o.name != e.name && o.path == dst_rel)
    {
        anyhow::bail!(
            "'{dst_rel}' is the base path of entry '{}' — pick another path with --as",
            other.name
        );
    }
    Ok(())
}

/// Whole-profile push: memberships, package lists, and every variant the source
/// owns. Entries whose destination variant already differs are reported and
/// skipped unless `--force`.
fn push_profile(ctx: &Ctx, src: &str, dst: &str, opts: &TransferOpts) -> anyhow::Result<()> {
    copy(
        ctx,
        src,
        dst,
        None,
        true,
        opts.pkg.as_deref().or(Some("all")),
    )?;

    let manifest = ctx.load_raw()?;
    let with_variants: Vec<String> = manifest
        .entries
        .iter()
        .filter(|e| e.has_variant(src))
        .map(|e| e.name.clone())
        .collect();
    if with_variants.is_empty() {
        return Ok(());
    }
    println!("\n{} entry variant(s) to carry over:", with_variants.len());
    for name in &with_variants {
        // Each item re-reads the manifest, so earlier writes are visible.
        if let Err(err) = push_item(ctx, src, dst, name, opts) {
            println!("  {name}: skipped — {err}");
        }
    }
    Ok(())
}

/// `profile copy <src> <dst> [--only E|--dotfiles|--pkg [source]]` — copy
/// memberships and/or package lists from one profile to another. With no flags,
/// copies everything. The destination must already be declared.
fn copy(
    ctx: &Ctx,
    src: &str,
    dst: &str,
    only: Option<&str>,
    dotfiles: bool,
    pkg: Option<&str>,
) -> anyhow::Result<()> {
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

    println!(
        "copied {src} -> {dst}: {tagged} dotfile membership(s), {copied_pkgs} package list(s)."
    );
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

/// `profile use <name>` — record the active profile in the host binding
/// (ADR-013 §3). A host that has not been through `init` gets the legacy
/// `.dotfiles-profile` file and a pointer at `init`.
fn use_profile(ctx: &Ctx, name: &str) -> anyhow::Result<()> {
    let manifest = ctx.load_raw()?;
    if !manifest.profiles.contains_key(name) {
        anyhow::bail!("profile '{name}' is not declared — add it first with `profile add {name}`");
    }
    match ctx.binding.clone() {
        Some(mut b) if b.store.is_some() && ctx.bound => {
            b.store.as_mut().unwrap().profile = Some(name.to_string());
            b.save(&ctx.binding_path).map_err(|e| anyhow::anyhow!(e))?;
            println!(
                "active profile set to '{name}' (wrote {}).",
                ctx.binding_path.display()
            );
        }
        _ => {
            std::fs::write(ctx.repo_root.join(".dotfiles-profile"), format!("{name}\n"))?;
            println!("active profile set to '{name}' (wrote .dotfiles-profile).");
            println!("hint: run `dotfiles init` to move this host onto a binding.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_and_qualified_refs() {
        let bare = parse_ref("north").unwrap();
        assert_eq!(bare.profile, "north");
        assert!(bare.entry.is_none());

        let qualified = parse_ref("north/zsh").unwrap();
        assert_eq!(qualified.profile, "north");
        assert_eq!(qualified.entry.as_deref(), Some("zsh"));
        assert_eq!(qualified.to_string(), "north/zsh");

        // A trailing slash is just the bare form.
        assert!(parse_ref("north/").unwrap().entry.is_none());
        // Surrounding whitespace is tolerated.
        assert_eq!(
            parse_ref(" north / zsh ").unwrap().entry.as_deref(),
            Some("zsh")
        );
    }

    #[test]
    fn rejects_malformed_refs() {
        assert!(parse_ref("").is_err(), "empty");
        assert!(parse_ref("/zsh").is_err(), "no profile");
        assert!(parse_ref("north/zsh/extra").is_err(), "too many parts");
    }

    fn entry_with(name: &str, path: &str, profiles: &[&str], variants: &[(&str, &str)]) -> Entry {
        Entry {
            name: name.into(),
            path: path.into(),
            target: format!(".{name}"),
            enabled: true,
            mode: Default::default(),
            why: None,
            spec: None,
            profiles: profiles.iter().map(|s| s.to_string()).collect(),
            paths: variants
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn side_reports_provenance_per_profile() {
        let e = entry_with("nvim", "nvim", &["north", "slab"], &[("slab", "nvim-slab")]);
        assert!(matches!(Side::of(&e, "north"), Side::Base(_)));
        assert_eq!(Side::of(&e, "north").path(), Some("nvim"));
        assert!(matches!(Side::of(&e, "slab"), Side::Variant(_)));
        assert_eq!(Side::of(&e, "slab").path(), Some("nvim-slab"));
        // Scoped entry, uninvolved profile.
        assert!(matches!(Side::of(&e, "cube"), Side::Absent));
        assert_eq!(Side::of(&e, "cube").path(), None);
        // A universal entry is present in every profile.
        let u = entry_with("tmux", "tmux/.tmux.conf", &[], &[]);
        assert!(matches!(Side::of(&u, "anything"), Side::Base(_)));
    }

    #[test]
    fn destination_guard_refuses_shared_paths() {
        let e = entry_with("nvim", "nvim", &[], &[("slab", "nvim-slab")]);
        let other = entry_with("zsh", "zsh/.zshrc", &[], &[]);
        let manifest = Manifest {
            entries: vec![e.clone(), other],
            profiles: Default::default(),
            store: Default::default(),
        };

        // The happy path: a fresh, store-relative path nobody else uses.
        assert!(guard_destination(&manifest, &e, "cube", "nvim-cube").is_ok());
        // The base path — would change every profile resolving to it.
        assert!(guard_destination(&manifest, &e, "cube", "nvim").is_err());
        // Another profile's variant of the same entry.
        assert!(guard_destination(&manifest, &e, "cube", "nvim-slab").is_err());
        // Overwriting a profile's own variant is exactly what --force is for.
        assert!(guard_destination(&manifest, &e, "slab", "nvim-slab").is_ok());
        // Another entry's base path.
        assert!(guard_destination(&manifest, &e, "cube", "zsh/.zshrc").is_err());
        // Escapes and absolutes.
        assert!(guard_destination(&manifest, &e, "cube", "../outside").is_err());
        assert!(guard_destination(&manifest, &e, "cube", "/etc/nvim").is_err());
        assert!(guard_destination(&manifest, &e, "cube", "").is_err());
    }
}
