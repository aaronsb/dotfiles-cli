//! `dotfiles claude` — read and write Claude Code's user-scope settings.
//!
//! Config lives as **numbered JSON fragments** (the zshrc `conf.d` shape) under
//! `<store>/claude/settings.d/`, merged in filename order, with an optional
//! per-profile overlay under `profiles/<profile>/`. The compiled slice is the
//! *owned* portion of `~/.claude/settings.json`; [`dotfiles_core::settings_project`]
//! projects it in via a three-way merge that preserves every foreign key
//! (`/config` toggles, in-situ runtime writes, agent-ways' baseline).
//!
//! The projector runs standalone (`dotfiles claude project`) or is orchestrated
//! by `dotfiles deploy`.

use crate::Ctx;
use clap::{Args, Subcommand};
use dotfiles_core::settings_merge::OwnedSlice;
use dotfiles_core::settings_project as sp;
use serde_json::{Map, Value, json};
use std::path::PathBuf;

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
    /// Set a managed config item, then project (e.g. `set model opus`, or
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

/// Read every `*.json` fragment in a directory, sorted by filename.
fn read_fragments(dir: &std::path::Path) -> anyhow::Result<Vec<Value>> {
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
        out.push(v);
    }
    Ok(out)
}

/// Compile the store into the owned slice: L2 universal fragments in filename
/// order, then the L3 per-profile overlay. Returns the merged `ours` value.
fn load_store(ctx: &Ctx) -> anyhow::Result<Value> {
    let dir = store_dir(ctx);
    let mut acc = Map::new();
    for frag in read_fragments(&dir)? {
        merge_fragment(&mut acc, &frag);
    }
    // L3: per-profile overlay wins over universal.
    let profile_dir = dir.join("profiles").join(&ctx.profile);
    for frag in read_fragments(&profile_dir)? {
        merge_fragment(&mut acc, &frag);
    }
    Ok(Value::Object(acc))
}

/// Merge one fragment into the accumulator: last-wins on scalars/objects, but
/// `permissions.allow` / `permissions.deny` union (they are shared concat-lists).
fn merge_fragment(acc: &mut Map<String, Value>, frag: &Value) {
    let Some(obj) = frag.as_object() else { return };
    for (key, val) in obj {
        if key == "permissions" {
            let perms = acc.entry("permissions").or_insert_with(|| json!({}));
            merge_permissions(perms, val);
        } else {
            acc.insert(key.clone(), val.clone());
        }
    }
}

fn merge_permissions(acc: &mut Value, frag: &Value) {
    if !acc.is_object() {
        *acc = json!({});
    }
    let acc = acc.as_object_mut().expect("just ensured object");
    let Some(obj) = frag.as_object() else { return };
    for (key, val) in obj {
        if (key == "allow" || key == "deny") && val.is_array() {
            let entry = acc.entry(key.clone()).or_insert_with(|| json!([]));
            let list = entry.as_array_mut().expect("array");
            for item in val.as_array().unwrap() {
                if !list.contains(item) {
                    list.push(item.clone());
                }
            }
        } else {
            acc.insert(key.clone(), val.clone());
        }
    }
}

/// Derive the owned slice spanning **both** the currently-declared config and the
/// prior base. Covering base keys is essential: a key the operator just removed
/// from the store is gone from `ours` but still recorded in `base`, and the merge
/// only drops (relinquishes) a key it still counts as owned. Top-level keys are
/// exclusive; `permissions.allow`/`permissions.deny` are shared additive-union
/// lists.
fn slice_over(ours: &Value, base: &Value) -> OwnedSlice {
    let mut slice = OwnedSlice::default();
    let mut seen_excl = std::collections::BTreeSet::new();
    let mut seen_list = std::collections::BTreeSet::new();
    for source in [ours, base] {
        let Some(obj) = source.as_object() else { continue };
        for key in obj.keys() {
            if key == "permissions" {
                let Some(perms) = obj["permissions"].as_object() else { continue };
                for sub in ["allow", "deny"] {
                    if perms.contains_key(sub) {
                        let path = format!("permissions.{sub}");
                        if seen_list.insert(path.clone()) {
                            slice.union_lists.push(path);
                        }
                    }
                }
            } else if seen_excl.insert(key.clone()) {
                slice.exclusive.push(key.clone());
            }
        }
    }
    slice
}

