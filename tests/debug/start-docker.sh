#!/bin/bash
# SPDX-License-Identifier: BUSL-1.1
# Copyright (c) 2026 Alfred Jean LLC
# Debug helper: build coop test image, run in Docker, open browser terminal.
#
# Usage:
#   tests/debug/start-docker.sh claudeless                             # default scenario
#   tests/debug/start-docker.sh claudeless --scenario claude_tool_use.toml
#   tests/debug/start-docker.sh claude                                 # coop + claude CLI
#   tests/debug/start-docker.sh gemini                                 # coop + gemini CLI
#   tests/debug/start-docker.sh claude --port 8080 --no-build
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"

# First positional arg is the mode
MODE="${1:-claudeless}"
shift || true

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

# --- Resolve image and command per mode ---
case "$MODE" in
  claudeless)
    IMAGE_TARGET="claudeless"
    IMAGE_TAG="coop:claudeless"
    DOCKER_RUN_ARGS=(-p "$PORT:7070" "$IMAGE_TAG" \
      --port 7070 --log-format text --agent claude \
      -- claudeless --scenario "/scenarios/$SCENARIO" "hello")
    LABEL="scenario $SCENARIO"
    ;;
  claude)
    IMAGE_TARGET="claude"
    IMAGE_TAG="coop:claude"
    DOCKER_RUN_ARGS=(-p "$PORT:7070" "$IMAGE_TAG" \
      --port 7070 --log-format text --agent claude \
      -- claude)
    LABEL="claude CLI"
    ;;
  gemini)
    IMAGE_TARGET="gemini"
    IMAGE_TAG="coop:gemini"
    DOCKER_RUN_ARGS=(-p "$PORT:7070" "$IMAGE_TAG" \
      --port 7070 --log-format text --agent gemini \
      -- gemini)
    LABEL="gemini CLI"
    ;;
  *)
    echo "Unknown mode: $MODE (expected 'claudeless', 'claude', or 'gemini')" >&2
    exit 1
    ;;
esac

# --- Build Docker image ---
if [[ "$BUILD" -eq 1 ]]; then
  echo "Building $IMAGE_TAG (target: $IMAGE_TARGET)…"
  docker build --target "$IMAGE_TARGET" -t "$IMAGE_TAG" "$ROOT_DIR"
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

echo "Starting coop in Docker on port $PORT with $LABEL"
CONTAINER_ID=$(docker run -d "${DOCKER_RUN_ARGS[@]}")

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
