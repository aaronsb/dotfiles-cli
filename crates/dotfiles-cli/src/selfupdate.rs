//! Reconcile the running binary against the store's `.dotfiles-cli.version`
//! pin (ADR-200).
//!
//! The store and the CLI that projects it version independently, and only the
//! store half is reachable by a routine the operator runs. `pull` is that
//! routine, so it is where drift gets caught: after fast-forwarding the store,
//! compare the running binary to the pin and swap it if they differ.
//!
//! The pin is attacker-adjacent input. It is one line in a file that reviewers
//! skim, it is interpolated into a download URL, and the result is chmod'd and
//! put on `$PATH` — so it is parsed into a validated `Pin` before any of that,
//! never used raw.
//!
//! Every failure here is advisory. The git half of the pull has already
//! succeeded by the time this runs, so a missing `curl`, an unreachable
//! release host, or a read-only bin directory degrades to a warning naming the
//! drift and the manual remedy — never a non-zero exit.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "aaronsb/dotfiles-cli";
const PIN_FILE: &str = ".dotfiles-cli.version";
/// Set to any non-empty value to keep `pull` from touching the binary.
const OPT_OUT: &str = "DOTFILES_NO_SELF_UPDATE";
/// Every ELF binary starts with this; an HTML error page does not.
const ELF_MAGIC: &[u8; 4] = b"\x7fELF";

/// A validated release version.
///
/// Holding the parsed form (rather than the raw string) is what makes the URL
/// safe to build: a value of this type cannot contain `/` or `..`, so it cannot
/// steer the download at a different repo. Construct only via [`Pin::parse`].
#[derive(Debug, PartialEq, Eq)]
struct Pin {
    /// Version without the `v`, e.g. `0.5.0` or `0.5.0-rc1`.
    core: String,
}

impl Pin {
    /// Parse a pin, accepting `v0.5.0` / `0.5.0` / `V0.5.0` and rejecting
    /// everything else.
    ///
    /// Deliberately strict rather than merely sanitizing: an allowlist of
    /// `MAJOR.MINOR.PATCH[-prerelease]` cannot express a path segment, so
    /// traversal is impossible by construction instead of by filtering.
    fn parse(raw: &str) -> Option<Self> {
        let t = raw.trim();
        let s = t
            .strip_prefix('v')
            .or_else(|| t.strip_prefix('V'))
            .unwrap_or(t);
        let (version, pre) = match s.split_once('-') {
            Some((v, p)) => (v, Some(p)),
            None => (s, None),
        };

        let mut parts = version.split('.');
        let (major, minor, patch) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        for n in [major, minor, patch] {
            if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
        }
        // Pre-release stays alphanumeric + `.`; no `/`, no `..`.
        if let Some(p) = pre
            && (p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.'))
        {
            return None;
        }

        Some(Pin {
            core: s.to_string(),
        })
    }

    /// The git tag to download, always `v`-prefixed.
    ///
    /// Releases are tagged `vX.Y.Z`, so a store pinned `0.5.0` must still
    /// resolve. Comparing one spelling while fetching another is what made a
    /// bare pin compare clean forever and then 404 on the first real drift.
    fn tag(&self) -> String {
        format!("v{}", self.core)
    }

    /// Whether this pin denotes the same release as a reported version string.
    fn matches(&self, running: &str) -> bool {
        let r = running.trim();
        self.core == r.strip_prefix('v').unwrap_or(r)
    }

    /// `(major, minor, patch)` for ordering.
    ///
    /// Numeric, not lexicographic: `0.10.0` sorts *after* `0.9.0`, which a
    /// string compare gets backwards and would report as a downgrade.
    fn nums(&self) -> (u64, u64, u64) {
        let mut it = self
            .core
            .split('-')
            .next()
            .unwrap_or_default()
            .split('.')
            .map(|n| n.parse().unwrap_or(0));
        (
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
        )
    }

    /// Whether installing this pin would move the operator backward.
    ///
    /// Rolling the pin back is legitimate but surprising — it downgrades every
    /// machine on its next pull — so the swap says which direction it went.
    fn is_downgrade_from(&self, running: &str) -> bool {
        match Pin::parse(running) {
            Some(cur) => self.nums() < cur.nums(),
            None => false,
        }
    }
}

impl fmt::Display for Pin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.tag())
    }
}

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

/// Read the store's raw pin line, if the store declares one.
///
/// A store with no pin file predates ADR-200 and opts out: absent and
/// unreadable are the same answer here, since neither is a drift signal.
fn read_pin(repo_root: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(repo_root.join(PIN_FILE)).ok()?;
    let pin = raw.trim().to_string();
    (!pin.is_empty()).then_some(pin)
}

