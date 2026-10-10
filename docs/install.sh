#!/bin/sh
# aoraki-cli installer
#
#   curl --proto '=https' --tlsv1.2 -fsSL https://cli.aoraki.cloud/install.sh | sh
#
# Detects OS/arch, downloads the latest release binary, checks it against
# the release's published SHA-256, and installs it to /usr/local/bin (sudo
# only if that directory needs it). Everything runs inside main(), called
# on the last line, so a download cut off mid-script runs nothing.
set -eu

REPO="SF-Digital-Labs/aoraki-cli"

fetch() {
  curl --proto '=https' --tlsv1.2 -fsSL "$1" -o "$2"
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

main() {
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

  # cli.aoraki.cloud/download redirects to the latest release asset — one
  # stable URL regardless of where binaries are hosted.
  url="https://cli.aoraki.cloud/download/$target"
  sum_url="https://github.com/$REPO/releases/latest/download/aoraki-$target.tar.gz.sha256"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT

  echo "→ downloading $url"
  fetch "$url" "$tmp/aoraki.tar.gz" || {
    echo "download failed — no release yet, or no access to the repo." >&2
    echo "Build from source instead: https://github.com/$REPO#install" >&2
    exit 1
  }
  fetch "$sum_url" "$tmp/aoraki.sha256" || {
    echo "could not fetch the release checksum ($sum_url) — not installing an unverified binary" >&2
    exit 1
  }
  want="$(cut -d' ' -f1 < "$tmp/aoraki.sha256")"
  got="$(sha256_of "$tmp/aoraki.tar.gz")"
  if [ "$want" != "$got" ]; then
    echo "checksum mismatch (expected $want, got $got) — not installing." >&2
    echo "If a release was just published, re-run in a minute." >&2
    exit 1
  fi
  echo "→ checksum ok"
  tar xzf "$tmp/aoraki.tar.gz" -C "$tmp"

  # Prove it runs here (an older glibc can't load it) before replacing
  # whatever is installed.
  if ! version="$("$tmp/aoraki" --version 2>&1)"; then
    echo "the downloaded binary doesn't run on this system:" >&2
    echo "  $version" >&2
    echo "Build from source instead: https://github.com/$REPO#install" >&2
    exit 1
  fi

  if [ ! -d "$INSTALL_DIR" ]; then
    mkdir -p "$INSTALL_DIR" 2>/dev/null || sudo mkdir -p "$INSTALL_DIR"
  fi
  if [ -w "$INSTALL_DIR" ]; then
    install -m 755 "$tmp/aoraki" "$INSTALL_DIR/aoraki"
  else
    echo "→ $INSTALL_DIR needs sudo:"
    sudo install -m 755 "$tmp/aoraki" "$INSTALL_DIR/aoraki"
  fi

  echo "✓ installed: $version → $INSTALL_DIR/aoraki"
  echo
  echo "Next steps:"
  echo "  aoraki login"
  echo "  aoraki --help"
}

main "$@"
