---
status: Draft
date: 2026-06-21
deciders:
  - aaronsb
  - claude
related:
  - ADR-001
  - ADR-002
---

# ADR-003: Manifest serialization as TOML array-of-tables

## Context

ADR-002 decided the manifest is a self-documenting catalog — each entry carries
a *why* docstring — and explicitly deferred the **serialization**, because the
current single-line pipe format (`app|source|target|enabled|deploy_type`) has no
room for prose. That deferred fork is resolved here.

ADR-001 #1 framed the invariant as *plain-text, diffable, and legible enough to
apply by hand*. Note it said "the existing format"; this ADR evolves the literal
format while **preserving those properties** — the invariant was always the
properties, not the pipe syntax specifically.

## Decision

Migrate the manifest to **TOML, as an array-of-tables**. Each managed entry is a
discrete `[[entry]]` block; the docstring is a real multi-line string value, not
a comment:

```toml
[[entry]]
path    = "zsh/.zshrc"   # source, relative to the repo
target  = ".zshrc"       # deploy path, relative to $HOME
mode    = "symlink"      # symlink | copy
enabled = true
why     = """
Cross-machine shell baseline. Kept here so a new box gets
identical history + keybindings without re-deriving them.
"""
```

- **Reads:** `toml` + `serde` (deserialize into typed entry structs).
- **Tool-driven writes:** `toml_edit`, which preserves formatting, ordering, and
  prose when the tool rewrites the manifest (so an agent adding an entry does not
  clobber a human's hand-written docstrings).
- `why` is optional per entry; absence is valid, it just means undocumented.

This keeps every property ADR-001 #1 requires: plain-text, line-diffable (each
`[[entry]]` is its own block), and hand-editable. It makes the docstring a
**queryable, validatable value** rather than a fragile comment.

## Consequences

### Positive

- The docstring is a first-class value — the inventory/catalog (ADR-002) can
  read, validate, and render it; an agent can set it structurally.
- Array-of-tables keeps each entry a self-contained, diff-friendly block; adding
  or editing one entry touches only its lines.
- `toml_edit` lets the tool mutate the manifest without destroying human prose or
  comments — important for the mixed human/agent authoring model.
- Pure-data format with first-class `serde` support; no bespoke parser to write.

### Negative

- It is a **format migration**. The Bash reference tool (ADR-001 #3) parses
  pipes today; it must learn TOML, or the pipe→TOML conversion becomes the very
  moment Bash demotes to spec/fallback (ADR-001 #6). Either way both readers must
  move together to preserve the shared-format invariant.
- A one-time conversion of the existing `.dotfiles-manifest` is required.
- TOML is more verbose per entry than a pipe line (offset by carrying prose the
  pipe line never could).

### Neutral

- Partially revises ADR-001 #1's "existing format" wording — properties
  preserved, pipe syntax superseded.
- Field naming (`path`/`target` vs `source`/`target`) and the `mode` vocabulary
  are finalized at implementation against `dotfiles-core` (ADR-004).

## Alternatives Considered

- **Keep the pipe format + comment-docstring above each entry.** Rejected:
  prose-as-comment is non-queryable, easier to let rot, and `toml_edit`-style
  safe rewriting has no comment equivalent. (Chosen as the runner-up if a
  zero-migration path were mandatory.)
- **YAML.** Rejected: whitespace-fragile and error-prone to hand-edit — directly
  against the hand-appliable invariant.
- **A custom annotated line format.** Rejected: costs a parser to write and
  maintain for no benefit over TOML, which already gives blocks + multi-line
  strings + serde.