/// Whether `exe` looks like a cargo build artifact rather than an install.
///
/// `current_exe()` reads `/proc/self/exe`, which is fully resolved — so for a
/// developer whose `~/.local/bin/dotfiles` symlinks into `target/release/`,
/// a naive swap silently overwrites their build output and reappears on every
/// pull. Detect that shape and decline.
fn is_build_artifact(exe: &Path) -> bool {
    let mut comps = exe.components().rev().skip(1); // skip the file name
    matches!(comps.next().map(|c| c.as_os_str().to_owned()), Some(p) if p == "debug" || p == "release")
        && comps.next().is_some_and(|c| c.as_os_str() == "target")
}

/// Reconcile the running binary against the store's pin.
///
/// Called on every `pull` exit path, including the already-up-to-date early
/// return and the diverged-branch bail. A machine whose git state is current
/// (or stuck) and whose binary is stale is exactly the case that motivated
/// this (ADR-200); a check gated on a successful merge would never reach it.
pub fn reconcile(repo_root: &Path) {
    if std::env::var_os(OPT_OUT).is_some_and(|v| !v.is_empty()) {
        return;
    }
    let Some(raw) = read_pin(repo_root) else {
        return;
    };
    let running = env!("CARGO_PKG_VERSION");

    let Some(pin) = Pin::parse(&raw) else {
        eprintln!("\nwarning: {PIN_FILE} is not a version: {raw:?}");
        eprintln!("  expected something like v0.5.0 — refusing to fetch it");
        return;
    };
    if pin.matches(running) {
        return;
    }

    let direction = if pin.is_downgrade_from(running) {
        "DOWNGRADE"
    } else {
        "update"
    };
    println!("\nCLI drift: running {running}, store pins {pin} ({direction})");

    let Some(asset) = asset_name() else {
        // Nothing to install and no remedy that could work, so say that
        // plainly rather than pointing at an installer that also can't help.
        eprintln!(
            "  no prebuilt binary for {}/{} — build from source, or set {OPT_OUT}=1 to silence",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        return;
    };
    let Ok(target) = std::env::current_exe() else {
        warn(repo_root, &pin, "could not locate the running executable");
        return;
    };
    if is_build_artifact(&target) {
        eprintln!(
            "  {} is a build artifact — leaving it alone (set {OPT_OUT}=1 to silence)",
            target.display()
        );
        return;
    }

    match swap(&pin, asset, &target) {
        Ok(()) => {
            println!("updated {} to {pin}", target.display());
            println!("this shell is still running {running} — rerun your command to use {pin}");
        }
        Err(e) => warn(repo_root, &pin, &e.to_string()),
    }
}

/// Report drift we could not heal, naming the manual remedy.
///
/// The pull itself has succeeded; this is the only place the operator learns
/// the binary half is behind, so it must say what to run — with an absolute
/// path, since `pull` works from anywhere in the store.
fn warn(repo_root: &Path, pin: &Pin, reason: &str) {
    eprintln!("warning: could not self-update ({reason})");
    eprintln!(
        "  install {pin} manually: {}",
        repo_root.join("install.sh").display()
    );
}

/// Download the pinned release and atomically replace `target`.
///
/// The temp file is written *beside* the target so it lands on the same
/// filesystem and `rename` is atomic. On Linux this replaces the path while
/// the running process keeps its original inode, so a failed download can
/// never leave a half-written executable in place.
fn swap(pin: &Pin, asset: &str, target: &Path) -> anyhow::Result<()> {
    let dir = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("executable has no parent directory"))?;

    // Fail before the network round-trip if we could not install anyway.
    if !writable(dir) {
        anyhow::bail!("{} is not writable", dir.display());
    }

    let url = format!(
        "https://github.com/{REPO}/releases/download/{}/{asset}",
        pin.tag()
    );
    // PID-scoped: two concurrent pulls must not share a download buffer, or
    // one renames a file the other is still writing into.
    let tmp: PathBuf = dir.join(format!(
        ".dotfiles-{}-{}.tmp",
        pin.tag(),
        std::process::id()
    ));

    let result = fetch(&url, &tmp)
        .and_then(|()| verify(&tmp))
        .and_then(|()| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
            }
            std::fs::rename(&tmp, target)
                .map_err(|e| anyhow::anyhow!("could not replace {}: {e}", target.display()))
        });

    // One cleanup path for every failure mode, so no error branch can leak a
    // multi-megabyte temp file into the operator's bin directory.
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Fetch `url` into `tmp`.
fn fetch(url: &str, tmp: &Path) -> anyhow::Result<()> {
    let out = Command::new("curl")
        .args(["-fsSL", "--retry", "2", "-o"])
        .arg(tmp)
        .arg(url)
        .output()
        .map_err(|e| anyhow::anyhow!("curl unavailable: {e}"))?;
    if !out.status.success() {
        anyhow::bail!("download failed — is this a published release?");
    }
    Ok(())
}

