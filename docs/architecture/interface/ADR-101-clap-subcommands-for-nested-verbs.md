---
status: Draft
date: 2026-06-22
deciders:
  - aaronsb
  - claude
related:
  - ADR-008
---

# ADR-101: clap subcommands for nested verbs

## Context

The CLI's top-level verbs are a clap `Subcommand` enum, so clap derives their
help, `--help` per verb, argument validation, and shell completion. Verbs that
nest a second level of actions — `profile <action>`, `pkg <action>` — do not
follow this. Each takes a single `action: String` positional plus
`trailing_var_arg` and dispatches with a hand-rolled `match action.as_str()`,
falling through to a bespoke `unknown subcommand` error.

The two levels of the same tree therefore disagree about what a subcommand is.
The visible consequence: `dotfiles profile help` does not print help — clap
never sees `help`, so it lands in `action` as a string, misses every arm, and
errors. Per-action `--help`, argument validation, and completion past the verb
are absent at the nested level while present at the top. Hand-rolled error
strings and positional-name lookups (`name_at(args, 0)`) re-implement, less
well, what clap already provides one level up.

## Decision

All verb nesting uses clap's derive uniformly. A verb with sub-actions declares
them as a `#[derive(Subcommand)]` enum whose variants carry their own typed
arguments, the same way top-level verbs are declared. Hand-rolled
string-action dispatch and the bespoke `unknown subcommand` error arms are
removed; clap owns parsing, help, validation, and the unknown-subcommand error
at every level of the tree.

Already-shipped subcommand names are a compatibility surface and are preserved
(e.g. `profile add`, `pkg sync`); this decision changes how they are parsed,
not what they are called.

## Consequences

### Positive

- `help`, `<verb> <action> --help`, argument validation, and completion work at
  every level because clap generates them — no per-verb reimplementation.
- Net code reduction: the `action: String` plumbing, `name_at` positional
  lookups, and hand-written error arms are deleted in favor of typed variants.
- New sub-actions are added by adding an enum variant, not by extending a match
  and remembering to update an error string.

### Negative

- Per-action flags must be placed on the correct variant; a misplacement is a
  parsing bug clap will not catch for us.
- Renaming any shipped sub-action remains a breaking change; the typed surface
  makes such names more load-bearing, not less.

### Neutral

- The convention is now uniform, so future verbs that grow a second level have
  one obvious shape to follow.

## Alternatives Considered

- **Keep string-action dispatch, special-case `help`.** Patches the one symptom
  while leaving validation, completion, and per-action help missing, and keeps
  two divergent notions of "subcommand" in the tree. Rejected: treats the
  symptom, not the inconsistency.
- **A shared hand-rolled dispatch helper for nested verbs.** Re-implements clap's
  job in-house to make the fake-subcommand pattern less repetitive. Rejected:
  the standard tool already does this correctly one level up.
