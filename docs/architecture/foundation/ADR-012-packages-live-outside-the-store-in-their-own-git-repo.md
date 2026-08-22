---
status: Superseded
date: 2026-08-21
deciders:
  - aaronsb
  - claude
related:
  - ADR-013
  - ADR-002
  - ADR-003
  - ADR-008
  - ADR-201
---

# ADR-012: Packages live outside the store, in their own git repo

> **Superseded by [ADR-013](./ADR-013-three-repos-tool-registry-store-and-a-host-binding-outside-the-store.md)**
> §3–§4. The argument stands: package lists are not catalog content and need
> history. The remedy changed: the store itself is private and is a git repo,
> so packages return to `<store>/packages/<profile>/`, and the machine-local
> file moves out of the store into the host binding. The implementation on
> `feat/packages-outside-store` was not merged.

## Context

Package tracking was inherited from the bash tool as `packages/<host>/{native,
aur,flatpak}.txt` and reframed by ADR-008 §3 as `packages/<profile>/`. The path
has been fixed to the store ever since:

```rust
let packages_dir = ctx.repo_root.join("packages");   // pkg.rs:56
```

Two things now push against that.

**The store is becoming publishable.** ADR-201 turns the public repo into a
starter anybody can clone and deploy. A store carrying `packages/north/`,
`packages/slab/`, `packages/cube/`, and `packages/padnoir/` hands a stranger
four hostnames of package lists that mean nothing to them. The landing page
tells people to run it, so this is live confusion, not a hypothetical.

**And the deeper problem is that they were never catalog content.** The manifest
is a self-documenting catalog of *what is managed and why* (ADR-002). A list of
which packages happen to be installed on a machine called `slab` is neither
shared nor durable nor explanatory. It is per-machine, per-person state that has
been living in a repo built for the opposite kind of thing.

A third fact shapes the answer rather than the problem: `pkg capture` overwrites
the tracked file. Without version history, the previous state is simply gone —
so "when did this get installed, what did I prune, what did that box look like
in March" is unanswerable today.

## Decision

**The packages root is independent of the store, it is machine-local
configuration, and it is a git repo.**

### 1. Resolution

First non-empty wins, mirroring how `Ctx::resolve` already locates the store:

1. `--packages-dir`
2. `$DOTFILES_PACKAGES_DIR`
3. `packages.path` in the local config
4. `<store>/packages`

The default is unchanged, so every existing store keeps working with no config
and no migration.

### 2. The location is machine-local state, not manifest content

It is recorded in a **gitignored `.dotfiles-local.toml`** at the store root:

```toml
[packages]
path = "~/.local/share/dotfiles/packages"
```

`~` expands; a relative path resolves against the store root.

It does **not** go in the manifest. The manifest is committed and shared — a
starter that shipped a `packages.path` would ship one person's filesystem
layout, which is the bug this ADR exists to fix. ADR-008 §4 already established
`.dotfiles-profile` as gitignored per-machine state; this is the second such
item, and it gets a structured file the profile marker can later fold into.

### 3. One shape: packages live in a git repo

`pkg init` picks a path, runs `git init`, and writes the config. It then
*optionally* offers to create a remote (`gh repo create <user>/dotfiles-packages
--private`, `git remote add`, push).

There are no modes. A local repo and a published repo are the same thing minus a
remote, and upgrading is `git remote add` plus a push — ordinary git, not a
reconfiguration or a data move. Declining the remote leaves a fully tracked
repo, not a degraded state.

Private is the right default for a remote: package lists fingerprint what a
person runs.

### 4. The config stores a path and nothing else

Whether a remote exists is discoverable with `git remote`. Recording a `mode`
would create a second source of truth that drifts from the repo it describes.

### 5. The file is the authority; `pkg init` is a convenience

Any valid value is accepted whether a wizard wrote it or a person typed it.
There is no hidden state and no configuration the tool owns exclusively.
Changing your mind is opening the file.

### 6. `gh` is optional

Absent or unauthenticated, the offer reduces to local-only without complaint.
Putting a CLI dependency in front of setting up a machine is the kind of
requirement this project declines to make.

## Consequences

### Positive

- A published starter cannot ship host manifests, because host manifests are no
  longer something a store contains.
- Package state gets history. `capture` becomes a reviewable diff instead of a
  silent overwrite.
- The data model is forge-agnostic — the config holds a path, so the forge
  appears only in `pkg init`'s offer. GitLab or Gitea later is a branch in one
  function, not a schema change.
- Fully backward compatible; the default path is what it has always been.

### Negative

- **Two repos to commit.** `dotfiles push` operates on the store and will not
  see the packages repo. `pkg capture` leaves it dirty and the operator commits
  it themselves. A `--commit` flag on `capture` is the obvious ergonomic fix and
  is deliberately deferred rather than designed here.
- A second piece of intentionally-uncommitted state in the store, and a new file
  to know about.

### Neutral

- `pkg`'s internals need almost nothing: every function already takes
  `packages_dir: &Path`, so exactly one construction site changes.
- `.dotfiles-local.toml` is introduced with one key. It is shaped to absorb
  other machine-local settings later, including `.dotfiles-profile`.

## Alternatives Considered

- **A `packages.path` key in the manifest.** Rejected: the manifest is committed
  and shared, so a per-machine path in it propagates to everyone who clones the
  starter. The self-documenting-catalog argument (ADR-002) does not reach a
  value that is true of one filesystem.
- **Environment variable only.** Rejected: ambient, invisible, and lost on a new
  shell. Machine-local state should be a file someone can read.
- **An untracked local directory.** Rejected: `capture` overwrites, so the value
  of tracking package state is precisely the history that an untracked directory
  throws away.
- **A `mode` field (`local` / `repo` / `in-store`).** Rejected: derivable from
  the repo itself, and a stored copy is a second source of truth.
- **Teaching `push`/`pull` to span both repos.** Rejected as scope: it is one
  `git -C` away for the operator, and a two-repo git verb is a meaningfully
  larger commitment than this decision needs.
- **Leaving packages in the store and gitignoring the personal ones.** Rejected:
  it makes the starter's emptiness accidental rather than structural, and the
  next person to run `pkg capture` re-creates the problem.
