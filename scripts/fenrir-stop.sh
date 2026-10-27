#!/bin/bash
# ============================================================================
# Fenrir Stop Script
# ============================================================================
# Stops a running Fenrir instance gracefully.
# Usage: ./scripts/fenrir-stop.sh
# ============================================================================

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

# Colors
CYAN='\033[38;5;81m'
GRAY='\033[38;5;250m'
RED='\033[38;5;203m'
RESET='\033[0m'

# Load profile
if [ -f "$ROOT_DIR/bin/.fenrir-profile" ]; then
    source "$ROOT_DIR/bin/.fenrir-profile"
fi

PID_FILE="${FENRIR_PID_FILE:-/tmp/fenrir.pid}"
FENRIR_URL="${FENRIR_URL:-http://127.0.0.1:8080}"
FENRIRCTL_BIN="$ROOT_DIR/target/debug/fenrirctl"
if [ ! -x "$FENRIRCTL_BIN" ] && [ -x "$ROOT_DIR/target/release/fenrirctl" ]; then
    FENRIRCTL_BIN="$ROOT_DIR/target/release/fenrirctl"
fi

# Waits up to $2 seconds for every PID in $1 to exit; true when all are gone.
wait_gone() {
    local pids="$1" tries=$(( $2 * 4 )) p alive
    while [ "$tries" -gt 0 ]; do
        alive=""
        for p in $pids; do kill -0 "$p" 2>/dev/null && alive="$alive $p"; done
        [ -z "$alive" ] && return 0
        sleep 0.25
        tries=$(( tries - 1 ))
    done
    return 1
}

# Check if running
if [ ! -f "$PID_FILE" ]; then
    echo -e "${GRAY}   Fenrir not running${RESET}"
    exit 0
fi

PID=$(cat "$PID_FILE")
if ! kill -0 "$PID" 2>/dev/null; then
    echo -e "${GRAY}   Fenrir not running (stale pid file)${RESET}"
    rm -f "$PID_FILE"
    exit 0
fi

echo -e "${CYAN}   Stopping Fenrir (PID $PID)...${RESET}"

# The module processes the host started. A host that dies without its own
# shutdown leaves them running (re-parented to launchd), holding their ports.
CHILDREN=$(pgrep -P "$PID" 2>/dev/null | tr '\n' ' ' || true)

# Try graceful shutdown via control plane first
resolve_token() {
    if [ -n "${FENRIR_CONTROL_TOKEN:-}" ]; then
        echo "$FENRIR_CONTROL_TOKEN"
        return
    fi
    if command -v security >/dev/null 2>&1; then
        security find-generic-password \
            -a fenrir-control-plane \
            -s fenrir-control-plane-token -w 2>/dev/null && return
    fi
    if [ -n "${FENRIR_HTTP_TOKEN_ADMIN:-}" ]; then
        echo "$FENRIR_HTTP_TOKEN_ADMIN"
        return
    fi
}

TOKEN=$(resolve_token)

if [ -n "$TOKEN" ] && [ -x "$FENRIRCTL_BIN" ]; then
    echo -e "${GRAY}   Requesting graceful shutdown...${RESET}"
    FENRIR_CONTROL_TOKEN="$TOKEN" "$FENRIRCTL_BIN" --url "$FENRIR_URL" shutdown >/dev/null 2>&1 || true
    wait_gone "$PID" 10 || true
fi

# SIGINT is what the host waits for (ctrl_c); SIGTERM would end it at once.
if kill -0 "$PID" 2>/dev/null; then
    kill -INT "$PID" 2>/dev/null || true
    wait_gone "$PID" 5 || { kill "$PID" 2>/dev/null || true; wait_gone "$PID" 3 || true; }
fi

# Force kill if still running
if kill -0 "$PID" 2>/dev/null; then
    echo -e "${GRAY}   Force killing...${RESET}"
    kill -9 "$PID" 2>/dev/null || true
fi

# Modules the host did not take down with it.
LEFT=""
for p in $CHILDREN; do kill -0 "$p" 2>/dev/null && LEFT="$LEFT $p"; done
if [ -n "$LEFT" ]; then
    echo -e "${GRAY}   Stopping modules:$LEFT${RESET}"
    kill $LEFT 2>/dev/null || true
    if ! wait_gone "$LEFT" 10; then
        echo -e "${GRAY}   Force killing modules...${RESET}"
        for p in $LEFT; do kill -9 "$p" 2>/dev/null || true; done
    fi
fi

rm -f "$PID_FILE"
echo -e "${CYAN}   [ok]${RESET} ${GRAY}Fenrir stopped${RESET}"
