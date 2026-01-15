#!/usr/bin/env bash
set -euo pipefail

# Prepare embedded DB runtime directories and seed sqlite db (if configured)
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNTIME_DIR="${ROOT}/runtime/db"
SQLITE_FILE="${RUNTIME_DIR}/fenrir.db"

mkdir -p "${RUNTIME_DIR}"

if [[ ! -f "${SQLITE_FILE}" ]]; then
  echo "Creating sqlite db at ${SQLITE_FILE}"
  sqlite3 "${SQLITE_FILE}" "PRAGMA journal_mode=WAL;" >/dev/null 2>&1 || true
fi

echo "db-runtime init completed (runtime dir: ${RUNTIME_DIR})"

art