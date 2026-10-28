#!/usr/bin/env bash
# Load config/default.toml + infra/beta/config/beta.toml with the variables of
# env/fenrir.env.example through the host's own `fenrir --check-config`
# (parses and validates only; starts nothing, opens no port, touches no DB).
#
#   infra/beta/tests/config-check.sh <path-to-fenrir-binary>
#
# The *_FILE secrets point to throwaway random files in a temp dir. Also
# checks that production refuses dev settings and the tracked dev profile.
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
run() { # extra KEY=VALUE... -> output of fenrir --check-config, exit code kept
	env -i PATH="$PATH" HOME="$tmp" FENRIR_BIN="$fenrir" bash -c 'set -a; . ./env; for kv in "$@"; do export "$kv"; done; set +a; exec "$FENRIR_BIN" --check-config' _ "$@" 2>&1
}
fail=0
expect_ok() { # name extra...
	local name=$1 out
	shift
	if out=$(run "$@"); then printf 'ok    %s: %s\n' "$name" "$out"; else
		printf 'FAIL  %s: %s\n' "$name" "$out"
		fail=1
	fi
}
expect_refused() { # name pattern extra...
	local name=$1 pattern=$2 out
	shift 2
	if out=$(run "$@"); then
		printf 'FAIL  %s: accepted (%s)\n' "$name" "$out"
		fail=1
	elif printf '%s' "$out" | grep -q -- "$pattern"; then
		printf 'ok    %s: refused (%s)\n' "$name" "$(printf '%s' "$out" | grep -o -- "$pattern" | head -n 1)"
	else
		printf 'FAIL  %s: refused for another reason: %s\n' "$name" "$out"
		fail=1
	fi
}

expect_ok "beta profile, FENRIR_ENV=prod (the server)"
expect_ok "beta profile, FENRIR_ENV unset (fail-safe production)" FENRIR_ENV=
expect_refused "production with dev module sources" "dev module sources" FENRIR_DEV_MODULE_PATH=/tmp/modules
expect_refused "production with a short control token" "shorter than 32" FENRIR_HTTP_TOKEN_ADMIN_FILE= FENRIR_HTTP_TOKEN_ADMIN=short-token

# The tracked developer profile (bin/.fenrir-profile of this repo), loaded
# the way the host loads it from its working directory, must not pass as
# production. Its values are not printed.
if git -C "$repo" cat-file -e HEAD:bin/.fenrir-profile 2>/dev/null; then
	mkdir -p "$tmp/bin"
	git -C "$repo" show HEAD:bin/.fenrir-profile >"$tmp/bin/.fenrir-profile"
	expect_refused "tracked dev profile with FENRIR_ENV=prod" "dev-only settings are active"
	rm -rf -- "$tmp/bin"
fi
exit "$fail"
