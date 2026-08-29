#!/bin/sh
# aoraki-cli installer
#
#   curl -fsSL https://sf-digital-labs.github.io/aoraki-cli/install.sh | sh
#
# Detects OS/arch, downloads the latest release binary from GitHub, and
# installs it to /usr/local/bin (sudo only if that directory needs it).
set -eu

REPO="SF-Digital-Labs/aoraki-cli"
INSTALL_DIR="${AORAKI_INSTALL_DIR:-/usr/local/bin}"

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin)
    case "$arch" in
      arm64)  target="aarch64-apple-darwin" ;;
      x86_64) target="x86_64-apple-darwin" ;;
      *) echo "unsupported macOS arch: $arch" >&2; exit 1 ;;
    esac ;;
  Linux)
    case "$arch" in
      x86_64) target="x86_64-unknown-linux-gnu" ;;
      *) echo "unsupported Linux arch: $arch (build from source: https://github.com/$REPO)" >&2; exit 1 ;;
    esac ;;
  *) echo "unsupported OS: $os" >&2; exit 1 ;;
esac

echo "→ aoraki-cli installer (${target})"

url="https://github.com/$REPO/releases/latest/download/aoraki-$target.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "→ downloading $url"
curl -fsSL "$url" -o "$tmp/aoraki.tar.gz" || {
  echo "download failed — no release yet, or no access to the repo." >&2
  echo "Build from source instead: https://github.com/$REPO#install" >&2
  exit 1
}
tar xzf "$tmp/aoraki.tar.gz" -C "$tmp"

if [ -w "$INSTALL_DIR" ]; then
  install -m 755 "$tmp/aoraki" "$INSTALL_DIR/aoraki"
else
  echo "→ $INSTALL_DIR needs sudo:"
  sudo install -m 755 "$tmp/aoraki" "$INSTALL_DIR/aoraki"
fi

echo "✓ installed: $("$INSTALL_DIR/aoraki" --version 2>/dev/null || echo aoraki)"
echo
echo "Next steps:"
echo "  aoraki login --url https://aoraki.cloud/api/v1"
echo "  aoraki --help"
