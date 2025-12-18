#!/bin/bash
# ============================================================================
# Fenrir Start Script
# ============================================================================
# Starts Fenrir with the correct environment.
# Automatically runs setup if no password is configured.
# Works with both file-based and database-backed identity storage.
# Usage: ./scripts/fenrir-start.sh [--no-attach]
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
LOG_FILE="${FENRIR_LOG:-/tmp/fenrir-start.log}"
FENRIR_URL="${FENRIR_URL:-http://127.0.0.1:8080}"
FENRIR_SSH_USER="${FENRIR_USER:-admin}"
FENRIR_SSH_PORT="${FENRIR_SSH_PORT:-2222}"
RUNTIME_DIR="${FENRIR_RUNTIME_DIR:-$ROOT_DIR/runtime}"
IDENTITY_DIR="$RUNTIME_DIR/identity"
PENDING_FILE="$IDENTITY_DIR/.pending-password"

NO_ATTACH=false
[ "$1" = "--no-attach" ] && NO_ATTACH=true

# Find Fenrir binary
find_fenrir_binary() {
    if [ -f "$ROOT_DIR/fenrir" ]; then
        echo "$ROOT_DIR/fenrir"
    elif [ -f "$ROOT_DIR/target/release/fenrir" ]; then
        echo "$ROOT_DIR/target/release/fenrir"
    elif [ -f "$ROOT_DIR/target/debug/fenrir" ]; then
        echo "$ROOT_DIR/target/debug/fenrir"
    else
        echo ""
    fi
}

FENRIR_BIN=$(find_fenrir_binary)

# Check if password is configured (supports both file and db modes)
password_is_set() {
    # First check for pending password
    if [ -f "$PENDING_FILE" ]; then
        return 0
    fi

    # If we have a Fenrir binary, use it to check password status
    if [ -n "$FENRIR_BIN" ] && [ -f "$FENRIR_BIN" ]; then
        if "$FENRIR_BIN" --password-status "$FENRIR_SSH_USER" >/dev/null 2>&1; then
            return 0
        else
            return 1
        fi
    fi

    # Fallback: check JSON file (file-based storage)
    local store_file="$IDENTITY_DIR/store.json"
    if [ -f "$store_file" ] && grep -q "password_hash" "$store_file" 2>/dev/null; then
        return 0
    fi

    # Fallback: check DB marker file (db-based storage)
    if [ -f "$IDENTITY_DIR/.db-password-set" ]; then
        return 0
    fi

    return 1
}

# Check if password is configured, run setup if not
if ! password_is_set; then
    echo -e "${GRAY}   No password configured - running setup...${RESET}"
    "$SCRIPT_DIR/fenrir-setup.sh" || exit 1
    echo ""
fi

# Check if already running
fenrir_running() {
    [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null
}

if fenrir_running; then
    PID=$(cat "$PID_FILE")
    echo -e "${CYAN}   Fenrir already running (PID $PID)${RESET}"
    if [ "$NO_ATTACH" = false ]; then
        echo -e "${GRAY}   Attaching to SSH...${RESET}"
        ssh -o PreferredAuthentications=password \
            -o PubkeyAuthentication=no \
            "${FENRIR_SSH_USER}@127.0.0.1" -p "$FENRIR_SSH_PORT"
    fi
    exit 0
fi

# Build if needed
if [ -z "$FENRIR_BIN" ]; then
    echo -e "${GRAY}   Building Fenrir...${RESET}"
    (cd "$ROOT_DIR" && cargo build >/dev/null 2>&1)
    FENRIR_BIN="$ROOT_DIR/target/debug/fenrir"
fi

# Run database migrations (ensures tables exist before starting)
echo -e "${GRAY}   Checking database migrations...${RESET}"
if ! "$FENRIR_BIN" --migrate >/dev/null 2>&1; then
    echo -e "${RED}   [!]${RESET} Database migration check failed"
    "$FENRIR_BIN" --migrate 2>&1 | head -5
    exit 1
fi

# Start Fenrir
echo -e "${CYAN}   Starting Fenrir...${RESET}"
cd "$ROOT_DIR"
nohup "$FENRIR_BIN" >>"$LOG_FILE" 2>&1 &
echo $! > "$PID_FILE"

# Wait for health
echo -e "${GRAY}   Waiting for health check...${RESET}"
RETRIES=90
until curl -fs "$FENRIR_URL/health/ready" >/dev/null 2>&1; do
    RETRIES=$((RETRIES - 1))
    if [ $RETRIES -le 0 ]; then
        echo -e "${RED}   [!]${RESET} Timeout waiting for Fenrir"
        exit 1
    fi
    sleep 1
done

PID=$(cat "$PID_FILE")
echo -e "${CYAN}   [ok]${RESET} ${GRAY}Fenrir running (PID $PID)${RESET}"

# Attach to SSH
if [ "$NO_ATTACH" = false ]; then
    echo ""
    ssh -o PreferredAuthentications=password \
        -o PubkeyAuthentication=no \
        "${FENRIR_SSH_USER}@127.0.0.1" -p "$FENRIR_SSH_PORT"
fi
