//! Pure three-way merge for the Claude `~/.claude/settings.json` projector
//! (ADR-010). No I/O — the portable algorithm, ported from agent-ways'
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
//! # Ownership — leaf paths, not whole objects
//!
//! A writer owns a set of **dotted leaf paths** (`env.FOO`, `statusLine.command`,
//! `permissions.defaultMode`) plus a set of **shared concat-lists**
//! (`permissions.allow`, `permissions.deny`). Owning leaves rather than whole
//! objects is what lets the projector assert `env.FOO` while leaving a foreign
//! `env.HTTP_PROXY` sibling untouched — and it keeps the self-audit honest, since a
//! clobbered foreign sibling now shows up in the stripped user view. This projector
//! owns no `hooks` (agent-ways keeps those), so there is no keyed-collection /
//! structural-backstop machinery here.

use serde_json::{Map, Value};

/// The keys a writer owns. Everything else in the live file is foreign and is
/// preserved untouched.
#[derive(Debug, Clone, Default)]
pub struct OwnedSlice {
    /// Dotted **leaf** paths owned outright — exclusive scalar/array override
    /// (`env.FOO`, `statusLine.command`). No other writer may declare the same
    /// leaf (the coexistence contract, ADR-010).
    pub exclusive: Vec<String>,
    /// Dotted paths of shared concat-lists this writer contributes to
    /// (`permissions.allow`, `permissions.deny`). Additive-union-with-
    /// deprecated-base-removal: the writer adds and removes only entries its own
    /// base recorded.
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

// --- nested-path helpers (dotted keys over a JSON object) ---

/// Read the value at a dotted `path`. `None` if any segment is missing or a
/// non-final segment is not an object. Exposed so callers share one traversal.
pub fn get_path<'a>(obj: &'a Map<String, Value>, path: &str) -> Option<&'a Value> {
    let mut segs = path.split('.');
    let first = segs.next()?;
    let mut cur = obj.get(first)?;
    for seg in segs {
        cur = cur.as_object()?.get(seg)?;
    }
    Some(cur)
}

/// Set the value at a dotted `path`, creating intermediate objects and coercing a
/// non-object intermediate to an object. Exposed for fragment authoring (`set`).
pub fn set_path(obj: &mut Map<String, Value>, path: &str, value: Value) {
    let segs: Vec<&str> = path.split('.').collect();
    let (leaf, parents) = segs.split_last().expect("non-empty path");
    let mut cur = obj;
    for seg in parents {
        let child = cur.entry((*seg).to_string()).or_insert_with(|| Value::Object(Map::new()));
        if !child.is_object() {
            *child = Value::Object(Map::new());
        }
        cur = child.as_object_mut().expect("just ensured object");
    }
    cur.insert((*leaf).to_string(), value);
}

/// Remove the value at a dotted `path`, pruning any parent object left empty.
/// Exposed for fragment authoring (`unset`).
pub fn remove_path(obj: &mut Map<String, Value>, path: &str) {
    let segs: Vec<&str> = path.split('.').collect();
    remove_path_inner(obj, &segs);
}

fn remove_path_inner(obj: &mut Map<String, Value>, segs: &[&str]) {
    let (head, rest) = segs.split_first().expect("non-empty path");
    if rest.is_empty() {
        obj.remove(*head);
        return;
    }
    if let Some(child) = obj.get_mut(*head).and_then(|v| v.as_object_mut()) {
        remove_path_inner(child, rest);
        if child.is_empty() {
            obj.remove(*head);
        }
    }
}

/// Read the array at a dotted `path`, or an empty vec if absent / not an array.
/// Entries are kept as `Value`s (not coerced to strings) so a non-string foreign
/// entry round-trips through merge and self-audit identically.
fn get_array(obj: &Map<String, Value>, path: &str) -> Vec<Value> {
    get_path(obj, path).and_then(|v| v.as_array()).cloned().unwrap_or_default()
}

/// Set (or, if empty, remove) the array at a dotted `path`, pruning empty parents.
fn set_or_remove_array(obj: &mut Map<String, Value>, path: &str, entries: Vec<Value>) {
    if entries.is_empty() {
        remove_path(obj, path);
    } else {
        set_path(obj, path, Value::Array(entries));
    }
}