/// The owned slice of the currently-declared config alone (no base) — for `show`,
/// which reports what is managed now, not what is being relinquished.
fn derive_slice(ours: &Value) -> OwnedSlice {
    slice_over(ours, &Value::Null)
}

fn get(ctx: &Ctx, key: &str) -> anyhow::Result<()> {
    let live = sp::read_json_or_empty(&sp::settings_path(&ctx.home)).map_err(|e| anyhow::anyhow!(e))?;
    match sp::get(&live, key) {
        Some(v) => println!("{}", serde_json::to_string_pretty(&v)?),
        None => {
            println!("{key}: not set");
        }
    }
    Ok(())
}

fn show(ctx: &Ctx) -> anyhow::Result<()> {
    let ours = load_store(ctx)?;
    let slice = derive_slice(&ours);
    let live = sp::read_json_or_empty(&sp::settings_path(&ctx.home)).map_err(|e| anyhow::anyhow!(e))?;
    println!("Managed Claude settings — profile: {}", ctx.profile);
    println!("  store: {}", store_dir(ctx).display());
    println!("  file:  {}", sp::settings_path(&ctx.home).display());
    println!();
    let owned_keys: Vec<String> =
        slice.exclusive.iter().cloned().chain(slice.union_lists.iter().cloned()).collect();
    if owned_keys.is_empty() {
        println!("  (no managed keys — add fragments under the store dir, or `dotfiles claude set`)");
        return Ok(());
    }
    for key in owned_keys {
        let declared = sp::get(&ours, &key);
        let livev = sp::get(&live, &key);
        let synced = declared == livev;
        let mark = if synced { "=" } else { "≠" };
        println!("  {mark} {key}");
        if let Some(d) = declared {
            println!("      declared: {}", compact(&d));
        }
        println!("      live:     {}", livev.map(|v| compact(&v)).unwrap_or_else(|| "(unset)".into()));
    }
    Ok(())
}

/// A one-line JSON rendering for the `show` table.
fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

fn project(ctx: &Ctx, dry_run: bool) -> anyhow::Result<()> {
    let ours = load_store(ctx)?;
    let settings = sp::settings_path(&ctx.home);
    let base_path = sp::base_path(&ctx.home);
    // Span the slice over ours + the prior base so a removed key is relinquished,
    // not silently left behind.
    let base = sp::read_json_or_empty(&base_path).map_err(|e| anyhow::anyhow!(e))?;
    let slice = slice_over(&ours, &base);
    let out = sp::project(&slice, &ours, &settings, &base_path, dry_run).map_err(|e| anyhow::anyhow!(e))?;
    if dry_run {
        if out.changed {
            println!("would update {} (dry-run)", settings.display());
        } else {
            println!("{} already up to date", settings.display());
        }
    } else if out.changed {
        println!("updated {}", settings.display());
    } else {
        println!("{} already up to date", settings.display());
    }
    Ok(())
}

fn set(ctx: &Ctx, key: &str, value: &str) -> anyhow::Result<()> {
    // Parse the value as JSON, falling back to a bare string.
    let parsed: Value = serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()));
    let frag_path = manual_fragment(ctx);
    let mut frag = sp::read_json_or_empty(&frag_path).map_err(|e| anyhow::anyhow!(e))?;
    set_dotted(&mut frag, key, Some(parsed));
    sp::atomic_write_json(&frag_path, &frag).map_err(|e| anyhow::anyhow!(e))?;
    println!("set {key} in {}", frag_path.display());
    project(ctx, false)
}

