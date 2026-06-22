---
status: Draft
date: 2026-06-21
deciders:
  - aaronsb
  - claude
related:
  - ADR-001
  - ADR-100
---

# ADR-004: Crate workspace shape and library stack

## Context

ADR-001 #4 mandates **one core, two front-ends** — a non-interactive JSON CLI
(agent surface) and a Ratatui TUI (human surface) over a shared core. Before
implementation we fix the concrete Rust structure and dependency choices. The
library choices below were grounded in a mid-2026 survey (see PR/commit history),
not chosen from memory.

## Decision

A single Cargo **workspace** with three crates:

| Crate | Role |
|-------|------|
| **`dotfiles-core`** | Pure library, no UI, no interactive I/O. Owns: TOML manifest parse/validate (ADR-003), symlink/copy deploy ops, git working-tree status/diff, the change-watch loop, and inventory/status computation. Both front-ends depend only on this. |
| **`dotfiles-cli`** | The non-interactive **JSON** surface (`--format json`). Thin shell over `dotfiles-core`. The agent's surface. |
| **`dotfiles-tui`** | The **Ratatui** human surface: inventory browse + observe mode (ADR-100). Thin over `dotfiles-core`. |

Library stack:

| Concern | Crate | Notes |
|---------|-------|-------|
| TUI | `ratatui` (~0.30) + `ratatui-crossterm` + `crossterm` | immediate-mode; modular backends as of 0.30 |
| Git read (status/diff) | `gix` (gitoxide, ~0.84) + `gix-status`/`gix-diff` | pure-Rust, **no libgit2 C dependency** — read-only paths only |
| CLI args + JSON | `clap` 4 (derive) + `serde` / `serde_json` | declarative args; serialize result structs |
| Manifest | `toml` (~0.9) + `serde`, `toml_edit` for writes | per ADR-003 |
| Inline diff | `similar` (`inline`) | intra-line +/− for observe mode (ADR-100) |
| Syntax highlight | `syntect` (~5) | observe-mode rendering (ADR-100) |
| File watch | `notify` (~8) + poll fallback | low-latency observe redraw |

Binary name (vs. repo/workspace name `dotfiles-tui`) stays deferred per
ADR-001 — candidates `dotf`/`dotctl`.

## Consequences

### Positive

- The pure `dotfiles-core` makes both front-ends thin and the logic unit-testable
  without a terminal or an agent in the loop.
- `gix` avoids a C toolchain → static, easily cross-compiled release binaries,
  directly serving ADR-001 #5's release-distribution model.
- `syntect`'s data-driven grammars avoid per-language compilation, keeping the
  shipped binary small.
- Read-only git usage sidesteps `gix`'s less-mature write/merge paths entirely.

### Negative

- Workspace + three crates is more upfront scaffolding than a single binary.
- Ratatui's immediate-mode means the TUI owns its own layout/state each frame.
- `gix`'s write paths are immature — acceptable only because we never write via
  it (commits stay with the user / the Bash tool / plain `git`).

### Neutral

- Pinned versions will drift; treat the table as the starting baseline, managed
  per the project's lockfile-hygiene practice.
- `similar` + `syntect` appear here as the stack but are *applied* in ADR-100.

## Alternatives Considered

- **Single monolithic crate.** Rejected: blends the agent CLI and the TUI, makes
  the core hard to test in isolation, and muddies the ADR-001 #4 separation.
- **`git2` (libgit2 bindings) instead of `gix`.** Rejected: drags a C dependency
  that complicates static release builds, is slower on diffs, and its main
  advantage (battle-tested write/merge) is irrelevant to a read-only consumer.
- **`tree-sitter` for highlighting.** Rejected: per-language C grammars add build
  cost and binary bloat; `syntect` is the better fit (decided alongside ADR-100).
- **`lexopt`/`pico-args` over `clap`.** Rejected: smaller, but `clap` derive's
  ergonomics and JSON-output plumbing are worth the compile cost.