/// Three-way merge of `ours` into `live`, given the prior `base`. Pure and fully
/// testable.
pub fn merge(slice: &OwnedSlice, live: &Value, ours: &Value, base: &Value) -> Merged {
    let mut out = live.as_object().cloned().unwrap_or_default();
    let ours_obj = ours.as_object().cloned().unwrap_or_default();
    let base_obj = base.as_object().cloned().unwrap_or_default();

    // --- exclusive leaf paths ---
    for path in &slice.exclusive {
        match get_path(&ours_obj, path) {
            // Assert (override an existing value, or adopt a foreign one). Only this
            // leaf is touched, so foreign siblings in the same object survive.
            Some(v) => set_path(&mut out, path, v.clone()),
            // We no longer assert it. Relinquish it *only if the live value is still
            // the one we wrote* (base == live). If a foreign writer changed it since,
            // leave their value in place rather than deleting their edit.
            None => {
                if get_path(&base_obj, path).is_some()
                    && get_path(&out, path) == get_path(&base_obj, path)
                {
                    remove_path(&mut out, path);
                }
            }
        }
    }

    // --- shared concat-lists: additive union with deprecated-base removal ---
    for path in &slice.union_lists {
        let theirs = get_array(&out, path);
        let ours_entries = get_array(&ours_obj, path);
        let base_entries = get_array(&base_obj, path);

        // Entries we added before and no longer assert. NOTE: under the disjoint-
        // ownership contract (ADR-010/169) no entry is co-owned, so removing what our
        // base recorded never revokes another writer's entry. If that contract is
        // violated (two writers assert the same string), this removes a co-owned
        // entry — an accepted bound of additive union, matching the reference.
        let deprecated: Vec<Value> =
            base_entries.iter().filter(|e| !ours_entries.contains(e)).cloned().collect();

        // Keep their entries except our deprecated and current ones (current
        // re-appended below — dropping first dedupes a re-apply). Value-based, so a
        // non-string foreign entry is preserved, not silently coerced away.
        let mut result: Vec<Value> = theirs
            .into_iter()
            .filter(|e| !deprecated.contains(e) && !ours_entries.contains(e))
            .collect();
        result.extend(ours_entries);

        set_or_remove_array(&mut out, path, result);
    }

    Merged { settings: Value::Object(out), base: base_for(slice, &ours_obj) }
}

/// The base to persist: exactly the slice we asserted this run.
fn base_for(slice: &OwnedSlice, ours_obj: &Map<String, Value>) -> Value {
    let mut base = Map::new();
    for path in &slice.exclusive {
        if let Some(v) = get_path(ours_obj, path) {
            set_path(&mut base, path, v.clone());
        }
    }
    for path in &slice.union_lists {
        let entries = get_array(ours_obj, path);
        if !entries.is_empty() {
            set_or_remove_array(&mut base, path, entries);
        }
    }
    Value::Object(base)
}

/// The user's portion of a settings doc: everything **except** the leaves and list
/// entries `base` says we own. Two docs with equal user-views differ only in our
/// slice — the invariant the post-write self-audit asserts.
pub fn stripped_user_view(slice: &OwnedSlice, settings: &Value, base: &Value) -> Value {
    let mut obj = settings.as_object().cloned().unwrap_or_default();
    let base_obj = base.as_object().cloned().unwrap_or_default();

    for path in &slice.exclusive {
        if get_path(&base_obj, path).is_some() {
            remove_path(&mut obj, path);
        }
    }
    for path in &slice.union_lists {
        let owned = get_array(&base_obj, path);
        if owned.is_empty() {
            continue;
        }
        let kept: Vec<Value> =
            get_array(&obj, path).into_iter().filter(|v| !owned.contains(v)).collect();
        set_or_remove_array(&mut obj, path, kept);
    }
    Value::Object(obj)
}

