//! Pure three-way merge for the Claude `~/.claude/settings.json` projector
//! (ADR-010). No I/O — this is the portable algorithm, ported from agent-ways'
//! `settings_merge.rs` and the shared merge spec, so dotfiles-tui owns its own
//! merger with no dependency on the `ways` binary (shared design lineage, not a
//! shared dependency).
//!
//! # Why a merge, not a compile
//!
//! `~/.claude/settings.json` is a single, shared-write JSON object with several
//! uncoordinated writers: Claude Code's `/config` and its in-situ runtime writes
//! (`advisorModel`, `effortLevel`), agent-ways' self-management baseline, and this
//! projector. There is no user-scope `settings.local.json` to isolate `/config`
//! into. So a tool that rewrote the file from its own sources would clobber the
//! operator's live toggles. The only safe design writes **only its owned slice**,
//! preserves everything else exactly, and proves it did (see [`stripped_user_view`]).
//!
//! # Ownership
//!
//! This projector owns no `hooks` (agent-ways keeps those), so — unlike the
//! reference — there is no keyed-collection / structural-backstop machinery here.
//! An owner declares an [`OwnedSlice`]: top-level keys it owns **exclusively**
//! (scalar/object override) and dotted paths of **shared concat-lists** it
//! contributes to (`permissions.allow`, `permissions.deny`) via additive union.

use serde_json::{Map, Value};

/// The keys a writer owns. Everything else in the live file is foreign and is
/// preserved untouched.
#[derive(Debug, Clone, Default)]
pub struct OwnedSlice {
    /// Top-level keys owned outright — exclusive scalar/object override. No other
    /// writer may declare the same key (the coexistence contract, ADR-010).
    pub exclusive: Vec<String>,
    /// Dotted paths of shared concat-lists this writer contributes to, e.g.
    /// `"permissions.allow"`. Additive-union-with-deprecated-base-removal: the
    /// writer adds and removes only entries its own base recorded.
    pub union_lists: Vec<String>,
}

