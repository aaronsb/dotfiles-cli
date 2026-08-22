#!/usr/bin/env bash
#
# One-line installer for the `dotfiles` CLI — downloads the latest prebuilt
# binary from GitHub Releases (no Rust toolchain required):
#
#   curl -fsSL https://raw.githubusercontent.com/dotarchy/dotfiles-cli/main/install.sh | bash
#
# Overrides: DOTFILES_BIN_DIR (install location), DOTFILES_VERSION (a tag like
# v0.1.0 instead of the latest release).

set -euo pipefail

REPO="dotarchy/dotfiles-cli"
BIN_DIR="${DOTFILES_BIN_DIR:-$HOME/.local/bin}"
VERSION="${DOTFILES_VERSION:-latest}"

os="$(uname -s)"
arch="$(uname -m)"
case "$os/$arch" in
    Linux/x86_64 | Linux/amd64) asset="dotfiles-x86_64-linux" ;;
    *)
        echo "dotfiles: no prebuilt binary for $os/$arch." >&2
        echo "  Build from source instead:" >&2
        echo "    git clone https://github.com/$REPO && cd dotfiles-cli" >&2
        echo "    cargo build --release && cp target/release/dotfiles \"$BIN_DIR/\"" >&2
        exit 1
        ;;
esac

if [[ "$VERSION" == "latest" ]]; then
    url="https://github.com/$REPO/releases/latest/download/$asset"
else
    url="https://github.com/$REPO/releases/download/$VERSION/$asset"
fi

command -v curl >/dev/null || { echo "dotfiles: curl is required" >&2; exit 1; }

mkdir -p "$BIN_DIR"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

echo "Downloading $asset ($VERSION) ..."
curl -fSL --proto '=https' "$url" -o "$tmp"
chmod +x "$tmp"
mv "$tmp" "$BIN_DIR/dotfiles"
trap - EXIT

echo "Installed: $BIN_DIR/dotfiles ($("$BIN_DIR/dotfiles" --version 2>/dev/null || echo installed))"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "warning: $BIN_DIR is not on your PATH — add: export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac
echo
echo "Next: clone your dotfiles store and run \`dotfiles deploy\`."
