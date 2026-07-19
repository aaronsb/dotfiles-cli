//! `dotfiles claude` — read and write Claude Code's user-scope settings.
//!
//! Config lives as **numbered JSON fragments** (the zshrc `conf.d` shape) under
//! `<store>/claude/settings.d/`, deep-merged in filename order with an optional
//! per-profile overlay under `profiles/<profile>/`. The compiled slice is the
//! *owned* portion of `~/.claude/settings.json`; [`dotfiles_core::settings_project`]
//! projects it in via a three-way merge that preserves every foreign key
//! (`/config` toggles, in-situ runtime writes, agent-ways' baseline).
//!
//! The projector runs standalone (`dotfiles claude project`) or is orchestrated
//! by `dotfiles deploy`.

use crate::Ctx;
use clap::{Args, Subcommand};
use dotfiles_core::settings_merge::{self, OwnedSlice};
use dotfiles_core::settings_project as sp;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The shared concat-list paths — additive-union, not whole-object owned. Every
/// other leaf a fragment declares is owned exclusively.
/// The shared concat-list paths — additive-union, not whole-object owned. These
/// are the permission rule/dir lists that several writers contribute to.
const UNION_LISTS: [&str; 4] = [
    "permissions.allow",
    "permissions.deny",
    "permissions.ask",
    "permissions.additionalDirectories",
];

fn is_union_list(path: &str) -> bool {
    UNION_LISTS.contains(&path)
}

#[derive(Args)]
pub struct ClaudeArgs {
    #[command(subcommand)]
    command: ClaudeCmd,
}

#[derive(Subcommand)]
enum ClaudeCmd {
    /// Read a config item from the live settings (e.g. `permissions.allow`).
    Get { key: String },
    /// Show the managed (owned) config and its live values.
    Show,
    /// Project the config store into ~/.claude/settings.json.
    Project {
        /// Show what would change without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Set a managed config item, then project (e.g. `set env.FOO bar`, or
    /// `set permissions.allow '["Bash(git:*)"]'`). Value is parsed as JSON,
    /// falling back to a string.
    Set { key: String, value: String },
    /// Unset a managed config item (removes it from the manual fragment), then
    /// project — the projector drops the key from settings.json.
    Unset { key: String },
}

pub fn run(ctx: &Ctx, args: &ClaudeArgs) -> anyhow::Result<()> {
    match &args.command {
        ClaudeCmd::Get { key } => get(ctx, key),
        ClaudeCmd::Show => show(ctx),
        ClaudeCmd::Project { dry_run } => project(ctx, *dry_run),
        ClaudeCmd::Set { key, value } => set(ctx, key, value),
        ClaudeCmd::Unset { key } => unset(ctx, key),
    }
}

/// The store directory: `<repo>/claude/settings.d/`.
fn store_dir(ctx: &Ctx) -> PathBuf {
    ctx.repo_root.join("claude").join("settings.d")
}

/// Projection as a `dotfiles deploy` step (the orchestrated mode): a no-op unless
/// the repo carries a `claude/settings.d` store. Prints a small section header.
pub fn deploy_step(ctx: &Ctx, dry_run: bool) -> anyhow::Result<()> {
    if !store_dir(ctx).exists() {
        return Ok(());
    }
    println!("\n=== Claude settings ===");
    project(ctx, dry_run)
}

/// The manual-edits fragment that `set` / `unset` author.
fn manual_fragment(ctx: &Ctx) -> PathBuf {
    store_dir(ctx).join("50-manual.json")
}

/// Read every `*.json` fragment in a directory, sorted by filename, paired with
/// its path (for validation error messages).
fn read_fragments(dir: &Path) -> anyhow::Result<Vec<(PathBuf, Value)>> {
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(anyhow::anyhow!("reading {}: {e}", dir.display())),
    };
    paths.sort();
    let mut out = Vec::new();
    for p in paths {
        let v = sp::read_json_or_empty(&p).map_err(|e| anyhow::anyhow!(e))?;
        out.push((p, v));
    }
    Ok(out)
}

