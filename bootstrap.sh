#!/usr/bin/env bash
# Bootstrap a host (ADR-013 §5): install the CLI, run `dotfiles init`, offer a
# deploy. Safe to re-run — every step is idempotent.
#
#   curl -fsSL https://raw.githubusercontent.com/aaronsb/dotfiles-cli/main/bootstrap.sh | bash
#
# Non-interactive: pass init's flags through, e.g.
#   bash bootstrap.sh --config com.example.dotfiles --remote github --profile north
set -euo pipefail

REPO="${DOTFILES_CLI_REPO:-aaronsb/dotfiles-cli}"
BIN_DIR="${DOTFILES_BIN_DIR:-$HOME/.local/bin}"

echo "=== dotfiles bootstrap ==="
echo
echo "Step 1: install the dotfiles CLI"
if [ -n "${DOTFILES_VERSION:-}" ]; then
    echo "  pinned: $DOTFILES_VERSION"
fi
curl -fsSL "https://raw.githubusercontent.com/$REPO/main/install.sh" | bash
DOTFILES="$BIN_DIR/dotfiles"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "  note: $BIN_DIR is not on PATH; add it to your shell rc." ;;
esac
echo

echo "Step 2: bind this host to a store"
# Re-attach stdin to the terminal when piped from curl, so init can ask.
if [ -t 1 ] && [ ! -t 0 ] && [ -r /dev/tty ]; then
    "$DOTFILES" init "$@" < /dev/tty
else
    "$DOTFILES" init "$@"
fi
echo

echo "Step 3: deploy"
"$DOTFILES" status || true
echo
if [ -t 0 ] || [ -r /dev/tty ]; then
    read -r -p "Deploy now? [d]ry-run / [y]es (backs up existing files) / [N]o: " choice < /dev/tty || choice=n
    case "$choice" in
        d|D) "$DOTFILES" deploy --dry-run ;;
        y|Y) "$DOTFILES" deploy --force ;;
        *) echo "skipped — \`dotfiles deploy --dry-run\` when ready." ;;
    esac
else
    echo "non-interactive — \`dotfiles deploy --dry-run\` when ready."
fi
echo
echo "bootstrap complete."
