---
status: Draft
date: 2026-06-21
deciders:
  - aaronsb
  - claude
related:
  - ADR-002
---

# ADR-001: Clean-room agent-native dotfiles tool on the shared manifest

## Context

The dotfiles config store (`aaronsb/dotfiles`) is managed today by a small Bash
script driving a plain-text `.dotfiles-manifest`: symlink deployment (with a
copy mode for nested git repos) and the verbs `status`, `deploy`, `add`,
`enable`, `disable`. Because configs deploy as symlinks, an edit to the repo
file and the deployed file are the *same bytes* — there is no apply/sync step.

We want to iterate this into something richer. The *primary* altitude is a
**documented inventory** — what we have dotfiles for, their properties, and
*why* each exists (see ADR-002) — not byte-level inspection of file contents.
On top of that sits an **optional** collaborative-review surface: a live
"observe" view of the git working-tree diff, for drilling into an actual
change. Review is never a forced step — a human may trust the change or open
their own editor — and the value holds with or without an agent: an agent
*amplifies* the workflow, it is not a prerequisite. This makes the tool
**git-native**, not AI-specific; the agent-friendliness falls out of surfacing
git, which agents already speak.

A survey of the field found that TUI-based dotfiles managers barely exist; the
one serious example, **DotState** (Rust/Ratatui, MIT), is a striking
*convergent design* — symlink deployment, `activate`/`deactivate`, `add`/`sync`
— but it ships its own storage/profile engine, and notably does **not** do live
file watching. Forking it would force us to retain its license/attribution and
either gut or adopt a foreign storage model, and would collide on the
`dotfiles` name. The lesson (learned before): once you have seen the *shape* of
a tool, a clean build on your own model is usually faster than bending someone
else's frame to fit.

The live status view is not an exotic bet. The config store is a git repo and
managed files are symlinks into it, so *what changed* is exactly the git
working-tree diff (`git status` / `git diff`) — the source of truth this repo
already declares. The mechanism is a **git working-tree watcher**, not a bespoke
filesystem-event layer; it is well-understood and cheap to reverse, so no
prototype gate is warranted.

## Decision

Build **`dotfiles-tui`**, a clean-room, **git-native** lifecycle tool —
agent-friendly by construction, not agent-specific — on the following
invariants:

1. **The manifest is the durable contract; tools are interchangeable, optional
   readers of it.** `dotfiles-tui` must read and write the *existing*
   `.dotfiles-manifest` format and symlink semantics natively. The format stays
   plain-text, diffable, and **legible enough to apply by hand** — the tool may
   build machinery *around* the manifest but must never make the manifest
   itself require a tool to interpret.

2. **Clean-room, not a fork.** DotState validated the lifecycle-TUI shape and
   revealed the missing live-watch niche; we take the *shape*, not the code. No
   attribution burden, no foreign engine, no name collision.

3. **The config store stays a pure config store.** It holds dotfiles + the
   manifest + (for now) the Bash script — which is reframed from "legacy tool"
   to the **reference implementation / executable specification** of the
   manifest, and the dependency-free fallback. Cloning the config store never
   *requires* `dotfiles-tui`. Dependency arrow: `manifest ← bash (spec/fallback)`
   and `manifest ← dotfiles-tui (accelerator)`, with the manifest answerable to
   neither.

4. **One core, two front-ends.** A pure core crate (manifest parsing, symlink
   ops, watch loop, status) behind: a **non-interactive JSON CLI** (the agent
   surface — fully scriptable, structured output) and a **Ratatui TUI** (the
   human surface — primarily the documented inventory (ADR-002), with an
   *optional* "observe" view that renders the live git-diff of a changing
   file). Both present the same state.

5. **Polyrepo + submodule, release-based production install.** `dotfiles-tui`
   is its own repo, linked into the config store as a submodule at
   `dotfiles-tui/` for development. Production installs do **not** recurse the
   submodule; they fetch a prebuilt binary from GitHub Releases. The config
   store records the desired tool version as a plain string (a lockfile-like
   pin); `git pull` + re-run installer is a controlled, in-place upgrade.

