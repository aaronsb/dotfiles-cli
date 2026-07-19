//! `dotfiles-core` — the pure state model behind the `dotfiles` CLI (ADR-004).
//!
//! No UI, no interactive I/O. This crate parses the TOML manifest (ADR-003) into
//! the self-documenting catalog (ADR-002, +`spec` ADR-006) and derives each
//! entry's deploy status against the filesystem — the state the CLI reports.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub mod deploy;
pub mod edit;
pub mod pkg;
pub mod settings_merge;

/// How an entry is deployed (ADR-001 #1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Symlink the target to the source in the repo (default).
    #[default]
    Symlink,
    /// Recursively copy — for directories like nested git repos.
    Copy,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Mode::Symlink => "symlink",
            Mode::Copy => "copy",
        })
    }
}

/// One managed dotfile — a row in the self-documenting catalog (ADR-002/003).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Stable handle, e.g. `"zsh"`.
    pub name: String,
    /// Source path, relative to the repo root.
    pub path: String,
    /// Deploy target, relative to `$HOME`.
    pub target: String,
    /// Whether the entry is currently managed.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Deployment mode.
    #[serde(default)]
    pub mode: Mode,
    /// The durable *why* docstring (ADR-002). Advisory; may be absent.
    #[serde(default)]
    pub why: Option<String>,
    /// Optional structured spec (ADR-006). Absent for simple entries.
    #[serde(default)]
    pub spec: Option<Spec>,
    /// Profiles this entry belongs to. Empty = universal (active in every
    /// profile). Non-empty = active only in the listed profiles.
    #[serde(default)]
    pub profiles: Vec<String>,
}

impl Entry {
    /// Is this entry active in `profile`? Universal entries (no `profiles`
    /// listed) are active everywhere.
    pub fn active_in(&self, profile: &str) -> bool {
        self.profiles.is_empty() || self.profiles.iter().any(|p| p == profile)
    }
}

fn default_true() -> bool {
    true
}

/// A declared profile — a named scope over dotfiles and packages (a machine or
/// role). Registered under `[profiles.<name>]` in the manifest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    /// Human rationale for the profile (self-documenting, ADR-002 spirit).
    #[serde(default)]
    pub description: Option<String>,
    /// Optional hostname glob (`*`/`?`). When the active profile is not given
    /// explicitly, a host whose name matches this pattern resolves to this
    /// profile — so a fleet (`vm-*`) maps many hosts to one profile.
    #[serde(default, rename = "match")]
    pub match_pattern: Option<String>,
    /// Unrecognized `profiles.<name>.*` keys, captured and surfaced (ADR-006).
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

/// Structured requirements an entry declares (ADR-006). All fields optional.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Requires {
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub binaries: Vec<String>,
    #[serde(default)]
    pub configs: Vec<String>,
    /// Other managed entries this one depends on.
    #[serde(default)]
    pub entries: Vec<String>,
    /// Unrecognized `requires.*` keys, captured and surfaced (ADR-006).
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

/// An optional structured specification of what an entry is and needs (ADR-006).
///
/// Recognized keys parse into fields; anything else is captured in `extra` and
/// surfaced as "unrecognized" — never silently dropped nor hard-rejected, so the
/// format stays forward-compatible while typos remain visible.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Spec {
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub concern: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub provides: Vec<String>,
    /// Other managed entries this one depends on.
    #[serde(default)]
    pub depends: Vec<String>,
    #[serde(default)]
    pub requires: Option<Requires>,
    /// Unrecognized `spec.*` keys, captured and surfaced (ADR-006).
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

/// The parsed manifest: a TOML array-of-tables of `[[entry]]` (ADR-003), plus an
/// optional `[profiles]` registry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default, rename = "entry")]
    pub entries: Vec<Entry>,
    /// Declared profiles, keyed by name (`[profiles.<name>]`). Optional — a
    /// manifest with no profiles behaves as a single implicit profile.
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

impl Manifest {
    /// Parse a TOML manifest string.
    pub fn from_toml(src: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(src)
    }
}

/// Resolve the active profile name.
///
/// Precedence: an `explicit` choice (from `--profile` / `$DOTFILES_PROFILE` /
/// the `.dotfiles-profile` file) wins; otherwise the first declared profile
/// whose `match` glob matches `hostname` (fleet support); otherwise `hostname`
/// itself. The result is always a name, even if no profile is declared for it.
pub fn resolve_profile(manifest: &Manifest, explicit: Option<&str>, hostname: &str) -> String {
    if let Some(name) = explicit
        && !name.is_empty()
    {
        return name.to_string();
    }
    for (name, profile) in &manifest.profiles {
        if let Some(pattern) = &profile.match_pattern
            && glob_match(pattern, hostname)
        {
            return name.clone();
        }
    }
    hostname.to_string()
}