/// Confirm the download is actually an executable before it replaces the CLI.
///
/// `curl -f` rejects HTTP errors, but a captive portal or MITM proxy can
/// answer 200 with an HTML interstitial. Installing that bricks the CLI —
/// including the `pull` that would otherwise heal it — so check the magic
/// bytes, not just the length.
fn verify(tmp: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(tmp).map_err(|e| anyhow::anyhow!("unreadable download: {e}"))?;
    if bytes.len() < 1024 {
        anyhow::bail!("downloaded asset looks truncated");
    }
    if !bytes.starts_with(ELF_MAGIC) {
        anyhow::bail!("downloaded asset is not a Linux executable (captive portal?)");
    }
    Ok(())
}

/// Whether we can create and remove a file in `dir`.
///
/// Probing beats reading permission bits: it accounts for read-only mounts and
/// the effective user, which a mode check alone would miss.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".dotfiles-write-probe-{}", std::process::id()));
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
    fn accepts_both_spellings_and_fetches_the_tagged_one() {
        // The bug this encodes: `0.5.0` compares clean but the release is
        // tagged `v0.5.0`, so fetching the raw pin 404s forever.
        for raw in ["v0.5.0", "0.5.0", "V0.5.0", " v0.5.0\n"] {
            let pin = Pin::parse(raw).unwrap_or_else(|| panic!("{raw} should parse"));
            assert_eq!(pin.tag(), "v0.5.0", "{raw}");
            assert!(pin.matches("0.5.0") && pin.matches("v0.5.0"), "{raw}");
        }
    }

    #[test]
    fn rejects_a_pin_that_could_steer_the_download() {
        // curl collapses dot-segments before the request, so `..` in a pin
        // redirects to an arbitrary repo on the same host — and the result is
        // chmod'd and put on $PATH. These must never reach a URL.
        for evil in [
            "../../../../attacker/repo/releases/download/v1",
            "..",
            "v0.5.0/../../../evil",
            "0.5.0/extra",
            "latest",
            "v0.5.0 ; rm -rf /",
            "",
            "v1.2",
            "v1.2.3.4",
            "v1.2.x",
        ] {
            assert_eq!(Pin::parse(evil), None, "{evil:?} must be rejected");
        }
    }

    #[test]
    fn downgrade_detection_orders_numerically() {
        // The lexicographic trap: "0.10.0" < "0.9.0" as strings.
        assert!(!Pin::parse("v0.10.0").unwrap().is_downgrade_from("0.9.0"));
        assert!(Pin::parse("v0.9.0").unwrap().is_downgrade_from("0.10.0"));
        assert!(Pin::parse("v0.4.0").unwrap().is_downgrade_from("0.5.0"));
        assert!(!Pin::parse("v1.0.0").unwrap().is_downgrade_from("0.5.0"));
    }

    #[test]
    fn accepts_a_prerelease_tag() {
        let pin = Pin::parse("v0.6.0-rc1").unwrap();
        assert_eq!(pin.tag(), "v0.6.0-rc1");
        assert!(pin.matches("0.6.0-rc1"));
    }

    #[test]
    fn a_store_without_a_usable_pin_opts_out() {
        let d = tmp("nopin");
        assert_eq!(read_pin(&d), None);
        std::fs::write(d.join(PIN_FILE), "  \n").unwrap();
        assert_eq!(read_pin(&d), None, "an empty pin is treated as absent");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn build_artifacts_are_left_alone() {
        // A dev symlinking ~/.local/bin/dotfiles into target/release/ would
        // otherwise have their build output silently overwritten every pull.
        assert!(is_build_artifact(Path::new(
            "/home/a/src/cli/target/release/dotfiles"
        )));
        assert!(is_build_artifact(Path::new(
            "/home/a/src/cli/target/debug/dotfiles"
        )));
        assert!(!is_build_artifact(Path::new("/home/a/.local/bin/dotfiles")));
        assert!(!is_build_artifact(Path::new("/usr/local/bin/dotfiles")));
        // `target` alone isn't enough — only the cargo profile layout counts.
        assert!(!is_build_artifact(Path::new("/home/a/target/dotfiles")));
    }

    #[test]
    fn non_executables_are_refused_before_install() {
        let d = tmp("verify");
        let html = d.join("portal");
        std::fs::write(&html, vec![b'<'; 4096]).unwrap();
        let err = verify(&html).unwrap_err().to_string();
        assert!(err.contains("not a Linux executable"), "{err}");

        let short = d.join("short");
        std::fs::write(&short, b"\x7fELF").unwrap();
        assert!(
            verify(&short)
                .unwrap_err()
                .to_string()
                .contains("truncated")
        );

        let good = d.join("good");
        let mut body = ELF_MAGIC.to_vec();
        body.extend(std::iter::repeat_n(0u8, 2048));
        std::fs::write(&good, body).unwrap();
        assert!(verify(&good).is_ok());
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
        let pin = Pin::parse("v9.9.9").unwrap();
        let err = swap(&pin, "asset", &d.join("dotfiles")).unwrap_err();
        assert!(err.to_string().contains("not writable"), "{err}");
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }
}
