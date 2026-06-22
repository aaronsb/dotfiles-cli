# Goal: full bash parity for the Rust `dotfiles`, verified command-by-command

**Date:** 2026-06-22 · **Status:** in progress

## The goal

The Rust `dotfiles` should reach **capability parity** with the bash tool, and we
**prove it command-by-command** with a convergence harness: run each verb on both
tools against an identical sandbox and assert the *effects* match (filesystem,
manifest, git, package-list files) — not the stdout text, which differs (bash is
colored/prose, Rust is plain/structured).

Once parity holds and the harness is green, `dotfiles-bash` is retired.

## Verb matrix

| bash verb | Rust | Parity notes |
|---|---|---|
| `status`  | ✅ | human + `--format json`; convergence compares the JSON projection |
| `deploy`  | ✅ | `--dry-run`/`--force`; backups to `~/.dotfiles-backup` |
| `enable`  | ✅ | |
| `disable` | ✅ | also removes a live symlink |
| `add`     | ✅ | `<app> <system-path> [repo-path] --mode --why` |
| `list`    | ✅ | |
| `push`    | ✅ | `-m` required when dirty (agent-surface analog of the interactive prompt) |
| `pull`    | ⬜ | git fast-forward pull from `origin/<branch>` |
| `diff`    | ⬜ | preview local vs `origin/<branch>` (dirty + ahead/behind) |
| `pkg`     | ⬜ | `capture`/`status`/`sync`/`diff`, per-host, pacman/AUR/flatpak — largest gap |
| `update`  | ⬜ | see divergence below |
| `install` | ⬜ | see divergence below |
| `remove`  | ⚠️ | **naming clash** — see below |
| `help`    | ⬜ | render `HELP.md` (low value; `--help` already exists via clap) |

## Accepted divergences (not strict parity)

1. **Lifecycle verbs (`install`/`update`/`remove`-tool).** Bash installs by
   symlinking the script into `~/.local/bin` and self-updates via `git pull` of
   the store. The Rust binary installs from GitHub Releases and `.dotfiles` pins a
   version string — a different model (ADR-001/ADR-007). So these get
   *binary-appropriate* semantics, not byte-parity:
   - `install` → place/symlink the binary on PATH.
   - `update`  → fetch the pinned/latest release binary (not `git pull` + redeploy).
   - tool-`remove` → uninstall the binary.
2. **`remove` naming.** Bash `remove` = *uninstall the tool*. Rust `remove` =
   *drop a manifest entry* (the symmetric inverse of `add`, which bash lacks).
   **Resolution:** keep Rust `remove` = manifest-entry removal; tool uninstall is a
   packaging concern (`uninstall`/package manager), not a daily verb. Documented so
   the convergence harness doesn't compare them.
3. **Colored/prose output.** Rust output is plain and structured; bash is colored
   prose. The harness asserts *effect* equivalence, not stdout bytes.

## Convergence harness (the verification backbone)

A script in the tool repo (`tests/convergence/` or `scripts/converge.sh`) that:

1. Builds the Rust `dotfiles`; locates `dotfiles-bash`.
2. Stands up a **sandbox** twice (one per tool): a temp "dotfiles store" git repo,
   a temp `$HOME`, and a **bare git remote**. Seeds config files + both manifest
   formats (pipe for bash, TOML for Rust) describing the same entries.
3. Runs a **scripted verb sequence** on each tool, then asserts effect-equivalence:
   - `add`/`enable`/`disable`/`remove` → manifest state matches (enabled flags,
     entry set).
   - `deploy` (+`--force`) → identical symlink graph under `$HOME`; backups present.
   - `status` → Rust JSON vs bash status agree on per-entry deployed/missing.
   - `pkg capture`/`status`/`diff` → identical `packages/<host>/*.txt` and the same
     drift verdict, using **fixture package lists** (read-only; `sync` is NOT run —
     it mutates the live system).
   - **Remote flow**: `push` to the bare remote, `pull` back into a second clone,
     `diff` → same commits/refs.
4. Prints a per-verb PASS/FAIL table; non-zero exit on any FAIL (CI-able).

The sandbox is the user's "small new dotfiles thing to track" — disposable, but the
harness script is committed and is the living proof of parity.

## Execution order

1. **pkg** — port `capture`/`status`/`diff` (read-only) + `sync` (guarded to local
   host). Likely a new module `dotfiles-core::pkg` + a `pkg` subcommand. Per-host
   `packages/<host>/{native,aur,flatpak}.txt`. (Possibly its own ADR — package
   tracking is a distinct subsystem.)
2. **git verbs** — `pull`, `diff` (shell out to `git`, like `push`).
3. **convergence harness** — build it as the verbs land; wire it into `cargo test`
   or a `just`/`make` target.
4. **lifecycle** — `install`/`update`/`uninstall` with binary-appropriate semantics
   (ties into `install.sh`/`bootstrap.sh` and the version-pin file).
5. **live cutover** (decided: Rust primary, bash fallback) — rename bash
   `dotfiles`→`dotfiles-bash`, install Rust as `~/.local/bin/dotfiles`, update
   install scripts, drop the submodule (`.gitmodules` now untracked; gitignore the
   nested `dotfiles-cli/` source dir in `.dotfiles`).

## Retiring `dotfiles-bash` (the endgame — less surface to maintain)

`dotfiles-bash` is a temporary fallback. Nothing functional blocks its removal:
the 10 daily/git/pkg verbs are convergence-proven, and install/update are covered
by `install.sh` (re-run to update). It stays only as a safety net until the Rust
CLI is proven in real daily use. When that confidence is there, removal is one
cleanup commit in `~/.dotfiles`:

- delete `dotfiles-bash` and `lib/{common,configs,git,pkg,lifecycle}.sh`;
- delete the legacy pipe `.dotfiles-manifest` (the TOML manifest is now primary);
- fold anything still useful from `HELP.md` into the Rust `--help` / README, then
  drop it;
- remove the `~/.local/bin/dotfiles-bash` symlink;
- keep `scripts/converge.sh` for the historical record, or retire it too once the
  comparison target is gone.

Until then the convergence harness is the proof mechanism; run it whenever the
Rust tool changes.

## Pointers

- Bash source of truth: `~/.dotfiles/{dotfiles,lib/*.sh}`; pkg = `lib/pkg.sh`.
- Rust: `~/.dotfiles/dotfiles-tui/crates/{dotfiles-core,dotfiles-cli}` (on-disk dir
  is still `dotfiles-tui`; renaming it to `dotfiles-cli` to match the repo is a
  later cutover step).
- Repo: `github.com/aaronsb/dotfiles-cli` (renamed from `dotfiles-tui` 2026-06-22).
- Live TOML manifest: on branch `feat/toml-manifest` in `~/.dotfiles`.
