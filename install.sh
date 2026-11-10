#!/bin/sh
# Installs Switchyard on Linux or macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/NathanielInman/switchyard/main/install.sh | sh
#
# Downloads the latest release for this machine into ~/.local/bin and runs
# `yard setup`, which starts Switchyard at login. From a checkout of the repo
# it builds from source instead.
#
# Options (after `sh -s --` when piped):
#   --source         build from source (needs Rust; Linux also needs gtk4 and gtk4-layer-shell)
#   --version v0.2.0 install a specific release
#   --dir PATH       install somewhere other than ~/.local/bin
#   --no-setup       install the binaries only
set -eu

REPO="NathanielInman/switchyard"
dir="${YARD_INSTALL_DIR:-$HOME/.local/bin}"
version="latest"
source=""
setup=1

while [ $# -gt 0 ]; do
  case "$1" in
    --source) source=1 ;;
    --version) version="$2"; shift ;;
    --dir) dir="$2"; shift ;;
    --no-setup) setup=0 ;;
    -h|--help) sed -n '2,16p' "$0" 2>/dev/null || true; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# Running from a checkout means building it.
here=""
if [ -f "$0" ]; then here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd); fi
if [ -z "$source" ] && [ -n "$here" ] && [ -f "$here/Cargo.toml" ] && [ -d "$here/crates/yard" ]; then
  source=1
fi

os=$(uname -s)
arch=$(uname -m)
case "$arch" in
  x86_64|amd64) arch=x86_64 ;;
  arm64|aarch64) arch=aarch64 ;;
  *) die "unsupported CPU: $arch" ;;
esac
case "$os" in
  Linux) target="$arch-unknown-linux-gnu"; bins="yard yard-board" ;;
  Darwin) target="$arch-apple-darwin"; bins="yard yard-tray" ;;
  *) die "unsupported OS: $os (on Windows use install.ps1)" ;;
esac

mkdir -p "$dir"

if [ -n "$source" ]; then
  [ -n "$here" ] || die "--source needs a checkout: git clone https://github.com/$REPO && cd switchyard && ./install.sh"
  command -v cargo >/dev/null 2>&1 || die "Rust is not installed: https://rustup.rs"
  cd "$here"
  say "building from source"
  if [ "$os" = Linux ]; then pkgs="-p yard -p yard-board"; else pkgs="-p yard -p yard-tray"; fi
  # Keep a release build from starving the desktop.
  if command -v lowprio >/dev/null 2>&1; then run="lowprio"; else run="nice -n 19"; fi
  # shellcheck disable=SC2086
  $run cargo build --release --locked $pkgs
  for b in $bins; do install -m 755 "target/release/$b" "$dir/$b"; done
else
  command -v curl >/dev/null 2>&1 || die "curl is needed to download Switchyard"
  if [ "$version" = latest ]; then
    url="https://github.com/$REPO/releases/latest/download/switchyard-$target.tar.gz"
  else
    url="https://github.com/$REPO/releases/download/$version/switchyard-$target.tar.gz"
  fi
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  say "downloading $url"
  curl -fsSL "$url" -o "$tmp/switchyard.tar.gz" || die "no release for $target yet; try --source"
  tar -xzf "$tmp/switchyard.tar.gz" -C "$tmp"
  for b in $bins; do
    [ -f "$tmp/$b" ] && install -m 755 "$tmp/$b" "$dir/$b"
  done
fi
say "installed $(echo "$bins" | sed 's/ / and /') to $dir"

if [ "$os" = Linux ] && command -v ldd >/dev/null 2>&1; then
  if ldd "$dir/yard-board" 2>/dev/null | grep -q 'not found'; then
    printf '\nThe board needs GTK 4 and gtk4-layer-shell:\n  Arch: sudo pacman -S gtk4 gtk4-layer-shell\n  Debian/Ubuntu: sudo apt install libgtk-4-1 libgtk4-layer-shell0\n  Fedora: sudo dnf install gtk4 gtk4-layer-shell\n\n'
  fi
fi

case ":$PATH:" in
  *":$dir:"*) ;;
  *) printf '\nAdd %s to your PATH, for example in ~/.profile:\n  export PATH="%s:$PATH"\n\n' "$dir" "$dir" ;;
esac

if [ "$setup" = 1 ]; then
  "$dir/yard" setup
fi
