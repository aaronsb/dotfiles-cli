//! I/O and orchestration for the Claude `~/.claude/settings.json` projector
//! (ADR-010). Reads the live settings and the host-local last-applied base, runs
//! the pure three-way [`merge`](crate::settings_merge::merge), verifies the
//! self-audit invariant (the projection touched nothing outside our owned slice),
//! and writes the result atomically.
//!
//! The projector is usable two ways (Aaron's framing): standalone — one command
//! that reads the store and reconciles the file — or orchestrated as a step of
//! `dotfiles deploy`. Both call [`project`].
//!
//! Errors are `String` messages (the core crate's convention — see
//! [`first_run_gate`](crate::first_run_gate)); the CLI wraps them into `anyhow`.

use crate::settings_merge::{OwnedSlice, merge, owned_union, stripped_user_view};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// The live user-scope settings file: `$CLAUDE_CONFIG_DIR/settings.json` if set,
/// else `~/.claude/settings.json`.
pub fn settings_path(home: &Path) -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir).join("settings.json"),
        None => home.join(".claude").join("settings.json"),
    }
}

/// The host-local last-applied base: `$XDG_STATE_HOME/dotfiles/…` else
/// `~/.local/state/dotfiles/…`. Gitignored, per-host, never carried in the repo
/// (the artifact/config split, ADR-010 / agent-ways ADR-163).
pub fn base_path(home: &Path) -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local").join("state"));
    state.join("dotfiles").join("claude-settings-base.json")
}

/// Read a JSON object file, returning an empty object if the file is absent or
/// blank. Errors only on a present-but-unparseable file.
pub fn read_json_or_empty(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(Value::Object(Map::new())),
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("parsing {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(e) => Err(format!("reading {}: {e}", path.display())),
    }
}

/// Write `value` to `path` atomically (temp file + rename), creating parents.
/// Pretty-printed with a trailing newline, matching Claude Code's own format.
pub fn atomic_write_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    let mut text =
        serde_json::to_string_pretty(value).map_err(|e| format!("serializing settings: {e}"))?;
    text.push('\n');
    // A pid-suffixed temp neighbour keeps the rename on the same filesystem.
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("renaming into {}: {e}", path.display()))?;
    Ok(())
}

/// The outcome of a projection.
pub struct Projection {
    /// The merged settings document.
    pub settings: Value,
    /// The new last-applied base to persist.
    pub base: Value,
    /// Whether `settings` differs from the live file (i.e. a write is warranted).
    pub changed: bool,
}

/// Merge `ours` into `live` and verify the self-audit invariant. Pure: no I/O.
///
/// The invariant: the user's portion of the document — everything outside our
/// owned slice — must be identical before and after. Both sides are stripped by
/// the *union* of the prior base and `ours`, so a key that entered or left the
/// slice this run (an adopt or a relinquish) is removed from both and does not
/// trip a false alarm. A real perturbation of a foreign key is a hard error: we
/// refuse to write rather than risk clobbering the operator's config.
pub fn project_value(
    slice: &OwnedSlice,
    live: &Value,
    ours: &Value,
    base: &Value,
) -> Result<Projection, String> {
    // Refuse a non-object live document. Collapsing an array/scalar to `{}` and
    // writing our object over it would destroy the operator's file, and the
    // self-audit (which strips both sides to `{}`) could not see the loss.
    if !live.is_object() {
        return Err(
            "live settings.json is not a JSON object — refusing to overwrite it; fix or remove the file first"
                .to_string(),
        );
    }

    // Structural guard (the drift-proof root fix): the self-audit is blind to
    // foreign data destroyed *at a path we own*, because it strips that path from
    // both the before and after views. So refuse, up front, to overwrite structure
    // we did not write. It is **base-aware**: our own prior value (live == base) or
    // an already-equal value (live == ours) is safe, so idempotent re-projection is
    // never blocked — only genuinely foreign structure is.
    let live_obj = live.as_object().expect("checked object above");
    let ours_obj = ours.as_object().cloned().unwrap_or_default();
    let base_obj = base.as_object().cloned().unwrap_or_default();
    use crate::settings_merge::get_path;
    for path in &slice.exclusive {
        let Some(ours_v) = get_path(&ours_obj, path) else { continue }; // not asserting
        let Some(live_v) = get_path(live_obj, path) else { continue }; // nothing to clobber
        if !live_v.is_object() && !live_v.is_array() {
            continue; // overwriting a scalar is fine
        }
        if live_v == ours_v || get_path(&base_obj, path) == Some(live_v) {
            continue; // already what we'd write, or our own prior value — safe
        }
        return Err(format!(
            "refusing to project: settings.json has {} at '{path}' that this tool did not write — overwriting it would lose foreign data; reconcile settings.json or the fragment",
            if live_v.is_object() { "an object" } else { "an array" }
        ));
    }
    // Union lists: a foreign NON-array at a list path cannot be safely unioned into
    // (the merge would read it as empty and overwrite it). Refuse rather than lose it.
    for path in &slice.union_lists {
        if let Some(live_v) = get_path(live_obj, path)
            && !live_v.is_array()
        {
            return Err(format!(
                "refusing to project: settings.json has a non-array at '{path}' — cannot merge a managed list into it; reconcile settings.json"
            ));
        }
    }

    let audit = owned_union(slice, base, ours);
    let before = stripped_user_view(slice, live, &audit);
    let m = merge(slice, live, ours, base);
    let after = stripped_user_view(slice, &m.settings, &audit);
    if before != after {
        return Err(
            "self-audit failed: projection would alter settings outside the owned slice — refusing to write"
                .to_string(),
        );
    }
    let changed = &m.settings != live;
    Ok(Projection { settings: m.settings, base: m.base, changed })
}

