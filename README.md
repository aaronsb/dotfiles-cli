# dotfiles-tui

![License](https://img.shields.io/github/license/aaronsb/dotfiles-tui)
![Latest Release](https://img.shields.io/github/v/release/aaronsb/dotfiles-tui?include_prereleases&label=version)

An agent-native lifecycle and live-watch tool for a symlink-based dotfiles store.

> **Status: pre-implementation.** This repo currently holds only the founding
> architecture decision ([ADR-001](docs/architecture/foundation/), `Draft`).
> No code has been written yet.

## What this is

The companion *application* to a dotfiles **configuration store** (e.g.
[`aaronsb/dotfiles`](https://github.com/aaronsb/dotfiles)). The two are kept
deliberately separate:

- **The config store** holds the actual dotfiles plus a plain-text
  `.dotfiles-manifest`. It is the durable source of truth and stays legible
  enough to apply *by hand* with no tooling at all.
- **dotfiles-tui** is an *optional accelerator* that reads that same manifest.
  Cloning the config store never requires this tool.

## Shape (per ADR-001)

- **One core** (manifest + symlink semantics + a live file-watch loop) behind
  **two front-ends**:
  - a **non-interactive JSON CLI** — the surface an agent drives and parses;
  - a **Ratatui TUI** — the human surface, including a live view of files
    changing as an external actor (e.g. Claude Code) edits them.
- **Clean-room**, not a fork. The lifecycle-TUI shape was validated against
  prior art ([DotState](https://lib.rs/crates/dotstate), MIT); we keep our own
  manifest model rather than adopt or fork another engine.
- **Distribution**: production installs pull a prebuilt binary from GitHub
  Releases; this repo is linked into the config store as a submodule for
  development only (not recursed in production).

## Architecture decisions

See [`docs/architecture/`](docs/architecture/). Manage them with the bundled
CLI:

```bash
docs/scripts/adr list --group
docs/scripts/adr view 1
```

## License

[MIT](LICENSE)
