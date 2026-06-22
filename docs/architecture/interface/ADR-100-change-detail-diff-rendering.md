---
status: Superseded
date: 2026-06-21
deciders:
  - aaronsb
  - claude
related:
  - ADR-001
  - ADR-004
  - ADR-005
  - ADR-007
---

# ADR-100: Change-detail diff rendering

## Context

ADR-005 makes the tool a live projection of the dotfiles' always-fresh derived
state, demoting *watching* to a freshness mechanism and the *diff* to one zoom
view within that projection. This ADR specs that one view — the
**change-detail diff**: how a single changed entry renders when you zoom in.
ADR-001 set the git working-tree diff as the change feed (#7, *detection*) and
split *detection* from *rendering*; this is the rendering half.

The target experience (the human's words): present the changed file, the general
line-area being edited, with `+added` / `-removed` shown inline. Read-only — a
human who wants to do more opens their own editor.

## Decision

The change-detail view is a pipeline over `dotfiles-core` (ADR-004), rendered in
the TUI:

1. **Watch** the repo working tree with `notify` (inotify/FSEvents) for
   low-latency events, with a periodic `git status` poll as a fallback for
   inotify edge cases.
2. **Detect** the changed managed file(s) and compute the working-tree diff via
   `gix` (read-only).
3. **Diff for display** with `similar` (`inline` feature) to get line- and
   intra-line add/remove spans.
4. **Highlight** with `syntect` (data-driven Sublime grammars) and map its styled
   spans onto Ratatui `Span`s.
5. **Present** the changed file with the edited line-area in context (surrounding
   lines), inline `+`/`−` coloring on changed spans, syntax-aware.

**Scope guard (MVP).** Per ADR-001's "review is optional, never forced" framing,
the change-detail view is **read-only review of a managed entry's diff**. No
staging, no merge resolution, no blame, no editing. Depth beyond a glance =
open `$EDITOR`.

## Consequences

### Positive

- Delivers the zoom UX — a changed entry's diff in context, syntax-aware, with
  intra-line +/− — reusing `dotfiles-core`'s diff rather than a bespoke layer.
- `notify` gives near-instant redraw instead of a polling stutter.
- Read-only keeps the surface small and safe and avoids `gix` write paths.

### Negative

- `syntect`'s regex highlighting is less precise than tree-sitter's incremental
  parsing — accepted (ADR-004) for the build-cost/binary-size win on a small
  tool.
- Mapping `similar` ops + `syntect` styles onto Ratatui spans is custom glue with
  its own edge cases (tabs, wide chars, very long lines).

### Neutral

- Themes and keybindings for the view are deferred to later interface ADRs.
- Copy-mode targets live outside the store's tree; they are observed via their
  own nested git repo, consistent with ADR-001 #7.

## Alternatives Considered

- **`tree-sitter` + `tree-sitter-highlight`.** Rejected: per-language C grammars
  must be bundled/compiled (build cost, binary bloat); even OpenAI's Codex TUI
  migrated *off* tree-sitter-highlight *to* `syntect`. Overkill for short config
  diffs.
- **Poll-only watching (no `notify`).** Rejected: adds latency and wastes CPU;
  `notify` carries its own poll fallback so we keep one API.
- **Shell out to an external pager/editor only.** Rejected: loses the integrated
  zoom view within the always-fresh projection (ADR-005); the external path
  already exists (open `$EDITOR`) and is offered as the deeper-inspection escape
  hatch.
