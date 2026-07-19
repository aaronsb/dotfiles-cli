---
status: Accepted
date: 2026-07-18
deciders:
  - aaronsb
  - claude
related:
  - ADR-001
  - ADR-002
  - ADR-007
  - ADR-008
---

# ADR-010: Standalone Claude settings projector for ~/.claude/settings.json

## Context

`~/.claude/settings.json` is user-scope configuration for Claude Code. Today it is
written by two uncoordinated authors on this machine:

1. **agent-ways' reconciler** (`settings_merge.rs`, run by `ways reconcile`) owns
   `hooks`, `permissions.allow` (its `WAYS_PERMS` baseline), and `permissions.deny`
   (its `WAYS_DENY` security baseline). It self-audits — re-adding its keys on every
   reconcile.
2. **Claude Code itself** writes into the same file — not only user preferences via
   `/config` and the TUI (`model`, `autoCompactEnabled`, `tui`, the notification
   toggles, …), but also *in-situ* mid-session writes such as `advisorModel` and
   `effortLevel`. `/config` is therefore not the only foreign writer; the runtime
   mutates the file on its own.

The dotfiles repo already carries a settings fragment store
(`claude/agent-ways/settings/`, deployed to `~/.config/agent-ways/settings`), but it
only projects a handful of disjoint keys (`statusLine`, `attribution`) and
deliberately skips `hooks` and `permissions`. So the user's own settings — including
their permission allow-list — are **not** managed by dotfiles: they live host-local
in `~/.claude/settings.json` and do not travel across machines.

This surfaced concretely as a launch warning: inert `Write(~/.claude/**)` /
`Write(~/.ssh/**)` permission rules (Claude Code honors only `Edit(path)` for file
checks) could be deleted by hand, but the deletion did not persist — the reconciler
re-adds them from source, and nothing in dotfiles owned the slice to fix it durably.

Two facts (confirmed against Claude Code docs) constrain any fix:

- **There is no user-scope `settings.local.json`.** The `.local.json` variant exists
  only at *project* scope. At user scope there is exactly one file,
  `~/.claude/settings.json`, and `/config` writes to it. There is no separate
  "safe zone" a managed tool could own while leaving the operator's toggles untouched.
- **Claude Code's mutable runtime state lives in `~/.claude.json`** (startups,
  onboarding, caches, per-project state), *not* in `settings.json`. `settings.json`
  is declarative config — but not write-free, because `/config` lands there.

The consequence: any tool that owns `settings.json` must **preserve foreign writes**.
A tool that regenerates the file wholesale would clobber the operator's live `/config`
choices on every deploy.

**Requirement (Aaron).** A dotfiles checkout plus a Claude Code install should manage
`~/.claude/settings.json` on their own — *without agent-ways installed at all*. That
rules out delegating the merge to agent-ways' `ways settings project`: a dependency on
the `ways` binary breaks the standalone case.

This decision is being made in coordination with a concurrent agent-ways ADR (owned by
that repo) which retires `settings_merge.rs` as a `settings.json` writer, ships its
security + operational baseline as an app-owned fragment layer, and makes
`ways settings project` its sole 3-way writer. The two tools must coexist on one file.

## Decision

Add a **standalone Claude-settings projector** to dotfiles-tui: a three-way merge that
projects a layered fragment store into `~/.claude/settings.json`, owning only a
declared subset of keys and preserving everything else. It has **no dependency on the
`ways` binary**.

**1. Layered fragment store (the zshrc `conf.d` shape).** Config is ordered fragments,
one engine composes them in layers — mirroring how `~/.zshrc` sources
`~/.zsh/conf.d/NN-*` with a per-host overlay:

- **L2 — user fragments.** The dotfiles-owned store (host-agnostic, travels with the
  repo). Numbered fragments, last-wins on scalars, union on lists.
- **L3 — host/profile overlay.** A per-machine overlay, resolved through the existing
  **profile** mechanism (ADR-008), not a raw hostname — so settings layering scopes
  per machine/role consistently with how dotfiles entries and packages already do.
- **L1 — app baseline** (`hooks`, security `deny`, operational `allow`) is **not**
  dotfiles' concern. When agent-ways is present it ships and self-audits L1. When it is
  absent, there is no L1 and L2+L3 stand alone — the standalone case works by
  construction.
- There is **no L4 local layer**: no user-scope `settings.local.json` exists.
  Machine-specific values live in the L3 profile overlay.

**2. Three-way engine with a host-local last-applied base.** Compile L2+L3 into the
*desired* managed subset; diff against a **last-applied base** (what this tool wrote
last time) to compute adds/removes; apply to the live file, touching only keys/entries
this tool owns. The base is host-local state under `$XDG_STATE_HOME`, gitignored and
**not** managed by dotfiles (mirroring agent-ways' ADR-163 split — the projector's base
is a host artifact, not carried config).

**3. Exclusive per-key ownership.** A scalar key is *either* fragment-managed (dotfiles
is source of truth) *or* foreign (`/config` or agent-ways) — never both. The tool never
declares a fragment for a key the operator steers via `/config` (e.g. `model`), because
doing so would revert their choice each deploy. List keys (`permissions.allow` /
`permissions.deny`) are **additive-union**: the tool adds and removes only the entries
its own base recorded, leaving foreign entries in place.

**4. Coexistence contract with agent-ways.** Two independent 3-way writers on one file
are safe **iff** each preserves the keys and list-entries the other authored, and
neither declares a fragment for a key the other owns. The owned sets are pinned by
agent-ways' baseline constants (cross-referenced with agent-ways ADR-169), so
disjointness holds **by-constant**, not by guesswork:

