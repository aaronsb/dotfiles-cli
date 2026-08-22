---
status: Accepted
date: 2026-08-21
deciders:
  - aaronsb
  - claude
related:
  - ADR-001
  - ADR-002
  - ADR-008
  - ADR-011
  - ADR-012
  - ADR-103
  - ADR-200
  - ADR-201
---

# ADR-013: Three repos — tool, registry, store — and a host binding outside the store

## Context

ADR-201 split one public repo into an engine and a private downstream store,
related by a git `upstream` remote, and declared that the CLI would not change.
ADR-012 moved package lists out of the store into a separate git repo, located
by a `.dotfiles-local.toml` at the store root. Both were drafted on the same day
and both are correct about the pressure: the store is one operator's
machine-specific state and was living in a public repo built for sharing.

What neither answers is *sharing configurations*. An operator should be able to
publish their dotfiles under a name, another operator should be able to run
that configuration on one of their hosts while running their own on the
others, and a third person should be able to contribute a configuration by
pull request. ADR-201's topology has one public "starter" and a private
descendant per operator; there is no place for a second public configuration,
and nothing names the one that exists.

Two structural facts shape the answer.

**A profile cannot choose its store.** ADR-008 profiles (`north`, `slab`) live in
`[profiles.<name>]` inside the manifest, inside the store. Whatever decides
*which* store a host reads therefore cannot be a profile — it has to be read
before the store is. The existing `.dotfiles-profile` marker and ADR-012's
`.dotfiles-local.toml` both sit at the store root and share the same flaw once
the store itself is a choice.

**The engine cannot travel inside the store.** ADR-201 lets every store inherit
`bootstrap.sh`, `install.sh`, and `.dotfiles-cli.version` from the starter by
merge. That is tolerable for one ancestor; with many published configurations
it means every one of them ships a copy of the engine at whatever age its
author last merged.

A third fact is a convenience rather than a constraint: `Ctx::resolve`
(`main.rs:157`) already locates the store by `--repo-root` → `$DOTFILES_DIR` →
`~/.dotfiles`, `pkg` already takes its root as a parameter, and the git verbs
are `git -C <store>`. Making the store a resolved value instead of a fixed path
is a small change in one function.

## Decision

Three repos with distinct owners, a host binding that lives outside all of them,
and an `init` verb that creates or reconfigures a host deterministically.

### 1. Three repos

| Repo | Owner | Contents | Written by |
|---|---|---|---|
| **tool** — `dotfiles-cli` | the project | the Rust CLI, `bootstrap.sh`, `install.sh`, releases | pull request |
| **registry** — `dotfiles` (public) | the project | `configs/<id>/`, one directory per published configuration: a manifest plus the content it names; `registry.toml` indexing them | pull request, reviewed by the project |
| **store** — `<operator>/<anything>` | the operator | one configuration tree (a copy of a registry entry), its manifest carrying the operator's ADR-008 profiles and ADR-011 variants, and `packages/<profile>/` | the operator, daily |

A store contains no engine files. `bootstrap.sh`, `install.sh`, and the README
that describes the tool move to the tool repo. The store keeps
`.dotfiles-cli.version` (ADR-200) because the store's *manifest* is what depends
on a CLI capability.

A registry entry is a store minus the operator: the base manifest with no
`[profiles.*]` tables and no `paths.*` variants, plus the content those entries
name. Each entry declares `cli = ">=<version>"` in its `registry.toml` row so the
tool can refuse an entry it cannot read.

### 2. Configuration ids are reverse-DNS

A published configuration is named `<reversed-domain>.<variant>`:
`com.bockelie.dotfiles`, `com.bockelie.minimal`, `sh.dotarchy.starter`. The
domain half is the namespace and carries ownership; the variant half lets one
owner publish several. Namespace ownership is enforced by pull-request review
and nothing else; at registry scale that is the right amount of ceremony.

The store records which entry it descends from as `[store] config = "<id>"` in
the host binding (§3), and `"unregistered"` when it descends from none.

### 3. The host binding — `$XDG_CONFIG_HOME/dotfiles/config.toml`

```toml
[store]
path    = "~/.dotfiles"               # local clone; a remote is optional
config  = "com.bockelie.dotfiles"     # registry entry it descends from
profile = "north"                     # ADR-008 profile within it
```

