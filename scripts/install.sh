#!/bin/sh
set -eu

REPO="${SAC_REPO:-secemp9/sac}"
CHANNEL="${SAC_CHANNEL:-edge}"
BASE_URL="${SAC_BASE_URL:-https://github.com/${REPO}/releases/download}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Darwin)
      case "$arch" in
        arm64|aarch64)
          echo "aarch64-apple-darwin"
          ;;
        *)
          echo "unsupported macOS architecture: $arch (Apple Silicon only for now)" >&2
          exit 1
          ;;
      esac
      ;;
    Linux)
      case "$arch" in
        x86_64|amd64)
          echo "x86_64-unknown-linux-musl"
          ;;
        *)
          echo "unsupported Linux architecture: $arch" >&2
          exit 1
          ;;
      esac
      ;;
    *)
      echo "unsupported operating system: $os" >&2
      exit 1
      ;;
  esac
}

download() {
  url="$1"
  output="$2"

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$output"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$output" "$url"
  else
    echo "need curl or wget to install sac" >&2
    exit 1
  fi
}

if command -v sac >/dev/null 2>&1; then
  existing="$(command -v sac)"
  echo "sac is already installed at $existing"
  echo "run 'sac upgrade' to update, or set INSTALL_DIR to install elsewhere"
  exit 0
fi

target="$(detect_target)"
asset="sac-${target}.tar.gz"
url="${BASE_URL}/${CHANNEL}/${asset}"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT INT TERM

archive="$tmpdir/$asset"
download "$url" "$archive"

mkdir -p "$INSTALL_DIR"
tar -xzf "$archive" -C "$tmpdir"
install -m 755 "$tmpdir/sac" "$INSTALL_DIR/sac"

echo "installed sac to $INSTALL_DIR/sac"

case ":$PATH:" in
  *":$INSTALL_DIR:"*)
    ;;
  *)
    echo "add $INSTALL_DIR to your PATH to run sac directly"
    ;;
esac
