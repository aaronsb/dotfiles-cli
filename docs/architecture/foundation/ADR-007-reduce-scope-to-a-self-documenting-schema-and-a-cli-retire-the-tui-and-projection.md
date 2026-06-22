---
status: Accepted
date: 2026-06-22
deciders:
  - aaronsb
  - claude
related:
  - ADR-005
  - ADR-100
---

# ADR-007: Reduce scope to a self-documenting schema and a CLI; retire the TUI and projection

## Context

ADR-001 founded this tool as a clean-room, agent-native dotfiles manager with
**two front-ends over one core** (ADR-004): a non-interactive JSON CLI (the agent
surface) and a Ratatui TUI (the human surface). The TUI's reason for being was a
*live, always-fresh projection* of derived dotfiles state — catalog + deploy
status + git status, re-derived on every filesystem change (ADR-005) — with a
change-detail diff view (ADR-100) so a human could watch edits land in real time.

Building toward that, two things became clear:

1. **The genuinely novel, genuinely useful discovery was the schema, not the
   surface.** The self-documenting manifest — a durable `why` per entry (ADR-002),
   serialized as TOML (ADR-003), optionally deepened into a structured `spec`
   vocabulary (ADR-006) — helps with or without any live tooling. It is the part
   of this project that does not already exist elsewhere.

2. **The live projection is complexity for its own sake** for a *personal*
   dotfiles tool. A persistent watch loop, a TUI render tree, and a diff view are
   a lot of surface to maintain for a workflow that is fundamentally "edit a
   config, commit it." Where a live TUI is genuinely wanted, DotState already
   exists (ADR-001 surveyed it). We were re-deriving an elaborate state model to
   feed a window the user would rarely keep open.

The working assets at the time of this decision were `dotfiles-core` (manifest
parse, deploy-status, git gate, spec model) and `dotfiles-cli` (`status --json`) —
both keepers — plus `dotfiles-tui` (the ratatui inventory), the cut.

Separately, the existing bash `dotfiles` tool still owns the real workflow
(`deploy`/`status`/`enable`/`disable`/`add`/`push`) against the live pipe-format
`.dotfiles-manifest`. ADR-001's strangler-fig posture always intended the new tool
to reach parity and take over; the question was *which surface* takes over.

## Decision

**Reduce the tool to a self-documenting schema plus a CLI. Retire the TUI and the
always-fresh projection.**

Concretely:

1. **Keep the schema** — ADR-002 (`why`), ADR-003 (TOML), ADR-006 (`spec`). This
   is the project's payoff and is unchanged.
2. **Keep `dotfiles-core` and `dotfiles-cli`.** The CLI becomes the *only* surface
   and grows into a full replacement for the bash tool: it "acts like the bash
   script version, written in Rust" — the same verbs and behavior
   (`deploy`/`status`/`enable`/`disable`/`add`/`remove`/`push`), reading the rich
   TOML schema instead of the pipe format, writing via `toml_edit` to preserve
   `why`/`spec`.
3. **Cut the `dotfiles-tui` crate** and its ratatui/crossterm dependencies.
4. **Supersede ADR-005** (live always-fresh projection) **and ADR-100**
   (change-detail diff rendering). Their motivating surface no longer exists.
   Deploy-status derivation — the still-useful core they justified — survives in
   `dotfiles-core` and now feeds `status` directly rather than a watch loop.
5. **Amend, not supersede, ADR-001 and ADR-004.** ADR-001's "two front-ends" (#4)
   collapses to one (the CLI); everything else it decided (clean-room, git-native,
   shared manifest, strangler-fig) stands. ADR-004's workspace trims from three
   crates to two (`core` + `cli`).
6. **The Rust binary is named `dotfiles`** — a drop-in replacement for the bash
   tool. During convergence testing the **bash tool is renamed `dotfiles-bash`**
   so both coexist on PATH and their output can be diffed against the same
   manifest; once the Rust tool is trusted, `dotfiles-bash` is deleted. The GitHub
   repo renames `dotfiles-tui` → `dotfiles`.
7. **Drop the submodule.** With no TUI source to vendor, the tool is an
   independently-installed binary (from GitHub Releases); `.dotfiles` pins the
   desired version as a plain string rather than carrying a git submodule.

Conformance — comparing an authored `spec` against a derived analysis of what a
dotfile actually does — returns later as a **CLI verb** (`dotfiles check`), not a
TUI. That is the schema's eventual payoff and stays on the roadmap.

## Consequences

### Positive

- One surface to build, test, and maintain instead of two; no ratatui/crossterm,
  no watch loop, no render tree.
- Every useful property is retained: the schema, deploy-status derivation, the
  git gate, JSON output for agents.
- The binary named `dotfiles` is a true drop-in for the bash tool; muscle memory
  and scripts carry over unchanged.
- Convergence testing is concrete: run `dotfiles` and `dotfiles-bash` over the
  same manifest and diff — parity is observable, not asserted.

### Negative

- No live human-watchable view of edits landing. (Accepted: `git status`/`git
  diff` in the config repo already serve this, and DotState exists if a TUI is
  ever wanted.)
- Work invested in the `dotfiles-tui` crate is retired. (Bounded: only that crate;
  core + cli — the bulk — are the keepers.)

### Neutral

- ADR-005 and ADR-100 become `Superseded`, not deleted — they remain an honest
  record of the projection model we built toward and turned away from.
- `dotfiles check` (spec ⇆ reality conformance) is deferred to a CLI verb; the
  `spec` vocabulary (ADR-006) exists ahead of its first consumer, which is fine —
  it is already used as durable documentation.

## Alternatives Considered

- **Keep the TUI and finish the projection (status quo of ADR-005/100).** Rejected
  as complexity for its own sake for a personal tool; the maintenance cost of a
  live surface is not repaid by how rarely it would be open.
- **Rewrite the tool in Go instead of Rust.** Rejected: Go would discard exactly
  the keepers (`core` + `cli`) to re-derive them, and the existing gix-based Rust
  build already yields a clean static binary with no C deps.
- **Delete ADR-005 and ADR-100 outright.** Rejected: ADRs are an append-only
  decision log; the superseded ones record the road not taken, which is the most
  instructive part of the history. Supersede, don't erase.
- **Keep bash as the primary tool, CLI as a secondary reader.** Rejected: it
  leaves two manifests (pipe + TOML) and two implementations indefinitely. The
  strangler-fig (ADR-001) always intended a single successor; this names it.
