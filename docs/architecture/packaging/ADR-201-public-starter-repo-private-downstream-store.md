---
status: Draft
date: 2026-08-21
deciders:
  - aaronsb
  - claude
related:
  - ADR-001
  - ADR-002
  - ADR-008
  - ADR-011
  - ADR-200
---

# ADR-201: Public starter repo, private downstream store

## Context

One public repo, `aaronsb/dotfiles`, has been carrying three things at once:

| Layer | Files | Belongs to |
|---|---|---|
| **Engine** | `bootstrap.sh`, `install.sh`, `.dotfiles-cli.version`, `readme.md`, `CLAUDE.md` | the project |
| **Catalog** | `.dotfiles-manifest.toml` | the operator |
| **Content** | `zsh/ nvim/ tmux/ mlterm/ claude/ oh-my-posh/ tealdeer/ packages/<host>/ github-backup/` | the operator |

That fusion was harmless while the repo was one person's config store with a
tool bolted on. It stopped being harmless when the project acquired a landing
page. `dotarchy.sh` publishes a one-liner that clones the repo to `~/.dotfiles`
and runs `bootstrap.sh`, whose third step offers `deploy --force`. A stranger
following that instruction gets one specific person's `.zshrc`, an mlterm
framebuffer setup for one specific console, a Claude Code permission allowlist,
and package manifests named `north`, `slab`, `cube`, and `padnoir`.

The catalog is the seam, and it does not float free. Entry paths resolve as
`repo_root.join(entry.path)` (`deploy.rs:55`), so the manifest must sit in the
same repo as the content it names. A split that keeps the manifest public and
the content private would publish a catalog describing files the repo does not
have. So the cut has to fall between the engine and the {catalog, content}
pair.

What made this cheap to act on: `Ctx::resolve` (`main.rs:157-167`) already
resolves the store as `--repo-root` → `$DOTFILES_DIR` → `~/.dotfiles`, and
`push`/`pull`/`diff` are `git -C <store>` against whatever `origin` happens to
be. Pointing the CLI at a different repo is already supported; only the
distribution story was missing.

## Decision

**The public repo is the store's ancestor, not its dependency.** An operator's
store is a private downstream of a public starter, related by a git remote and
nothing else.

### 1. Three repos, one org

- **`dotarchy/dotfiles`** — the public starter. Engine plus a generic default
  store: a manifest with a handful of universally applicable entries (tmux, zsh,
  zshenv), each keeping its real `why` text (ADR-002), and the content those
  entries name. Cloning it and deploying yields a working, unremarkable, nobody's
  setup.
- **`dotarchy/dotfiles-cli`** — the engine's upstream, transferred out of
  `aaronsb/`. `install.sh` fetches releases from here, and the repo is
  overridable via `DOTFILES_CLI_REPO` so a fork can point elsewhere.
- **`<operator>/<store>`** — private, one per operator. For aaronsb this is
  `aaronsb/dotfiles` flipped to private in place, keeping its full history and
  its remote URL.

Nothing in the public repos names a person. Documentation has exactly two slots:
`dotarchy/...` (fixed) and `<your-user>/...` (a placeholder).

### 2. The relationship is `upstream`, and it carries the engine only

A store adds `dotarchy/dotfiles` as `upstream` and merges from it to pick up
engine changes. GitHub cannot fork a public repo privately, so the provisioning
path is create-private-repo, push, add-remote — three commands, which
`bootstrap.sh` learns to run.

Merges from upstream are expected to touch `bootstrap.sh`, `install.sh`, and
`.dotfiles-cli.version`. They are **not** expected to touch the store's manifest
or content. The starter's default entries are a seed, not a maintained baseline:
once an operator edits their `.zshrc`, upstream has no further opinion about it.

### 3. `bootstrap.sh` provisions a store

After installing the CLI, bootstrap offers to make the checkout the operator's
own — `gh repo create --private`, repoint `origin`, add `upstream`. Declining
leaves a working standalone store sitting on the public defaults, which is a
legitimate way to use the tool and the reason the starter ships real content
rather than empty directories.

### 4. The CLI does not change

No Rust changes are required, and none are made. The store root is already
parameterized, the manifest already travels with its content, and `push`/`pull`
already follow `origin`. This decision is entirely about repo topology and the
shell scripts that establish it.

## Consequences

### Positive

- A stranger running the published one-liner gets a sane default configuration
  and a repo they own, instead of somebody else's machine.
- The operator's store keeps its full history, its remote URL, and its working
  tree. `~/.dotfiles` needs no re-clone and no reconfiguration.
- Engine improvements reach every store through a mechanism every git user
  already knows, with no invention in the tool.
- The axis count stays at three — `enabled`, `profiles`, `paths` (ADR-008,
  ADR-011). Visibility is a property of the repo, not a fourth thing the
  manifest has to model.

### Negative

- `github.com/aaronsb/dotfiles` starts returning 404 on the visibility flip.
  Anything linking to it — including the current landing page footer — breaks
  until repointed.
- Merging `upstream` into a store that has edited an engine file produces an
  ordinary merge conflict, with no tooling to help. The engine files are small
  and rarely edited downstream, so this is judged acceptable rather than solved.
- Improvements to the starter's *content* reach nobody. A better default
  `.zshrc` benefits new adopters only.

### Neutral

- The visibility flip is not retroactive. Everything already pushed to the
  public repo stays reachable through its history, forks, and archives. A
  credential scan of the working tree, plus a filename sweep of every path ever
  added across the 122 commits, turned up nothing — so the flip is going-forward
  hygiene and no history rewrite is performed. A full content scan of history
  has not been run; if one is wanted, it belongs before the flip, not after.
- The site repo (`aaronsb/dotarchy`) stays private and stays under `aaronsb/`.
  It is a deploy target and gains nothing from the org.
- Transferring `dotfiles-cli` leaves a GitHub redirect, so machines holding an
  existing pin (ADR-200) keep resolving. The pin and the URL are updated
  deliberately rather than left riding it.

## Alternatives Considered

- **Layered base + overlay stores** — the public store stays live at
  `~/.dotfiles`, a private overlay sits alongside, and the CLI merges two
  manifests with private-overrides-public by entry `name`. Rejected: it needs
  manifest merge semantics, provenance in `status`/`list`/`show`, a `--store`
  selector on the git verbs, and `target` collision rules, and it adds a fourth
  axis to a mental model ADR-011 already flagged as widening. Its one real
  advantage — upstream content improvements arriving as clean overrides instead
  of conflicts — is worth little for files whose whole purpose is to be personal.

- **Private content nested inside the public repo** as a gitignored clone at
  `private/`, with manifest entries pointing into it. Rejected: the manifest
  stays public while describing private rows, which is the incoherent shape the
  Context rules out.

- **Git submodule** for the private content. Rejected: the submodule URL is
  public metadata that advertises the private repo's existence, and detached-HEAD
  submodule mechanics are a poor fit for a tree edited in place every day.

- **Public repo as a scrubbed export** of the private one, regenerated by a
  command. Rejected: it drifts the moment the command is not run, and it needs
  tooling that does not exist to solve a problem the fork model solves with a
  remote.

- **Fresh history for the private store** instead of flipping visibility in
  place. Rejected: it discards the commit archaeology `CLAUDE.md` relies on
  (`git log --grep`), and it hides nothing that is not already public.