/// Structural fragment validation: it must be a JSON object, and any shared
/// concat-list it declares must be an array (so union handling is well-formed).
/// Foreign-object/array clobbering is caught by the projector's structural guard
/// against the *live* document, so no brittle key allowlist is needed here.
fn validate_fragment(frag: &Value, path: &Path) -> anyhow::Result<()> {
    if !frag.is_object() {
        anyhow::bail!("{}: fragment must be a JSON object (got {})", path.display(), kind(frag));
    }
    for ul in UNION_LISTS {
        if let Some(v) = sp::get(frag, ul)
            && !v.is_array()
        {
            anyhow::bail!("{}: {ul} must be a JSON array (got {})", path.display(), kind(&v));
        }
    }
    Ok(())
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Compile the store into the owned config: L2 universal fragments in filename
/// order, then the L3 per-profile overlay, deep-merged. Returns the merged `ours`.
fn load_store(ctx: &Ctx) -> anyhow::Result<Value> {
    let dir = store_dir(ctx);
    let mut acc = Map::new();
    for (path, frag) in read_fragments(&dir)? {
        validate_fragment(&frag, &path)?;
        if let Some(obj) = frag.as_object() {
            deep_merge(&mut acc, obj, "");
        }
    }
    // L3: per-profile overlay wins over universal.
    let profile_dir = dir.join("profiles").join(&ctx.profile);
    for (path, frag) in read_fragments(&profile_dir)? {
        validate_fragment(&frag, &path)?;
        if let Some(obj) = frag.as_object() {
            deep_merge(&mut acc, obj, "");
        }
    }
    Ok(Value::Object(acc))
}

/// Deep-merge `overlay` into `acc`: objects recurse (so a partial fragment adds a
/// sub-key without wiping its siblings), the shared concat-lists union, and every
/// other leaf is last-wins.
fn deep_merge(acc: &mut Map<String, Value>, overlay: &Map<String, Value>, prefix: &str) {
    for (key, val) in overlay {
        let path = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
        if is_union_list(&path) {
            // Validated as an array upstream; union it in.
            let entry = acc.entry(key.clone()).or_insert_with(|| json!([]));
            if let (Some(list), Some(items)) = (entry.as_array_mut(), val.as_array()) {
                for item in items {
                    if !list.contains(item) {
                        list.push(item.clone());
                    }
                }
            }
        } else if let Some(vo) = val.as_object() {
            let child = acc.entry(key.clone()).or_insert_with(|| json!({}));
            if !child.is_object() {
                *child = json!({});
            }
            deep_merge(child.as_object_mut().expect("just ensured object"), vo, &path);
        } else {
            acc.insert(key.clone(), val.clone());
        }
    }
}

/// Collect the dotted **leaf** paths of a config value — recursing into objects,
/// skipping the shared concat-lists (handled as union lists).
fn leaf_paths(value: &Value, prefix: &str, out: &mut Vec<String>) {
    let Some(obj) = value.as_object() else { return };
    for (key, val) in obj {
        let path = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
        if is_union_list(&path) {
            continue;
        }
        if val.is_object() {
            leaf_paths(val, &path, out);
        } else {
            out.push(path); // scalar or non-union array = an owned leaf
        }
    }
}

/// Derive the owned slice spanning **both** the currently-declared config and the
/// prior base. Covering base leaves is essential: a leaf the operator just removed
/// is absent from `ours` but still in `base`, and the merge only relinquishes a
/// leaf it still counts as owned.
fn slice_over(ours: &Value, base: &Value) -> OwnedSlice {
    let mut exclusive = Vec::new();
    let mut seen = BTreeSet::new();
    for source in [ours, base] {
        let mut paths = Vec::new();
        leaf_paths(source, "", &mut paths);
        for p in paths {
            if seen.insert(p.clone()) {
                exclusive.push(p);
            }
        }
    }
    let mut union_lists = Vec::new();
    for ul in UNION_LISTS {
        if sp::get(ours, ul).is_some() || sp::get(base, ul).is_some() {
            union_lists.push(ul.to_string());
        }
    }
    OwnedSlice { exclusive, union_lists }
}

/// The owned slice of the currently-declared config alone (no base) — for `show`.
fn derive_slice(ours: &Value) -> OwnedSlice {
    slice_over(ours, &Value::Null)
}

fn get(ctx: &Ctx, key: &str) -> anyhow::Result<()> {
    let live =
        sp::read_json_or_empty(&sp::settings_path(&ctx.home)).map_err(|e| anyhow::anyhow!(e))?;
    match sp::get(&live, key) {
        Some(v) => println!("{}", serde_json::to_string_pretty(&v)?),
        None => println!("{key}: not set"),
    }
    Ok(())
}

fn show(ctx: &Ctx) -> anyhow::Result<()> {
    let ours = load_store(ctx)?;
    let slice = derive_slice(&ours);
    let live =
        sp::read_json_or_empty(&sp::settings_path(&ctx.home)).map_err(|e| anyhow::anyhow!(e))?;
    println!("Managed Claude settings — profile: {}", ctx.profile);
    println!("  store: {}", store_dir(ctx).display());
    println!("  file:  {}", sp::settings_path(&ctx.home).display());
    println!();
    if slice.exclusive.is_empty() && slice.union_lists.is_empty() {
        println!("  (no managed keys — add fragments under the store dir, or `dotfiles claude set`)");
        return Ok(());
    }
    for path in &slice.exclusive {
        let declared = sp::get(&ours, path);
        let livev = sp::get(&live, path);
        // A leaf is synced when its live value equals what we declare.
        print_row(declared == livev, path, declared, livev);
    }
    for path in &slice.union_lists {
        let declared = sp::get(&ours, path);
        let livev = sp::get(&live, path);
        // A shared list is synced when every declared entry is present live (the
        // live list also carries foreign entries, so equality would never hold).
        let synced = match (&declared, &livev) {
            (Some(Value::Array(d)), Some(Value::Array(l))) => d.iter().all(|e| l.contains(e)),
            (None, _) => true,
            _ => false,
        };
        print_row(synced, path, declared, livev);
    }
    Ok(())
}

fn print_row(synced: bool, key: &str, declared: Option<Value>, live: Option<Value>) {
    println!("  {} {key}", if synced { "=" } else { "≠" });
    if let Some(d) = declared {
        println!("      declared: {}", compact(&d));
    }
    println!(
        "      live:     {}",
        live.map(|v| compact(&v)).unwrap_or_else(|| "(unset)".into())
    );
}

/// A one-line JSON rendering for the `show` table.
fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

fn project(ctx: &Ctx, dry_run: bool) -> anyhow::Result<()> {
    let ours = load_store(ctx)?;
    project_loaded(ctx, &ours, dry_run)
}

/// Project an already-compiled store (so callers that just loaded it — `unset` —
/// don't re-read every fragment).
fn project_loaded(ctx: &Ctx, ours: &Value, dry_run: bool) -> anyhow::Result<()> {
    let settings = sp::settings_path(&ctx.home);
    let base_path = sp::base_path(&ctx.home);
    // Read the base once here — both to span the slice over ours + base (so a
    // removed leaf is relinquished) and to hand to the projector.
    let base = sp::read_json_or_empty(&base_path).map_err(|e| anyhow::anyhow!(e))?;
    let slice = slice_over(ours, &base);
    let out = sp::project(&slice, ours, &settings, &base, &base_path, dry_run)
        .map_err(|e| anyhow::anyhow!(e))?;
    match (dry_run, out.changed) {
        (true, true) => println!("would update {} (dry-run)", settings.display()),
        (false, true) => println!("updated {}", settings.display()),
        (_, false) => println!("{} already up to date", settings.display()),
    }
    Ok(())
}

fn set(ctx: &Ctx, key: &str, value: &str) -> anyhow::Result<()> {
    // Parse the value as JSON, falling back to a bare string.
    let parsed: Value =
        serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()));
    // A shared concat-list must be an array — otherwise the merge would treat our
    // contribution as empty and strip every previously-owned entry.
    if is_union_list(key) && !parsed.is_array() {
        anyhow::bail!(
            "{key} is a list — pass a JSON array, e.g. '[\"Bash(git:*)\"]' (got {})",
            kind(&parsed)
        );
    }
    let frag_path = manual_fragment(ctx);
    let mut frag = sp::read_json_or_empty(&frag_path).map_err(|e| anyhow::anyhow!(e))?;
    let message = if is_union_list(key) {
        // Lists are additive: union the new entries into what the manual fragment
        // already declares, so successive `set`s accumulate rather than replace.
        // (To replace or drop entries, edit the fragment or `unset` the whole key.)
        let mut entries = sp::get(&frag, key).and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let mut n = 0;
        for e in parsed.as_array().cloned().unwrap_or_default() {
            if !entries.contains(&e) {
                entries.push(e);
                n += 1;
            }
        }
        set_dotted(&mut frag, key, Some(Value::Array(entries)));
        format!("added {n} entr{} to {key}", if n == 1 { "y" } else { "ies" })
    } else {
        set_dotted(&mut frag, key, Some(parsed));
        format!("set {key}")
    };
    // Validate structure before persisting (a malformed list errors without leaving
    // a bad fragment on disk), then write-and-project transactionally.
    validate_fragment(&frag, &frag_path)?;
    write_and_project(ctx, &frag_path, &frag, &message)
}

