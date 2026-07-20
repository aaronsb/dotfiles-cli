//! Reconcile the running binary against the store's `.dotfiles-cli.version`
//! pin (ADR-200).
//!
//! The store and the CLI that projects it version independently, and only the
//! store half is reachable by a routine the operator runs. `pull` is that
//! routine, so it is where drift gets caught: after fast-forwarding the store,
//! compare the running binary to the pin and swap it if they differ.
//!
//! Every failure here is advisory. The git half of the pull has already
//! succeeded by the time this runs, so a missing `curl`, an unreachable
//! release host, or a read-only bin directory degrades to a warning naming the
//! drift and the manual remedy — never a non-zero exit.

use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "aaronsb/dotfiles-cli";
const PIN_FILE: &str = ".dotfiles-cli.version";

/// The release asset for this platform, or `None` where no prebuilt exists.
///
/// Mirrors the case list in the upstream `install.sh`; a platform absent from
/// both builds from source and is expected to opt out of pin enforcement.
fn asset_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("dotfiles-x86_64-linux"),
        _ => None,
    }
}

/// Strip a leading `v` so `v0.5.0` and `0.5.0` compare equal.
fn normalize(v: &str) -> &str {
    v.trim().strip_prefix('v').unwrap_or(v.trim())
}

/// Read the store's pinned version, if the store declares one.
///
/// A store with no pin file predates ADR-200 and opts out: absent and
/// unreadable are the same answer here, since neither is a drift signal.
fn read_pin(repo_root: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(repo_root.join(PIN_FILE)).ok()?;
    let pin = raw.trim().to_string();
    (!pin.is_empty()).then_some(pin)
}

/// Reconcile the running binary against the store's pin.
///
/// Called on *both* pull paths — after a merge and on the already-up-to-date
/// early return. A machine whose git state is current and whose binary is
/// stale is exactly the case that motivated this (ADR-200), and a check gated
/// on a successful merge would never reach it.
pub fn reconcile(repo_root: &Path) {
    let Some(pin) = read_pin(repo_root) else {
        return;
    };
    let running = env!("CARGO_PKG_VERSION");
    if normalize(&pin) == normalize(running) {
        return;
    }

    println!("\nCLI drift: running {running}, store pins {pin}");

    let Some(asset) = asset_name() else {
        warn(&pin, "no prebuilt binary for this platform");
        return;
    };
    let Ok(target) = std::env::current_exe() else {
        warn(&pin, "could not locate the running executable");
        return;
    };

    match swap(&pin, asset, &target) {
        Ok(()) => {
            println!("updated {} to {pin}", target.display());
            println!("(this process is still running {running})");
        }
        Err(e) => warn(&pin, &e.to_string()),
    }
}

/// Report drift we could not heal, naming the manual remedy.
///
/// The pull itself has succeeded; this is the only place the operator learns
/// the binary half is behind, so it must say what to run.
fn warn(pin: &str, reason: &str) {
    eprintln!("warning: could not self-update ({reason})");
    eprintln!("  install {pin} manually: ./install.sh");
}

/// Download the pinned release and atomically replace `target`.
///
/// The temp file is written *beside* the target so it lands on the same
/// filesystem and `rename` is atomic. On Linux this replaces the path while
/// the running process keeps its original inode, so a failed download can
/// never leave a half-written executable in place.
fn swap(pin: &str, asset: &str, target: &Path) -> anyhow::Result<()> {
    let dir = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("executable has no parent directory"))?;

    // Fail before the network round-trip if we could not install anyway.
    if !writable(dir) {
        anyhow::bail!("{} is not writable", dir.display());
    }

    let url = format!("https://github.com/{REPO}/releases/download/{pin}/{asset}");
    let tmp: PathBuf = dir.join(format!(".dotfiles-{pin}.tmp"));

    let out = Command::new("curl")
        .args(["-fsSL", "--retry", "2", "-o"])
        .arg(&tmp)
        .arg(&url)
        .output();

    let out = match out {
        Ok(o) => o,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            anyhow::bail!("curl unavailable: {e}");
        }
    };
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("download failed — is {pin} a published release?");
    }

    // Guard against a "successful" download of something that isn't a binary
    // (a proxy error page, an empty body) before it replaces a working CLI.
    match std::fs::metadata(&tmp) {
        Ok(m) if m.len() > 1024 => {}
        _ => {
            let _ = std::fs::remove_file(&tmp);
            anyhow::bail!("downloaded asset looks truncated");
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }

    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("could not replace {}: {e}", target.display());
    }
    Ok(())
}

/// Whether we can create and remove a file in `dir`.
///
/// Probing beats reading permission bits: it accounts for read-only mounts and
/// the effective user, which a mode check alone would miss.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(".dotfiles-write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dft-selfupdate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn normalize_strips_the_v_prefix_and_whitespace() {
        assert_eq!(normalize("v0.5.0"), "0.5.0");
        assert_eq!(normalize("0.5.0"), "0.5.0");
        assert_eq!(normalize(" v0.5.0\n"), "0.5.0");
        // The pin and a binary's reported version must compare equal across
        // both spellings — this is the whole comparison.
        assert_eq!(normalize("v0.5.0"), normalize("0.5.0"));
    }

    #[test]
    fn a_store_without_a_pin_opts_out() {
        let d = tmp("nopin");
        assert_eq!(read_pin(&d), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_empty_pin_is_treated_as_absent() {
        let d = tmp("emptypin");
        std::fs::write(d.join(PIN_FILE), "  \n").unwrap();
        assert_eq!(read_pin(&d), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_pin_is_read_trimmed() {
        let d = tmp("goodpin");
        std::fs::write(d.join(PIN_FILE), "v0.5.0\n").unwrap();
        assert_eq!(read_pin(&d).as_deref(), Some("v0.5.0"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn writable_probe_leaves_nothing_behind() {
        let d = tmp("probe");
        assert!(writable(&d));
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unwritable_dir_fails_before_the_network() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("readonly");
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o500)).unwrap();
        assert!(!writable(&d));
        // swap() must refuse on the permission check rather than downloading;
        // a bogus tag would fail at curl if the ordering were reversed.
        let err = swap("v9.9.9", "asset", &d.join("dotfiles")).unwrap_err();
        assert!(err.to_string().contains("not writable"), "{err}");
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }
}
