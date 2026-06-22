---
status: Superseded
date: 2026-06-21
deciders:
  - aaronsb
  - claude
related:
  - ADR-001
  - ADR-002
  - ADR-100
  - ADR-007
---

# ADR-005: Live projection of a derived always-fresh dotfiles state

## Context

ADR-001 makes the documented inventory primary; ADR-002 makes the manifest a
self-documenting catalog; ADR-100 specs how a single change renders. A framing
hazard remained across all of them: presenting *live file watching* as a
centerpiece reads as a gimmick — "we made a thing to watch files be edited" —
and skews the tool AI-specific.

The correction reframes what the tool *is*. The value is not watching edits; it
is that the **interdependent state of your dotfiles — what you manage, what each
thing is for and why, how it is deployed, and how the pieces group — is always
fresh in the TUI.** When anything underneath changes, that state is reprocessed,
so you watch your configuration's *meaning* refactor in real time, not bytes
scroll by. Liveness becomes a property of a *well-constructed state projector*,
coherent with or without an agent — an agent amplifies it, it is not the point.

## Decision

`dotfiles-tui` is a **live projection of a derived state model** of the dotfiles.

The state model is *derived from ground truth, never hand-authored*. It composes:

- the **catalog** — entries (path/target/mode/enabled) and their `why`
  docstrings (ADR-002/003);
- **deploy status** per entry — checked against the filesystem (symlink present
  / broken / drifted; copy in/out of sync);
- **git status** for managed files — via `gix` (ADR-004);
- **interdependence** — entries grouped into the *concern* they configure (e.g.
  `zsh` + `.zsh` + `.zprofile`; the `mlterm-*` family), so a change to one
  surfaces against the concern it belongs to.

Two rules:

1. **Derived, never authored — so it cannot drift.** Every field traces to a
   ground-truth source (manifest, filesystem, git). The projection re-derives
   rather than being maintained, making the TUI an *anti-drift surface*. The one
   advisory, non-derived element is the `why` prose (ADR-002): it is displayed,
   not computed.

2. **Reprocess on change; the TUI is always fresh.** Any change to an input — a
   manifest edit, a managed-file edit, a link/deploy change, a git-state change —
   triggers re-derivation of the affected slice and a re-projection. Re-derive
   incrementally (the touched entry/concern) where it matters for responsiveness;
   the *contract* is simply "always fresh."

This **demotes watching and diffing** to mechanisms in service of the projection:

- the change-watcher (`notify` + git, ADR-004/100) is the **freshness mechanism**
  *implied* by "always fresh" — it is never advertised as a feature;
- the change-detail diff (ADR-100) is **one zoom view** within the always-fresh
  state, not the centerpiece.

The tool is therefore described as a **state projector, not a file watcher.**

## Consequences

### Positive

- A coherent product spine — "your dotfiles' interdependent state, always fresh"
  — that is general-purpose, not an AI-watching gimmick.
- Anti-drift by construction: the TUI cannot show stale state because it
  re-derives rather than caches.
- Subsumes watching and diffing as mechanisms/views, removing the "file watcher"
  framing the prior ADRs risked.
- The derived model is exactly what an agent benefits from too — amplifier, not
  prerequisite — without the tool being built *for* the agent.

### Negative

- "Derive everything on change" is more engineering than "render a diff": it
  needs a clean state model and incremental re-derivation, not just a watch loop.
- Modelling **interdependence/concerns** adds surface — how entries group, and
  what a "concern" is — beyond a flat list.

### Neutral

- Reframes ADR-100 (the diff is a view) and ADR-001's liveness language.
- The concern-grouping model is sketched here (group by the thing configured);
  if it grows teeth (explicit concern metadata in the catalog), that earns its
  own follow-up ADR rather than expanding this one.

## Alternatives Considered

- **Centerpiece "observe / watch files" framing (the prior framing).** Rejected:
  reads as a gimmick, skews AI-specific, and undersells the real value — the
  always-fresh interdependent state.
- **Static inventory with manual refresh.** Rejected: stale between refreshes,
  which forfeits the "fresh" property that makes the projection trustworthy.
- **Surface raw git status/diff only (no derived model).** Rejected: that is
  bytes, not meaning — no catalog / deploy / concern synthesis, which is the
  high-altitude "what do I have and what does it do" view the tool exists for.