fn unset(ctx: &Ctx, key: &str) -> anyhow::Result<()> {
    let frag_path = manual_fragment(ctx);
    let mut frag = sp::read_json_or_empty(&frag_path).map_err(|e| anyhow::anyhow!(e))?;
    set_dotted(&mut frag, key, None);
    validate_fragment(&frag, &frag_path)?;
    let previous = std::fs::read(&frag_path).ok();
    sp::atomic_write_json(&frag_path, &frag).map_err(|e| anyhow::anyhow!(e))?;
    // Compile the store once, reused for the declared-elsewhere check and the
    // projection. `unset` only edits the manual fragment; if another fragment still
    // declares the key, it stays managed. Say so rather than implying removal.
    let ours = load_store(ctx)?;
    if sp::get(&ours, key).is_some() {
        println!("note: {key} is still declared in another fragment — it remains managed");
    }
    match project_loaded(ctx, &ours, false) {
        Ok(()) => {
            println!("unset {key} in {}", frag_path.display());
            Ok(())
        }
        Err(e) => {
            rollback_fragment(previous, &frag_path);
            Err(e)
        }
    }
}

/// Persist `frag` to `frag_path`, then project. If the projection is refused (e.g.
/// it would clobber foreign structure) or fails, roll the fragment back to its
/// prior content — so a bad edit never lingers on disk to wedge later `deploy`s.
fn write_and_project(
    ctx: &Ctx,
    frag_path: &Path,
    frag: &Value,
    done_msg: &str,
) -> anyhow::Result<()> {
    let previous = std::fs::read(frag_path).ok();
    sp::atomic_write_json(frag_path, frag).map_err(|e| anyhow::anyhow!(e))?;
    match project(ctx, false) {
        Ok(()) => {
            println!("{done_msg}");
            Ok(())
        }
        Err(e) => {
            rollback_fragment(previous, frag_path);
            Err(e)
        }
    }
}

