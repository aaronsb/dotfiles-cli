---
status: Proposed
date: 2026-07-19
deciders:
  - aaronsb
  - claude
related:
  - ADR-010
---

# ADR-011: Markdown+frontmatter settings fragments validated against the Claude Code schema

## Context

ADR-010 built the Claude settings projector on **raw JSON fragments**
(`claude/settings.d/NN-*.json`) and a **hand-rolled** `validate_fragment`
(object-ness + "permission lists must be arrays") backed by a runtime, base-aware
structural guard. Four rounds of adversarial review kept surfacing the same weakness:
a *hand-maintained* notion of "valid config" is brittle — it repeatedly missed
type-error vectors (a scalar at an object key, a non-array permission list, an
object-typed key outside a small allowlist). Each round patched one vector; the
class only closed once the guard was made general.

Two improvements close the class properly and make the store more humane:

1. **Fragments as Markdown + YAML frontmatter**, so each fragment carries its
   *rationale* alongside its data — self-documenting config, matching this repo's
   ADR/ways ethos and the (now-retired) agent-ways fragment convention. Raw JSON has
   nowhere to say *why*.
2. **Validate/lint fragments against Anthropic's real settings schema** rather than
   against our guesses. Every `~/.claude/settings.json` carries
   `"$schema": "https://json.schemastore.org/claude-code-settings.json"`; that schema
   is the authority on what a valid settings document is.

The schema was inspected (2026-07-19) and is a good fit:

- **JSON Schema Draft-07**, ~50+ top-level properties with declared types.
- **No `required` properties** — so a *partial* document (a fragment, or the
  compiled owned slice, which only ever sets some keys) validates cleanly.
- `permissions` declares `allow`/`deny`/`ask`/`additionalDirectories` as **string
  arrays** and `defaultMode` as a string enum — matching ADR-010's `UNION_LISTS`
  exactly, so that constant could be *derived* from the schema.
- `additionalProperties: true` at the top level — it validates the **types of known
  keys** but does **not** reject an unknown/misspelled key (`stausLine`). That bounds
  what schema validation alone can catch (see Decision 4).

## Decision

Adopt a Markdown+frontmatter fragment format as a **front-end swap** over ADR-010's
pipeline, and add **schema validation** as the principled replacement for the
type-checking half of `validate_fragment`. The merge, leaf-path ownership, base-aware
guard, self-audit, and projection I/O are all **unchanged** — they operate on
`serde_json::Value`, and this only changes how a fragment *becomes* a `Value` and how
its shape is checked.

**1. Fragment format — `NN-name.md` (frontmatter + body).**

```markdown
---
settings:
  env:
    ENABLE_PROMPT_CACHING_1H: "1"
  permissions:
    allow:
      - "Bash(dotfiles:*)"
title: prompt caching + dotfiles allow   # optional meta, for `show`
---

# Why prompt caching is on here

The rationale travels with the config. The projector ignores this body; `show`
surfaces it.
```

- The frontmatter's **`settings:` mapping** is the fragment's partial settings object
  (YAML → `serde_json::Value`). A `settings:` key (rather than the whole frontmatter
  being the settings) lets meta (`title`, `description`) coexist without ambiguity
  against a real settings key.
- The **Markdown body is documentation** — the durable *why*; ignored by the
  projector, surfaced by `show` (and optionally a `claude doc <fragment>` command).
- Loader: split the frontmatter between the leading `---` fences, parse it as YAML,
  take `.settings`. Everything downstream is identical to today.

**2. Schema validation replaces the type-checking in `validate_fragment`.**

- **Vendor** the Draft-07 schema in-repo
  (`crates/dotfiles-core/schema/claude-code-settings.schema.json`), embedded via
  `include_str!` so validation works offline and deterministically.
- Add the `jsonschema` crate (Draft-07) and a YAML parser (`serde_yaml`/`serde_yml`).
- Validate the **compiled owned slice** (`ours`) at `set`, `load`, and `project`
  time — the compiled result is what actually lands. Optionally validate each fragment
  individually too, for better error attribution (which file is wrong).
- Emit errors with the offending JSON path + expected type (e.g. `env: expected
  object, got string`; `permissions.allow[0]: expected string`).
- `validate_fragment` shrinks to **well-formedness** only: the file parses, has a
  frontmatter block, and `settings` is a mapping. All *shape/type* correctness moves
  to the schema.
- The runtime **base-aware structural guard (ADR-010) stays.** Schema validation is
  *author-time type-correctness against the spec*; the guard is *runtime protection
  against clobbering live foreign structure*. Different jobs — keep both.