impl OwnedSlice {
    /// Convenience constructor from string slices.
    pub fn new(exclusive: &[&str], union_lists: &[&str]) -> Self {
        OwnedSlice {
            exclusive: exclusive.iter().map(|s| s.to_string()).collect(),
            union_lists: union_lists.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// The result of a merge: the new settings document and the base to persist.
pub struct Merged {
    /// The merged `settings.json` content to write.
    pub settings: Value,
    /// The new last-applied base — exactly the slice we asserted this run. Persist
    /// host-local, gitignored, and feed it back as `base` next run.
    pub base: Value,
}

/// Split a dotted path (`"permissions.allow"`) into its segments. Flat keys yield
/// a single segment.
fn segments(path: &str) -> Vec<&str> {
    path.split('.').collect()
}

/// Read the array at a dotted `path` in `obj`, or an empty vec if absent / not an
/// array.
fn get_array(obj: &Map<String, Value>, path: &str) -> Vec<Value> {
    let mut cur: &Value = &Value::Null;
    let mut first = true;
    for seg in segments(path) {
        let map = if first {
            first = false;
            Some(obj)
        } else {
            cur.as_object()
        };
        match map.and_then(|m| m.get(seg)) {
            Some(v) => cur = v,
            None => return Vec::new(),
        }
    }
    cur.as_array().cloned().unwrap_or_default()
}

/// String view of a JSON array, dropping non-strings.
fn as_strings(arr: &[Value]) -> Vec<String> {
    arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()
}

/// Set (or, if empty, remove) the array at a dotted `path`, pruning parent objects
/// that become empty.
fn set_or_remove_array(obj: &mut Map<String, Value>, path: &str, entries: Vec<Value>) {
    let segs = segments(path);
    set_or_remove_inner(obj, &segs, entries);
}

fn set_or_remove_inner(obj: &mut Map<String, Value>, segs: &[&str], entries: Vec<Value>) {
    let (head, rest) = segs.split_first().expect("non-empty path");
    if rest.is_empty() {
        if entries.is_empty() {
            obj.remove(*head);
        } else {
            obj.insert(head.to_string(), Value::Array(entries));
        }
        return;
    }
    // Descend, creating the child object if needed.
    let child = obj.entry(head.to_string()).or_insert_with(|| Value::Object(Map::new()));
    if !child.is_object() {
        *child = Value::Object(Map::new());
    }
    let child_obj = child.as_object_mut().expect("just ensured object");
    set_or_remove_inner(child_obj, rest, entries);
    // Prune an emptied parent so we never leave `{"permissions": {}}` behind.
    if child_obj.is_empty() {
        obj.remove(*head);
    }
}

/// Three-way merge of `ours` into `live`, given the prior `base`. Pure and fully
/// testable.
///
/// - `live` (theirs) — the current file, possibly with foreign edits.
/// - `ours` — the slice we assert this run (a settings-shaped object holding only
///   our owned keys).
/// - `base` — what we wrote last run (empty on first run, or stale).
pub fn merge(slice: &OwnedSlice, live: &Value, ours: &Value, base: &Value) -> Merged {
    let mut out = live.as_object().cloned().unwrap_or_default();
    let ours_obj = ours.as_object().cloned().unwrap_or_default();
    let base_obj = base.as_object().cloned().unwrap_or_default();

    // --- exclusive scalar/object keys ---
    for key in &slice.exclusive {
        match ours_obj.get(key) {
            // Assert (override an existing value, or adopt a foreign one).
            Some(v) => {
                out.insert(key.clone(), v.clone());
            }
            // We no longer assert it. If we wrote it last time, it is deprecated-ours
            // — drop it (the operator removed the fragment). Otherwise it is foreign;
            // leave it be.
            None => {
                if base_obj.contains_key(key) {
                    out.remove(key);
                }
            }
        }
    }

    // --- shared concat-lists: additive union with deprecated-base removal ---
    for path in &slice.union_lists {
        let theirs = as_strings(&get_array(&out, path));
        let ours_entries = as_strings(&get_array(&ours_obj, path));
        let base_entries = as_strings(&get_array(&base_obj, path));

        // Entries we added before and no longer assert.
        let deprecated: Vec<String> =
            base_entries.iter().filter(|e| !ours_entries.contains(e)).cloned().collect();

        // Keep their entries except our deprecated ones and our current ones (the
        // latter re-appended below — dropping first dedupes a re-apply).
        let mut result: Vec<Value> = theirs
            .into_iter()
            .filter(|e| !deprecated.contains(e) && !ours_entries.contains(e))
            .map(Value::String)
            .collect();
        result.extend(ours_entries.into_iter().map(Value::String));

        set_or_remove_array(&mut out, path, result);
    }

    Merged { settings: Value::Object(out), base: base_for(slice, &ours_obj) }
}

/// The base to persist: exactly the slice we asserted this run (exclusive keys we
/// set, plus our current list entries). Matches the reference: base records what we
/// wrote, so next run's deprecated-removal and self-audit target precisely our slice.
fn base_for(slice: &OwnedSlice, ours_obj: &Map<String, Value>) -> Value {
    let mut base = Map::new();
    for key in &slice.exclusive {
        if let Some(v) = ours_obj.get(key) {
            base.insert(key.clone(), v.clone());
        }
    }
    for path in &slice.union_lists {
        let ours_entries = get_array(ours_obj, path);
        if !ours_entries.is_empty() {
            set_or_remove_array(&mut base, path, ours_entries);
        }
    }
    Value::Object(base)
}

/// The user's portion of a settings doc: everything **except** the slice `base`
/// says we own. Two docs with equal user-views differ only in our fields — the
/// invariant the post-write self-audit asserts (write only your slice, prove it).
pub fn stripped_user_view(slice: &OwnedSlice, settings: &Value, base: &Value) -> Value {
    let mut obj = settings.as_object().cloned().unwrap_or_default();
    let base_obj = base.as_object().cloned().unwrap_or_default();

    // Strip our exclusive keys (only those the base says we wrote).
    for key in &slice.exclusive {
        if base_obj.contains_key(key) {
            obj.remove(key);
        }
    }

    // Strip our recorded entries from each shared list.
    for path in &slice.union_lists {
        let owned = as_strings(&get_array(&base_obj, path));
        if owned.is_empty() {
            continue;
        }
        let kept: Vec<Value> = get_array(&obj, path)
            .into_iter()
            .filter(|v| v.as_str().map(|s| !owned.contains(&s.to_string())).unwrap_or(true))
            .collect();
        set_or_remove_array(&mut obj, path, kept);
    }

    Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The slice used across tests: dotfiles owns statusLine/attribution/env
    /// exclusively and contributes to the two permission lists.
    fn slice() -> OwnedSlice {
        OwnedSlice::new(
            &["statusLine", "attribution", "env"],
            &["permissions.allow", "permissions.deny"],
        )
    }

    fn allow(settings: &Value) -> Vec<String> {
        as_strings(&get_array(settings.as_object().unwrap(), "permissions.allow"))
    }

    #[test]
    fn fresh_merge_adds_owned_keys_and_perms() {
        let live = json!({});
        let ours = json!({
            "statusLine": { "type": "command", "command": "s.sh" },
            "permissions": { "allow": ["Bash(dotfiles:*)", "Bash(oh-my-posh:*)"] }
        });
        let m = merge(&slice(), &live, &ours, &json!({}));
        assert_eq!(m.settings["statusLine"]["command"], "s.sh");
        assert_eq!(allow(&m.settings), ["Bash(dotfiles:*)", "Bash(oh-my-posh:*)"]);
        // Base records exactly what we asserted.
        assert_eq!(m.base["statusLine"]["command"], "s.sh");
    }

    #[test]
    fn merge_is_idempotent() {
        let ours = json!({
            "statusLine": { "command": "s.sh" },
            "permissions": { "allow": ["Bash(dotfiles:*)"] }
        });
        let first = merge(&slice(), &json!({}), &ours, &json!({}));
        let second = merge(&slice(), &first.settings, &ours, &first.base);
        assert_eq!(first.settings, second.settings);
        // No duplicate allow entry on re-apply.
        assert_eq!(allow(&second.settings), ["Bash(dotfiles:*)"]);
    }

    #[test]
    fn preserves_unrelated_user_and_foreign_keys() {
        // model + a /config toggle + a user deny + another tool's allow entry.
        let live = json!({
            "model": "opus",
            "autoCompactEnabled": true,
            "permissions": {
                "allow": ["Bash(ways:*)"],           // agent-ways' entry
                "deny": ["Read(~/.ssh/**)"]           // agent-ways' entry
            }
        });
        let ours = json!({ "permissions": { "allow": ["Bash(dotfiles:*)"] } });
        let m = merge(&slice(), &live, &ours, &json!({}));
        // Foreign scalars untouched.
        assert_eq!(m.settings["model"], "opus");
        assert_eq!(m.settings["autoCompactEnabled"], true);
        // Other tool's allow entry survives; ours is appended.
        assert_eq!(allow(&m.settings), ["Bash(ways:*)", "Bash(dotfiles:*)"]);
        // Untouched foreign deny survives.
        let deny = as_strings(&get_array(m.settings.as_object().unwrap(), "permissions.deny"));
        assert_eq!(deny, ["Read(~/.ssh/**)"]);
    }

    #[test]
    fn strips_previously_owned_deprecated_allow() {
        // We used to assert an entry (base has it, live still carries it), now we
        // don't. It must be removed; the user's own entry and our current one stay.
        let live = json!({ "permissions": {
            "allow": ["Bash(dotfiles:*)", "Bash(old-tool:*)", "Bash(user-thing:*)"]
        }});
        let base = json!({ "permissions": { "allow": ["Bash(old-tool:*)"] } });
        let ours = json!({ "permissions": { "allow": ["Bash(dotfiles:*)"] } });
        let m = merge(&slice(), &live, &ours, &base);
        // Additive-union order: kept foreign entries first, then ours re-appended.
        assert_eq!(allow(&m.settings), ["Bash(user-thing:*)", "Bash(dotfiles:*)"]);
    }

    #[test]
    fn exclusive_override_and_idempotent() {
        let live = json!({ "statusLine": { "command": "stale.sh" } });
        let ours = json!({ "statusLine": { "command": "fresh.sh" } });
        let base = json!({ "statusLine": { "command": "stale.sh" } });
        let m = merge(&slice(), &live, &ours, &base);
        assert_eq!(m.settings["statusLine"]["command"], "fresh.sh");
    }

    #[test]
    fn adopt_foreign_object_no_gap() {
        // Handoff: agent-ways left a live statusLine (foreign), our base is empty,
        // our fragment asserts the same value. We adopt it with no gap, idempotently.
        let live = json!({ "statusLine": { "command": "s.sh" } });
        let ours = json!({ "statusLine": { "command": "s.sh" } });
        let m = merge(&slice(), &live, &ours, &json!({}));
        assert_eq!(m.settings["statusLine"]["command"], "s.sh");
        assert_eq!(m.base["statusLine"]["command"], "s.sh"); // base now seeded
        // Second run is a no-op.
        let m2 = merge(&slice(), &m.settings, &ours, &m.base);
        assert_eq!(m.settings, m2.settings);
    }

    #[test]
    fn relinquish_drops_opted_out_exclusive_keeps_foreign() {
        // We owned statusLine (base has it); the operator removed the fragment so
        // `ours` no longer asserts it -> drop it. A foreign key stays.
        let live = json!({ "statusLine": { "command": "s.sh" }, "model": "opus" });
        let base = json!({ "statusLine": { "command": "s.sh" } });
        let ours = json!({});
        let m = merge(&slice(), &live, &ours, &base);
        assert!(m.settings.get("statusLine").is_none());
        assert_eq!(m.settings["model"], "opus");
    }

    #[test]
    fn empty_list_removes_key_and_parent() {
        // Our only deny entry is deprecated and nothing else remains -> the whole
        // permissions object disappears rather than leaving `{}` or `[]`.
        let live = json!({ "permissions": { "deny": ["Read(~/.config/gh/**)"] } });
        let base = json!({ "permissions": { "deny": ["Read(~/.config/gh/**)"] } });
        let ours = json!({});
        let m = merge(&slice(), &live, &ours, &base);
        assert!(m.settings.get("permissions").is_none(), "emptied permissions pruned");
    }

    #[test]
    fn deny_additive_union_preserves_user_entries() {
        let live = json!({ "permissions": { "deny": ["Read(~/.aws/**)"] } });
        let ours = json!({ "permissions": { "deny": ["Read(~/.config/gh/**)"] } });
        let m = merge(&slice(), &live, &ours, &json!({}));
        let deny = as_strings(&get_array(m.settings.as_object().unwrap(), "permissions.deny"));
        assert_eq!(deny, ["Read(~/.aws/**)", "Read(~/.config/gh/**)"]);
    }

    #[test]
    fn user_view_invariant_holds_across_merge() {
        // The stripped user-view before and after a merge must be identical: we
        // touched nothing outside our slice.
        let live = json!({
            "model": "opus",
            "statusLine": { "command": "old.sh" },
            "permissions": { "allow": ["Bash(ways:*)", "Bash(old-tool:*)"] }
        });
        let base = json!({
            "statusLine": { "command": "old.sh" },
            "permissions": { "allow": ["Bash(old-tool:*)"] }
        });
        let ours = json!({
            "statusLine": { "command": "new.sh" },
            "permissions": { "allow": ["Bash(dotfiles:*)"] }
        });
        let before = stripped_user_view(&slice(), &live, &base);
        let m = merge(&slice(), &live, &ours, &base);
        let after = stripped_user_view(&slice(), &m.settings, &m.base);
        assert_eq!(before, after, "user view must be invariant under our merge");
        // And the foreign entry genuinely survived in the merged doc.
        assert!(allow(&m.settings).contains(&"Bash(ways:*)".to_string()));
    }

    #[test]
    fn lost_base_does_not_duplicate_our_list_entries() {
        // Base lost (empty) but live already carries our entry: additive union's
        // `- ours` dedup means no duplicate on re-apply.
        let live = json!({ "permissions": { "allow": ["Bash(dotfiles:*)"] } });
        let ours = json!({ "permissions": { "allow": ["Bash(dotfiles:*)"] } });
        let m = merge(&slice(), &live, &ours, &json!({}));
        assert_eq!(allow(&m.settings), ["Bash(dotfiles:*)"]);
    }
}
