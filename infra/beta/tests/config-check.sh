#!/usr/bin/env bash
# Load config/default.toml + infra/beta/config/beta.toml with the variables of
# env/fenrir.env.example through the host's own `fenrir --check-config`
# (parses and validates only; starts nothing, opens no port, touches no DB).
#
#   infra/beta/tests/config-check.sh <path-to-fenrir-binary>
#
# The *_FILE secrets point to throwaway random files in a temp dir.
set -euo pipefail

fenrir=${1:?path to a fenrir binary (cargo build --bin fenrir)}
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
repo=$(cd -- "$here/../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
mkdir -p "$tmp/config" "$tmp/secrets"
cp "$repo/config/default.toml" "$repo/config/prod.toml" "$tmp/config/"
cp "$here/config/beta.toml" "$tmp/config/beta.toml"

# The template's variables, with every /etc/athenos/secrets/<name> replaced
# by a random file of the same name.
while IFS= read -r line; do
	case $line in
	*=/etc/athenos/secrets/*)
		name=${line##*/}
		[ -e "$tmp/secrets/$name" ] || openssl rand -hex 24 >"$tmp/secrets/$name"
		printf '%s\n' "${line%%=*}=$tmp/secrets/$name"
		;;
	*) printf '%s\n' "$line" ;;
	esac
done < <(grep -E '^[A-Z_][A-Z0-9_]*=' "$here/env/fenrir.env.example") >"$tmp/env"

cd "$tmp"
env -i PATH="$PATH" HOME="$tmp" FENRIR_BIN="$fenrir" bash -c 'set -a; . ./env; set +a; exec "$FENRIR_BIN" --check-config'
