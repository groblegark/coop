#!/bin/bash
# SPDX-License-Identifier: BUSL-1.1
# Copyright (c) 2026 Alfred Jean LLC
#
# Human-driven state capture for coop + claude onboarding.
#
# Builds the Docker claude image, mounts a local directory as
# CLAUDE_CONFIG_DIR, opens a browser terminal, and enters a
# snapshot REPL.  At each step of the onboarding flow, type a
# snapshot name to capture the config directory and see what changed.
#
# Usage:
#   tests/debug/capture-claude.sh                       # empty config, full onboarding
#   tests/debug/capture-claude.sh --config trusted      # pre-trusted workspace
#   tests/debug/capture-claude.sh --config auth-only    # auth'd, need workspace trust
#   tests/debug/capture-claude.sh --name my-session     # custom session name
#   tests/debug/capture-claude.sh --local               # run locally instead of Docker
#
# Snapshots are saved to tests/debug/captures/<session>/
#
# Each snapshot captures:
#   - Full copy of CLAUDE_CONFIG_DIR (.claude.json, .claude/, etc.)
#   - Agent state from coop API
#   - Terminal screen text from coop API
#   - Diff from the previous snapshot
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"

# --- Defaults ---
PORT=7070
CONFIG_MODE="empty"
SESSION_NAME=""
BUILD=1
OPEN=1
USE_DOCKER=1
PASS_AUTH=0

# --- Parse args ---
while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)      PORT="$2"; shift 2 ;;
    --config)    CONFIG_MODE="$2"; shift 2 ;;
    --name)      SESSION_NAME="$2"; shift 2 ;;
    --docker)    USE_DOCKER=1; shift ;;
    --local)     USE_DOCKER=0; shift ;;
    --auth)      PASS_AUTH=1; shift ;;
    --no-build)  BUILD=0; shift ;;
    --no-open)   OPEN=0; shift ;;
    -h|--help)
      echo "Usage: $(basename "$0") [OPTIONS]"
      echo ""
      echo "Interactive state capture for claude onboarding flow."
      echo ""
      echo "Options:"
      echo "  --config MODE    Config preset (default: empty)"
      echo "                     empty     — no config, full onboarding"
      echo "                     auth-only — onboarding done, no workspace trust"
      echo "                     trusted   — fully trusted, skip to idle prompt"
      echo "  --name NAME      Session name (default: timestamp)"
      echo "  --port PORT      Coop HTTP port (default: 7070)"
      echo "  --docker         Run in Docker (default)"
      echo "  --local          Run locally instead of Docker"
      echo "  --auth           Pass CLAUDE_CODE_OAUTH_TOKEN into container"
      echo "  --no-build       Skip image/binary build"
      echo "  --no-open        Don't open browser"
      echo ""
      echo "In the REPL, type a snapshot name and press Enter."
      echo "Examples: security-notes, login, trust, theme, idle"
      exit 0
      ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
done

[[ -z "$SESSION_NAME" ]] && SESSION_NAME="$(date +%Y%m%d-%H%M%S)"

# --- Output layout ---
CAPTURE_DIR="$SCRIPT_DIR/captures/$SESSION_NAME"
CONFIG_DIR="$CAPTURE_DIR/config"
SNAP_DIR="$CAPTURE_DIR/snapshots"
DIFF_DIR="$CAPTURE_DIR/diffs"
AGENT_DIR="$CAPTURE_DIR/agent-state"
SCREEN_DIR="$CAPTURE_DIR/screens"

WORKSPACE="$(mktemp -d)"
WORKSPACE="$(cd "$WORKSPACE" && pwd -P)"

mkdir -p "$CONFIG_DIR" "$SNAP_DIR" "$DIFF_DIR" "$AGENT_DIR" "$SCREEN_DIR"

# --- Resolve workspace path (differs between Docker and local) ---
if [[ "$USE_DOCKER" -eq 1 ]]; then
  CONTAINER_WS="/workspace"
else
  CONTAINER_WS="$WORKSPACE"
fi

# --- Seed config ---
case "$CONFIG_MODE" in
  trusted)
    cat > "$CONFIG_DIR/.claude.json" <<JSON
{
  "hasCompletedOnboarding": true,
  "projects": {
    "$CONTAINER_WS": {
      "hasTrustDialogAccepted": true,
      "allowedTools": [],
      "hasCompletedProjectOnboarding": true
    }
  }
}
JSON
    touch "$WORKSPACE/CLAUDE.md"
    ;;
  auth-only)
    cat > "$CONFIG_DIR/.claude.json" <<JSON
{
  "hasCompletedOnboarding": true,
  "projects": {}
}
JSON
    ;;
  empty)
    ;; # nothing — full onboarding
  *)
    echo "Unknown config mode: $CONFIG_MODE (expected: empty, auth-only, trusted)" >&2
    exit 1
    ;;
esac

# --- Snapshot machinery ---
SNAP_NUM=0
PREV_TAG=""

