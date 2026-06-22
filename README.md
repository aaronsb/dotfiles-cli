# dotfiles

![License](https://img.shields.io/github/license/aaronsb/dotfiles-tui)
![Latest Release](https://img.shields.io/github/v/release/aaronsb/dotfiles-tui?include_prereleases&label=version)

A small, agent-native CLI for a symlink-based dotfiles store, built around a
**self-documenting manifest**.

> **Status: in progress.** The `dotfiles-core` state model and a `dotfiles status`
> command exist; the remaining lifecycle verbs (`deploy`/`enable`/`disable`/`add`/
> `push`) are being ported from the reference bash tool. See
> [ADR-007](docs/architecture/foundation/) for the current scope.

## What this is

The companion *application* to a dotfiles **configuration store** (e.g.
[`aaronsb/dotfiles`](https://github.com/aaronsb/dotfiles)). The two are kept
deliberately separate:

- **The config store** holds the actual dotfiles plus the manifest. It is the
  durable source of truth and stays legible enough to apply *by hand* with no
  tooling at all.
- **This tool** is an *optional accelerator* that reads that same manifest.
  Cloning the config store never requires it.

## The idea: a self-documenting manifest

The manifest is a TOML catalog of managed dotfiles ([ADR-003](docs/architecture/foundation/)).
Each entry carries a durable **`why`** — the rationale for the entry's existence
([ADR-002](docs/architecture/foundation/)) — and may optionally deepen into a
structured **`spec`** describing what the dotfile is and needs
([ADR-006](docs/architecture/foundation/)):

```toml
[[entry]]
name = "zsh"
path = "zsh/.zshrc"
target = ".zshrc"
why = "Interactive shell baseline — a fresh box behaves like the others without re-deriving settings."
```

This is the project's payoff: documentation that travels *with* the config and is
machine-readable, with or without the tooling.

## Shape (per ADR-001, amended by ADR-007)

- **One core, one CLI surface.** `dotfiles-core` owns manifest parsing, deploy-status
  derivation, and the git gate; `dotfiles-cli` is the scriptable command surface.
  It grows into a drop-in replacement for the reference bash tool — same verbs,
  reading the rich TOML schema. (An earlier two-front-end design with a live
  Ratatui TUI was retired; see ADR-005/ADR-100, now `Superseded`.)
- **Clean-room**, not a fork. Validated against prior art
  ([DotState](https://lib.rs/crates/dotstate), MIT); we keep our own manifest model.
- **Git-native.** The tool operates only inside a git repo — your dotfiles store
  *is* the database.

## Architecture decisions

See [`docs/architecture/`](docs/architecture/). Manage them with the bundled CLI:

```bash
docs/scripts/adr list --group
docs/scripts/adr view 7
```

## License

[MIT](LICENSE)
