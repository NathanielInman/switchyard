#!/usr/bin/env bash
# Builds Switchyard and installs it for the current user.
#
#   ./install.sh               build, install to ~/.local/bin, enable and (re)start the user units
#   ./install.sh --no-units    install the binaries only
#   PREFIX=/opt/yard ./install.sh
set -euo pipefail
cd "$(dirname "$0")"

prefix="${PREFIX:-$HOME/.local}"
bindir="$prefix/bin"
units=1
for a in "$@"; do
  case "$a" in
    --no-units) units=0 ;;
    -h|--help) sed -n '2,7p' "$0"; exit 0 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

# A release build compiles GTK bindings; keep it from starving the desktop.
build=(cargo build --release --locked)
if command -v lowprio >/dev/null 2>&1; then
  lowprio "${build[@]}"
else
  nice -n 19 "${build[@]}"
fi

install -Dm755 target/release/yard "$bindir/yard"
install -Dm755 target/release/yard-board "$bindir/yard-board"
echo "installed yard and yard-board to $bindir"

if [ "$units" = 1 ]; then
  unitdir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
  mkdir -p "$unitdir"
  for u in switchyard.service switchyard-board.service; do
    sed "s|@BINDIR@|$bindir|g" "contrib/systemd/$u" > "$unitdir/$u"
  done
  systemctl --user daemon-reload
  systemctl --user enable switchyard.service switchyard-board.service >/dev/null
  # Restart so a new build takes over; running tasks live in their own units.
  systemctl --user restart switchyard.service
  systemctl --user restart switchyard-board.service || true
  echo "enabled switchyard.service and switchyard-board.service"
fi

"$bindir/yard" doctor || true