/// The union of two owned-slice bases: every exclusive leaf present in either, and,
/// per shared list, the union of both sides' entries. Used by the self-audit so a
/// leaf that entered or left the slice this run is stripped from both before and
/// after documents and does not trip a false alarm.
pub fn owned_union(slice: &OwnedSlice, a: &Value, b: &Value) -> Value {
    let ao = a.as_object().cloned().unwrap_or_default();
    let bo = b.as_object().cloned().unwrap_or_default();
    let mut out = Map::new();
    for path in &slice.exclusive {
        if let Some(v) = get_path(&ao, path).or_else(|| get_path(&bo, path)) {
            set_path(&mut out, path, v.clone());
        }
    }
    for path in &slice.union_lists {
        let mut entries = get_array(&ao, path);
        for e in get_array(&bo, path) {
            if !entries.contains(&e) {
                entries.push(e);
            }
        }
        set_or_remove_array(&mut out, path, entries);
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Leaf-path slice: two env vars + a statusLine field, plus the two lists.
    fn slice() -> OwnedSlice {
        OwnedSlice::new(
            &["statusLine.command", "env.FOO"],
            &["permissions.allow", "permissions.deny"],
        )
    }

    fn allow(settings: &Value) -> Vec<Value> {
        get_array(settings.as_object().unwrap(), "permissions.allow")
    }

    #[test]
    fn fresh_merge_adds_owned_leaves_and_perms() {
        let ours = json!({
            "statusLine": { "command": "s.sh" }, "env": { "FOO": "1" },
            "permissions": { "allow": ["Bash(dotfiles:*)"] }
        });
        let m = merge(&slice(), &json!({}), &ours, &json!({}));
        assert_eq!(m.settings["statusLine"]["command"], "s.sh");
        assert_eq!(m.settings["env"]["FOO"], "1");
        assert_eq!(allow(&m.settings), vec![json!("Bash(dotfiles:*)")]);
    }

    #[test]
    fn merge_is_idempotent() {
        let ours = json!({ "env": { "FOO": "1" }, "permissions": { "allow": ["Bash(dotfiles:*)"] } });
        let first = merge(&slice(), &json!({}), &ours, &json!({}));
        let second = merge(&slice(), &first.settings, &ours, &first.base);
        assert_eq!(first.settings, second.settings);
    }

    #[test]
    fn owning_a_leaf_preserves_foreign_siblings() {
        // Regression (#1/#5): we own env.FOO; a foreign env.HTTP_PROXY must survive,
        // and the self-audit must see it (equal before/after).
        let live = json!({ "env": { "FOO": "old", "HTTP_PROXY": "http://p" } });
        let ours = json!({ "env": { "FOO": "new" } });
        let m = merge(&slice(), &live, &ours, &json!({}));
        assert_eq!(m.settings["env"]["FOO"], "new");
        assert_eq!(m.settings["env"]["HTTP_PROXY"], "http://p", "foreign sibling preserved");
        let before = stripped_user_view(&slice(), &live, &owned_union(&slice(), &json!({}), &ours));
        let after = stripped_user_view(&slice(), &m.settings, &owned_union(&slice(), &json!({}), &ours));
        assert_eq!(before, after, "self-audit sees the preserved sibling");
    }

    #[test]
    fn relinquish_preserves_a_foreign_edit() {
        // Regression (#3): we owned statusLine.command=X; a foreign writer changed it
        // to Z; we drop the fragment. Z must survive, not be deleted.
        let live = json!({ "statusLine": { "command": "Z" } });
        let base = json!({ "statusLine": { "command": "X" } });
        let m = merge(&slice(), &live, &json!({}), &base);
        assert_eq!(m.settings["statusLine"]["command"], "Z", "foreign edit kept");
    }

    #[test]
    fn relinquish_drops_our_unchanged_value() {
        let live = json!({ "statusLine": { "command": "X" }, "model": "opus" });
        let base = json!({ "statusLine": { "command": "X" } });
        let m = merge(&slice(), &live, &json!({}), &base);
        assert!(m.settings.get("statusLine").is_none(), "our own unchanged value dropped");
        assert_eq!(m.settings["model"], "opus");
    }

    #[test]
    fn preserves_unrelated_and_foreign_list_entries() {
        let live = json!({
            "model": "opus", "autoCompactEnabled": true,
            "permissions": { "allow": ["Bash(ways:*)"], "deny": ["Read(~/.ssh/**)"] }
        });
        let ours = json!({ "permissions": { "allow": ["Bash(dotfiles:*)"] } });
        let m = merge(&slice(), &live, &ours, &json!({}));
        assert_eq!(m.settings["model"], "opus");
        assert_eq!(m.settings["autoCompactEnabled"], true);
        assert_eq!(allow(&m.settings), vec![json!("Bash(ways:*)"), json!("Bash(dotfiles:*)")]);
        assert_eq!(get_array(m.settings.as_object().unwrap(), "permissions.deny"), vec![json!("Read(~/.ssh/**)")]);
    }

    #[test]
    fn strips_previously_owned_deprecated_allow() {
        let live = json!({ "permissions": {
            "allow": ["Bash(dotfiles:*)", "Bash(old-tool:*)", "Bash(user-thing:*)"]
        }});
        let base = json!({ "permissions": { "allow": ["Bash(old-tool:*)"] } });
        let ours = json!({ "permissions": { "allow": ["Bash(dotfiles:*)"] } });
        let m = merge(&slice(), &live, &ours, &base);
        assert_eq!(allow(&m.settings), vec![json!("Bash(user-thing:*)"), json!("Bash(dotfiles:*)")]);
    }

    #[test]
    fn non_string_foreign_list_entry_is_preserved_no_lockout() {
        // Regression (#6): a non-string foreign entry must survive merge AND stay in
        // the stripped view, so before==after and the self-audit does not lock out.
        let live = json!({ "permissions": { "allow": ["Bash(ways:*)", {"weird": true}] } });
        let ours = json!({ "permissions": { "allow": ["Bash(dotfiles:*)"] } });
        let audit = owned_union(&slice(), &json!({}), &ours);
        let before = stripped_user_view(&slice(), &live, &audit);
        let m = merge(&slice(), &live, &ours, &json!({}));
        let after = stripped_user_view(&slice(), &m.settings, &audit);
        assert!(allow(&m.settings).contains(&json!({"weird": true})), "non-string entry preserved");
        assert_eq!(before, after, "no spurious self-audit divergence");
    }

    #[test]
    fn adopt_foreign_leaf_no_gap() {
        let live = json!({ "statusLine": { "command": "s.sh" } });
        let ours = json!({ "statusLine": { "command": "s.sh" } });
        let m = merge(&slice(), &live, &ours, &json!({}));
        assert_eq!(m.settings["statusLine"]["command"], "s.sh");
        let m2 = merge(&slice(), &m.settings, &ours, &m.base);
        assert_eq!(m.settings, m2.settings);
    }

    #[test]
    fn empty_list_removes_key_and_parent() {
        let live = json!({ "permissions": { "deny": ["Read(~/.config/gh/**)"] } });
        let base = json!({ "permissions": { "deny": ["Read(~/.config/gh/**)"] } });
        let m = merge(&slice(), &live, &json!({}), &base);
        assert!(m.settings.get("permissions").is_none(), "emptied permissions pruned");
    }
}
