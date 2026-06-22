# Continuance: simplify to schema + CLI, retire the TUI

**Date:** 2026-06-21 · **Status:** proposed pivot, awaiting execution (likely post-compaction)

## TL;DR

The valuable discovery is the **self-documenting dotfiles schema** (`why` + optional
structured `spec`, ADR-002/003/006). The TUI / always-fresh projection / observe-mode
stack (ADR-005/100) is complexity for its own sake — **cut it**. Keep the schema and the
**Rust CLI** (`dotfiles-core` + `dotfiles-cli`), which already do parsing, deploy-status,
the git gate, and the spec model. Finish it into a clean independent binary that
**replaces the bash `dotfiles` tool**. Conformance (spec ⇆ reality) returns later as a CLI
verb, not a TUI.

We are **not throwing away work** — core+cli are the keepers; only the `dotfiles-tui`
crate is dropped.

## Why

A persistent live projection of dotfiles state is elegant but inessential for a personal
dotfiles tool (DotState already exists if a TUI is ever wanted). The schema helps with or
without fancy tooling and is genuinely novel. Reducing to schema + CLI keeps every useful
property and drops the heaviest, least-used surface.

## Current state (snapshot)

- **Repo:** github.com/aaronsb/dotfiles-tui (public). Submodule under `~/.dotfiles/dotfiles-tui`.
- **main:** ADRs 001–006 (006 `Draft`; 001–005,100 `Accepted`); v0.1 code merged (PR #1).
- **branch `feat/entry-spec`:** **PR #2 OPEN** — adds `spec` to core + TUI render + fixture spec. Not merged. 7 tests green, clippy clean.
- **Crates:** `dotfiles-core` (manifest parse, deploy-status, git gate; +`spec` on the PR branch), `dotfiles-cli` (`status --json`), `dotfiles-tui` (ratatui inventory — TO BE CUT).
- **Bash tool (live):** `~/.dotfiles/dotfiles` + `~/.dotfiles/lib/{common,configs,git,lifecycle}.sh`. Reads `~/.dotfiles/.dotfiles-manifest` (PIPE format) via `read_manifest()` (`grep -v '^#'`); consumers do `IFS='|' read -r app source target enabled deploy_type`; write verbs rewrite pipe lines.
- **Live manifest is still PIPE.** TOML schema exists only in `examples/dotfiles-manifest.toml`.
- **Staged-not-committed:** the `.dotfiles` superproject has `.gitmodules` + `dotfiles-tui` gitlink staged (was "PR3").

## Keep / Cut

**Keep:** schema ADRs 001 (clean-room git-native), 002 (why), 003 (TOML), 006 (spec); `dotfiles-core`; `dotfiles-cli`. ADR-004 (workspace/libs) trims from 3 crates to 2 (core+cli).

**Cut/retire:** the `dotfiles-tui` crate; ratatui/crossterm deps; ADR-005 (always-fresh projection) and ADR-100 (change-detail diff) → **Superseded** by ADR-007.

## Open decisions (confirm with user before executing)

1. **Language: Rust (recommended) vs Go.** Rust reuses the working core+cli; Go discards exactly the keepers to re-derive them. The gix choice already yields a clean static binary, no C deps. *Pick Rust unless there is a strong Go reason.*
2. **Repo/binary name.** "dotfiles-tui" is now wrong (no TUI). Rename repo + name the binary (ADR-001 deferred `dotf`/`dotctl`). *Decision needed.*
3. **Bash's fate.** Replace the bash tool entirely with the Rust binary, or keep bash as the dependency-free fallback (ADR-001 #3)? *The pivot leans toward Rust-binary-as-the-tool.*
4. **Submodule story.** With no TUI, does the tool still belong as a submodule of `.dotfiles`, or is it just installed independently? Resolve the staged submodule link accordingly.

> **Decisions recorded this session:**
> - **#1 Language: Rust** (confirmed — reuse `dotfiles-core` + `dotfiles-cli`).
> - **#3 Bash's fate: the Rust binary _is_ the tool** — it "acts like the bash script version, written in Rust": same verbs and behavior (`deploy`/`status`/`enable`/`disable`/`add`/`push`), reading the rich TOML schema instead of the pipe format. Bash may linger as a reference/fallback (ADR-001 #3) but is no longer primary.
> - **#2 Name (confirmed):** the binary is named **`dotfiles`** — a drop-in replacement for the bash tool (same name, same muscle memory). **The bash tool is renamed `dotfiles-bash`** (temporary, for convergence testing) so the Rust `dotfiles` takes the canonical name *now* and both coexist on PATH. Convergence test = run `dotfiles` (Rust) and `dotfiles-bash` against the same manifest and diff the output; once the Rust one is trusted, **delete `dotfiles-bash`**. The GitHub repo renames `dotfiles-tui` → `dotfiles` (GitHub redirects the old URL).
> - **#4 Submodule (confirmed):** **drop it.** Unstage `.gitmodules` + the `dotfiles-tui` gitlink from the `.dotfiles` superproject. The tool is an independently-installed binary (from GitHub Releases); `.dotfiles` pins the desired version as a plain string. The submodule existed mainly to vendor TUI source, which is gone.

## Execution steps (for the post-compaction agent)

1. Re-confirm the open decisions above (esp. #1, #2).
2. **Write ADR-007** "Reduce scope to a self-documenting schema + a CLI; retire the TUI/projection." Supersede ADR-005 + ADR-100 (flip their `status:` → `Superseded`, set `related: [ADR-007]`). Note ADR-004 trimmed to core+cli. Use `docs/scripts/adr new foundation "..."`.
3. **Merge PR #2** (the `spec` consumption in core is a keeper). Its TUI-render parts get removed in step 4.
4. **Remove the `dotfiles-tui` crate** (delete `crates/dotfiles-tui`, drop ratatui/crossterm, update `[workspace] members`). Scrub doc-comments that say "two front-ends / projection / always-fresh."
5. **Port remaining bash verbs into `dotfiles-cli`**, reading the rich TOML schema: `deploy`, `enable`, `disable`, `add`, `remove`, `push` (`status` exists; deploy-status lives in core). Writes via `toml_edit` to preserve `why`/`spec`. Match bash behavior (back up on overwrite, etc.).
6. **Migrate the live manifest** `~/.dotfiles/.dotfiles-manifest` (pipe → rich TOML, `why` + optional `spec`). On a branch in `~/.dotfiles`, tested.
7. **Switch `~/.dotfiles` to the Rust binary** (replace bash, or keep bash as fallback per decision #3). Update `install.sh`/`bootstrap.sh`.
8. Rename repo/binary if decided (#2).
9. Resolve the submodule question (#4): commit or drop the staged `.dotfiles` submodule link.
10. **Future (not now):** `dotfiles check` = conformance (authored `spec` ⇆ derived analyzer) — the schema's payoff, CLI-only.

## Pointers

- Tool repo: `~/.dotfiles/dotfiles-tui` · build/test: `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace`.
- Schema example (the model): `~/.dotfiles/dotfiles-tui/examples/dotfiles-manifest.toml`.
- Bash tool: `~/.dotfiles/dotfiles`, `~/.dotfiles/lib/*.sh`, `~/.dotfiles/.dotfiles-manifest`.
- ADRs: `~/.dotfiles/dotfiles-tui/docs/architecture/`; CLI: `docs/scripts/adr`.
- Schema reference: ADR-006 + the zprofile spec example in the fixture.