/// Full projection with I/O: read the live settings, merge against the caller-
/// supplied `base` + self-audit, and (unless `dry_run`) atomically write the
/// settings and persist the new base to `base_path`.
///
/// The caller passes `base` (already read, so it can derive the owned slice from
/// `ours` ∪ `base`) and `base_path` (where the new base is written). The live file
/// is backed up to a sibling `.json.bak` before an overwrite — and if that backup
/// cannot be written, the projection **aborts** rather than overwriting with no
/// recovery copy.
pub fn project(
    slice: &OwnedSlice,
    ours: &Value,
    settings_path: &Path,
    base: &Value,
    base_path: &Path,
    dry_run: bool,
) -> Result<Projection, String> {
    let live = read_json_or_empty(settings_path)?;
    let out = project_value(slice, &live, ours, base)?;
    if !dry_run {
        if out.changed {
            if settings_path.exists() {
                let bak = settings_path.with_extension("json.bak");
                std::fs::copy(settings_path, &bak)
                    .map_err(|e| format!("backing up {}: {e}", settings_path.display()))?;
            }
            atomic_write_json(settings_path, &out.settings)?;
        }
        // Persist the base even when nothing changed: on first run over a
        // pre-existing install this seeds the base from the adopted values. A base
        // write failure is non-fatal: settings.json (the source of truth) is already
        // written correctly, and the base re-syncs on the next successful run —
        // failing here would wrongly trip a caller's fragment rollback and orphan
        // the just-projected value.
        if &out.base != base {
            let _ = atomic_write_json(base_path, &out.base);
        }
    }
    Ok(out)
}

