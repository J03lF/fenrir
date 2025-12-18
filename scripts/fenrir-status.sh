#!/bin/bash
# ============================================================================
# Fenrir Status Script
# ============================================================================
# Shows the current status of Fenrir.
# Works with both file-based and database-backed identity storage.
# Usage: ./scripts/fenrir-status.sh
# ============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

# Colors
CYAN='\033[38;5;81m'
GRAY='\033[38;5;250m'
GREEN='\033[38;5;82m'
RED='\033[38;5;203m'
RESET='\033[0m'

# Load profile
if [ -f "$ROOT_DIR/bin/.fenrir-profile" ]; then
    source "$ROOT_DIR/bin/.fenrir-profile"
fi

PID_FILE="${FENRIR_PID_FILE:-/tmp/fenrir.pid}"
FENRIR_URL="${FENRIR_URL:-http://127.0.0.1:8080}"
FENRIR_USER="${FENRIR_USER:-admin}"

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

echo ""
echo -e "${CYAN}   Fenrir Status${RESET}"
echo -e "${GRAY}   ─────────────────────────────${RESET}"

# Process status
if [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
    PID=$(cat "$PID_FILE")
    echo -e "   Process:  ${GREEN}running${RESET} (PID $PID)"
else
    echo -e "   Process:  ${RED}stopped${RESET}"
fi

# Health check
if curl -fs "$FENRIR_URL/health/ready" >/dev/null 2>&1; then
    echo -e "   Health:   ${GREEN}ready${RESET}"
else
    echo -e "   Health:   ${RED}not ready${RESET}"
fi

# Password status (supports both file and db modes)
check_password_status() {
    # Use Fenrir CLI if available
    if [ -n "$FENRIR_BIN" ] && [ -f "$FENRIR_BIN" ]; then
        if "$FENRIR_BIN" --password-status "$FENRIR_USER" 2>&1 | grep -q "PWD-SET\|PWD-PENDING"; then
            return 0
        else
            return 1
        fi
    fi
    
    # Fallback: check for marker files
    local identity_dir="${FENRIR_RUNTIME_DIR:-$ROOT_DIR/runtime}/identity"
    
    # Check DB marker
    if [ -f "$identity_dir/.db-password-set" ]; then
        return 0
    fi
    
    # Check JSON file
    if [ -f "$identity_dir/store.json" ] && grep -q "password_hash" "$identity_dir/store.json" 2>/dev/null; then
        return 0
    fi
    
    # Check pending password
    if [ -f "$identity_dir/.pending-password" ]; then
        return 0
    fi
    
    return 1
}

if check_password_status; then
    echo -e "   Password: ${GREEN}configured${RESET}"
else
    echo -e "   Password: ${RED}not configured${RESET}"
fi

# Config
echo -e "   User:     ${GRAY}${FENRIR_USER}${RESET}"
echo -e "   Profile:  ${GRAY}${FENRIR_PROFILE:-default}${RESET}"
echo -e "   URL:      ${GRAY}${FENRIR_URL}${RESET}"
echo ""
