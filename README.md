# dotfiles

![License](https://img.shields.io/github/license/aaronsb/dotfiles-cli)
![Latest Release](https://img.shields.io/github/v/release/aaronsb/dotfiles-cli?include_prereleases&label=version)

A small, agent-native CLI for a symlink-based dotfiles store, built around a
**self-documenting manifest**.

## Getting started

One line installs the CLI, binds this machine to a store, and offers a deploy:

```bash
curl -fsSL https://raw.githubusercontent.com/aaronsb/dotfiles-cli/main/bootstrap.sh | bash
```

> **Don't trust this. Read it first.** The same rule the AUR applies to every
> package applies here: a script you pipe into `bash` runs with your
> permissions, in your home directory, and `bootstrap.sh` will offer to
> overwrite your shell and editor configuration. Fetch it, read it, then run
> it — it is short on purpose:
>
> ```bash
> curl -fsSLO https://raw.githubusercontent.com/aaronsb/dotfiles-cli/main/bootstrap.sh
> less bootstrap.sh          # and install.sh, which it calls
> bash bootstrap.sh
> ```
>
> Everything it does is also a plain `dotfiles` command you can run by hand,
> and every step previews with `--what-if` / `--dry-run` before it changes
> anything.

What happens:

1. **Install** — `install.sh` downloads the release binary for your platform
   into `~/.local/bin` (no Rust toolchain needed). `DOTFILES_VERSION=v0.8.1`
   pins a release; `DOTFILES_BIN_DIR` moves it. To build from source instead:
   `cargo build --release`.
2. **Bind** — `dotfiles init` asks where your store comes from: a published
   configuration from the [registry](https://github.com/dotarchy/dotfiles)
   (`sh.dotarchy.starter` is a working, unremarkable default) or a store you
   already keep in git. It creates the store, optionally a private GitHub
   remote for it, and writes the host binding at
   `~/.config/dotfiles/config.toml`.
3. **Deploy** — `dotfiles deploy --dry-run` shows what would be symlinked;
   `dotfiles deploy --force` backs up anything in the way to
   `~/.dotfiles-backup/` and links your configs.

Non-interactive, for a second machine:

```bash
curl -fsSL https://raw.githubusercontent.com/aaronsb/dotfiles-cli/main/bootstrap.sh \
  | bash -s -- --config com.example.dotfiles --remote github --profile laptop
```

`dotfiles init` is idempotent — re-running it on a set-up host prints the
binding and exits — and it is also how a host is reconfigured later
(`--mode migrate|clean|rebase`; see [Hosts, stores, and the registry](#hosts-stores-and-the-registry)).

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

## Profiles

A profile is a named scope — a machine or a role. It decides two things about
each entry: whether it deploys here (membership, ADR-008), and *which bytes* it
deploys (content variants, ADR-011).

```toml
[[entry]]
name   = "nvim"
path   = "nvim"            # base — what every profile gets by default
target = ".config/nvim"
paths.slab = "nvim-slab"   # …except slab, which has its own copy
```

Every profile operation addresses a **ref** — `<profile>` for the whole scope,
`<profile>/<entry>` for one item — so comparing, moving, and dropping share one
grammar:

```bash
dotfiles profile diff                  # coverage matrix across all profiles
dotfiles profile diff slab --details   # active vs slab, with the content diff
dotfiles profile pull slab/nvim        # take slab's nvim here (creates a variant)
dotfiles profile push north/nvim slab  # …or send this machine's version there
dotfiles profile remove slab/nvim      # drop the variant; slab falls back to base
```

Overwriting an existing variant needs `--force`, and refusing shows the diff that
justified it. Nothing ever writes onto the base path behind another profile's
back.

## Hosts, stores, and the registry

Three repos with three owners (ADR-013): this **tool**; a public **registry**
of named configurations (`configs/com.example.dotfiles/`, contributed by pull
request); and your private **store** — one configuration tree, your profiles
and variants, and `packages/<profile>/`. A store descends from a registry
entry and carries no engine files.

Each machine records which store it uses, what that store descends from, and
its active profile in a **host binding** outside the store,
`~/.config/dotfiles/config.toml`. `dotfiles init` writes it:

```bash
dotfiles init                                   # interactive: pick an entry, create the store
dotfiles init --config com.example.dotfiles \
              --remote github --profile north   # the same, non-interactively
dotfiles init --existing git@github.com:you/store.git   # adopt a store you already push to
dotfiles init --mode migrate                    # a pre-binding store: fold markers, strip engine files
dotfiles init --mode rebase --config com.other.dotfiles  # keep the store, change its ancestry
dotfiles init --mode clean --config sh.dotarchy.starter  # back it up, start over
dotfiles store                                  # the binding and the store's git state
dotfiles store registry                         # what the registry publishes
dotfiles store upstream pull                    # merge the entry's latest into the store
```

Every `init` path is idempotent and takes `--what-if`. The registry URL comes
from `$DOTFILES_REGISTRY`; the clone is cached under `~/.cache/dotfiles/`.

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
