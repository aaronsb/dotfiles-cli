//! The `pkg` verb — per-host package tracking (pacman / AUR / flatpak), ported
//! from the bash `lib/pkg.sh`.
//!
//! Tracked lists live under `<repo>/packages/<host>/{native,aur,flatpak}.txt` and
//! are the *desired* state; a live query is the *actual* state. This module does
//! the I/O (shelling out to the package managers, reading/writing the tracked
//! files) and delegates the set math to [`dotfiles_core::pkg`].

use crate::Ctx;
use dotfiles_core::pkg::{self, Source, normalize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Arguments to the `pkg` verb: `pkg [capture|status|sync|diff] [--host N]
/// [--prune] [host-args...]`. A missing action defaults to `status`.
#[derive(clap::Args)]
pub struct PkgArgs {
    /// Subcommand: `capture`, `status`, `sync`, or `diff`.
    #[arg(default_value = "status")]
    action: String,
    /// Inspect another host's tracked lists (capture/status), or a diff target.
    #[arg(long)]
    host: Option<String>,
    /// `sync` only: also remove untracked (live-but-not-tracked) packages.
    #[arg(long)]
    prune: bool,
    /// `diff` host arguments: none = all hosts, 1 = this vs HOST, 2 = A B.
    #[arg(trailing_var_arg = true)]
    args: Vec<String>,
}

/// Dispatch the `pkg` verb.
pub fn run(ctx: &Ctx, args: &PkgArgs) -> anyhow::Result<()> {
    let packages_dir = ctx.repo_root.join("packages");
    let local = short_hostname();
    let host = args.host.clone().unwrap_or_else(|| local.clone());

    match args.action.as_str() {
        "capture" => capture(&packages_dir, &host),
        "status" => status(&packages_dir, &host, &local),
        "sync" => sync(&packages_dir, &host, &local, args.prune),
        "diff" => diff(&packages_dir, &local, &args.args),
        other => anyhow::bail!(
            "unknown pkg subcommand '{other}' — try: pkg [capture|status|sync|diff] [--host <name>] [--prune]"
        ),
    }
}

/// `pkg capture` — write the live list of every available source to disk.
fn capture(packages_dir: &Path, host: &str) -> anyhow::Result<()> {
    let dir = packages_dir.join(host);
    println!("=== Capturing packages for '{host}' ===\n");
    std::fs::create_dir_all(&dir)?;

    let mut captured = false;
    for src in Source::ALL {
        if !source_available(src) {
            println!("{}: not available here — skipped", src.name());
            continue;
        }
        let list = live_list(src);
        let file = dir.join(format!("{}.txt", src.name()));
        let body = if list.is_empty() { String::new() } else { format!("{}\n", list.join("\n")) };
        std::fs::write(&file, body)?;
        println!("{}: wrote {} package(s) -> packages/{host}/{}.txt", src.name(), list.len(), src.name());
        captured = true;
    }

    println!();
    if captured {
        println!("Review with `git diff`, then `dotfiles push` to record.");
    } else {
        println!("No supported package managers found on this host.");
    }
    Ok(())
}

/// `pkg status` — per-source drift between tracked and live.
fn status(packages_dir: &Path, host: &str, local: &str) -> anyhow::Result<()> {
    let is_local = host == local;
    println!("=== Package status for '{host}' ===");
    if !is_local {
        println!("(remote host — showing tracked lists only, no live diff)");
    }
    println!();

    // Remote host: report tracked desired state only.
    if !is_local {
        let mut any = false;
        for src in Source::ALL {
            let tracked = read_tracked(packages_dir, host, src);
            if !tracked_file_exists(packages_dir, host, src) {
                continue;
            }
            any = true;
            println!("{}: {} tracked package(s)", src.name(), tracked.len());
        }
        println!();
        if !any {
            println!("No tracked lists for '{host}' yet — run `dotfiles pkg capture` on it.");
        }
        return Ok(());
    }

    let mut any = false;
    for src in Source::ALL {
        let has_file = tracked_file_exists(packages_dir, host, src);
        let available = source_available(src);
        if !has_file && !available {
            continue; // not relevant to this host
        }
        any = true;
        if !has_file {
            println!("{}: installed here but not tracked — run `dotfiles pkg capture`", src.name());
            continue;
        }
        if !available {
            println!("{}: tracked, but {} not installed on this host", src.name(), src.name());
            continue;
        }
        let d = pkg::drift(&read_tracked(packages_dir, host, src), &live_list(src));
        if d.in_sync() {
            println!("{}: in sync", src.name());
            continue;
        }
        println!("{}:", src.name());
        if !d.missing.is_empty() {
            println!("  tracked but not installed (sync installs):");
            for p in &d.missing {
                println!("    + {p}");
            }
        }
        if !d.extra.is_empty() {
            println!("  installed but not tracked (capture records / --prune removes):");
            for p in &d.extra {
                println!("    - {p}");
            }
        }
    }

    println!();
    if !any {
        println!("No tracked lists or supported package managers for '{host}'.");
    }
    Ok(())
}

/// `pkg sync` — install tracked-but-missing; with `--prune`, remove untracked.
/// Guarded to the local host, since it mutates the live system.
fn sync(packages_dir: &Path, host: &str, local: &str, prune: bool) -> anyhow::Result<()> {
    if host != local {
        anyhow::bail!("refusing to sync: target host '{host}' is not this machine ('{local}'). sync changes the live system, so it only runs locally.");
    }
    println!("=== Syncing packages for '{host}' ===\n");

    let mut acted = false;
    for src in Source::ALL {
        if !tracked_file_exists(packages_dir, host, src) {
            continue;
        }
        if !source_available(src) {
            println!("{}: tracked but {} not installed here — skipped", src.name(), src.name());
            continue;
        }
        let d = pkg::drift(&read_tracked(packages_dir, host, src), &live_list(src));
        if !d.missing.is_empty() {
            acted = true;
            println!("{}: installing {} missing package(s)...", src.name(), d.missing.len());
            install(src, &d.missing)?;
        }
        if prune && !d.extra.is_empty() {
            acted = true;
            println!("{}: removing {} untracked package(s)...", src.name(), d.extra.len());
            remove(src, &d.extra)?;
        }
    }

    println!();
    if acted {
        println!("Sync complete.");
    } else {
        println!("Nothing to do — already in sync.");
    }
    Ok(())
}

/// `pkg diff` — 0 args: N-way across all tracked hosts; 1: this vs HOST; 2: A B.
fn diff(packages_dir: &Path, local: &str, host_args: &[String]) -> anyhow::Result<()> {
    match host_args.len() {
        0 => diff_all(packages_dir),
        1 => diff_pair(packages_dir, local, &host_args[0]),
        2 => diff_pair(packages_dir, &host_args[0], &host_args[1]),
        _ => anyhow::bail!("diff takes 0 args (all hosts), 1 (this host vs HOST), or 2 (A B)."),
    }
}

/// Pairwise diff of two hosts, per source.
fn diff_pair(packages_dir: &Path, a: &str, b: &str) -> anyhow::Result<()> {
    for host in [a, b] {
        if !packages_dir.join(host).is_dir() {
            anyhow::bail!("no tracked packages for host '{host}'.");
        }
    }
    println!("=== {a} <-> {b} ===\n");
    for src in Source::ALL {
        let la = read_tracked(packages_dir, a, src);
        let lb = read_tracked(packages_dir, b, src);
        let p = pkg::pair_diff(&la, &lb);
        println!("{:<8} {} shared   {} {a}-only   {} {b}-only", src.name(), p.shared, p.only_a.len(), p.only_b.len());
        if !p.only_a.is_empty() {
            println!("  only on {a}: {}", p.only_a.join(" "));
        }
        if !p.only_b.is_empty() {
            println!("  only on {b}: {}", p.only_b.join(" "));
        }
        println!();
    }
    Ok(())
}

/// N-way diff across every tracked host, per source.
fn diff_all(packages_dir: &Path) -> anyhow::Result<()> {
    let hosts = tracked_hosts(packages_dir);
    if hosts.len() < 2 {
        anyhow::bail!("need at least 2 tracked hosts to diff (have {}).", hosts.len());
    }
    println!("=== packages across {} hosts: {} ===\n", hosts.len(), hosts.join(" · "));
    for src in Source::ALL {
        println!("{}", src.name());
        let lists: Vec<(String, Vec<String>)> = hosts
            .iter()
            .map(|h| (h.clone(), read_tracked(packages_dir, h, src)))
            .collect();
        let n = pkg::nway_diff(&lists);
        let participating = n.unique.len();
        if participating == 0 {
            println!("  (no host has any {} packages)\n", src.name());
            continue;
        }
        println!("  ● {} common to all {participating}", n.common);
        for (host, uniq) in &n.unique {
            println!("  ◆ {host:<8} {} unique", uniq.len());
            if !uniq.is_empty() {
                println!("      {}", uniq.join(" "));
            }
        }
        println!();
    }
    Ok(())
}

// --- I/O helpers ----------------------------------------------------------

/// Path to a tracked list file.
fn tracked_path(packages_dir: &Path, host: &str, src: Source) -> PathBuf {
    packages_dir.join(host).join(format!("{}.txt", src.name()))
}

fn tracked_file_exists(packages_dir: &Path, host: &str, src: Source) -> bool {
    tracked_path(packages_dir, host, src).is_file()
}

/// Read a tracked list (empty if the file is absent).
fn read_tracked(packages_dir: &Path, host: &str, src: Source) -> Vec<String> {
    match std::fs::read_to_string(tracked_path(packages_dir, host, src)) {
        Ok(s) => normalize(&s),
        Err(_) => Vec::new(),
    }
}

/// Tracked hosts = subdirectories of `packages/`, sorted.
fn tracked_hosts(packages_dir: &Path) -> Vec<String> {
    let mut hosts: Vec<String> = match std::fs::read_dir(packages_dir) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    hosts.sort();
    hosts
}

/// Short hostname (cut at the first dot), matching the bash `${HOST%%.*}` rule.
fn short_hostname() -> String {
    let raw = std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            Command::new("uname")
                .arg("-n")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .unwrap_or_default();
    raw.split('.').next().unwrap_or("").to_string()
}

/// First available AUR helper, if any.
fn aur_helper() -> Option<&'static str> {
    ["paru", "yay"].into_iter().find(|h| on_path(h))
}

