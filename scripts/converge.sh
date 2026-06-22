#!/usr/bin/env bash
#
# converge.sh — command-by-command convergence harness (bash parity).
#
# Runs the same logical verb sequence on the Rust `dotfiles` and the bash tool,
# each in its own disposable sandbox (store git repo + $HOME + bare remote), and
# asserts the *effects* match — manifest meaning, symlink graph, push result, and
# captured package lists — not the stdout text (which differs by design).
#
# The two tools read different manifest formats (bash: pipe `.dotfiles-manifest`;
# Rust: TOML `.dotfiles-manifest.toml`), so each sandbox is seeded with the format
# its tool reads, describing the SAME entries; snapshots normalize across formats.
#
# Usage: scripts/converge.sh            (builds the release binary first)
#        DOTFILES_BIN=/path bash scripts/converge.sh   (use a prebuilt binary)
#
# Exit non-zero if any check FAILs.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASH_TOOL_SRC="${BASH_TOOL_SRC:-$HOME/.dotfiles}"   # where the bash `dotfiles` + lib/ live
HOST="$(hostname 2>/dev/null | cut -d. -f1)"; HOST="${HOST:-$(uname -n | cut -d. -f1)}"

for dep in jq git; do
    command -v "$dep" >/dev/null || { echo "converge: missing dependency: $dep" >&2; exit 2; }
done

# Locate the Rust binary (build if not provided).
BIN="${DOTFILES_BIN:-}"
if [[ -z "$BIN" ]]; then
    echo "Building release binary..."
    ( cd "$REPO_ROOT" && cargo build --release --quiet ) || { echo "converge: build failed" >&2; exit 2; }
    BIN="$REPO_ROOT/target/release/dotfiles"
fi
[[ -x "$BIN" ]] || { echo "converge: no Rust binary at $BIN" >&2; exit 2; }

BASH_TOOL="$BASH_TOOL_SRC/dotfiles"
[[ -x "$BASH_TOOL" ]] || { echo "converge: no bash tool at $BASH_TOOL" >&2; exit 2; }

ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
SB="$ROOT/bash"          # bash sandbox = store + the copied bash tool
SR="$ROOT/rust"          # rust sandbox = store
HB="$ROOT/home-bash"
HR="$ROOT/home-rust"
mkdir -p "$HB" "$HR"
REAL_HOME="$HOME"   # pkg queries (flatpak per-user install) must see the real $HOME

FAILS=0
check() { # label expected actual
    if [[ "$2" == "$3" ]]; then
        printf '  \033[32mPASS\033[0m  %s\n' "$1"
    else
        printf '  \033[31mFAIL\033[0m  %s\n        bash: %s\n        rust: %s\n' "$1" "${2//$'\n'/ ; }" "${3//$'\n'/ ; }"
        FAILS=$((FAILS + 1))
    fi
}

# ---- seed both sandboxes with the same three entries ------------------------
# entry: name | repo-source | target(relative to home)
SEED=(
    "alpha|alpha/conf|.alpharc"
    "beta|beta/conf|.config/beta/beta.conf"
    "gamma|gamma|.gammadir"
)

seed_sandbox() { # dir
    local d="$1"
    mkdir -p "$d"
    git -C "$d" init -q -b main
    git -C "$d" config user.email converge@test
    git -C "$d" config user.name converge
    for row in "${SEED[@]}"; do
        IFS='|' read -r _ src _ <<<"$row"
        mkdir -p "$d/$(dirname "$src")"
        echo "content of $src" > "$d/$src"
    done
}

seed_sandbox "$SB"
seed_sandbox "$SR"
# Copy the bash tool into its store so DOTFILES_DIR resolves to the sandbox.
cp "$BASH_TOOL" "$SB/dotfiles"
cp -r "$BASH_TOOL_SRC/lib" "$SB/lib"

# pipe manifest for bash
{
    echo "# Dotfiles Manifest"
    for row in "${SEED[@]}"; do IFS='|' read -r n s t <<<"$row"; echo "$n|$s|$t|true|symlink"; done
} > "$SB/.dotfiles-manifest"
# TOML manifest for rust
{
    for row in "${SEED[@]}"; do
        IFS='|' read -r n s t <<<"$row"
        printf '[[entry]]\nname = "%s"\npath = "%s"\ntarget = "%s"\n\n' "$n" "$s" "$t"
    done
} > "$SR/.dotfiles-manifest.toml"

# bare remotes + initial push
git init -q -b main --bare "$ROOT/bash.git"
git init -q -b main --bare "$ROOT/rust.git"
git -C "$SB" add -A && git -C "$SB" commit -qm seed && git -C "$SB" remote add origin "$ROOT/bash.git" && git -C "$SB" push -qu origin main
git -C "$SR" add -A && git -C "$SR" commit -qm seed && git -C "$SR" remote add origin "$ROOT/rust.git" && git -C "$SR" push -qu origin main

# ---- tool drivers -----------------------------------------------------------
rust() { "$BIN" --manifest "$SR/.dotfiles-manifest.toml" --repo-root "$SR" --home "$HR" "$@"; }
bash_t() { HOME="$HB" "$SB/dotfiles" "$@" </dev/null; }
# pkg verbs touch only the store's packages/ dir + hostname (not HOME), so run
# them with the real HOME — otherwise flatpak sees a different per-user install
# than the Rust tool (which inherits the real env), a harness artifact not a bug.
bash_pkg() { HOME="$REAL_HOME" "$SB/dotfiles" "$@" </dev/null; }

