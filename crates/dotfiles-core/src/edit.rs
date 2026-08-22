//! Format-preserving manifest edits for the mutating verbs (`enable`/`disable`/
//! `add`/`remove`).
//!
//! Reads and rewrites the manifest through `toml_edit` so that comments, key
//! ordering, and every entry's `why`/`spec` survive a write untouched — only the
//! one field (or one `[[entry]]`) being changed moves.

use crate::Mode;
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, value};

/// Parse a manifest into an editable document.
pub fn parse(src: &str) -> Result<DocumentMut, toml_edit::TomlError> {
    src.parse()
}

/// Fields for a new `[[entry]]`. Defaulted fields (`enabled = true`,
/// `mode = "symlink"`) are omitted from the written table to keep it clean.
#[derive(Debug, Clone)]
pub struct NewEntry<'a> {
    pub name: &'a str,
    pub path: &'a str,
    pub target: &'a str,
    pub mode: Mode,
    pub why: Option<&'a str>,
}

/// Borrow the `[[entry]]` array of tables, creating an empty one if absent.
fn entries_mut(doc: &mut DocumentMut) -> &mut ArrayOfTables {
    if doc
        .get("entry")
        .and_then(Item::as_array_of_tables)
        .is_none()
    {
        doc["entry"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    doc["entry"]
        .as_array_of_tables_mut()
        .expect("just ensured it is an array of tables")
}

fn index_of(aot: &ArrayOfTables, name: &str) -> Option<usize> {
    aot.iter()
        .position(|t| t.get("name").and_then(Item::as_str) == Some(name))
}

/// Set an entry's `enabled` flag. Returns `false` if no entry has that name.
pub fn set_enabled(doc: &mut DocumentMut, name: &str, enabled: bool) -> bool {
    let aot = entries_mut(doc);
    match index_of(aot, name) {
        Some(i) => {
            aot.get_mut(i).expect("index from position")["enabled"] = value(enabled);
            true
        }
        None => false,
    }
}

/// Remove an entry by name. Returns `false` if no entry has that name.
pub fn remove_entry(doc: &mut DocumentMut, name: &str) -> bool {
    let aot = entries_mut(doc);
    match index_of(aot, name) {
        Some(i) => {
            aot.remove(i);
            true
        }
        None => false,
    }
}

/// Append a new entry. Errors if one with the same name already exists.
pub fn add_entry(doc: &mut DocumentMut, e: NewEntry) -> Result<(), String> {
    let aot = entries_mut(doc);
    if index_of(aot, e.name).is_some() {
        return Err(format!("entry '{}' already exists in the manifest", e.name));
    }
    let mut t = Table::new();
    t["name"] = value(e.name);
    t["path"] = value(e.path);
    t["target"] = value(e.target);
    if e.mode == Mode::Copy {
        t["mode"] = value("copy");
    }
    if let Some(why) = e.why {
        t["why"] = value(why);
    }
    aot.push(t);
    Ok(())
}

// --- profiles -------------------------------------------------------------

/// Borrow the `[profiles]` table, creating it (implicit, so only the
/// `[profiles.<name>]` sub-tables render) if absent.
fn profiles_mut(doc: &mut DocumentMut) -> &mut Table {
    if doc.get("profiles").and_then(Item::as_table).is_none() {
        let mut t = Table::new();
        t.set_implicit(true);
        doc["profiles"] = Item::Table(t);
    }
    doc["profiles"]
        .as_table_mut()
        .expect("just ensured it is a table")
}

/// Declare a profile `[profiles.<name>]`. Errors if it already exists.
pub fn add_profile(
    doc: &mut DocumentMut,
    name: &str,
    description: Option<&str>,
    match_pattern: Option<&str>,
) -> Result<(), String> {
    let profiles = profiles_mut(doc);
    if profiles.contains_key(name) {
        return Err(format!("profile '{name}' already exists"));
    }
    let mut t = Table::new();
    if let Some(d) = description {
        t["description"] = value(d);
    }
    if let Some(m) = match_pattern {
        t["match"] = value(m);
    }
    profiles.insert(name, Item::Table(t));
    Ok(())
}

/// Remove a profile: drop `[profiles.<name>]`, strip `name` from every entry's
/// `profiles` array (an entry left with none becomes universal again), and drop
/// every `paths.<name>` variant it owned (ADR-011) — a variant keyed to a
/// profile that no longer exists could never resolve.
/// Returns whether the `[profiles.<name>]` table existed.
pub fn remove_profile(doc: &mut DocumentMut, name: &str) -> bool {
    let existed = doc
        .get_mut("profiles")
        .and_then(Item::as_table_mut)
        .map(|p| p.remove(name).is_some())
        .unwrap_or(false);

    let aot = entries_mut(doc);
    for i in 0..aot.len() {
        let Some(t) = aot.get_mut(i) else { continue };
        if let Some(arr) = t.get_mut("profiles").and_then(Item::as_array_mut) {
            arr.retain(|v| v.as_str() != Some(name));
            if arr.is_empty() {
                t.remove("profiles");
            }
        }
        if let Some(paths) = t.get_mut("paths").and_then(Item::as_table_mut) {
            paths.remove(name);
            if paths.is_empty() {
                t.remove("paths");
            }
        }
    }
    existed
}

/// Add `profile` to an entry's `profiles` array (idempotent). Returns `false`
/// if no entry has that name.
pub fn add_entry_profile(doc: &mut DocumentMut, entry: &str, profile: &str) -> bool {
    let aot = entries_mut(doc);
    let Some(i) = index_of(aot, entry) else {
        return false;
    };
    let t = aot.get_mut(i).expect("index from position");
    if t.get("profiles").and_then(Item::as_array).is_none() {
        t["profiles"] = value(Array::new());
    }
    let arr = t["profiles"].as_array_mut().expect("just ensured array");
    if !arr.iter().any(|v| v.as_str() == Some(profile)) {
        arr.push(profile);
    }
    true
}

// --- per-profile content variants (ADR-011) -------------------------------

/// Set an entry's variant path for `profile` (`paths.<profile> = "<path>"`),
/// written as a dotted key so it sits inline with the entry's other fields
/// rather than opening a sub-table. Returns `false` if no entry has that name.
pub fn set_entry_path(doc: &mut DocumentMut, entry: &str, profile: &str, path: &str) -> bool {
    let aot = entries_mut(doc);
    let Some(i) = index_of(aot, entry) else {
        return false;
    };
    let t = aot.get_mut(i).expect("index from position");
    if t.get("paths").and_then(Item::as_table).is_none() {
        let mut paths = Table::new();
        paths.set_dotted(true);
        t.insert("paths", Item::Table(paths));
    }
    let paths = t["paths"]
        .as_table_mut()
        .expect("just ensured it is a table");
    paths.set_dotted(true);
    paths[profile] = value(path);
    true
}

/// Drop an entry's variant for `profile`, so it falls back to the base `path`.
/// Removes the whole `paths` table once it is empty. Returns whether a variant
/// was actually there.
pub fn remove_entry_path(doc: &mut DocumentMut, entry: &str, profile: &str) -> bool {
    let aot = entries_mut(doc);
    let Some(i) = index_of(aot, entry) else {
        return false;
    };
    let t = aot.get_mut(i).expect("index from position");
    let Some(paths) = t.get_mut("paths").and_then(Item::as_table_mut) else {
        return false;
    };
    let existed = paths.remove(profile).is_some();
    if paths.is_empty() {
        t.remove("paths");
    }
    existed
}

/// Strip `profile` from an entry's `profiles` array (an entry left with none
/// becomes universal again). Returns whether the tag was there.
pub fn remove_entry_profile(doc: &mut DocumentMut, entry: &str, profile: &str) -> bool {
    let aot = entries_mut(doc);
    let Some(i) = index_of(aot, entry) else {
        return false;
    };
    let t = aot.get_mut(i).expect("index from position");
    let Some(arr) = t.get_mut("profiles").and_then(Item::as_array_mut) else {
        return false;
    };
    let before = arr.len();
    arr.retain(|v| v.as_str() != Some(profile));
    let removed = arr.len() != before;
    if arr.is_empty() {
        t.remove("profiles");
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Manifest;

    const SRC: &str = r#"# a manifest
[[entry]]
name = "zsh"
path = "zsh/.zshrc"
target = ".zshrc"
why = "shell baseline"

[[entry]]
name = "tmux"
path = "tmux/.tmux.conf"
target = ".tmux.conf"
"#;

    #[test]
    fn disable_flips_only_the_one_flag_and_keeps_why() {
        let mut doc = parse(SRC).unwrap();
        assert!(set_enabled(&mut doc, "zsh", false));
        let out = doc.to_string();
        assert!(out.contains("enabled = false"));
        assert!(out.contains("why = \"shell baseline\""), "why preserved");
        assert!(out.contains("# a manifest"), "comment preserved");
        // tmux untouched.
        let m = Manifest::from_toml(&out).unwrap();
        assert!(!m.entries.iter().find(|e| e.name == "zsh").unwrap().enabled);
        assert!(m.entries.iter().find(|e| e.name == "tmux").unwrap().enabled);
    }

    #[test]
    fn add_appends_minimal_entry_and_rejects_duplicates() {
        let mut doc = parse(SRC).unwrap();
        add_entry(
            &mut doc,
            NewEntry {
                name: "nvim",
                path: "nvim",
                target: ".config/nvim",
                mode: Mode::Symlink,
                why: Some("editor"),
            },
        )
        .unwrap();
        let out = doc.to_string();
        let m = Manifest::from_toml(&out).unwrap();
        let nvim = m.entries.iter().find(|e| e.name == "nvim").unwrap();
        assert_eq!(nvim.target, ".config/nvim");
        assert!(nvim.enabled, "defaulted true (key omitted)");
        assert!(!out.contains("mode = "), "symlink mode omitted");
        assert_eq!(nvim.why.as_deref(), Some("editor"));

        let dup = add_entry(
            &mut doc,
            NewEntry {
                name: "nvim",
                path: "x",
                target: "y",
                mode: Mode::Symlink,
                why: None,
            },
        );
        assert!(dup.is_err());
    }

    #[test]
    fn remove_drops_the_entry() {
        let mut doc = parse(SRC).unwrap();
        assert!(remove_entry(&mut doc, "tmux"));
        assert!(!remove_entry(&mut doc, "tmux"), "second remove is a no-op");
        let m = Manifest::from_toml(&doc.to_string()).unwrap();
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.entries[0].name, "zsh");
    }

    #[test]
    fn add_and_remove_profile_round_trip() {
        let mut doc = parse(SRC).unwrap();
        add_profile(&mut doc, "desktop", Some("workstation"), None).unwrap();
        add_profile(&mut doc, "vm", None, Some("vm-*")).unwrap();
        assert!(
            add_profile(&mut doc, "desktop", None, None).is_err(),
            "duplicate rejected"
        );

        // tag zsh into desktop, then remove the profile and confirm the tag is stripped.
        assert!(add_entry_profile(&mut doc, "zsh", "desktop"));
        assert!(add_entry_profile(&mut doc, "zsh", "desktop"), "idempotent");
        assert!(!add_entry_profile(&mut doc, "ghost", "desktop"));

        let m = Manifest::from_toml(&doc.to_string()).unwrap();
        assert_eq!(m.profiles["vm"].match_pattern.as_deref(), Some("vm-*"));
        assert_eq!(
            m.entries.iter().find(|e| e.name == "zsh").unwrap().profiles,
            ["desktop"]
        );
        assert!(
            doc.to_string().contains("# a manifest"),
            "comment preserved"
        );

        assert!(remove_profile(&mut doc, "desktop"));
        let m = Manifest::from_toml(&doc.to_string()).unwrap();
        assert!(!m.profiles.contains_key("desktop"));
        // zsh's only profile was desktop -> stripped -> universal again.
        assert!(
            m.entries
                .iter()
                .find(|e| e.name == "zsh")
                .unwrap()
                .profiles
                .is_empty()
        );
        assert!(
            !remove_profile(&mut doc, "desktop"),
            "second remove is a no-op"
        );
    }

    #[test]
    fn variant_paths_round_trip_as_dotted_keys() {
        let mut doc = parse(SRC).unwrap();
        assert!(set_entry_path(&mut doc, "zsh", "slab", "zsh/.zshrc-slab"));
        assert!(set_entry_path(&mut doc, "zsh", "cube", "zsh/.zshrc-cube"));
        assert!(!set_entry_path(&mut doc, "ghost", "slab", "x"));

        let out = doc.to_string();
        assert!(
            out.contains(r#"paths.slab = "zsh/.zshrc-slab""#),
            "dotted, not a sub-table: {out}"
        );
        assert!(out.contains("# a manifest"), "comment preserved");
        assert!(out.contains(r#"why = "shell baseline""#), "why preserved");

        let m = Manifest::from_toml(&out).unwrap();
        let zsh = m.entries.iter().find(|e| e.name == "zsh").unwrap();
        assert_eq!(zsh.path_for("slab"), "zsh/.zshrc-slab");
        assert_eq!(zsh.path_for("cube"), "zsh/.zshrc-cube");
        assert_eq!(
            zsh.path_for("north"),
            "zsh/.zshrc",
            "untouched profiles keep the base"
        );

        // Overwriting an existing variant replaces it in place.
        assert!(set_entry_path(&mut doc, "zsh", "slab", "zsh/.zshrc-slab2"));
        let m = Manifest::from_toml(&doc.to_string()).unwrap();
        assert_eq!(m.entries[0].path_for("slab"), "zsh/.zshrc-slab2");

        // Removing the last variant drops the `paths` table entirely.
        assert!(remove_entry_path(&mut doc, "zsh", "slab"));
        assert!(
            !remove_entry_path(&mut doc, "zsh", "slab"),
            "second remove is a no-op"
        );
        assert!(remove_entry_path(&mut doc, "zsh", "cube"));
        assert!(!doc.to_string().contains("paths"), "empty table removed");
        let m = Manifest::from_toml(&doc.to_string()).unwrap();
        assert!(m.entries[0].paths.is_empty());
    }

    #[test]
    fn remove_entry_profile_strips_one_tag() {
        let mut doc = parse(SRC).unwrap();
        add_entry_profile(&mut doc, "zsh", "slab");
        add_entry_profile(&mut doc, "zsh", "cube");
        assert!(remove_entry_profile(&mut doc, "zsh", "slab"));
        assert!(
            !remove_entry_profile(&mut doc, "zsh", "slab"),
            "already gone"
        );
        assert!(
            !remove_entry_profile(&mut doc, "tmux", "slab"),
            "no profiles array"
        );

        let m = Manifest::from_toml(&doc.to_string()).unwrap();
        assert_eq!(m.entries[0].profiles, ["cube"]);

        // Stripping the last tag makes the entry universal again.
        assert!(remove_entry_profile(&mut doc, "zsh", "cube"));
        let m = Manifest::from_toml(&doc.to_string()).unwrap();
        assert!(m.entries[0].profiles.is_empty());
        assert!(!doc.to_string().contains("profiles"), "empty array removed");
    }
}

/// Carry the store's profile tables and per-entry `paths.*` variants from
/// `store` onto `merged`, which came from a registry entry that never has
/// them (ADR-013 §1, §6). Entries are matched by `name`; a variant whose entry
/// the upstream dropped is dropped with it.
pub fn graft_store_sections(merged: &mut DocumentMut, store: &DocumentMut) {
    if let Some(profiles) = store.get("profiles") {
        merged["profiles"] = profiles.clone();
    }
    if let Some(local) = store.get("store") {
        merged["store"] = local.clone();
    }
    let Some(src) = store.get("entry").and_then(Item::as_array_of_tables) else {
        return;
    };
    let variants: Vec<(String, Item)> = src
        .iter()
        .filter_map(|t| {
            let name = t.get("name")?.as_str()?.to_string();
            let paths = t.get("paths")?.clone();
            Some((name, paths))
        })
        .collect();
    if variants.is_empty() {
        return;
    }
    let dst = entries_mut(merged);
    for (name, paths) in variants {
        if let Some(i) = index_of(dst, &name)
            && let Some(t) = dst.get_mut(i)
        {
            t["paths"] = paths;
        }
    }
}

#[cfg(test)]
mod graft_tests {
    use super::*;

    #[test]
    fn graft_restores_profiles_and_variants_by_name() {
        let store = parse(
            r#"
[[entry]]
name = "nvim"
path = "nvim"
target = ".config/nvim"
paths.slab = "nvim-slab"

[[entry]]
name = "gone"
path = "gone"
target = ".gone"
paths.slab = "gone-slab"

[profiles.slab]
description = "laptop"

[store]
local = ["zsh/.zsh/host.d"]
"#,
        )
        .unwrap();
        let mut merged = parse(
            r#"
[[entry]]
name = "nvim"
path = "nvim"
target = ".config/nvim"
why = "upstream changed this"
"#,
        )
        .unwrap();
        graft_store_sections(&mut merged, &store);
        let out = merged.to_string();
        assert!(out.contains("[profiles.slab]"));
        assert!(out.contains("zsh/.zsh/host.d"));
        assert!(out.contains("nvim-slab"));
        assert!(out.contains("upstream changed this"));
        assert!(!out.contains("gone-slab"));
    }
}