fn unset(ctx: &Ctx, key: &str) -> anyhow::Result<()> {
    let frag_path = manual_fragment(ctx);
    let mut frag = sp::read_json_or_empty(&frag_path).map_err(|e| anyhow::anyhow!(e))?;
    set_dotted(&mut frag, key, None);
    sp::atomic_write_json(&frag_path, &frag).map_err(|e| anyhow::anyhow!(e))?;
    println!("unset {key} in {}", frag_path.display());
    project(ctx, false)
}

/// Set (or, with `None`, remove) a dotted key in a JSON object, creating
/// intermediate objects and pruning emptied ones on removal.
fn set_dotted(root: &mut Value, dotted: &str, value: Option<Value>) {
    if !root.is_object() {
        *root = json!({});
    }
    let segs: Vec<&str> = dotted.split('.').collect();
    set_dotted_inner(root.as_object_mut().unwrap(), &segs, value);
}

fn set_dotted_inner(obj: &mut Map<String, Value>, segs: &[&str], value: Option<Value>) {
    let (head, rest) = segs.split_first().expect("non-empty path");
    if rest.is_empty() {
        match value {
            Some(v) => {
                obj.insert(head.to_string(), v);
            }
            None => {
                obj.remove(*head);
            }
        }
        return;
    }
    if value.is_none() && !obj.contains_key(*head) {
        return; // nothing to remove
    }
    let child = obj.entry(head.to_string()).or_insert_with(|| json!({}));
    if !child.is_object() {
        *child = json!({});
    }
    let child_obj = child.as_object_mut().unwrap();
    set_dotted_inner(child_obj, rest, value);
    if child_obj.is_empty() {
        obj.remove(*head);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_fragment_unions_permissions_and_last_wins_scalars() {
        let mut acc = Map::new();
        merge_fragment(&mut acc, &json!({ "model": "opus", "permissions": { "allow": ["a"] } }));
        merge_fragment(&mut acc, &json!({ "model": "sonnet", "permissions": { "allow": ["b"] } }));
        let v = Value::Object(acc);
        assert_eq!(v["model"], "sonnet"); // last wins
        assert_eq!(v["permissions"]["allow"], json!(["a", "b"])); // union
    }

    #[test]
    fn derive_slice_classifies_keys() {
        let ours = json!({
            "statusLine": {}, "env": {},
            "permissions": { "allow": ["a"], "deny": ["b"] }
        });
        let s = derive_slice(&ours);
        assert!(s.exclusive.contains(&"statusLine".to_string()));
        assert!(s.exclusive.contains(&"env".to_string()));
        assert!(!s.exclusive.contains(&"permissions".to_string()));
        assert!(s.union_lists.contains(&"permissions.allow".to_string()));
        assert!(s.union_lists.contains(&"permissions.deny".to_string()));
    }

    #[test]
    fn slice_over_covers_base_only_keys() {
        // Regression: a key the operator just removed is absent from `ours` but
        // still in `base`; the slice must include it so the merge relinquishes it.
        let ours = json!({ "permissions": { "allow": ["Bash(dotfiles:*)"] } });
        let base = json!({ "statusLine": { "command": "s.sh" }, "permissions": { "deny": ["x"] } });
        let s = slice_over(&ours, &base);
        assert!(s.exclusive.contains(&"statusLine".to_string()), "base-only exclusive covered");
        assert!(s.union_lists.contains(&"permissions.allow".to_string()));
        assert!(s.union_lists.contains(&"permissions.deny".to_string()), "base-only list covered");
        // And no duplicates when a key is in both.
        let both = slice_over(&ours, &ours);
        assert_eq!(both.union_lists.iter().filter(|p| *p == "permissions.allow").count(), 1);
    }

    #[test]
    fn set_and_unset_dotted() {
        let mut v = json!({});
        set_dotted(&mut v, "permissions.allow", Some(json!(["x"])));
        assert_eq!(v["permissions"]["allow"], json!(["x"]));
        set_dotted(&mut v, "model", Some(json!("opus")));
        assert_eq!(v["model"], "opus");
        set_dotted(&mut v, "permissions.allow", None);
        // permissions became empty -> pruned entirely.
        assert!(v.get("permissions").is_none());
        assert_eq!(v["model"], "opus");
    }
}