/// Is a package source usable on this machine?
fn source_available(src: Source) -> bool {
    match src {
        Source::Native => on_path("pacman"),
        Source::Aur => aur_helper().is_some(),
        Source::Flatpak => on_path("flatpak"),
    }
}

/// Live (actual) explicitly-installed list for a source, sorted.
fn live_list(src: Source) -> Vec<String> {
    match src {
        Source::Native => query("pacman", &["-Qqen"]),
        Source::Aur => query("pacman", &["-Qqem"]),
        Source::Flatpak => query("flatpak", &["list", "--app", "--columns=application"]),
    }
}

/// Run a query command and normalize its stdout (empty on spawn failure;
/// a non-zero exit with empty output — e.g. `pacman -Qqem` with no foreign
/// packages — yields an empty list, not an error).
fn query(cmd: &str, args: &[&str]) -> Vec<String> {
    match Command::new(cmd).args(args).output() {
        Ok(o) => normalize(&String::from_utf8_lossy(&o.stdout)),
        Err(_) => Vec::new(),
    }
}

/// Does `name` resolve on `$PATH`?
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
    })
}

/// Install a package set (additive). Stdio is inherited so the package
/// manager's confirmation prompt keeps the terminal.
fn install(src: Source, pkgs: &[String]) -> anyhow::Result<()> {
    match src {
        Source::Native => run_inherit("sudo", &["pacman", "-S", "--needed"], pkgs),
        Source::Aur => {
            let helper = aur_helper().ok_or_else(|| anyhow::anyhow!("no AUR helper found"))?;
            run_inherit(helper, &["-S", "--needed"], pkgs)
        }
        Source::Flatpak => run_inherit("flatpak", &["install", "-y", "flathub"], pkgs),
    }
}

/// Remove a package set. `pacman -Rns` also removes orphaned deps — confirm at
/// the prompt. Stdio inherited for the same reason as [`install`].
fn remove(src: Source, pkgs: &[String]) -> anyhow::Result<()> {
    match src {
        Source::Native | Source::Aur => run_inherit("sudo", &["pacman", "-Rns"], pkgs),
        Source::Flatpak => run_inherit("flatpak", &["uninstall"], pkgs),
    }
}

/// Run `cmd args... pkgs...` with inherited stdio.
fn run_inherit(cmd: &str, args: &[&str], pkgs: &[String]) -> anyhow::Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .args(pkgs)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| anyhow::anyhow!("running {cmd}: {e}"))?;
    if !status.success() {
        anyhow::bail!("{cmd} exited with failure");
    }
    Ok(())
}