/// Minimal glob matcher supporting `*` (any run, including empty) and `?` (any
/// one character). Anchored at both ends. No character classes — just enough for
/// hostname fleet patterns like `vm-*` or `build-??`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    // Iterative backtracking match.
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut star_ti): (Option<usize>, usize) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// The deployment status of an entry, derived from the filesystem.
///
/// Internally tagged so it flattens into [`EntryState`] as a `status` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DeployStatus {
    /// Symlink present and pointing at the expected source.
    Linked,
    /// Symlink present but pointing somewhere else.
    WrongTarget { points_to: String },
    /// Target exists but is not the symlink we expect (a real file/dir).
    Conflict,
    /// Symlink points at the expected source, but that source is missing
    /// (a dangling link).
    Broken,
    /// Copy-mode target exists (content drift not yet checked).
    Present,
    /// Target does not exist.
    Missing,
    /// The filesystem check itself failed.
    Error { message: String },
}

/// Compute an entry's deploy status against the filesystem.
///
/// `repo_root` is where source `path`s resolve; `home` is where `target`s resolve.
pub fn deploy_status(entry: &Entry, repo_root: &Path, home: &Path) -> DeployStatus {
    let expected = repo_root.join(&entry.path);
    let target = home.join(&entry.target);

    let meta = match std::fs::symlink_metadata(&target) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return DeployStatus::Missing,
        Err(e) => return DeployStatus::Error { message: e.to_string() },
    };

    if meta.file_type().is_symlink() {
        let link = match std::fs::read_link(&target) {
            Ok(l) => l,
            Err(e) => return DeployStatus::Error { message: e.to_string() },
        };
        let resolved = if link.is_absolute() {
            link
        } else {
            target.parent().unwrap_or(home).join(link)
        };
        if same_path(&resolved, &expected) {
            // Points at the expected source — but is that source actually there?
            if target.exists() {
                DeployStatus::Linked
            } else {
                DeployStatus::Broken
            }
        } else {
            DeployStatus::WrongTarget { points_to: resolved.display().to_string() }
        }
    } else {
        // A real file/dir sits at the target.
        match entry.mode {
            Mode::Copy => DeployStatus::Present,
            Mode::Symlink => DeployStatus::Conflict,
        }
    }
}

/// Compare two paths, preferring canonicalized equality, falling back to a
/// literal comparison when canonicalize fails (e.g. a missing source).
///
/// The literal fallback is best-effort: it does not lexically normalize `..`/`.`
/// in relative link targets, so an unusual relative symlink could be misjudged.
/// Broken (dangling) links are handled by the caller.
fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

/// An entry plus its derived deploy status — one row of the reported state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryState {
    #[serde(flatten)]
    pub entry: Entry,
    #[serde(flatten)]
    pub status: DeployStatus,
}

/// The derived dotfiles state the CLI reports: catalog + deploy status.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub entries: Vec<EntryState>,
}

impl State {
    /// Derive the state from a manifest and the filesystem.
    pub fn derive(manifest: &Manifest, repo_root: &Path, home: &Path) -> Self {
        let entries = manifest
            .entries
            .iter()
            .map(|e| EntryState {
                entry: e.clone(),
                status: deploy_status(e, repo_root, home),
            })
            .collect();
        State { entries }
    }
}

