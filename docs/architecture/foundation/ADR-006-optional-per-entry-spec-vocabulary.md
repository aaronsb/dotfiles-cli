---
status: Accepted
date: 2026-06-21
deciders:
  - aaronsb
  - claude
related:
  - ADR-002
  - ADR-003
  - ADR-005
  - ADR-007
---

# ADR-006: Optional per-entry spec vocabulary

## Context

ADR-002 gave each entry a `why` prose docstring; ADR-003 made the manifest TOML;
ADR-005 made the state *derived*, with `why` the one advisory authored field. We
now want richer, *structured* per-entry understanding — a declarative
**specification** of what a dotfile is and what it requires — without:

- breaking the dotfile (it can be **any** format, some with no comments),
- breaking existing or older parsers, or
- forcing structure onto simple entries.

An experiment this session settled the shape. The real `~/.dotfiles/zsh/.zprofile`
(a guarded framebuffer-terminal launch hook) was transcribed into a deep `spec`
and parsed by the **current v0.1 binary**, which models only
`name/path/target/enabled/mode/why`. It parsed cleanly, kept `why` + deploy
status, and **silently tolerated the entire `spec` block**. That proves
TOML-native structure is forward-compatible and disturbs neither the file nor
simple entries.

Two earlier dead-ends were ruled out and stay ruled out: embedding YAML/JSON
*inside* a string field (the manifest is already TOML — nesting a foreign format
needs a second parser and breaks diffability), and sigil markers *inside the
configs* (per-format comment fragility, no-comment formats, directory entries,
formatter clobbering). Authored structured intent belongs in the manifest as
native TOML.

## Decision

Add an **optional, native-TOML `spec`** table per entry: a small **recognized
vocabulary** of authored, structured intent. Fully optional, per-entry and
per-key — a bare `why`, or nothing, stays valid. `spec` is a strict, additive
superset, never a requirement.

```toml
[[entry]]
name = "zprofile"
path = "zsh/.zprofile"
target = ".zprofile"
why  = "Login-shell hook: auto-launches the framebuffer terminal, lockout-proof."

spec.summary  = "Auto-launch mlterm-fb on bare-VT login, guarded."
spec.concern  = "terminal-bootstrap"
spec.platform = "linux-vt"
spec.requires.packages = ["mlterm-fb"]
spec.requires.groups   = ["video", "input"]
spec.requires.entries  = ["mlterm-main", "mlterm-aafont", "mlterm-color"]
```

**Recognized v1 vocabulary** (the tool acts on these): `summary`, `concern`,
`tags`, `platform`, `provides`, `depends` (other entry names), and `requires`
(sub-table: `packages`, `groups`, `binaries`, `configs`, `entries`). The set is
deliberately small and generic, expected to grow by later amendment.

**Capture-and-surface unknown keys.** Parse recognized keys into typed fields;
collect everything else into a catch-all map. Unknown keys are neither a hard
error (which would break forward-compat the moment a new key is coined) nor
silently dropped (which would hide typos) — they are stored and **surfaced** in
the UI as "unrecognized." This resolves the forward-compat ⇄ typo-safety tension
the experiment exposed.

`why` stays prose (human rationale); `spec` is the structured/machine-readable
layer. The two are not merged — one field, one meaning.

**Scope.** This ADR decides the *schema* and that the tool **consumes and
displays** the spec. The north-star payoff — pairing the authored `spec` against
a *derived* analyzer (ADR-005) to compute **conformance** ("does the file still
do what its spec says?") — needs the analyzer layer and is **deferred to a
follow-up ADR**. The tool learns to read and show specs first; checking them
comes later.

## Consequences

### Positive

- Effort-proportional depth: nothing → `why` → full `spec`, one format, no
  breakage. Proven forward-compatible (older binaries tolerate richer manifests).
- Interdependence becomes declared, queryable data: `requires.entries` / `depends`
  reference other managed entries, making ADR-005's concern graph real.
- Sets up conformance (spec ⇆ derived) as a future second status axis without
  committing to it now.
- Never touches the config files; no per-format comment handling.

### Negative

- More schema to model, parse, validate, and render; capture-and-surface adds a
  catch-all map plus an "unrecognized" UI affordance.
- The recognized vocabulary must be curated and governed over time (core vs.
  extra), via ADR amendments.
- Authoring deep specs is real effort — mitigated: optional, agent-draftable,
  degrades to `why`.

### Neutral

- Extends ADR-002/003 (authored schema); leans on ADR-005 (derived) for the
  deferred conformance loop.
- The v1 vocabulary is a starting set, expected to grow.

## Alternatives Considered

- **YAML/JSON structure inside the `why` string.** Rejected: the manifest is
  already TOML; nesting a foreign format needs a second parser, loses
  diffability, and adds escaping pain. TOML expresses the structure natively.
- **Sigil-tagged markers inside the dotfiles.** Rejected (carried forward):
  per-format comment fragility, no-comment formats, directory entries, formatter
  clobbering. The manifest spec touches no config file.
- **`deny_unknown_fields` on `spec` (strict schema).** Rejected: catches typos
  but breaks forward-compat the instant a new recognized key is added.
  Capture-and-surface gets both properties.
- **Overload `why` as either prose or a structured object (untagged).**
  Rejected: muddies one field with two meanings; separate `why` (prose) + `spec`
  (structure) keeps each simple.
- **Fold conformance into this ADR.** Rejected: conformance needs the derived
  analyzer layer (substantial). Scoping this ADR to the schema keeps it
  shippable and lets the tool consume specs now.
