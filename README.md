# dotfiles

![License](https://img.shields.io/github/license/aaronsb/dotfiles-cli)
![Latest Release](https://img.shields.io/github/v/release/aaronsb/dotfiles-cli?include_prereleases&label=version)

A small CLI for a symlink-based dotfiles store whose manifest explains itself:
every managed file carries the reason it is managed.

It exists so a machine can be stood up with low effort and still be explained,
part by part, afterward. The tool is a *trainer* in the old DOS sense —
external, optional, removable, every toggle listed — never a distribution.
Your configuration is a git repo you could apply by hand; `dotfiles` reads the
same manifest and does it faster. It is the deployment half of
[dotarchy](https://dotarchy.sh), a meta-distro for Arch that ships reasoning
rather than an ISO; nothing here requires Arch.

## Getting started

One line installs the CLI, binds this machine to a store, and offers a deploy:

```bash
curl -fsSL https://raw.githubusercontent.com/aaronsb/dotfiles-cli/main/bootstrap.sh | bash
```

> **Don't trust this. Read it first.** The rule the AUR applies to every
> PKGBUILD applies here: a script you pipe into `bash` runs with your
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
> Comprehension is the security model. Everything the script does is also a
> plain `dotfiles` command you can run by hand, every step previews with
> `--what-if` / `--dry-run` before it changes anything, and every entry it
> deploys carries a `why` you can read before you accept it.

What happens:

1. **Install** — `install.sh` downloads the release binary for your platform
   into `~/.local/bin` (no Rust toolchain needed). `DOTFILES_VERSION=v0.8.2`
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

Leaving is as short as arriving: `dotfiles disable <app>` removes one link,
and deleting the binding and the store removes the rest. Nothing is installed
system-wide.

## The vocabulary

| Word | What it is | Where it lives |
|---|---|---|
| **entry** | one managed file or directory, with its `why` | a row in the manifest |
| **configuration** | a whole coherent setup — one person's machine, end to end, every entry explained; dotarchy calls this an *opinion* | `configs/<id>/` in the registry |
| **store** | your private copy of a configuration, plus what is yours alone: profiles, variants, package lists | a git repo, usually `~/.dotfiles` |
| **profile** | a named scope inside a store — a machine or a role | `[profiles.<name>]` in the manifest |
| **host binding** | which store this machine uses, what it descends from, which profile is active | `~/.config/dotfiles/config.toml` |

## The manifest

The manifest is a TOML catalog of managed dotfiles ([ADR-003](docs/architecture/foundation/)).
Each entry carries a durable **`why`** — the rationale for the entry's existence
([ADR-002](docs/architecture/foundation/)) — and may deepen into a structured
**`spec`** describing what the dotfile is and needs
([ADR-006](docs/architecture/foundation/)):

```toml
[[entry]]
name = "zsh"
path = "zsh/.zshrc"
target = ".zshrc"
why = "Interactive shell baseline — a fresh box behaves like the others without re-deriving settings."
```

Prose for the human, structure for the machine, both primary. The `why` is
what lets a stranger — or you, a year on — decide whether an entry still
deserves to exist; the `spec` and `--format json` are what let a script or an
agent act on the same catalog. A manifest whose `spec` is rich and whose
`why` is thin has drifted toward the machine, and that drift is the one this
project guards against.

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

Package lists ride along per profile (`dotfiles pkg capture` / `sync` / `diff`),
so a profile is a machine's whole declared state: what is linked and what is
installed.

## Hosts, stores, and the registry

Three repos with three owners (ADR-013):

- this **tool**, which ships releases and the bootstrap;
- a public **registry**, [`dotarchy/dotfiles`](https://github.com/dotarchy/dotfiles),
  of named configurations — `configs/com.example.dotfiles/`, contributed by
  pull request, reverse-DNS so the namespace is the author's;
- your private **store** — one configuration tree, your profiles and
  variants, and `packages/<profile>/`. A store descends from a registry entry
  and carries no engine files.

A registry entry is one person's configuration, published under their own
name, with the `why` for each piece. Where an entry builds on someone else's
work — a theme, a prompt, a plugin — the `why` names the upstream; attribution
runs toward the people being pointed at.

Each machine records which store it uses, what that store descends from, and
its active profile in the host binding. `dotfiles init` writes it:

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

Every `init` path is idempotent and takes `--what-if`. An upstream merge never
deletes what is the store's alone — package lists, profile tables, variants.
The registry URL comes from `$DOTFILES_REGISTRY`; the clone is cached under
`~/.cache/dotfiles/`.

One operator can run two people's configurations on two machines with two
bindings, and a host can change ancestry later with `--mode rebase`.

## Agents

The tool is agent-native: every verb has structured output, the manifest is
machine-readable, and an agent can read the `why` before it touches anything.
It is also fully usable with no agent in the loop — every operation is a plain
command, and nothing in the store or the registry routes through a model.
Both are first-class; neither is a translation of the other.

## Shape (per ADR-001, amended by ADR-007)

- **One core, one CLI surface.** `dotfiles-core` owns manifest parsing, deploy-status
  derivation, the host binding, and the git gate; `dotfiles-cli` is the
  scriptable command surface. (An earlier two-front-end design with a live
  Ratatui TUI was retired; see ADR-005/ADR-100, now `Superseded`.)
- **Clean-room**, not a fork. Validated against prior art
  ([DotState](https://lib.rs/crates/dotstate), MIT); we keep our own manifest model.
- **Git-native.** The tool operates only inside a git repo — your store *is*
  the database, and `dotfiles push` / `pull` / `diff` are git against it.

## Architecture decisions

See [`docs/architecture/`](docs/architecture/). Manage them with the bundled CLI:

```bash
docs/scripts/adr list --group
docs/scripts/adr view 13
```

## License

[MIT](LICENSE)