/// Walk up from `start` to find a git repo root: a directory containing `.git`
/// (a dir, or — for submodules/worktrees — a file).
pub fn discover_git_repo(start: &Path) -> Option<PathBuf> {
    let mut current = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// First-run precondition (ADR-001 #7): the tool only operates inside a git repo.
///
/// Returns the discovered repo root, or a ready-to-print message for the user.
pub fn first_run_gate(repo_root: &Path) -> Result<PathBuf, String> {
    discover_git_repo(repo_root).ok_or_else(|| {
        format!(
            "no git repo found at {} — init your dotfiles repo to begin (`git init`).",
            repo_root.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_array_of_tables_with_why() {
        let src = r#"
            [[entry]]
            name = "zsh"
            path = "zsh/.zshrc"
            target = ".zshrc"
            why = "Cross-machine shell baseline."

            [[entry]]
            name = "nvim"
            path = "nvim"
            target = ".config/nvim"
            mode = "symlink"
            enabled = false
        "#;
        let m = Manifest::from_toml(src).expect("parses");
        assert_eq!(m.entries.len(), 2);
        assert_eq!(m.entries[0].name, "zsh");
        assert_eq!(m.entries[0].mode, Mode::Symlink); // defaulted
        assert!(m.entries[0].enabled); // defaulted true
        assert_eq!(m.entries[0].why.as_deref(), Some("Cross-machine shell baseline."));
        assert!(!m.entries[1].enabled);
        assert!(m.entries[1].why.is_none());
        assert!(m.entries[0].spec.is_none());
    }

    #[test]
    fn parses_spec_and_captures_unknown_keys() {
        let src = r#"
            [[entry]]
            name = "zprofile"
            path = "zsh/.zprofile"
            target = ".zprofile"
            why = "hook"
            spec.summary = "launch fb terminal"
            spec.concern = "terminal-bootstrap"
            spec.platform = "linux-vt"
            spec.requires.packages = ["mlterm-fb"]
            spec.requires.entries = ["mlterm-main"]
            spec.launches = ["~/.local/bin/mlterm-fb"]
            spec.run_mode = "run-not-exec"
        "#;
        let m = Manifest::from_toml(src).expect("parses");
        let spec = m.entries[0].spec.as_ref().expect("has a spec");

        // recognized keys -> typed fields
        assert_eq!(spec.concern.as_deref(), Some("terminal-bootstrap"));
        let requires = spec.requires.as_ref().expect("has requires");
        assert_eq!(requires.packages, ["mlterm-fb"]);
        assert_eq!(requires.entries, ["mlterm-main"]);

        // unrecognized keys -> captured, not dropped (ADR-006 capture-and-surface)
        assert!(spec.extra.contains_key("launches"));
        assert!(spec.extra.contains_key("run_mode"));
    }

    #[cfg(unix)]
    #[test]
    fn deploy_status_detects_link_and_missing() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!("dft-test-{}", std::process::id()));
        let repo = base.join("repo");
        let home = base.join("home");
        std::fs::create_dir_all(repo.join("zsh")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(repo.join("zsh/.zshrc"), "x").unwrap();

        let entry = Entry {
            name: "zsh".into(),
            path: "zsh/.zshrc".into(),
            target: ".zshrc".into(),
            enabled: true,
            mode: Mode::Symlink,
            why: None,
            spec: None,
            profiles: vec![],
        };

        assert_eq!(deploy_status(&entry, &repo, &home), DeployStatus::Missing);

        symlink(repo.join("zsh/.zshrc"), home.join(".zshrc")).unwrap();
        assert_eq!(deploy_status(&entry, &repo, &home), DeployStatus::Linked);

        // Source removed out from under the link -> dangling -> Broken.
        std::fs::remove_file(repo.join("zsh/.zshrc")).unwrap();
        assert_eq!(deploy_status(&entry, &repo, &home), DeployStatus::Broken);

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn parses_profiles_and_entry_membership() {
        let src = r#"
            [profiles.desktop]
            description = "Main workstation"
            [profiles.vm]
            match = "vm-*"

            [[entry]]
            name = "shared"
            path = "a"
            target = ".a"

            [[entry]]
            name = "kde"
            path = "b"
            target = ".b"
            profiles = ["desktop"]
        "#;
        let m = Manifest::from_toml(src).expect("parses");
        assert_eq!(m.profiles.len(), 2);
        assert_eq!(m.profiles["desktop"].description.as_deref(), Some("Main workstation"));
        assert_eq!(m.profiles["vm"].match_pattern.as_deref(), Some("vm-*"));

        let shared = &m.entries[0];
        let kde = &m.entries[1];
        // Universal entry is active everywhere; tagged entry only in its profile.
        assert!(shared.active_in("desktop") && shared.active_in("vm") && shared.active_in("anything"));
        assert!(kde.active_in("desktop"));
        assert!(!kde.active_in("vm"));
    }

    #[test]
    fn glob_matches_fleet_patterns() {
        assert!(glob_match("vm-*", "vm-01"));
        assert!(glob_match("vm-*", "vm-"));
        assert!(!glob_match("vm-*", "web-01"));
        assert!(glob_match("build-??", "build-42"));
        assert!(!glob_match("build-??", "build-4"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exacto"));
        assert!(glob_match("a*c", "abbbc"));
    }

    #[test]
    fn resolve_profile_precedence() {
        let src = r#"
            [profiles.vm]
            match = "vm-*"
            [profiles.desktop]
            description = "ws"
        "#;
        let m = Manifest::from_toml(src).unwrap();
        // explicit wins
        assert_eq!(resolve_profile(&m, Some("desktop"), "vm-01"), "desktop");
        // pattern match when no explicit
        assert_eq!(resolve_profile(&m, None, "vm-09"), "vm");
        // fall back to hostname when nothing matches
        assert_eq!(resolve_profile(&m, None, "north"), "north");
        // empty explicit is ignored
        assert_eq!(resolve_profile(&m, Some(""), "vm-09"), "vm");
    }

    #[test]
    fn discovers_git_repo_and_gates() {
        let base = std::env::temp_dir().join(format!("dft-git-{}", std::process::id()));
        let nested = base.join("a/b");
        std::fs::create_dir_all(&nested).unwrap();

        assert!(discover_git_repo(&nested).is_none());
        assert!(first_run_gate(&nested).is_err());

        std::fs::create_dir_all(base.join(".git")).unwrap();
        let found = discover_git_repo(&nested).expect("finds the repo root");
        assert_eq!(found, std::fs::canonicalize(&base).unwrap());
        assert!(first_run_gate(&nested).is_ok());

        std::fs::remove_dir_all(&base).ok();
    }
}