**3. Derive `UNION_LISTS` from the schema (refinement).**

The schema is authoritative about which `permissions.*` keys are string arrays. Where
cheap, derive the union-list set from it (array-typed permission sub-properties)
instead of the hardcoded const, killing drift. If derivation proves fiddly, keep the
const but add a test asserting it equals what the schema declares.

**4. Strictness — a soft unknown-key warning.**

Because `additionalProperties: true`, the schema passes a misspelled top-level key. To
catch typos, **warn (not error)** when a fragment declares a top-level key absent from
the schema's `properties`. Warn-not-error because the vendored schema may lag a
newly-added Claude Code key, and we must not block a legitimately new setting.

**5. Schema freshness — vendored + refreshable.**

The embedded schema is the offline default. Ship `dotfiles claude schema [--refresh]`
that re-fetches from the source URL, rewrites the vendored copy, and records the source
URL + fetch date (sidecar or header). This makes the inevitable drift *visible and
closable* (the freshness way) rather than silent. A test asserts the embedded schema
parses as Draft-07 and validates a known-good sample.

**6. Migration — support both, convert the four.**

- The loader reads **both `.json` and `.md`** in `settings.d/` during the transition
  (both yield a `Value`), so the change is non-breaking.
- Convert the four current `.json` fragments (`10-statusline`, `20-attribution`,
  `30-env`, `40-permissions`) to `.md` with a starter docstring each. Optionally a
  `dotfiles claude migrate` command automates `.json` → `.md`.

## Consequences

### Positive
- Config is **self-documenting** — the *why* lives with the *what*, like ADRs/ways.
- Validation is **spec-grounded**, catching at author time exactly the type-error
  class four review rounds kept finding at runtime.
- The schema-refresh story keeps validation tracking upstream instead of rotting.
- `UNION_LISTS` (and future array keys like `ask`/`additionalDirectories`) can come
  from the schema, not a hand-maintained list.

### Negative
- New dependencies: `jsonschema` + a YAML parser.
- A **vendored schema is a freshness liability** (mitigated by the refresh command +
  recorded fetch date).
- `additionalProperties: true` bounds typo-catching (mitigated by the soft warning).
- Two fragment formats coexist during migration.

### Neutral
- The merge / guard / self-audit / projection **core is untouched** — this is
  additive and front-end only.

## Alternatives Considered

- **Keep raw JSON fragments** — rejected: no place for rationale; validation stays
  ad-hoc and brittle (the thing the reviews flagged).
- **TOML fragments** (matching the manifest) — rejected: YAML-frontmatter-in-Markdown
  is the ways/agent-ways convention and pairs naturally with a prose doc body.
- **Fetch the schema at runtime** (no vendoring) — rejected: offline/network
  fragility; vendor + explicit refresh is robust and deterministic.
- **Validate each fragment in isolation only** — insufficient: fragments can each be
  valid while the *compiled* result is what lands; validate the compiled slice
  (optionally also per-fragment for error locality).

## Implementation Plan (for the build session)

1. Add deps to `dotfiles-core`: `jsonschema` (Draft-07) and `serde_yaml`/`serde_yml`.
2. Vendor `crates/dotfiles-core/schema/claude-code-settings.schema.json` (a fetched
   snapshot) and embed via `include_str!`.
3. Extend the fragment loader (`claude.rs::read_fragments`) to handle `.md`: split
   frontmatter, parse YAML → `Value`, take `.settings`; keep `.json` support.
4. New validator in `dotfiles-core` (e.g. `settings_schema.rs`): compile the embedded
   schema once; `validate(&Value) -> Result<(), Vec<SchemaError>>` (path + message).
5. Wire validation into `load_store` / `set` / `project` (validate compiled `ours`);
   add the soft unknown-top-level-key warning.
6. Slim `validate_fragment` to well-formedness; derive `UNION_LISTS` from the schema
   (or add the cross-check test).
7. Add `dotfiles claude schema [--refresh]` (records source URL + fetch date).
8. Migrate the four `.json` fragments to `.md` (+ docstrings); optional
   `dotfiles claude migrate`.
9. Tests: schema rejects `env` scalar and non-string `allow[0]`; a partial validates;
   the unknown-key warning fires; `.md` parses and equals its `.json` equivalent; the
   embedded schema parses as Draft-07.
10. Update ADR-010's cross-reference and the README/docs; move this ADR to Accepted.

## See Also
- ADR-010 — the projector this extends (merge, leaf-path ownership, base-aware guard).
- Source schema — `https://www.schemastore.org/claude-code-settings.json` (Draft-07).
