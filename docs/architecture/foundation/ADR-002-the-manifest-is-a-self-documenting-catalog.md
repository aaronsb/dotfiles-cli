---
status: Draft
date: 2026-06-21
deciders:
  - aaronsb
  - claude
related:
  - ADR-001
  - ADR-003
---

# ADR-002: The manifest is a self-documenting catalog

## Context

ADR-001 establishes the `.dotfiles-manifest` as the durable contract every tool
(and human) reads, and reframes the tool's *primary* altitude as a **documented
inventory** — "what do I have dotfiles for, what are its properties, and why
does it exist" — rather than byte-level inspection. This ADR decides *what the
manifest carries* to make that inventory real.

Today the manifest is a mechanical table. Each line is
`app|source|target|enabled|deploy_type`, with `#` comments at the top:

```
tmux|tmux/.tmux.conf|.tmux.conf|true|symlink
zsh|zsh/.zshrc|.zshrc|true|symlink
```

It answers *what is managed and how* but not the question with real value when
you need it: **why does this entry exist, and what is it for?** Every incumbent
(Stow, dotbot, chezmoi, yadm, DotState) is mechanical the same way — none
document the manifest itself.

The motivating case is the config that *worked for years and then broke one
day*: at that moment the explanation you need is the one you wrote-and-forgot,
and the most useful place for it to live is the surface you are already staring
at. The value compounds across three modes, none of which require an agent:

- **Debugging (no agent):** the *why* is colocated with the entry, exactly when
  you have forgotten it.
- **Authoring (agent-assisted):** an agent can trivially fill the rationale for
  new entries as it adds them.
- **Reading (agent-assisted):** an agent gets the intent for free, improving
  every downstream suggestion.

So the docstring is **agent-amplified, not agent-dependent** — and even with no
agent it is a *forcing function* for keeping notes, the same friction-that-
forces-thinking pattern this project already applies via ADRs and ways.

Crucially, the proto-version of this already exists and is misplaced. The Bash
tool's `cmd_push` (`lib/git.sh`) prompts `read -rp "What changed? "` and
**refuses an empty message** before committing — it already *forces* a
justification. But that string is (a) stranded in **git history** as
per-*change* archaeology rather than a per-*entry* property, and (b) framed as
*"what changed"* — the mechanical *what*, the drift-prone kind — instead of the
durable *why*. The instinct is right; the home and the framing are wrong.

## Decision

Make the manifest a **self-documenting catalog**. Each managed entry carries,
alongside its structural fields (path, mode, enabled), a **docstring**: prose
explaining what the entry is for and — primarily — *why* it exists.

Two rules keep the prose honest rather than a maintenance liability:

1. **Separate self-verifying structure from drift-prone prose.** Structural
   fields are verifiable against the filesystem — the symlink exists or it does
   not. Prose is not. Keep them distinct so the tool validates the former and
   treats the latter as advisory.

2. **Favor durable *why* (intent) over mechanical *what-it-does*.** Intent ages
   slowly ("custom keymap for cross-machine muscle memory"); mechanics drift
   fast ("remaps `<C-h>` to pane-left") and merely duplicate what the file
   already says. Let the file/diff be the source of *what it does now*; let the
   manifest be the source of *why it is here*. Do not copy mechanics into prose
   that must be maintained in lockstep — and note this is the corrective to the
   existing "what changed?" prompt, which captures exactly the wrong half.

The catalog stays plain-text, diffable, and hand-appliable (ADR-001 #1); the
docstring is additive, not a new dependency. The **concrete serialization** is
deferred — but it is a real fork, because the current single-line pipe format
has no room for prose. The leading candidate is migrating to **TOML
array-of-tables** (`[[entry]]` with `path`/`mode`/`enabled` and a multi-line
string for the docstring — a real, queryable value), versus keeping the pipe
format and attaching a comment-convention docstring *above* each entry. Either
way the Bash reference tool (ADR-001 #3) and `dotfiles-tui` must agree on it, so
the serialization decision is recorded separately.

## Consequences

### Positive

- The *why* survives to the moment it is needed — the broke-after-years case —
  colocated with the entry.
- A self-documenting manifest is a genuine differentiator; no incumbent does it.
- Composes across debugging / authoring / reading and across agent / no-agent;
  the agent multiplies value without being required.
- Acts as a forcing function for good notes-hygiene even for a solo human, and
  promotes the existing `cmd_push` justification instinct from throwaway commit
  archaeology to a first-class, durable property.
- Gives the "documented inventory" review surface (ADR-001) real content to show.

### Negative

- Prose can go stale; the why-over-what rule mitigates but does not eliminate
  drift, and nothing *fails* when a docstring rots — a freshness, not a
  correctness, hazard.
- Slightly more format to specify, parse, and validate than a bare table, and it
  forces a serialization decision (pipe-plus-comments vs. TOML migration) that
  both the Bash tool and `dotfiles-tui` must honor in lockstep.
- Authoring friction for a solo human writing rationale by hand (mitigated:
  optional per entry; an agent removes most of it).

### Neutral

- Refines, does not supersede, ADR-001's "manifest is the durable contract."
- Exact serialization/schema is deferred to a separate format-spec decision.
- The catalog becomes the primary browse surface in both front-ends (ADR-001
  #4); rendering details are an interface concern.

## Alternatives Considered

- **Keep the manifest mechanical (status quo / all incumbents).** Rejected:
  discards the highest-value information (the why), precisely what is lost over
  time and most wanted when debugging.
- **Rely on git commit messages for the why (what the tool does today).**
  Rejected: `cmd_push` already proves the limitation — it records *when/what
  changed* as per-change history, not *why the entry exists* as a queryable
  per-entry property. It is archaeology, not a catalog.
- **Put rationale in a separate doc (README/wiki).** Rejected: drifts worse (not
  colocated), gives no point-of-use surfacing, no forcing function, and an agent
  cannot pick it up "for free" while reading an entry.
- **Inline comments inside each dotfile.** Rejected: not every config format
  supports comments, they cannot be aggregated into a cross-cutting inventory,
  and they answer "why this line" not "why I manage this thing."
