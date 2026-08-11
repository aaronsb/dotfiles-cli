---
status: Draft
date: 2026-08-11
deciders:
  - aaronsb
  - claude
related:
  - ADR-002
  - ADR-003
  - ADR-006
  - ADR-008
  - ADR-101
  - ADR-103
---

# ADR-011: Per-profile content variants and a ref-addressed profile CRUD

## Context

ADR-008 gave profiles a *membership* axis: an entry is universal, or it belongs
to a listed set of profiles, and the active profile decides what deploys. That
answers "does this machine get nvim at all?"

It does not answer "does this machine get *the same* nvim?" Every entry still
resolves to exactly one `path` in the store, so two profiles that both take an
entry take byte-identical content. The only way to diverge was to fork the entry
into a second `[[entry]]` with a different name and the same `target` — which
duplicates the catalog row, splits the `why` (ADR-002), and makes the two halves
invisible to each other.

The concrete pressure came from shell config. One host drifts (a plugin swapped
out, a completion tweak) while the others should stay put. Today that drift is
either committed — and lands everywhere — or held as an uncommitted working-tree
edit forever, which is not management, just deferral. There was also no way to
*see* the difference: `pkg diff` compares package sets between hosts, but nothing
compared dotfiles between profiles, and the profile verbs (`copy`) could move
memberships and package lists but never content.

So two gaps, and they are the same gap seen from two sides: profiles cannot hold
different content, and profiles cannot be compared or reconciled item by item.

## Decision

Give an entry **per-profile content variants**, and address profile operations by
a **ref** so read, write, and delete share one grammar.

### 1. Variants — a `paths` override map on the entry

`Entry` gains `paths: BTreeMap<String, String>` (TOML `paths.<profile> =
"<repo-relative path>"`). `path` stays the **base** — the content every profile
gets unless it overrides:

```toml
[[entry]]
name   = "nvim"
path   = "nvim"            # base
target = ".config/nvim"
paths.slab = "nvim-slab"   # slab's variant
paths.cube = "nvim-minimal"
```

Resolution is `entry.path_for(profile) = paths.get(profile).unwrap_or(path)`.
One catalog row still describes one config: one `name`, one `target`, one `why`,
one membership list — variants branch only the bytes.

The override is **explicit in the manifest**, not an implicit filesystem overlay
(`profiles/<name>/<path>` shadowing the base). The manifest is the catalog
(ADR-002/003); a variant is a fact about the config and belongs where the rest of
its facts are, greppable and reviewable in a diff. An overlay directory would
make divergence invisible until deploy time.

### 2. Resolution happens once, at the edge

`Manifest::resolved(profile)` returns a copy with every entry's `path` replaced by
its `path_for(profile)`. The CLI resolves at load; `deploy`, `deploy_status`,
`status`, `list`, and `show` stay profile-ignorant and need no signature change.
Only the `profile` verb reads the raw, unresolved manifest.

### 3. Refs — one address for every profile operation

A **ref** is `<profile>` or `<profile>/<entry>`. A bare profile means the whole
scope; a qualified ref means one item. Every profile verb takes refs, so the CRUD
surface is uniform:

| verb | ref form | effect |
|---|---|---|
| `diff [A] [B] [--details]` | either | compare two profiles, or one entry across them |
| `push <src> <dst> [--force] [--as P]` | either | copy src's content/scope onto dst |
| `pull <src> [<dst>] [--force] [--as P]` | either | the same, dst defaulting to the active profile |
| `remove <ref> [--purge]` | either | drop the profile, or drop one item's variant |

`push north/nvim slab` and `pull slab/nvim` are the same operation from the two
ends; `pull` exists because "bring that machine's version here" is the direction
the operator is usually standing in.

### 4. Push/pull semantics — overwrite or create, never silently

Pushing entry *E* from *S* to *D*:

1. *S*'s effective path must exist in the store, else error.
2. *D* has a variant → **overwrite** it, but only under `--force`; without it,
   print the diff and stop.
3. *D* has no variant → **create** one at `<base>-<D>` (override with `--as`),
   write `paths.<D>`, and copy the content.