RSYNC_EXCLUDES=(
  --exclude='debug/' --exclude='cache/' --exclude='statsig/'
  --exclude='.claude.json.backup.*'
)

# Copy config into a directory, filtering out noisy files.
copy_config() {
  local dest="$1"
  mkdir -p "$dest"
  if [[ -d "$CONFIG_DIR" ]]; then
    if command -v rsync >/dev/null 2>&1; then
      rsync -a "${RSYNC_EXCLUDES[@]}" "$CONFIG_DIR/" "$dest/"
    else
      cp -a "$CONFIG_DIR/." "$dest/"
      rm -rf "$dest/debug" "$dest/cache" "$dest/statsig"
      rm -f "$dest"/.claude.json.backup.*
    fi
  fi
}

snapshot() {
  local name="$1"
  local tag
  tag="$(printf '%03d' "$SNAP_NUM")-${name}"
  local dest="$SNAP_DIR/$tag"

  # Copy config into a temp dir first to check for changes
  local tmp_snap
  tmp_snap="$(mktemp -d)"
  copy_config "$tmp_snap"

  # If we have a previous snapshot, diff against it — skip if nothing changed
  if [[ -n "$PREV_TAG" ]]; then
    if diff -rq "$SNAP_DIR/$PREV_TAG" "$tmp_snap" >/dev/null 2>&1; then
      rm -rf "$tmp_snap"
      return 0
    fi
  fi

  # Something changed (or first snapshot) — commit it
  mv "$tmp_snap" "$dest"

  echo ""
  echo "━━━ [$(printf '%03d' "$SNAP_NUM")] $tag ━━━"

  # Capture coop agent state
  curl -sf "http://localhost:$PORT/api/v1/agent/state" \
    > "$AGENT_DIR/$tag.json" 2>/dev/null || echo '{}' > "$AGENT_DIR/$tag.json"

  # Capture terminal screen text
  curl -sf "http://localhost:$PORT/api/v1/screen/text" \
    > "$SCREEN_DIR/$tag.txt" 2>/dev/null || true

  # Show .claude.json if it exists in this snapshot
  if [[ -f "$dest/.claude.json" ]]; then
    echo "  .claude.json:"
    if command -v python3 >/dev/null 2>&1; then
      python3 -m json.tool "$dest/.claude.json" 2>/dev/null | head -30 | sed 's/^/    /'
      local lines
      lines="$(wc -l < "$dest/.claude.json" | tr -d ' ')"
      if [[ "$lines" -gt 30 ]]; then
        echo "    … ($lines lines total)"
      fi
    else
      head -30 "$dest/.claude.json" | sed 's/^/    /'
    fi
  fi

  # Generate diff from previous snapshot
  if [[ -n "$PREV_TAG" ]]; then
    local diff_file="$DIFF_DIR/$tag.diff"
    diff -ruN \
      --label "a/$PREV_TAG" "$SNAP_DIR/$PREV_TAG" \
      --label "b/$tag" "$dest" \
      > "$diff_file" 2>/dev/null || true

    echo ""
    echo "  Changes from $PREV_TAG:"
    grep -E '^(---|[+][+][+]|Only in)' "$diff_file" \
      | grep -v '^--- /dev/null' | grep -v '^+++ /dev/null' \
      | sed 's|^--- a/[^/]*/||; s|^+++ b/[^/]*/||; s/^/    /' \
      | sort -u | head -15
    echo ""
    if grep -q '\.claude\.json' "$diff_file" 2>/dev/null; then
      echo "  .claude.json diff:"
      awk '/^diff.*\.claude\.json/,/^diff [^.]/' "$diff_file" \
        | head -40 | sed 's/^/    /'
    fi
  else
    local count
    count="$(cd "$dest" && find . -type f | wc -l | tr -d ' ')"
    if [[ "$count" -gt 0 ]]; then
      echo ""
      echo "  Files ($count):"
      (cd "$dest" && find . -type f | sort | sed 's/^/    /')
    else
      echo "  (empty config directory)"
    fi
  fi

  PREV_TAG="$tag"
  SNAP_NUM=$((SNAP_NUM + 1))
}

# --- Banner ---
MODE_LABEL="docker"
[[ "$USE_DOCKER" -eq 0 ]] && MODE_LABEL="local"

echo ""
echo "╔═══════════════════════════════════════════════╗"
echo "║  State Capture: $SESSION_NAME"
printf "║  %-45s ║\n" "Config: $CONFIG_MODE"
printf "║  %-45s ║\n" "Mode:   $MODE_LABEL"
printf "║  %-45s ║\n" "Port:   $PORT"
echo "╚═══════════════════════════════════════════════╝"
echo ""
echo "  Workspace: $WORKSPACE"
echo "  Output:    $CAPTURE_DIR"
echo ""

# --- Take initial snapshot (before coop starts) ---
echo "━━━ [000] initial ━━━"
snapshot "initial"
echo ""