# ---- snapshots (normalized across formats) ----------------------------------
# entries: name|source|target|enabled, sorted.
#
# NOTE: bash `enable`/`disable` redirect their rewrite loop's stdout to the temp
# manifest, so `log_success` writes `[SUCCESS] …` lines INTO the pipe manifest
# (a latent bash defect the Rust tool does not reproduce). Worse, a later mutation
# re-emits that junk line WITH trailing pipes, so a field-count filter won't catch
# it — we keep only rows whose name field is a clean identifier.
NAME_RE='^[A-Za-z0-9._-]+$'
entries_rust() { rust status --format json | jq -r '.entries[]|"\(.name)|\(.path)|\(.target)|\(.enabled)"' | sort; }
entries_bash() { grep -v '^#' "$SB/.dotfiles-manifest" | awk -F'|' -v re="$NAME_RE" '$1 ~ re && NF>=5{print $1"|"$2"|"$3"|"$4}' | sort; }

# deploy graph: name|link:<source-relative-to-store> | file | missing, sorted.
deploy_snap() { # store home triples-cmd
    local store="$1" home="$2"
    "$3" | while IFS='|' read -r name _src target; do
        local t="$home/$target"
        if [[ -L "$t" ]]; then
            local r; r="$(readlink "$t")"
            echo "$name|link:${r#"$store"/}"
        elif [[ -e "$t" ]]; then echo "$name|file"
        else echo "$name|missing"; fi
    done | sort
}
triples_rust() { rust status --format json | jq -r '.entries[]|"\(.name)|\(.path)|\(.target)"'; }
triples_bash() { grep -v '^#' "$SB/.dotfiles-manifest" | awk -F'|' -v re="$NAME_RE" '$1 ~ re && NF>=5{print $1"|"$2"|"$3}'; }
deploy_rust() { deploy_snap "$SR" "$HR" triples_rust; }
deploy_bash() { deploy_snap "$SB" "$HB" triples_bash; }

echo "=== convergence: bash (dotfiles-bash) ⇄ rust (dotfiles) ==="
echo

echo "[seed] manifests describe the same entries"
check "entries match at seed" "$(entries_bash)" "$(entries_rust)"

echo "[deploy]"
bash_t deploy >/dev/null 2>&1
rust deploy >/dev/null 2>&1
check "symlink graph matches after deploy" "$(deploy_bash)" "$(deploy_rust)"

echo "[disable beta]"
bash_t disable beta >/dev/null 2>&1
rust disable beta >/dev/null 2>&1
check "entries match after disable" "$(entries_bash)" "$(entries_rust)"
check "symlink graph matches after disable" "$(deploy_bash)" "$(deploy_rust)"

echo "[enable beta + redeploy]"
bash_t enable beta >/dev/null 2>&1; bash_t deploy >/dev/null 2>&1
rust enable beta >/dev/null 2>&1; rust deploy >/dev/null 2>&1
check "entries match after enable" "$(entries_bash)" "$(entries_rust)"
check "symlink graph matches after re-deploy" "$(deploy_bash)" "$(deploy_rust)"

echo "[add delta + deploy]"
mkdir -p "$SB/delta" "$SR/delta"; echo x > "$SB/delta/conf"; echo x > "$SR/delta/conf"
bash_t add delta .deltarc delta/conf >/dev/null 2>&1; bash_t deploy >/dev/null 2>&1
rust add delta .deltarc delta/conf >/dev/null 2>&1; rust deploy >/dev/null 2>&1
check "entries match after add" "$(entries_bash)" "$(entries_rust)"
check "symlink graph matches after add+deploy" "$(deploy_bash)" "$(deploy_rust)"

echo "[push]"
bash_t push -m "converge" >/dev/null 2>&1; b_push=$?
rust push -m "converge" >/dev/null 2>&1; r_push=$?
check "push exit status matches (0)" "0 $b_push" "0 $r_push"
b_remote="$(git -C "$ROOT/bash.git" rev-parse refs/heads/main)"
b_local="$(git -C "$SB" rev-parse HEAD)"
r_remote="$(git -C "$ROOT/rust.git" rev-parse refs/heads/main)"
r_local="$(git -C "$SR" rev-parse HEAD)"
check "remote advanced to local HEAD" "$([[ "$b_remote" == "$b_local" ]] && echo yes)" "$([[ "$r_remote" == "$r_local" ]] && echo yes)"

echo "[pkg capture]"
if rust pkg --help >/dev/null 2>&1; then
    bash_pkg pkg capture >/dev/null 2>&1
    rust pkg capture >/dev/null 2>&1
    # Compare as SETS: bash `sort` uses locale collation, the Rust tool sorts by
    # byte value, so re-sort both with LC_ALL=C before diffing.
    for src in native aur flatpak; do
        bf="$SB/packages/$HOST/$src.txt"; rf="$SR/packages/$HOST/$src.txt"
        [[ -f "$bf" || -f "$rf" ]] || continue
        check "pkg capture $src set identical" \
            "$(LC_ALL=C sort "$bf" 2>/dev/null)" "$(LC_ALL=C sort "$rf" 2>/dev/null)"
    done
else
    echo "  SKIP  pkg not yet implemented in the Rust binary"
fi

echo
if [[ "$FAILS" -eq 0 ]]; then
    echo "All convergence checks passed."
else
    echo "$FAILS convergence check(s) FAILED."
fi
exit $(( FAILS > 0 ? 1 : 0 ))