/// Read the config item at a dotted `path` (e.g. `permissions.allow`) from a
/// settings document. `None` if any segment is missing. Shares the traversal in
/// [`crate::settings_merge::get_path`].
pub fn get(settings: &Value, dotted: &str) -> Option<Value> {
    crate::settings_merge::get_path(settings.as_object()?, dotted).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn slice() -> OwnedSlice {
        // Leaf-path ownership (as the CLI derives): own statusLine.command, not the
        // whole statusLine object.
        OwnedSlice::new(&["statusLine.command"], &["permissions.allow", "permissions.deny"])
    }

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dft-proj-{}-{tag}", std::process::id()))
    }

    #[test]
    fn project_writes_settings_and_base_preserving_foreign() {
        let dir = tmp("write");
        std::fs::create_dir_all(&dir).unwrap();
        let sp = dir.join("settings.json");
        let bp = dir.join("base.json");
        // A live file with a foreign /config key and another tool's allow entry.
        atomic_write_json(&sp, &json!({
            "model": "opus",
            "permissions": { "allow": ["Bash(ways:*)"] }
        }))
        .unwrap();

        let ours = json!({
            "statusLine": { "command": "s.sh" },
            "permissions": { "allow": ["Bash(dotfiles:*)"] }
        });
        let base = read_json_or_empty(&bp).unwrap();
        let out = project(&slice(), &ours, &sp, &base, &bp, false).unwrap();
        assert!(out.changed);

        let written = read_json_or_empty(&sp).unwrap();
        assert_eq!(written["model"], "opus"); // foreign preserved
        assert_eq!(written["statusLine"]["command"], "s.sh");
        let allow = get(&written, "permissions.allow").unwrap();
        assert_eq!(allow, json!(["Bash(ways:*)", "Bash(dotfiles:*)"]));

        // Base persisted; a second run is a no-op.
        let base2 = read_json_or_empty(&bp).unwrap();
        let out2 = project(&slice(), &ours, &sp, &base2, &bp, false).unwrap();
        assert!(!out2.changed);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dry_run_touches_nothing() {
        let dir = tmp("dry");
        std::fs::create_dir_all(&dir).unwrap();
        let sp = dir.join("settings.json");
        let bp = dir.join("base.json");
        let ours = json!({ "statusLine": { "command": "s.sh" } });
        let base = read_json_or_empty(&bp).unwrap();
        let out = project(&slice(), &ours, &sp, &base, &bp, true).unwrap();
        assert!(out.changed);
        assert!(!sp.exists(), "dry-run must not create the settings file");
        assert!(!bp.exists(), "dry-run must not create the base file");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn get_reads_dotted_paths() {
        let s = json!({ "permissions": { "allow": ["a", "b"] }, "model": "opus" });
        assert_eq!(get(&s, "model").unwrap(), json!("opus"));
        assert_eq!(get(&s, "permissions.allow").unwrap(), json!(["a", "b"]));
        assert!(get(&s, "permissions.deny").is_none());
        assert!(get(&s, "nope").is_none());
    }

    #[test]
    fn self_audit_passes_for_a_normal_projection() {
        let live = json!({ "model": "opus", "permissions": { "allow": ["Bash(ways:*)"] } });
        let ours = json!({ "permissions": { "allow": ["Bash(dotfiles:*)"] } });
        let out = project_value(&slice(), &live, &ours, &json!({})).unwrap();
        assert!(out.changed);
    }

    #[test]
    fn refuses_a_non_object_live_document() {
        // Regression (round-2 #2): a non-object live file must not be collapsed to
        // {} and overwritten — refuse instead.
        let ours = json!({ "statusLine": { "command": "s.sh" } });
        assert!(project_value(&slice(), &json!([1, 2, 3]), &ours, &json!({})).is_err());
        assert!(project_value(&slice(), &json!("scalar"), &ours, &json!({})).is_err());
    }

    #[test]
    fn refuses_to_overwrite_a_foreign_object_or_array_at_an_owned_leaf() {
        // Regression (round-3): the structural guard. Asserting an owned leaf whose
        // live value is an object (foreign sub-keys) or a non-managed array must be
        // refused — the self-audit alone is blind to this loss.
        let s = OwnedSlice::new(&["env", "hooks.PreToolUse"], &[]);
        // live.env is a foreign object; ours declares env as a scalar → refuse.
        let live_obj = json!({ "env": { "HTTP_PROXY": "http://p" } });
        assert!(project_value(&s, &live_obj, &json!({ "env": "x" }), &json!({})).is_err());
        // live.hooks.PreToolUse is a foreign array (agent-ways) → refuse.
        let live_arr = json!({ "hooks": { "PreToolUse": [{ "x": 1 }] } });
        assert!(
            project_value(&s, &live_arr, &json!({ "hooks": { "PreToolUse": [{ "y": 2 }] } }), &json!({})).is_err()
        );
        // But asserting a scalar *leaf* (env.A) over a scalar live value is fine —
        // the foreign sibling is preserved and no structure is clobbered.
        let s2 = OwnedSlice::new(&["env.A"], &[]);
        let ok = project_value(
            &s2,
            &json!({ "env": { "A": "old", "HTTP_PROXY": "http://p" } }),
            &json!({ "env": { "A": "new" } }),
            &json!({}),
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn own_array_leaf_reprojects_idempotently() {
        // Regression (round-4 #4): the guard must be base-aware — a non-union array
        // leaf WE wrote must not read as "foreign" and wedge the second projection.
        let s = OwnedSlice::new(&["someList"], &[]);
        let ours = json!({ "someList": ["a", "b"] });
        let first = project_value(&s, &json!({}), &ours, &json!({})).unwrap();
        let second = project_value(&s, &first.settings, &ours, &first.base);
        assert!(second.is_ok(), "our own array leaf must reproject, not refuse");
        // A genuinely foreign array (base did not record it) is still refused.
        let foreign = project_value(&s, &json!({ "someList": ["x"] }), &ours, &json!({}));
        assert!(foreign.is_err());
    }

    #[test]
    fn union_list_with_non_array_live_is_refused() {
        // Regression (round-4 #1): a foreign non-array at a union-list path must be
        // refused, not silently overwritten.
        let s = OwnedSlice::new(&[], &["permissions.allow"]);
        let live = json!({ "permissions": { "allow": { "weird": true } } });
        let ours = json!({ "permissions": { "allow": ["Bash(x:*)"] } });
        assert!(project_value(&s, &live, &ours, &json!({})).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn aborts_without_overwriting_when_backup_fails() {
        // Regression (#10): if the .bak backup cannot be written, the projection
        // must error and leave settings.json untouched, not overwrite with no
        // recovery copy.
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("bak");
        std::fs::create_dir_all(&dir).unwrap();
        let sp = dir.join("settings.json");
        let bp = dir.join("base.json");
        atomic_write_json(&sp, &json!({ "model": "opus" })).unwrap();
        // Read-only dir: reads still work, but the .bak copy cannot be created.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();

        let res = project(
            &slice(),
            &json!({ "statusLine": { "command": "s.sh" } }),
            &sp,
            &json!({}),
            &bp,
            false,
        );

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(res.is_err(), "must abort when the backup cannot be written");
        let after = read_json_or_empty(&sp).unwrap();
        assert!(after.get("statusLine").is_none(), "settings.json must be left untouched");
        assert_eq!(after["model"], "opus");
        std::fs::remove_dir_all(&dir).ok();
    }
}
