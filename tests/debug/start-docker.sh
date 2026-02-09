#!/bin/bash
# SPDX-License-Identifier: BUSL-1.1
# Copyright (c) 2026 Alfred Jean LLC
# Debug helper: build coop test image, run in Docker, open browser terminal.
#
# Usage:
#   tests/debug/start-docker.sh                                    # default scenario
#   tests/debug/start-docker.sh --scenario claude_tool_use.toml    # specific scenario
#   tests/debug/start-docker.sh --port 8080 --no-build             # custom port, skip build
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"

PORT=7070
BUILD=1
OPEN=1
SCENARIO="claude_hello.toml"

# --- Parse arguments ---
while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)       PORT="$2"; shift 2 ;;
    --scenario)   SCENARIO="$2"; shift 2 ;;
    --no-build)   BUILD=0; shift ;;
    --no-open)    OPEN=0; shift ;;
    *)            echo "Unknown option: $1" >&2; exit 1 ;;
  esac
done

# --- Build Docker image ---
if [[ "$BUILD" -eq 1 ]]; then
  echo "Building coop test image…"
  docker build --target test -t coop:test "$ROOT_DIR"
fi

# --- Run container ---
CONTAINER_ID=""

cleanup() {
  if [[ -n "$CONTAINER_ID" ]]; then
    echo "Stopping container $CONTAINER_ID…"
    docker rm -f "$CONTAINER_ID" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT INT TERM

echo "Starting coop in Docker on port $PORT with scenario $SCENARIO"
CONTAINER_ID=$(docker run -d -p "$PORT:7070" coop:test \
  --port 7070 --log-format text --agent claude \
  -- claudeless --scenario "/scenarios/$SCENARIO" "hello")

# --- Wait for health ---
echo -n "Waiting for coop to be ready"
for i in $(seq 1 50); do
  if curl -sf "http://localhost:$PORT/api/v1/health" >/dev/null 2>&1; then
    echo " ok"
    break
  fi
  if ! docker ps -q --filter "id=$CONTAINER_ID" | grep -q .; then
    echo " failed (container exited)"
    docker logs "$CONTAINER_ID" 2>&1 || true
    exit 1
  fi
  echo -n "."
  sleep 0.2
done

# Final check
if ! curl -sf "http://localhost:$PORT/api/v1/health" >/dev/null 2>&1; then
  echo " timed out"
  docker logs "$CONTAINER_ID" 2>&1 || true
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

# --- Tail container logs ---
echo "Tailing container logs (Ctrl+C to stop)…"
docker logs -f "$CONTAINER_ID"
