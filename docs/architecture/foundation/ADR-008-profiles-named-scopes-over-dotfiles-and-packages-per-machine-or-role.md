---
status: Accepted
date: 2026-06-22
deciders:
  - aaronsb
  - claude
related:
  - ADR-001
  - ADR-002
  - ADR-003
  - ADR-007
---

# ADR-008: Profiles — named scopes over dotfiles and packages, per machine or role

## Context

The tool manages configs across multiple machines. Two surfaces had grown
asymmetric:

- **Packages were already per-host** — tracked under `packages/<host>/{native,
  aur,flatpak}.txt` (carried over from the bash tool). Each machine has its own
  desired package set.
- **Dotfiles were global** — one manifest, a single `enabled` bool per entry. The
  founding assumption (ADR-001) was "the same dotfiles everywhere," which is true
  for the common case but breaks down once a machine genuinely differs (a headless
  server wants shell + tmux but not the KDE/plasma configs; a fleet of throwaway
  build VMs wants a minimal subset).

That asymmetry *was* the gap: there was no first-class way to say "this
machine/role gets this subset of dotfiles **and** this package set," nor to
declare such a grouping up front, copy it to another machine, or copy just a part
(one dotfile, all dotfiles, or a package config) between machines.

ADR-001 deliberately did **not** adopt DotState's profile/storage model when the
tool was founded — correctly, because the need was not yet concrete and the
storage model was foreign. The need is now concrete (a multi-machine setup with a
VM fleet), so we revisit — adopting the *concept* of profiles on our own
clean-room schema, not DotState's engine.

## Decision

Introduce **profiles**: a named scope over **both** dotfiles and packages. A
profile names a machine or role; the active profile selects which entries deploy
and which package set applies.

1. **Per-entry tag.** `Entry` gains `profiles: Vec<String>` (TOML `profiles =
   ["desktop"]`). Empty = *universal* (active in every profile). Non-empty =
   active only in the listed profiles. `enabled` stays orthogonal (an entry can be
   enabled yet inactive in the current profile).
2. **Registry.** A `[profiles.<name>]` table declares each profile with an
   optional `description` (self-documenting, ADR-002 in spirit) and an optional
   `match` glob. Unknown keys are captured-and-surfaced (ADR-006 style).
3. **Packages become per-profile** — `packages/<profile>/`. The default profile is
   the hostname, so existing `packages/<host>/` layouts keep working unchanged.
4. **Active-profile resolution** (first non-empty wins): the `--profile` flag →
   `$DOTFILES_PROFILE` → a `<store>/.dotfiles-profile` file → the first declared
   profile whose `match` glob matches the short hostname (**fleet support** —
   `match = "vm-*"` maps vm-01…vm-09 to one `vm` profile) → the hostname itself.
   `.dotfiles-profile` is machine-specific and gitignored.
5. **The `profile` verb:** `list` (table, active marked), `add <name> [--desc]
   [--match]`, `remove <name> [--purge]` (strips the registry entry and per-entry
   tags; **keeps** `packages/<name>/` unless `--purge` — destructive deletion is
   opt-in; deployed files are always left intact), `copy <src> <dst>
   [--only <entry> | --dotfiles | --pkg [source]]` (no flags = copy everything;
   universal entries stay universal), and `use <name>` (writes `.dotfiles-profile`).
6. **`deploy` and `status` honor the active profile** — `deploy` skips entries not
   active in the profile; `status` notes the active profile in its title.

Backward compatibility is the hard constraint: a manifest with no `[profiles]` and
no per-entry `profiles` behaves exactly as before (universal entries; active
profile = hostname). The convergence harness (whose manifests declare no profiles)
stays green.

## Consequences

### Positive

- A machine/role can take a curated subset of dotfiles **and** its own package
  set — the asymmetry is gone; profiles unify the two.
- Fleet-friendly: one `match` glob maps an arbitrarily large set of similarly
  named hosts to a single profile, so a fleet of VMs needs no per-host setup.
- Profiles are *declared* and self-documenting (`description`), and *portable*
  (`copy` with whole/partial granularity) — the operations the multi-machine
  workflow actually needs.
- Fully backward compatible; existing single-profile users see no change.

### Negative

- A second axis of "should this deploy here": `enabled` (is it managed) vs
  `profiles` (is it for this profile). Kept orthogonal and documented, but it is
  more to understand than a single bool.
- `packages/<host>/` is reframed as `packages/<profile>/` — a conceptual rename;
  no data moves (default profile = hostname), but the mental model shifts.

### Neutral

- `.dotfiles-profile` is per-machine state living in the store but gitignored —
  the one piece of intentionally-uncommitted state in an otherwise all-committed
  repo.
- Glob matching is a tiny in-house `*`/`?` matcher (no regex dependency).

## Alternatives Considered

- **Separate manifest per profile** (`.dotfiles-manifest.<profile>.toml` + a base)
  — rejected: duplicates entries across files and fragments the single
  self-documenting catalog (ADR-002/003).
- **Per-profile `enabled` map** (`enabled = { desktop = true, server = false }`) —
  rejected: more verbose than a membership tag and conflates "managed" with "in
  this profile."
- **Keep packages per-host, dotfiles global** (status quo) — rejected: the
  asymmetry was precisely the gap; a half-measure leaves dotfiles unable to vary.
- **Adopt DotState's profile + storage model** — rejected per ADR-001 (clean-room;
  we keep our own manifest). We take the concept, not the engine.
