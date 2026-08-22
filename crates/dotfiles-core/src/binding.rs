//! The host binding and the registry index (ADR-013 §1–§3).
//!
//! The binding is the one file that lives outside every store:
//! `$XDG_CONFIG_HOME/dotfiles/config.toml`. It names the store this machine
//! uses, the registry entry that store descends from, and the active profile.
//! The registry index is `registry.toml` at the root of the public registry
//! repo, one row per published configuration.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The `config` value for a store that descends from no registry entry.
pub const UNREGISTERED: &str = "unregistered";

/// `[store]` — where this host's store is and what it descends from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreBinding {
    /// Store path. `~` expands; relative paths resolve against `$HOME`.
    pub path: String,
    /// Registry id (`com.example.dotfiles`) or [`UNREGISTERED`].
    pub config: String,
    /// Active ADR-008 profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// The whole binding file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HostBinding {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<StoreBinding>,
}

impl HostBinding {
    /// `$XDG_CONFIG_HOME/dotfiles/config.toml`, falling back to `~/.config`.
    pub fn default_path(home: &Path) -> PathBuf {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config"))
            .join("dotfiles")
            .join("config.toml")
    }

    /// Read the binding. A missing file is `None`; a malformed one is an error.
    pub fn load(path: &Path) -> Result<Option<Self>, String> {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("reading {}: {e}", path.display())),
        };
        toml::from_str(&src)
            .map(Some)
            .map_err(|e| format!("parsing {}: {e}", path.display()))
    }

    /// Write via a temporary file and rename, so a reader never sees a torn
    /// binding (ADR-013 §3).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let dir = path
            .parent()
            .ok_or_else(|| format!("{} has no parent", path.display()))?;
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        let body = format!(
            "# dotfiles host binding (ADR-013). Per machine; never committed.\n\n{}",
            toml::to_string(self).map_err(|e| e.to_string())?
        );
        let tmp = dir.join(format!(".config.toml.tmp-{}", std::process::id()));
        std::fs::write(&tmp, body).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("renaming into {}: {e}", path.display())
        })
    }

    /// The store path with `~` expanded and relative paths anchored at `home`.
    pub fn store_path(&self, home: &Path) -> Option<PathBuf> {
        self.store.as_ref().map(|s| expand_home(&s.path, home))
    }
}

/// Expand a leading `~` and anchor relative paths at `home`.
pub fn expand_home(raw: &str, home: &Path) -> PathBuf {
    if raw == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    let p = PathBuf::from(raw);
    if p.is_absolute() { p } else { home.join(p) }
}

/// Render a path under `home` as `~/...` for display and for the binding.
pub fn contract_home(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// One row of `registry.toml`: `[configs."com.example.dotfiles"]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistryEntry {
    /// Minimum CLI this entry's manifest needs, e.g. `">=0.8.0"`.
    #[serde(default)]
    pub cli: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Unknown keys are kept so a newer registry degrades gracefully.
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

/// `registry.toml` — the index of published configurations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Registry {
    #[serde(default)]
    pub configs: BTreeMap<String, RegistryEntry>,
}

impl Registry {
    pub fn from_toml(src: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(src)
    }

    /// The subdirectory an entry lives in: `configs/<id>`.
    pub fn entry_dir(id: &str) -> String {
        format!("configs/{id}")
    }
}