- **agent-ways owns** — `hooks`; `permissions.allow` ⊇ `{Bash(ways:*), Bash(attend:*),
  Bash(attend-chat:*), Bash(way-embed:*), Edit(~/.claude/**)}`; `permissions.deny` ⊇
  `WAYS_DENY` (`Read`/`Edit` of `~/.ssh`, plus `Read` of `~/.aws`, `~/.gnupg`,
  `~/.config/gcloud`, `~/.kube/config`, `~/.netrc`, `./.env`, `./.env.local`). This
  baseline stays **minimal** — it does not expand to agent-ways-adjacent tooling.
- **dotfiles owns** — everything else user-scope: `statusLine`, `attribution`, `env`,
  and every `permissions.allow` entry outside the baseline (the dotfiles CLI,
  oh-my-posh/posh-theme, `Read(~/**)`, generic shell allows, and the "gray-zone" tools
  that are *not* agent-ways-shipped binaries — `way-match`, `kg`, `mmaid`, `adr`, the
  optional MCP servers).
- **Neither owns; both preserve** — Claude Code's own runtime keys, written by `/config`
  and in-situ: `model`, `tui`, `autoCompactEnabled`, `teammateMode`,
  `remoteControlAtStartup`, `skipWorkflowUsageWarning`, the notification toggles,
  `advisorModel`, `effortLevel`.

This invariant is documented in both repos' ADRs (dotfiles ADR-010 ↔ agent-ways
ADR-169).

**5. Trigger.** `dotfiles deploy` invokes the projector after linking, so a deploy
brings `~/.claude/settings.json` into agreement with the fragment store. Exact CLI verb
shape is an interface concern (ADR-101 domain) and is deferred.

This is **not** a revival of the derived-state projection retired in ADR-007. That was
a general always-fresh mirror of the whole dotfiles tree. This is a narrow, explicit
merge into one external file, owning a declared key subset — closer in spirit to a
managed `deploy` step than to live projection.

## Migration and handoff from agent-ways

`statusLine` and `attribution` are today asserted into `settings.json` by agent-ways'
`ways settings project` (reading the dotfiles fragment store). Ownership of these keys
transfers to this projector. Because a scalar/object owner drops a key it recorded but
no longer asserts (deprecated-base removal), a careless handoff could have one tool
delete what the other just wrote, order-dependently. The transfer is therefore governed
by a **relinquish protocol** (agent-ways ADR-169):

- agent-ways **relinquishes** these keys by removing its projector outright and doing
  **no final cleanup pass** — it simply stops asserting them, leaving the live values in
  place as foreign content. It must *not* run deprecated-base removal on them.
- this projector **adopts** each key via the `migrating` path: read the live foreign
  value, assert `ours` equal to it, seed the base, and converge idempotently — no gap,
  no clobber, independent of run order.

After one migration cycle agent-ways no longer touches these keys and this projector is
their sole writer, so steady-state disjoint ownership (the coexistence contract above) is
restored. Two dedicated test vectors pin the handoff: **adopt-foreign** (adopt a live
`statusLine` with no gap) and **relinquish** (clear base, value survives, no clobber).

## Consequences

### Positive

- User settings — including the permission allow-list — become dotfiles-managed and
  travel across hosts. Durable fixes (e.g. never emitting inert `Write` rules) are
  authored once in the store, not re-fixed per machine.
- Satisfies the standalone requirement: dotfiles + Claude Code manage `settings.json`
  with zero agent-ways dependency.
- `/config` coexistence is preserved: base preservation means the operator's live
  toggles survive every deploy.
- Settings layering reuses the profile model (ADR-008), so per-machine settings scope
  the same way dotfiles and packages already do.

### Negative

- **Duplicates safety-critical merge machinery.** The same three-way + base-tracking +
  hands-off-foreign-keys logic exists in agent-ways' `ways settings project`. Two
  implementations must stay correct. Mitigation: the managed surface is small and the
  merge is exhaustively specified here; cover it with tests, including a foreign-writer
  (`/config`) preservation case and a two-writer coexistence case.
- Two writers on one file demand a disciplined disjoint-ownership contract. Overlap is
  silent: a key both sides declare gets reverted on whichever deploys last. The
  contract must be enforced by convention and, ideally, a lint.

### Neutral

- The last-applied base is host-local `$XDG_STATE_HOME` state, gitignored, outside the
  manifest — a deliberate artifact/config split.
- Requires manifest/deploy wiring so `dotfiles deploy` runs projection; the fragment
  store path (`claude/agent-ways/settings`) may be renamed to drop the `agent-ways`
  prefix now that dotfiles owns it independently.

## Alternatives Considered

- **Reuse `ways settings project` as the engine (dotfiles = fragments + trigger only).**
  Rejected: adds a hard dependency on the `ways` binary, violating the standalone
  requirement. It remains the right choice *inside* agent-ways.
- **Naive compiler (ordered deep-merge → overwrite `settings.json`).** Rejected: with
  no user-scope safe zone and `/config` writing directly into the file, a wholesale
  overwrite clobbers the operator's live preferences on every deploy.
- **Isolate managed keys into a user-scope `settings.local.json`, leaving
  `settings.json` to the user.** Rejected: Claude Code has no user-scope
  `settings.local.json` (project scope only, per docs) — such a file would never be
  read.
- **Put managed fragments in project-scope `.claude/settings.json`.** Rejected: this is
  user-global config that applies across all projects; project scope is the wrong
  altitude and would not travel as user config.
