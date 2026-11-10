#!/usr/bin/env bash
# Runs Switchyard against the fake projects in this folder, fully isolated
# from any real config, state or daemon.
#
#   examples/demo/run-demo.sh            daemon + board
#   examples/demo/run-demo.sh daemon     daemon only (foreground)
#
# With Roundhouse installed (`rh` on PATH, or YARD_DEMO_RH=/path/to/rh), an
# isolated Roundhouse runs next to it, so the board's Roundhouse link opens a
# drawer that links back. YARD_DEMO_RH=none leaves it out.
#
# Then, in another shell, point the CLI at it the same way:
#   source <(examples/demo/run-demo.sh env)
#   yard ls; yard run storefront; yard toggle
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="${YARD_DEMO_ROOT:-/tmp/switchyard-demo}"
export YARD_DEMO="$here"
export SWITCHYARD_CONFIG_DIR="$root/config"
export SWITCHYARD_STATE_DIR="$root/state"
export SWITCHYARD_RUNTIME_DIR="$root/run"
# Roundhouse gets its own directories too: the installed one is never asked,
# and the demo's (if any) talks only to the demo's Switchyard.
export ROUNDHOUSE_RUNTIME_DIR="$root/roundhouse"
export ROUNDHOUSE_STATE_DIR="$root/roundhouse-state"
export ROUNDHOUSE_CONFIG_DIR="$root/roundhouse-config"
rh="${YARD_DEMO_RH:-$(command -v rh || true)}"
[ "$rh" = none ] && rh=""
export PATH="$here/bin:$PATH"
mkdir -p "$SWITCHYARD_CONFIG_DIR" "$SWITCHYARD_STATE_DIR"
cp "$here/config.toml" "$SWITCHYARD_CONFIG_DIR/config.toml"
# A week of believable history, so the History tab has a story on day one.
if [ ! -f "$SWITCHYARD_STATE_DIR/history/runs.jsonl" ]; then
  python3 "$here/seed-history.py" "$SWITCHYARD_STATE_DIR" >/dev/null 2>&1 || true
fi
bin="${YARD_BIN:-$here/../../target/debug}"
case "${1:-}" in
  env)
    echo "export YARD_DEMO='$YARD_DEMO' SWITCHYARD_CONFIG_DIR='$SWITCHYARD_CONFIG_DIR' SWITCHYARD_STATE_DIR='$SWITCHYARD_STATE_DIR' SWITCHYARD_RUNTIME_DIR='$SWITCHYARD_RUNTIME_DIR' ROUNDHOUSE_RUNTIME_DIR='$ROUNDHOUSE_RUNTIME_DIR' ROUNDHOUSE_STATE_DIR='$ROUNDHOUSE_STATE_DIR' ROUNDHOUSE_CONFIG_DIR='$ROUNDHOUSE_CONFIG_DIR' PATH='$here/bin:$bin':\"\$PATH\""
    ;;
  daemon)
    exec "$bin/yard" daemon
    ;;
  *)
    "$bin/yard" daemon &
    daemon=$!
    rhd=""
    if [ -n "$rh" ] && [ -x "$rh" ]; then
      mkdir -p "$ROUNDHOUSE_CONFIG_DIR" "$ROUNDHOUSE_STATE_DIR"
      "$rh" daemon >/dev/null 2>&1 &
      rhd=$!
    fi
    # The Roundhouse daemon starts its drawer as a child; stop both.
    trap 'kill $daemon 2>/dev/null; if [ -n "$rhd" ]; then pkill -P "$rhd" 2>/dev/null; kill "$rhd" 2>/dev/null; fi' EXIT
    sleep 0.5
    "$bin/yard-board"
    ;;
esac