/// A configuration id is reverse-DNS: at least three dot-separated labels of
/// `[a-z0-9-]`, e.g. `com.bockelie.dotfiles` (ADR-013 §2).
pub fn valid_config_id(id: &str) -> bool {
    let labels: Vec<&str> = id.split('.').collect();
    labels.len() >= 3
        && labels.iter().all(|l| {
            !l.is_empty()
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

/// Does a running version satisfy a `>=x.y.z` (or bare `x.y.z`) requirement?
/// Malformed requirements are treated as satisfied with the reason returned,
/// so a typo in a registry row degrades to a warning rather than a refusal.
pub fn cli_satisfies(requirement: &str, running: &str) -> Result<bool, String> {
    let want = requirement
        .trim()
        .trim_start_matches(">=")
        .trim_start_matches('v')
        .trim();
    let have = running.trim().trim_start_matches('v');
    // `0.8.0-rc1` compares as `0.8.0`; the pin parser (selfupdate) does the same.
    let parse = |s: &str| -> Result<(u64, u64, u64), String> {
        let core = s.split_once('-').map_or(s, |(c, _)| c);
        let mut it = core.split('.').map(|p| p.parse::<u64>());
        let mut next = || {
            it.next()
                .unwrap_or(Ok(0))
                .map_err(|_| format!("bad version '{s}'"))
        };
        Ok((next()?, next()?, next()?))
    };
    Ok(parse(have)? >= parse(want)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_round_trips_through_toml() {
        let b = HostBinding {
            store: Some(StoreBinding {
                path: "~/.dotfiles".into(),
                config: "com.bockelie.dotfiles".into(),
                profile: Some("north".into()),
            }),
        };
        let dir = std::env::temp_dir().join(format!("dotfiles-binding-{}", std::process::id()));
        let path = dir.join("dotfiles").join("config.toml");
        b.save(&path).unwrap();
        assert_eq!(HostBinding::load(&path).unwrap(), Some(b.clone()));
        assert_eq!(
            b.store_path(Path::new("/home/x")).unwrap(),
            PathBuf::from("/home/x/.dotfiles")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_binding_is_none() {
        assert_eq!(
            HostBinding::load(Path::new("/nonexistent/config.toml")).unwrap(),
            None
        );
    }

    #[test]
    fn registry_parses_and_keeps_unknown_keys() {
        let r = Registry::from_toml(
            r#"
            [configs."sh.dotarchy.starter"]
            cli = ">=0.8.0"
            description = "A working, unremarkable, nobody's setup."
            homepage = "https://dotarchy.sh"
            "#,
        )
        .unwrap();
        let e = &r.configs["sh.dotarchy.starter"];
        assert_eq!(e.cli.as_deref(), Some(">=0.8.0"));
        assert!(e.extra.contains_key("homepage"));
        assert_eq!(
            Registry::entry_dir("sh.dotarchy.starter"),
            "configs/sh.dotarchy.starter"
        );
    }

    #[test]
    fn config_ids_are_reverse_dns() {
        assert!(valid_config_id("com.bockelie.dotfiles"));
        assert!(valid_config_id("sh.dotarchy.starter"));
        assert!(!valid_config_id("dotfiles"));
        assert!(!valid_config_id("com.bockelie"));
        assert!(!valid_config_id("Com.Bockelie.Dotfiles"));
        assert!(!valid_config_id("com..dotfiles"));
    }

    #[test]
    fn cli_requirement_compares_semver_tuples() {
        assert!(cli_satisfies(">=0.8.0", "0.8.0").unwrap());
        assert!(cli_satisfies(">=0.8.0", "0.10.1").unwrap());
        assert!(!cli_satisfies(">=0.8.0", "0.7.9").unwrap());
        assert!(cli_satisfies("0.8", "v0.8.0").unwrap());
        assert!(cli_satisfies(">=0.8.0", "1.0.0").unwrap());
        assert!(cli_satisfies(">=0.8.x", "0.9.0").is_err());
        assert!(cli_satisfies(">=0.8.0-rc1", "0.8.0").unwrap());
        assert!(cli_satisfies(">=0.8.0", "0.8.0-rc1").unwrap());
    }

    #[test]
    fn home_expansion_and_contraction() {
        let home = Path::new("/home/x");
        assert_eq!(
            expand_home("~/.dotfiles", home),
            PathBuf::from("/home/x/.dotfiles")
        );
        assert_eq!(expand_home("/srv/store", home), PathBuf::from("/srv/store"));
        assert_eq!(expand_home("store", home), PathBuf::from("/home/x/store"));
        assert_eq!(
            contract_home(Path::new("/home/x/.dotfiles"), home),
            "~/.dotfiles"
        );
        assert_eq!(contract_home(Path::new("/srv/store"), home), "/srv/store");
    }
}
