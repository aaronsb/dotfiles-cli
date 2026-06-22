---
status: Draft
date: 2026-06-22
deciders:
  - aaronsb
  - claude
related:
  - ADR-008
  - ADR-101
---

# ADR-102: status reflects active profile state

## Context

`status` reports the deployed state of dotfiles *for the active profile*, and
the active profile silently shapes that report: the resolved profile name
appears only as title chrome (`(profile: north)`), and entries scoped to other
profiles are dimmed without the report saying that a scope is being applied.

The active profile is resolved by precedence — an explicit choice
(`--profile` / `$DOTFILES_PROFILE` / `.dotfiles-profile`), else a declared
profile whose `match` glob hits the hostname, else the bare hostname. The
resolved name may or may not be a *declared* `[profiles.<name>]` scope; `status`
draws no distinction between a declared scope and an implicit hostname fallback.

The consequence surfaces when a profile is declared: `dotfiles profile add <host>`
produces **no visible change** in `status`. The name was already shown (it was
the implicit active profile), entries are universal by default so the scope
filter selects the same set, and deploy state is independent of declaration.
The reader cannot tell from `status` whether a profile is declared, nor that a
profile filter is being applied at all — so declaring one looks like a no-op.

## Decision

`status` treats the active profile as part of the machine state it reports, and
makes legible two things it currently leaves silent:

1. **Provenance.** Annotate the active profile as `declared` (a real
   `[profiles.<name>]` scope) or `implicit` (resolved by hostname fallback, not
   declared) — the same vocabulary `profile list` uses, so the two surfaces
   agree. Declaring the active profile then changes `status` visibly
   (`implicit` → `declared`).

2. **The scope being applied.** When any profile is declared, print a scope
   summary — how many entries are universal, how many are scoped to the active
   profile, and how many belong to other profiles — so a filtered report
   announces its own basis instead of passing as the complete set. When no
   profile is declared, no scope line is shown; a profile-indifferent repo stays
   uncluttered.

The governing principle: a surface that derives or filters its output by some
context must make that context legible. `status` answers "what is the state of
this machine now"; the active profile and its provenance are part of that state,
not decoration.

This decision covers the human-readable surface. JSON `status` carries no
profile block today; extending the machine surface for parity is deferred to a
follow-up so this change stays scoped to the reporting model.

## Consequences

### Positive

- Declaring a profile produces a visible `status` change, closing the feedback
  loop a reader expects.
- A filtered `status` cannot be mistaken for a complete one — the scope line
  states what was selected and what was withheld.
- `status` and `profile list` describe profile provenance in one shared
  vocabulary.

### Negative

- `status` grows a provenance annotation and a conditional scope line; kept to
  one word and one line respectively to avoid clutter, but it is more to render.
- Provenance must be computed consistently with profile resolution; drift
  between them would mislead rather than inform.

### Neutral

- JSON `status` and human `status` now differ in what they expose about
  profiles until the deferred parity follow-up lands.
- Reopens, but does not decide, whether `status` should also reflect the active
  profile's *package* sync, since profiles scope packages too (ADR-008).

## Alternatives Considered

- **Leave provenance to `profile list` only.** A reader reaches for `status` as
  the machine-state surface; forcing a second command to learn whether a filter
  is active splits one question across two tools. Rejected.
- **Make `profile add` mutate deploy or membership so `status` changes.**
  Conflates declaring a scope, tagging membership, and deploying files — three
  axes ADR-008 deliberately keeps orthogonal. The gap is a reporting gap; the
  fix belongs in the report, not in coupling the axes. Rejected.
