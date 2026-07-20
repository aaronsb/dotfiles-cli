---
status: Draft
date: 2026-07-20
deciders:
  - aaronsb
  - claude
related:
  - ADR-010
---

# ADR-200: The store pins the CLI and pull enforces the pin

## Context

The dotfiles store carries a `.dotfiles-cli.version` file, documented in the
store's CLAUDE.md as the "pinned CLI release for reproducible installs."
Nothing reads it. The store's `install.sh` delegates to the upstream
`dotfiles-cli` installer, which defaults to `VERSION=latest`. The pin has
been decorative since it was introduced.

`dotfiles pull` fast-forwards the store and stops there. It has never had any
relationship to the binary running it. The two halves of the system — the
config store and the CLI that projects it — version independently, and only
the store half is reachable by a routine the operator actually runs.

This drifted silently until ADR-010 shipped a `claude/settings.d/` store that
requires a `dotfiles claude` subcommand to project. A second machine pulled
the store, deployed cleanly, reported every entry `already deployed`, and
still could not project the settings: it was running v0.3.0, several releases
behind, with no surface anywhere reporting that. Every individual command
succeeded. The failure was only visible by comparing `dotfiles --version`
against a release list by hand.

The general shape: a store can carry data whose meaning depends on a CLI
capability, so a store-only update is not a complete update. The operator has
no way to know which releases matter without reading changelogs, and the
system gives no signal when the binary is the stale half.

A related trap made this harder to see. A locally-built binary reports the
workspace version from `Cargo.toml`, which is only bumped at release time. A
developer machine and a release machine can both report `0.4.0` and contain
different code. Version strings are therefore trustworthy for *release*
binaries only, which is what the pin compares against.

## Decision

`.dotfiles-cli.version` becomes the authoritative statement of which CLI the
store expects, and both entry points honor it.

**`install.sh` installs the pinned version.** It reads
`.dotfiles-cli.version` and passes it to the upstream installer as
`DOTFILES_VERSION`. A `--latest` flag overrides the pin for the case where
the operator deliberately wants to move ahead of the store, which is also how
a new pin gets chosen in the first place. A missing pin file falls back to
`latest`, so an older store keeps working.

**`pull` reconciles the binary against the pin.** After the fast-forward, it
compares the running binary's version to the pin and, on mismatch, downloads
the pinned release and replaces the running executable in place. This is the
one routine an operator runs habitually, which makes it the only reliable
place to catch drift.

The reconcile runs on **both** pull paths — after a merge and on the
already-up-to-date early return. The motivating failure was a machine whose
git state was current and whose binary was three releases stale; a check that
only fires after a successful merge would never have healed it.

The swap writes to a temporary file in the same directory as the target and
`rename(2)`s over it. On Linux this atomically replaces the path while the
running process keeps its original inode, so a pull cannot leave a
half-written executable. The process continues running the old code and says
so; it does not re-exec itself.

**Self-update failure never fails the pull.** No network, no `curl`, a
read-only bin directory, or an unreachable release all degrade to a warning
that names the drift and the manual remedy. The git half of the pull has
already succeeded by that point and its result must stand on its own.

## Consequences

### Positive

- A machine that runs `dotfiles pull` converges on the CLI its store expects,
  without the operator tracking releases.
- Store data and the capability required to interpret it move together, so an
  ADR-010-style split (fragments present, projector absent) self-heals.
- The pin becomes real, making installs reproducible across machines as the
  store's documentation already claimed.
- Drift becomes visible at the moment it matters rather than at the moment
  some downstream command mysteriously fails.

### Negative

- `pull` acquires a second, heavier responsibility: it touches the network
  beyond git and rewrites an executable. The blast radius of a routine
  command grows, and its failure modes now include the release host.
- A pull can change the behavior of the next command in ways the operator did
  not ask for in that moment. The pin commit is the consent, which is a
  weaker and more distant signal than a direct instruction.
- Rolling the pin backward triggers a downgrade on every machine that pulls,
  which is correct but will surprise anyone who expects pins to ratchet.

### Neutral

- Releases become load-bearing. A merged feature that is never tagged is
  invisible to the pin, which is what stranded ADR-010's projector.
- Version comparison normalizes the `v` prefix (`v0.5.0` vs `0.5.0`) and
  treats any mismatch as drift rather than comparing semver ordering, so the
  pin can move in either direction.
- Locally-built binaries will read as drifted whenever their reported version
  differs from the pin. Developers working from source are expected to hit
  this and to use `--latest` or an unpinned store rather than fighting it.

## Alternatives Considered

- **Warn on drift, let the operator run `install.sh`.** Safer — the binary
  swap stays a deliberate human act. Rejected because it puts the remedy one
  manual step away from the notice, and the motivating case was an operator
  who had no idea the binary was even a moving part. A warning that must be
  acted on is a warning that gets scrolled past.

- **A separate `dotfiles update` subcommand.** Keeps `pull` honest about
  touching only git. Rejected because it only helps operators who already
  know to run it, which is precisely the knowledge that was missing.

- **Delete the pin; always install `latest`.** Simplest, and removes the
  decorative-file problem by deleting the file. Rejected because it gives up
  the ability to hold a machine back, and because "latest" is a moving target
  that makes two machines built a day apart quietly different.

- **Re-exec after a successful self-update** so the pulling process runs the
  new code. Rejected as too clever for the gain: it complicates argument and
  exit-code handling to save the operator one command, and a pull that
  silently restarts itself is harder to reason about than one that reports
  what changed.