This file is per machine, never inside a store, and never committed anywhere.
It absorbs `.dotfiles-profile` (ADR-008 §4) and `.dotfiles-local.toml` (ADR-012
§2), which are retired. Resolution order for the store root becomes
`--repo-root` → `$DOTFILES_DIR` → `store.path` → `~/.dotfiles`; for the active
profile, `--profile` → `$DOTFILES_PROFILE` → `store.profile` → ADR-008's `match`
glob → hostname. The flag and env var stay as the escape hatch for scripts and
for pointing the tool at a second store without rebinding the host.

Writes to the binding go to a temporary file and rename into place, so an
interrupted reconfigure never leaves a host pointing at a store it has not
pulled.

### 4. Packages return to the store

`packages/<profile>/` lives in the store, as ADR-008 §3 had it. ADR-012 exiled
packages because the store was public; a private store removes the reason, and
the history that ADR-012 wanted for `pkg capture` comes from the store's own
git log. ADR-012's "one shape: a git repo with an optional remote" survives
and widens from the packages directory to the whole store (§5). `--packages-dir`
and `$DOTFILES_PACKAGES_DIR` are kept as overrides; `packages.path` in a local
config is not, because the file it lived in is gone.

### 5. `dotfiles init` — first install and every reconfigure

`init` is the single entry point for making a host. `bootstrap.sh` shrinks to:
install the release binary, run `dotfiles init`, offer `deploy`. Every wizard
question has a flag, so a second machine is one non-interactive line:

```
dotfiles init --config com.bockelie.dotfiles --store ~/.dotfiles --remote github --profile north
```

**First install** (no binding, no store): choose a registry entry (default
`sh.dotarchy.starter`) or an existing store URL to clone; choose a store path;
choose local-only or create a remote (`gh repo create --private`, offered only
when `gh` is present and authenticated, declined without complaint otherwise);
choose a profile (default: hostname). The registry entry is copied in as the
store's first commit, `packages/<profile>/` is created, and the binding is
written.

**Reconfigure** (a binding or a store already exists): `init` asks one
question — keep, replace, or rebase — and each answer is one git operation:

| Path | Precondition | Effect |
|---|---|---|
| **migrate** | a pre-ADR-013 store: engine files at the root, `.dotfiles-profile` or `.dotfiles-local.toml` present | removes the engine files, folds the two marker files into the binding, records `config` (`"unregistered"` unless `--config` says otherwise), commits once on the existing history |
| **clean start** | any | moves the store to `~/.dotfiles-backup/<timestamp>/`, then runs first-install |
| **rebase** | a store that should now descend from a different entry | rewrites `config`, pulls the new entry over the base content, leaves conflicts in the working tree and renders them through the friendly diff (ADR-103) |

Every path is idempotent: `init` on an already-current host prints the binding
and exits 0. Every path honours `--what-if`, and rebase's preview lists the
files that would conflict before anything moves. Clean start is the only
destructive path and it backs up first, the same posture as `deploy --force`.

Rebase is the mechanism behind "run their configuration on this host": the
store keeps the operator's profiles, variants, and package lists; only the
base content changes ancestry. Entries the operator holds an ADR-011 variant for
cannot conflict, because the variant file is theirs and only the base moves.

### 6. Upstream in both directions is `git subtree`, wrapped

A registry entry is a subdirectory of another repo, so the store's upstream
relationship is a subtree, not a remote:

- **down** — `dotfiles store upstream pull` splits `configs/<id>` of the
  registry into a root-relative history (`git subtree split`), fetches it, and
  merges. A registry entry never carries store-only content, so the merge
  **cannot delete it**: `packages/`, `.dotfiles-cli.version`, `.gitignore`,
  the store's README and CLAUDE.md are restored if upstream lacks them, and
  the manifest's `[profiles.*]` tables and `paths.*` variants are grafted
  back onto the merged manifest. Per-machine content that lives *inside* a
  managed entry — a `host.d/` of per-host shell overlays — is declared in the
  manifest as `[store] local = ["zsh/.zsh/host.d"]`; the merge protects those
  paths the same way, and a registry entry carries neither the paths nor the
  table. First install is the same fetch onto an
  empty repo, so the store begins with the entry's history and later pulls
  are ordinary related merges.