4. If the two effective paths are the same path, or their content is identical,
   report that and change nothing.
5. If *E* is scoped and *D* is not a member, *D* is added to `profiles` — pushing
   content to a profile that would not deploy it is never what was meant.

The destructive step is always opt-in, and refusal always shows the diff that
justified it — the same posture as `remove --purge` and `deploy --force`.

### 5. `remove <profile>/<entry>` drops the variant, not the config

It deletes `paths.<profile>`, so that profile falls back to base. Files are kept
unless `--purge`. If there is no variant, it strips the membership tag instead,
and says which of the two it did. Deployed files are never touched.

### 6. `copy` becomes an alias of whole-profile `push`

ADR-008's `copy <src> <dst> [--only|--dotfiles|--pkg]` is exactly `push` on bare
refs, and `--only E` is exactly `push <src>/E <dst>`. `copy` is kept as an alias
so existing muscle memory and scripts keep working, and its flags are kept on
`push`.

### 7. `diff` reports provenance, not just difference

Each side of a row reads `base`, `variant`, `—` (not in that profile), or
`disabled`, and the state reads `same`, `differs (+n -m)`, or `only in <P>`.
Knowing *why* two profiles differ — one has a variant, versus one simply lacks the
entry — is the part that decides what to do next. `--details` renders the file
diff through the existing friendly diff view (ADR-103); package-list drift stays
`pkg diff`'s job and is summarized in one line with a pointer to it.

## Consequences

### Positive

- A machine can genuinely diverge on one config and stay converged on the rest,
  without forking the catalog row or holding the drift as an uncommitted edit.
- Divergence is declared and visible: `paths.slab` sits in the manifest next to
  the `why`, and `profile diff` names it.
- One ref grammar covers compare, push, pull, and remove, so the profile verb
  stops being a bag of differently-shaped flags.
- Resolving at the edge means the deploy and status paths — the parts that touch
  the filesystem — did not change at all.

### Negative

- **Variant granularity is the entry, not the file.** An entry whose `path` is a
  directory (`zsh/.zsh`) can only vary as a whole tree, so a one-line divergence
  costs a full copy that then drifts on its own. The escape is to split the
  divergent fragment into its own entry; nothing here makes that automatic, and
  a coarse variant is a real maintenance cost.
- A third axis to reason about: `enabled` (managed), `profiles` (in scope), and
  now `paths` (which bytes). The first two were already documented as a pair;
  this makes the mental model wider.
- Copied variants have no shared ancestry — the tool tracks no relationship
  between `nvim` and `nvim-slab`, so a fix made to one does not surface as
  missing from the other. `profile diff` is the only thing that will tell you.

### Neutral

- Fully backward compatible: an entry with no `paths` resolves to `path` exactly
  as before, and a manifest with no profiles is unaffected.
- Variant paths are ordinary store paths, so they are committed, deployed, and
  backed up by the same machinery as everything else.

## Alternatives Considered

- **Implicit overlay directory** (`profiles/<name>/<path>` shadows the base) —
  rejected: convention-over-config hides divergence from the catalog, and a
  stray directory would silently change what a machine deploys.
- **Separate `[[entry]]` per profile** (the status quo workaround) — rejected: it
  duplicates `target`, `why`, and `spec` across rows that describe one config,
  which is precisely what ADR-002 exists to prevent.
- **Per-profile manifests with a base** — rejected again for the reasons in
  ADR-008: it fragments the single self-documenting catalog.
- **Verb-in-the-middle grammar** (`profile north/zsh push slab/zsh`, as first
  sketched) — rejected: it reads well but is not a clap subcommand (ADR-101),
  and it breaks the shape every other verb in the tool has. Verb-first keeps
  `push`/`pull`/`diff`/`remove` siblings of `add`/`use`/`list`.
- **Three-way merge instead of overwrite** — rejected for now: without recorded
  ancestry there is no base to merge against, and inventing one would make the
  tool a VCS. The store is already in git; `push --force` plus a commit is the
  honest primitive.