# --- Cleanup handler ---
COOP_PID=""
CONTAINER_ID=""

cleanup() {
  if [[ -n "$CONTAINER_ID" ]]; then
    echo "Stopping container…"
    docker rm -f "$CONTAINER_ID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$COOP_PID" ]] && kill -0 "$COOP_PID" 2>/dev/null; then
    kill "$COOP_PID" 2>/dev/null
    wait "$COOP_PID" 2>/dev/null || true
  fi
  if [[ "$WORKSPACE" == /tmp/* || "$WORKSPACE" == /private/tmp/* ]] \
     && [[ -d "$WORKSPACE" ]]; then
    rm -rf "$WORKSPACE"
  fi
  echo ""
  echo "═══════════════════════════════════════════"
  echo "  $SNAP_NUM snapshots captured"
  echo ""
  echo "  Snapshots: $SNAP_DIR"
  echo "  Diffs:     $DIFF_DIR"
  echo "  Screens:   $SCREEN_DIR"
  echo "  Agent:     $AGENT_DIR"
  echo "═══════════════════════════════════════════"
}
trap cleanup EXIT INT TERM

# --- Launch ---
if [[ "$USE_DOCKER" -eq 1 ]]; then
  # --- Docker mode ---
  IMAGE_TAG="coop:claude"
  if [[ "$BUILD" -eq 1 ]]; then
    echo "Building $IMAGE_TAG…"
    docker build --target claude -t "$IMAGE_TAG" "$ROOT_DIR"
  fi

  echo "Starting container on port $PORT…"
  DOCKER_ARGS=(
    -d
    -p "$PORT:7070"
    -v "$CONFIG_DIR:/config"
    -v "$WORKSPACE:/workspace"
    -w /workspace
    -e "CLAUDE_CONFIG_DIR=/config"
  )
  # Pass auth credentials only if explicitly requested via --auth
  if [[ "$PASS_AUTH" -eq 1 ]]; then
    [[ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ]] && DOCKER_ARGS+=(-e "CLAUDE_CODE_OAUTH_TOKEN=$CLAUDE_CODE_OAUTH_TOKEN")
    [[ -n "${ANTHROPIC_API_KEY:-}" ]] && DOCKER_ARGS+=(-e "ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY")
  fi

  if ! CONTAINER_ID=$(docker run "${DOCKER_ARGS[@]}" "$IMAGE_TAG" \
    --port 7070 --log-format text --agent claude -- claude); then
    echo "error: docker run failed (exit $?)" >&2
    echo "Command: docker run ${DOCKER_ARGS[*]} $IMAGE_TAG --port 7070 --log-format text --agent claude -- claude" >&2
    exit 1
  fi
  echo "Container: ${CONTAINER_ID:0:12}"
else
  # --- Local mode ---
  if [[ "$BUILD" -eq 1 ]]; then
    echo "Building coop…"
    cargo build -p coop --manifest-path "$ROOT_DIR/Cargo.toml" 2>&1 | tail -1
  fi

  COOP_BIN="$ROOT_DIR/target/debug/coop"
  [[ -x "$COOP_BIN" ]] || { echo "error: $COOP_BIN not found" >&2; exit 1; }

  echo "Starting coop on port $PORT…"
  (cd "$WORKSPACE" && \
    COOP_AGENT=claude \
    CLAUDE_CONFIG_DIR="$CONFIG_DIR" \
    "$COOP_BIN" --port "$PORT" --log-format text -- claude \
  ) &
  COOP_PID=$!
fi

# --- Wait for health ---
echo -n "Waiting for coop"
for i in $(seq 1 50); do
  if curl -sf "http://localhost:$PORT/api/v1/health" >/dev/null 2>&1; then
    echo " ok"
    break
  fi
  if [[ -n "$CONTAINER_ID" ]]; then
    if ! docker ps -q --filter "id=$CONTAINER_ID" | grep -q .; then
      echo " failed (container exited)"
      docker logs "$CONTAINER_ID" 2>&1 | tail -5 || true
      exit 1
    fi
  elif [[ -n "$COOP_PID" ]] && ! kill -0 "$COOP_PID" 2>/dev/null; then
    echo " failed (process exited)"
    exit 1
  fi
  echo -n "."
  sleep 0.2
done

if ! curl -sf "http://localhost:$PORT/api/v1/health" >/dev/null 2>&1; then
  echo " timed out"
  exit 1
fi

# --- Open browser terminal ---
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

# --- Auto-snapshot loop ---
echo ""
echo "Watching for config changes (Ctrl-C to stop)…"
echo ""

while true; do
  sleep 1
  # Check if the process is still alive
  if [[ -n "$CONTAINER_ID" ]]; then
    docker ps -q --filter "id=$CONTAINER_ID" | grep -q . || break
  elif [[ -n "$COOP_PID" ]] && ! kill -0 "$COOP_PID" 2>/dev/null; then
    break
  fi

  # snapshot() diffs internally and returns early if nothing changed
  snapshot "snap"
done