6. **Strangler-fig migration, not a cutover.** The Bash script stays
   authoritative until `dotfiles-tui` reaches parity *against the same
   manifest*. Parity = the binary handles every verb the script does, at which
   point Bash demotes to reference spec / fallback (it is not necessarily
   deleted).

7. **Git is the change feed, and a hard precondition the tool establishes for
   itself.** Live status is computed from the git working-tree diff of the
   watched repo (`git status` / `git diff`) — not a bespoke inotify/stat layer.
   On first run against a location with no git repo, the tool refuses to operate
   and directs the user to create one (*"no git repo found — init your dotfiles
   repo to begin"*); from then on git is assumed. Both deployment modes already
   satisfy this: symlink targets live in the store's tree, and copy-mode targets
   are nested git repos with their own tree. This binds the *application*, not
   the manifest contract — the manifest stays git- and tool-independent and
   remains appliable by hand.

## Consequences

### Positive

- The config store outlives any tool that reads it; the manifest is the
  durable asset and remains hand-appliable with zero tooling.
- Clean separation of *state* (config store) from *engine* (tool) — independent
  release cadences, no Rust project history polluting personal config history.
- The dual front-end resolves the real constraint directly: an agent drives the
  JSON CLI, a human drives the TUI, both over one core and one source of truth.
- Live-watch is nearly free *because* of the symlink-equals-same-bytes
  invariant — no apply step to lag behind, which is exactly what copy-based
  managers (chezmoi, yadm) cannot do cleanly.
- No fork means no attribution/engine/name-collision debt.
- The watcher reuses git's existing semantic diff instead of inventing a change
  feed, and the first-run init gate guarantees its own precondition by
  construction — one mechanism, no degraded fallback path to maintain.

### Negative

- Two tools coexist during migration; the shared-manifest invariant must hold
  or they drift. This is a hard constraint, not a guideline.
- We reimplement symlink/manifest logic in Rust rather than inheriting a
  working engine — more upfront work than a fork.
- A submodule adds a (development-only) moving part and a version-pinning
  discipline the config store must maintain.
- Git becomes mandatory to *use the tool*: dotfiles kept outside version
  control must be `git init`ed first. This is a deliberate first-run gate, not a
  silent failure — and it does not constrain hand-application of the manifest.

### Neutral

- Establishes this repo's own ADR series (foundation / interface / packaging).
- Change *detection* (the git working-tree watcher) is distinct from change
  *rendering* (syntax-highlighting the diff in the TUI, e.g. via tree-sitter);
  the latter is an interface concern, deferred to an interface ADR.
- Binary name (vs. the repo name `dotfiles-tui`) is deferred to an interface
  ADR; `dotf`/`dotctl` are candidates, since the CLI is also the scripting
  surface.

## Alternatives Considered

- **Fork DotState and rebrand it.** Rejected: MIT obliges retaining its
  copyright/license, and we would inherit its storage/profile engine (foreign
  to our manifest model) only to gut it, plus a `dotfiles` name collision. The
  value was seeing the *shape*, which is not copyrightable.
- **Adopt DotState's profile/storage model wholesale.** Rejected: would force
  migrating the config store off its proven `.dotfiles-manifest`, lose copy
  mode for nested git repos, and break the hand-appliable / diffable property
  we explicitly want to keep.
- **Monorepo the tool inside the config store.** Rejected: couples a Rust
  application's release/versioning lifecycle to a personal config store and
  drags the tool's history into every config clone. Polyrepo + submodule keeps
  state and engine separate while still allowing in-tree development.
- **Rewrite the Bash script as a TUI in place.** Rejected: DotState already
  occupies the "lifecycle TUI" niche better than a Bash rewrite would, and it
  abandons the scriptable/agent surface. One-core-two-front-ends keeps both the
  human and the agent first-class.
- **Keep the Bash script only (do nothing).** Rejected: it offers no
  high-altitude "what do I manage and why" inventory (ADR-002) and no optional
  review view — the motivating capabilities.
