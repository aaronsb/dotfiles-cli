---
status: Accepted
date: 2026-06-22
deciders:
  - aaronsb
  - claude
related:
  - ADR-002
  - ADR-005
  - ADR-006
  - ADR-007
---

# ADR-009: Decline spec conformance; the spec stays documentary

## Context

ADR-006 added the optional per-entry `spec` vocabulary and explicitly **deferred**
its north-star payoff — *conformance* — to "a follow-up ADR": pairing the
authored `spec` against a *derived analyzer* to compute whether a file still does
what its spec says. That deferral has sat open. A parked deferral with no closing
decision reads, to the next person, as "conformance is still coming" — so the
question deserves an explicit answer rather than continued silence.

Two things have shifted since ADR-006:

- **The foundation conformance was to lean on is gone.** ADR-006 scoped
  conformance against ADR-005's *live projection of derived state*. ADR-007 then
  reduced scope and **retired the TUI and the live projection** (ADR-005 is now
  Superseded). The always-fresh derived layer conformance assumed no longer
  exists as a product surface — only `State::derive` survives, narrowly, as the
  deploy-status check behind `status`/`show`.
- **The tool's purpose has settled, and it is narrower than conformance.**
  `dotfiles` is for *organizing and tracking* dotfiles and *deploying whatever is
  in the repo, fast*. It is deliberately not a system that reasons about what a
  config is *for* or polices whether it still behaves as intended. Understanding
  purpose is a human's job; the tool's job is custody and deployment.

Meanwhile ADR-006's *descriptive* half shipped and is useful: `status --format
json` serializes the whole spec tree, and `dotfiles show <name>` renders it for
humans (including surfacing unrecognized keys). The spec already pays its way as
documentation. Conformance is a separate, much heavier bet — and the evidence now
points against making it.

## Decision

**Decline conformance.** The `spec` is and stays **documentary**: authored,
structured intent that the tool **stores, serializes, and displays** — and never
checks against the live system. ADR-006's deferred follow-up is hereby resolved
by declining it, not by building it. There is no `check`/`verify`/conformance
verb, no spec⇄derived analyzer, and no "does this file still match its spec?"
status axis.

The recognized-vs-unrecognized vocabulary distinction (ADR-006) is retained only
for **display fidelity** — `show` surfaces unknown keys so typos and
authored-but-unmodeled intent stay visible — not because any key is *acted upon*.
Under this ADR no `spec` key, recognized or not, drives tool behavior.

This is a posture decision, not a door slammed forever: if the tool's purpose
later expands to include reasoning about config behavior, a future ADR can
supersede this one. Until then, the absence is intentional and recorded.

## Consequences

### Positive

- The open deferral in ADR-006 is closed. The spec's role is now unambiguous:
  documentation, full stop.
- Keeps the tool aligned with its actual purpose — custody and fast deployment —
  and off the path toward config-semantics analysis it was never meant to walk.
- No analyzer layer to build, own, or keep correct as configs evolve; no second
  status axis to render or explain.
- The shipped value (authored `spec`, JSON serialization, `show`) is unaffected
  and remains the whole point of the vocabulary.

### Negative

- The richest idea in ADR-006 — declared intent checked against reality — is
  given up. Drift between what a spec claims (`requires.packages`,
  `depends`, `platform`) and the actual system stays undetected by the tool.
- `depends`/`requires.entries` edges (e.g. `waydesk` → `polkitctl`) remain
  documentation, not validated references; a dangling `depends` will not be
  flagged.

### Neutral

- `State::derive` keeps its current, narrow job (deploy status). This ADR does not
  revive or extend it.
- Growing the recognized `spec` vocabulary (e.g. promoting `kind`/`launches`/
  `run_mode` out of the catch-all) becomes a pure *display/documentation*
  question, decoupled from any conformance motive — a later ADR-006 amendment may
  still do it, or not.

## Alternatives Considered

- **Conformance-lite — a `dotfiles check` verb.** Validate the cheap, real parts
  of `spec` against the system: are `requires.packages` installed, do
  `requires.entries`/`depends` resolve to managed entries, does `platform` match
  the host. Buildable on the surviving `derive` layer without reviving the
  projection. Rejected: even the lite form pulls the tool toward reasoning about
  config requirements, which is outside its settled purpose (organize, track,
  deploy). Attractive but a scope expansion we are choosing not to make now.
- **Full conformance (the original ADR-006 vision).** A derived analyzer judging
  whether a file still behaves as its spec declares. Rejected: heavy, and its
  intended foundation (ADR-005's live projection) was retired by ADR-007. The
  most expensive option with its groundwork already removed.
- **Leave ADR-006's deferral open.** Rejected: an unresolved deferral silently
  implies the work is still planned. Recording the decline is the point.
