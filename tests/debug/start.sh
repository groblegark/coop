#!/bin/bash
# SPDX-License-Identifier: BUSL-1.1
# Copyright (c) 2026 Alfred Jean LLC
# Debug helper: build coop, spawn it wrapping a command, open browser terminal.
#
# Usage:
#   tests/debug/start.sh                                  # wrap bash
#   tests/debug/start.sh --port 8080 -- python3 -i        # wrap python
#   COOP_AGENT=claude tests/debug/start.sh -- claude       # with agent detection
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"

PORT=7070
BUILD=1
OPEN=1
CMD=()

# --- Parse arguments ---
while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)     PORT="$2"; shift 2 ;;
    --no-build) BUILD=0; shift ;;
    --no-open)  OPEN=0; shift ;;
    --)         shift; CMD=("$@"); break ;;
    *)          CMD=("$@"); break ;;
  esac
done

if [[ ${#CMD[@]} -eq 0 ]]; then
  CMD=(/bin/bash)
fi

# --- Build ---
if [[ "$BUILD" -eq 1 ]]; then
  echo "Building coop…"
  cargo build -p coop --manifest-path "$ROOT_DIR/Cargo.toml"
fi

COOP_BIN="$ROOT_DIR/target/debug/coop"
if [[ ! -x "$COOP_BIN" ]]; then
  echo "error: $COOP_BIN not found; run without --no-build" >&2
  exit 1
fi

# --- Spawn coop ---
COOP_PID=""

cleanup() {
  if [[ -n "$COOP_PID" ]] && kill -0 "$COOP_PID" 2>/dev/null; then
    kill "$COOP_PID" 2>/dev/null
    wait "$COOP_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

echo "Starting coop on port $PORT: ${CMD[*]}"
"$COOP_BIN" --port "$PORT" --log-format text -- "${CMD[@]}" &
COOP_PID=$!

# --- Wait for health ---
echo -n "Waiting for coop to be ready"
for i in $(seq 1 50); do
  if curl -sf "http://localhost:$PORT/api/v1/health" >/dev/null 2>&1; then
    echo " ok"
    break
  fi
  if ! kill -0 "$COOP_PID" 2>/dev/null; then
    echo " failed (process exited)"
    exit 1
  fi
  echo -n "."
  sleep 0.1
done

# Final check
if ! curl -sf "http://localhost:$PORT/api/v1/health" >/dev/null 2>&1; then
  echo " timed out"
  exit 1
fi

# --- Open browser ---
HTML="file://$SCRIPT_DIR/terminal.html?port=$PORT"
if [[ "$OPEN" -eq 1 ]]; then
  echo "Opening $HTML"
  if command -v open >/dev/null 2>&1; then
    open "$HTML"
  elif command -v xdg-open >/dev/null 2>&1; then
    xdg-open "$HTML"
  else
    echo "Open manually: $HTML"
  fi
fi

# --- Wait for coop to exit ---
wait "$COOP_PID"
EXIT_CODE=$?
COOP_PID=""
exit "$EXIT_CODE"
