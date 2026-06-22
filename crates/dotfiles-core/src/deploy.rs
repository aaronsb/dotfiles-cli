//! Deploying a manifest entry to the filesystem — the symlink/copy layer that
//! `dotfiles deploy` drives.
//!
//! Mirrors the reference bash tool's deploy semantics: skip already-deployed
//! entries, back up an existing target before overwriting (only with `force`),
//! create parent directories, and recursively copy for `mode = "copy"`.

use crate::{DeployStatus, Entry, Mode, deploy_status};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Knobs for a deploy run, matching the bash `deploy` flags.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeployOptions {
    /// Compute outcomes without touching the filesystem.
    pub dry_run: bool,
    /// Back up and overwrite an existing target instead of skipping it.
    pub force: bool,
}

/// What a deploy attempt did (or, under `dry_run`, would do) for one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployOutcome {
    /// Already correctly deployed; nothing to do.
    AlreadyDeployed,
    /// Linked or copied into place. `backed_up` is set when an existing target
    /// was moved aside first.
    Deployed { backed_up: Option<PathBuf> },
    /// A target exists and `force` was not set — skipped to avoid clobbering.
    Skipped { reason: String },
    /// The entry is disabled; deploy skips it.
    Disabled,
    /// The source is missing in the repo; cannot deploy.
    SourceMissing,
    /// A filesystem operation failed.
    Error { message: String },
}

/// Where backups of overwritten targets land: `$HOME/.dotfiles-backup`.
pub fn backup_dir(home: &Path) -> PathBuf {
    home.join(".dotfiles-backup")
}

/// Deploy one entry. Pure of any interactive I/O — callers report the outcome.
pub fn deploy_entry(
    entry: &Entry,
    repo_root: &Path,
    home: &Path,
    opts: DeployOptions,
) -> DeployOutcome {
    if !entry.enabled {
        return DeployOutcome::Disabled;
    }

    let source = repo_root.join(&entry.path);
    let target = home.join(&entry.target);

    if !source.exists() {
        return DeployOutcome::SourceMissing;
    }

    // Already correctly linked? (Copy mode never reports Linked, so it always
    // falls through to the existence check below.)
    if entry.mode == Mode::Symlink && deploy_status(entry, repo_root, home) == DeployStatus::Linked
    {
        return DeployOutcome::AlreadyDeployed;
    }

    // Handle anything already sitting at the target.
    let exists = match std::fs::symlink_metadata(&target) {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return DeployOutcome::Error { message: e.to_string() },
    };

    let mut backed_up = None;
    if exists {
        if !opts.force {
            return DeployOutcome::Skipped {
                reason: format!("target exists at ~/{} (use --force)", entry.target),
            };
        }
        let dest = backup_path(home, &target);
        if !opts.dry_run {
            let dir = backup_dir(home);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return DeployOutcome::Error { message: e.to_string() };
            }
            if let Err(e) = std::fs::rename(&target, &dest) {
                return DeployOutcome::Error { message: e.to_string() };
            }
        }
        backed_up = Some(dest);
    }

    // Ensure the parent directory exists.
    if let Some(parent) = target.parent()
        && !opts.dry_run
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return DeployOutcome::Error { message: e.to_string() };
    }

    if opts.dry_run {
        return DeployOutcome::Deployed { backed_up };
    }

    let result = match entry.mode {
        Mode::Symlink => symlink(&source, &target),
        Mode::Copy => copy_recursive(&source, &target),
    };
    match result {
        Ok(()) => DeployOutcome::Deployed { backed_up },
        Err(e) => DeployOutcome::Error { message: e.to_string() },
    }
}

/// Backup destination for an overwritten target: `<backup>/<basename>.<epoch>`.
///
/// The suffix is whole seconds since the Unix epoch — unique per target within a
/// run and sortable, which is all a recovery artifact needs. (The bash tool used
/// a local `YYYYmmdd_HHMMSS` stamp; the location is identical.)
fn backup_path(home: &Path, target: &Path) -> PathBuf {
    let base = target
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "backup".into());
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    backup_dir(home).join(format!("{base}.{secs}"))
}

