#!/usr/bin/env bash
# The host against a real Fenrir Depot, end to end (tests/depot_e2e.rs):
# depot-server on its Unix socket behind the depot's mTLS test proxy, on a
# throwaway PostgreSQL, distributions published with the real `depot` CLI.
#
#   scripts/depot-e2e.sh
#
# DEPOT_DIR          the fenrir-depot checkout (default ../fenrir-depot, the
#                    same path Cargo.toml patches depot-protocol to)
# DEPOT_E2E_TARGET   where the depot binaries are built (default
#                    $DEPOT_DIR/target/host-e2e; never its target/debug, from
#                    which a preview may be running)
#
# Postgres comes from $DEPOT_DIR/scripts/throwaway-pg.sh: initdb in a fresh
# /tmp dir, a free port >= 55000, socket inside that dir, 127.0.0.1 only,
# removed afterwards. Never port 5432, never a running Postgres.
set -euo pipefail
cd "$(dirname "$0")/.."

DEPOT_DIR=${DEPOT_DIR:-../fenrir-depot}
DEPOT_DIR=$(cd "$DEPOT_DIR" && pwd)
target=${DEPOT_E2E_TARGET:-$DEPOT_DIR/target/host-e2e}

echo "==> depot binaries ($target)"
(
  cd "$DEPOT_DIR"
  CARGO_TARGET_DIR="$target" cargo build --locked -p depot-server -p depot-cli --bins
  CARGO_TARGET_DIR="$target" cargo build --locked -p depot-cli --example mtls_proxy
)
bin="$target/debug"
cp "$bin/examples/mtls_proxy" "$bin/mtls_proxy"

echo "==> host end-to-end test"
FENRIR_DEPOT_E2E_BIN="$bin" "$DEPOT_DIR/scripts/throwaway-pg.sh" \
  cargo test --locked --test depot_e2e -- --nocapture