/// Restore a fragment to its prior bytes, or remove it if it did not exist before.
fn rollback_fragment(previous: Option<Vec<u8>>, path: &Path) {
    match previous {
        Some(bytes) => {
            std::fs::write(path, bytes).ok();
        }
        None => {
            std::fs::remove_file(path).ok();
        }
    }
}

/// Set (or, with `None`, remove) a dotted key in a JSON object, reusing the core
/// path helpers so the descend/prune logic lives in one place.
fn set_dotted(root: &mut Value, dotted: &str, value: Option<Value>) {
    if !root.is_object() {
        *root = json!({});
    }
    let obj = root.as_object_mut().expect("just ensured object");
    match value {
        Some(v) => settings_merge::set_path(obj, dotted, v),
        None => settings_merge::remove_path(obj, dotted),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_merge_recurses_objects_and_unions_permissions() {
        // Regression (#1): a partial env fragment must not wipe sibling env vars.
        let mut acc = Map::new();
        deep_merge(&mut acc, json!({ "env": { "A": "1" }, "permissions": { "allow": ["a"] } }).as_object().unwrap(), "");
        deep_merge(&mut acc, json!({ "env": { "B": "2" }, "permissions": { "allow": ["b"] } }).as_object().unwrap(), "");
        let v = Value::Object(acc);
        assert_eq!(v["env"]["A"], "1", "sibling preserved");
        assert_eq!(v["env"]["B"], "2");
        assert_eq!(v["permissions"]["allow"], json!(["a", "b"]));
    }

    #[test]
    fn leaf_paths_recurse_and_skip_union_lists() {
        let ours = json!({
            "statusLine": { "command": "s" }, "env": { "A": "1", "B": "2" },
            "permissions": { "allow": ["x"], "defaultMode": "acceptEdits" }
        });
        let mut paths = Vec::new();
        leaf_paths(&ours, "", &mut paths);
        assert!(paths.contains(&"statusLine.command".to_string()));
        assert!(paths.contains(&"env.A".to_string()) && paths.contains(&"env.B".to_string()));
        // Regression (#7): permissions.defaultMode is an owned leaf, not dropped.
        assert!(paths.contains(&"permissions.defaultMode".to_string()));
        assert!(!paths.iter().any(|p| p == "permissions.allow"), "allow is a union list, not a leaf");
    }

    #[test]
    fn slice_over_covers_base_only_leaves() {
        let ours = json!({ "permissions": { "allow": ["Bash(dotfiles:*)"] } });
        let base = json!({ "statusLine": { "command": "s.sh" }, "permissions": { "deny": ["x"] } });
        let s = slice_over(&ours, &base);
        assert!(s.exclusive.contains(&"statusLine.command".to_string()), "base-only leaf covered");
        assert!(s.union_lists.contains(&"permissions.allow".to_string()));
        assert!(s.union_lists.contains(&"permissions.deny".to_string()));
    }

    #[test]
    fn validate_fragment_checks_structure() {
        // A non-object fragment is rejected.
        assert!(validate_fragment(&json!([1, 2]), Path::new("f.json")).is_err());
        // Every shared concat-list must be an array (else union handling breaks).
        assert!(validate_fragment(&json!({ "permissions": { "allow": "x" } }), Path::new("f.json")).is_err());
        assert!(validate_fragment(&json!({ "permissions": { "ask": "x" } }), Path::new("f.json")).is_err());
        assert!(validate_fragment(&json!({ "permissions": { "allow": ["ok"], "ask": ["y"] } }), Path::new("f.json")).is_ok());
        // A scalar at an object-typed key is NOT a validate error — it is caught at
        // project time by the structural guard against the live document.
        assert!(validate_fragment(&json!({ "env": "x" }), Path::new("f.json")).is_ok());
    }

    #[test]
    fn set_and_unset_dotted() {
        let mut v = json!({});
        set_dotted(&mut v, "env.FOO", Some(json!("bar")));
        assert_eq!(v["env"]["FOO"], "bar");
        set_dotted(&mut v, "env.BAZ", Some(json!("qux")));
        set_dotted(&mut v, "env.FOO", None);
        assert!(v["env"].get("FOO").is_none());
        assert_eq!(v["env"]["BAZ"], "qux", "sibling survives removal");
    }
}