#[cfg(unix)]
fn symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(not(unix))]
fn symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(source, target)
}

/// Recursively copy a file or directory tree (for `mode = "copy"`).
///
/// On Unix, executable bits on `*.sh` files are preserved by the standard copy;
/// the bash tool additionally forced `+x` on shell scripts inside copied git
/// repos, which is not reproduced here (copy mode is unused by the live manifest).
fn copy_recursive(source: &Path, target: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(source)?;
    if meta.is_dir() {
        std::fs::create_dir_all(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &target.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(source, target)?;
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dft-deploy-{tag}-{}", std::process::id()))
    }

    fn entry(path: &str, target: &str, mode: Mode) -> Entry {
        Entry {
            name: "e".into(),
            path: path.into(),
            target: target.into(),
            enabled: true,
            mode,
            why: None,
            spec: None,
            profiles: vec![],
        }
    }

    #[test]
    fn symlinks_and_is_idempotent() {
        let base = tmp("link");
        let repo = base.join("repo");
        let home = base.join("home");
        std::fs::create_dir_all(repo.join("zsh")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(repo.join("zsh/.zshrc"), "x").unwrap();
        let e = entry("zsh/.zshrc", ".zshrc", Mode::Symlink);

        let out = deploy_entry(&e, &repo, &home, DeployOptions::default());
        assert_eq!(out, DeployOutcome::Deployed { backed_up: None });
        assert!(home.join(".zshrc").is_symlink());

        // Second run is a no-op.
        let again = deploy_entry(&e, &repo, &home, DeployOptions::default());
        assert_eq!(again, DeployOutcome::AlreadyDeployed);

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn refuses_existing_target_without_force_then_backs_up_with_it() {
        let base = tmp("force");
        let repo = base.join("repo");
        let home = base.join("home");
        std::fs::create_dir_all(repo.join("zsh")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(repo.join("zsh/.zshrc"), "managed").unwrap();
        std::fs::write(home.join(".zshrc"), "pre-existing").unwrap();
        let e = entry("zsh/.zshrc", ".zshrc", Mode::Symlink);

        let skipped = deploy_entry(&e, &repo, &home, DeployOptions::default());
        assert!(matches!(skipped, DeployOutcome::Skipped { .. }));
        // Untouched.
        assert_eq!(std::fs::read_to_string(home.join(".zshrc")).unwrap(), "pre-existing");

        let forced = deploy_entry(&e, &repo, &home, DeployOptions { force: true, dry_run: false });
        match forced {
            DeployOutcome::Deployed { backed_up: Some(p) } => {
                assert_eq!(std::fs::read_to_string(&p).unwrap(), "pre-existing");
            }
            other => panic!("expected backed-up deploy, got {other:?}"),
        }
        assert!(home.join(".zshrc").is_symlink());

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn dry_run_changes_nothing() {
        let base = tmp("dry");
        let repo = base.join("repo");
        let home = base.join("home");
        std::fs::create_dir_all(repo.join("zsh")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(repo.join("zsh/.zshrc"), "x").unwrap();
        let e = entry("zsh/.zshrc", ".zshrc", Mode::Symlink);

        let out = deploy_entry(&e, &repo, &home, DeployOptions { dry_run: true, force: false });
        assert_eq!(out, DeployOutcome::Deployed { backed_up: None });
        assert!(!home.join(".zshrc").exists());

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn disabled_and_missing_source_are_reported() {
        let base = tmp("states");
        let repo = base.join("repo");
        let home = base.join("home");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        let mut e = entry("nope/.x", ".x", Mode::Symlink);
        assert_eq!(deploy_entry(&e, &repo, &home, DeployOptions::default()), DeployOutcome::SourceMissing);

        e.enabled = false;
        assert_eq!(deploy_entry(&e, &repo, &home, DeployOptions::default()), DeployOutcome::Disabled);

        std::fs::remove_dir_all(&base).ok();
    }
}
