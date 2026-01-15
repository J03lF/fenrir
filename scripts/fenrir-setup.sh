#!/bin/bash
# ============================================================================
# Fenrir Setup Script
# ============================================================================
# Run this once before starting Fenrir to configure your password.
# Works with both file-based and database-backed identity storage.
# - Runs database migrations to create required tables
# - Sets up the initial user password
# Usage: ./scripts/fenrir-setup.sh
# ============================================================================

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

# Colors
CYAN='\033[38;5;81m'
GRAY='\033[38;5;250m'
GREEN='\033[38;5;114m'
RED='\033[38;5;203m'
RESET='\033[0m'
BOLD='\033[1m'

# Load profile
if [ -f "$ROOT_DIR/bin/.fenrir-profile" ]; then
    source "$ROOT_DIR/bin/.fenrir-profile"
fi

# Get configured user
FENRIR_USER="${FENRIR_USER:-admin}"
RUNTIME_DIR="${FENRIR_RUNTIME_DIR:-$ROOT_DIR/runtime}"
IDENTITY_DIR="$RUNTIME_DIR/identity"
PENDING_FILE="$IDENTITY_DIR/.pending-password"

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
echo -e "${CYAN}┌─────────────────────────────────────────────────────────────────┐${RESET}"
echo -e "${CYAN}│                                                                 │${RESET}"
echo -e "${CYAN}│   ${BOLD}FENRIR SETUP${RESET}${CYAN}                                                  │${RESET}"
echo -e "${CYAN}│                                                                 │${RESET}"
echo -e "${CYAN}│   ${GRAY}Configure your Fenrir installation.${CYAN}                          │${RESET}"
echo -e "${CYAN}│                                                                 │${RESET}"
echo -e "${CYAN}└─────────────────────────────────────────────────────────────────┘${RESET}"
echo ""

# Build if needed
if [ -z "$FENRIR_BIN" ]; then
    echo -e "${GRAY}   Building Fenrir...${RESET}"
    (cd "$ROOT_DIR" && cargo build --release >/dev/null 2>&1) || {
        echo -e "${RED}   [!]${RESET} Build failed. Run 'cargo build' manually to see errors."
        exit 1
    }
    FENRIR_BIN="$ROOT_DIR/target/release/fenrir"
fi

# Note: Migrations run automatically on Fenrir boot - no separate --migrate needed

# Check if password already exists (supports both file and db modes)
password_is_set() {
    # First check for pending password
    if [ -f "$PENDING_FILE" ]; then
        return 0
    fi

    # If we have a Fenrir binary, use it to check password status
    # Must run from ROOT_DIR so Fenrir can find its config files
    if [ -n "$FENRIR_BIN" ] && [ -f "$FENRIR_BIN" ]; then
        if (cd "$ROOT_DIR" && "$FENRIR_BIN" --password-status "$FENRIR_USER" >/dev/null 2>&1); then
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

if password_is_set; then
        echo -e "${GRAY}   Password already configured for user '${FENRIR_USER}'.${RESET}"
        echo ""
        read -p "   Reset password? [y/N] " -n 1 -r
        echo ""
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            echo -e "${GRAY}   Setup cancelled.${RESET}"
            exit 0
    fi
fi

echo -e "${GRAY}   User: ${FENRIR_USER}${RESET}"
echo ""

# Read password with asterisk feedback
read_password() {
    local password=""
    local char=""
    
    # Disable echo
    stty -echo 2>/dev/null || true
    
    while true; do
        IFS= read -r -s -n1 char
        
        # Enter pressed (empty char)
        if [[ -z "$char" ]]; then
            break
        fi
        
        # Backspace (ASCII 127 or 8)
        if [[ "$char" == $'\177' ]] || [[ "$char" == $'\010' ]]; then
            if [[ -n "$password" ]]; then
                password="${password%?}"
                printf "\b \b" >&2
            fi
        else
            password+="$char"
            printf "*" >&2
        fi
    done
    
    # Re-enable echo
    stty echo 2>/dev/null || true
    printf "\n" >&2
    
    printf "%s" "$password"
}

# Read password
while true; do
    printf "${CYAN}   >${RESET} New password:     "
    PASSWORD=$(read_password)
    
    # Basic validation
    if [ ${#PASSWORD} -lt 4 ]; then
        echo -e "${RED}   [!]${RESET} Password must be at least 4 characters."
        echo ""
        continue
    fi
    
    printf "${CYAN}   >${RESET} Confirm password: "
    PASSWORD_CONFIRM=$(read_password)
    
    if [ "$PASSWORD" != "$PASSWORD_CONFIRM" ]; then
        echo -e "${RED}   [!]${RESET} Passwords do not match. Please try again."
        echo ""
        continue
    fi
    
    break
done

# Create identity directory
mkdir -p "$IDENTITY_DIR"

# Store password as pending - Fenrir will hash and store it on first boot
# This works for both file-based and database-backed identity storage
echo "$PASSWORD" > "$PENDING_FILE"
chmod 600 "$PENDING_FILE"

# Remove any old marker files (password will be re-set on next boot)
rm -f "$IDENTITY_DIR/.db-password-set" 2>/dev/null || true

echo ""
echo -e "${CYAN}   [ok]${RESET} ${GRAY}Password configured for user '${FENRIR_USER}'.${RESET}"
echo -e "${GRAY}   Password will be hashed on first Fenrir start.${RESET}"
echo ""
echo -e "${GRAY}   Start Fenrir with: ${CYAN}./scripts/fenrir-start.sh${RESET}"
echo ""