- **up** — `dotfiles publish` runs `git subtree split` on the store, strips
  `[profiles.*]`, `paths.*`, and `packages/` from the split, pushes the result to
  a branch on the operator's fork of the registry, and opens a pull request
  when `gh` is available. For the owner of the id this is a self-merge; for
  anyone else it is the contribution path.

Subtree is the fiddliest thing in this decision. The tool owning both
directions is what keeps the registry current; if operators have to drive
subtree by hand they will copy files instead, and the registry goes stale.

### 7. What each existing ADR becomes

- **ADR-008, ADR-011** — unchanged. Profiles and variants are store-internal.
- **ADR-012** — superseded by §3 and §4. Its argument that package lists are
  not catalog content stands; its remedy is replaced.
- **ADR-200** — unchanged in mechanism, gains a second reader: `init` checks a
  registry entry's `cli` requirement before copying it in.
- **ADR-201** — keeps its thesis (public ancestor, private descendant). §1's
  repo list is replaced by §1 here, §4 ("the CLI does not change") is withdrawn,
  and the starter becomes the registry entry `sh.dotarchy.starter`.

## Consequences

### Positive

- A host is reproducible from one file and one command. The binding plus
  `dotfiles init` plus `deploy` is the entire provisioning story, and it is the
  same story on the first machine and the fifth.
- Published configurations are first-class: named, owned by namespace,
  contributed by pull request, and usable per host. One operator can run two
  people's configurations on two machines with two bindings.
- The store is one repo again: dotfiles, profiles, variants, and packages share
  a history, and `dotfiles push` commits all of it. ADR-012's two-repo commit
  problem disappears.
- The engine lives in exactly one place. A stale `bootstrap.sh` inside a store
  is no longer possible, because a store has none.
- Migration for existing hosts is `dotfiles init` and nothing else.

### Negative

- Subtree mechanics are in the tool and have to be right. `publish` rewrites
  the manifest on the way up, which is a transformation that has to be tested
  against every manifest shape the tool accepts.
- A merged registry means every published configuration's updates pass through
  the project's review. At a handful of entries that is a feature; at hundreds
  it is a queue, and the index-style alternative below would be revisited.
- The store resolution gains a step and the tool gains a file outside the
  store to know about. `dotfiles status` prints the binding so the answer to
  "which store am I on" is one command.
- Rebase can produce merge conflicts in base content, and the tool's job is to
  show them well rather than resolve them.

### Neutral

- `aaronsb/dotfiles` becomes the operator's store: flipped private, engine
  files removed, history kept. Its first `publish` seeds
  `configs/com.bockelie.dotfiles/`. The four existing hosts take the migrate
  path, which is the proof of that path before anyone else uses it.
- `feat/packages-outside-store` (ADR-012's implementation) is not merged.
- `registry.toml` is the one file in the registry the tool reads directly; the
  `configs/<id>/` directories are read through git.

## Alternatives Considered

- **Index-style registry** — `registry.toml` maps id to the author's own repo
  URL, content stays with the author, upstream is an ordinary remote. Rejected
  for now: it is the lighter mechanism, and it was not chosen because a merged
  registry puts every configuration under one review and one clone, which is
  what makes it a catalogue someone can browse. The subtree cost in §6 is the
  price of that. If the registry outgrows review, this is the fallback.
- **Vendoring the tool into the registry repo** (a monorepo). Rejected: the
  tool releases on its own cadence and the store pins it (ADR-200); a registry
  entry is content, and content should not drag a Rust workspace with it.
- **Keeping the host binding at the store root** (ADR-012's placement).
  Rejected: the binding names the store, so it cannot live inside it.
- **Profiles as host bindings** — letting `[profiles.north]` name a
  configuration. Rejected: the profile is read from the manifest, which is read
  from the store, which the binding selects. The dependency only runs one way.
- **Separate verbs for migrate, clean start, and rebase.** Rejected: they share
  every precondition check and write the same file; one verb with one question
  keeps bootstrap able to call `init` unconditionally.
- **Publishing variants and profiles with the configuration.** Rejected: they
  ship the operator's hostnames and per-machine drift, which is exactly the
  state a registry entry should not carry.
