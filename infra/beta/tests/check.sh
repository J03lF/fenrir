#!/usr/bin/env bash
# Static checks of infra/beta (no server needed):
#   bash -n for every script, TOML parse of config/beta.toml, the env
#   templates hold no secret values, `caddy validate` when caddy is on PATH
#   (or CADDY=/path/to/caddy), with the log path moved to a temp dir.
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
fail=0
err() {
	printf 'FAIL %s\n' "$*" >&2
	fail=1
}

while IFS= read -r f; do
	bash -n "$f" || err "bash -n $f"
done < <(find "$here" -type f -name '*.sh' | LC_ALL=C sort)
echo "bash -n: $(find "$here" -type f -name '*.sh' | wc -l | tr -d ' ') scripts"

toml_mod=""
if python3 -c 'import tomllib' 2>/dev/null; then toml_mod=tomllib; elif python3 -c 'import tomli' 2>/dev/null; then toml_mod=tomli; fi
if [ -n "$toml_mod" ]; then
	python3 -c "import sys, $toml_mod as t; t.load(open(sys.argv[1], 'rb'))" "$here/config/beta.toml" ||
		err "config/beta.toml is not valid TOML"
	echo "toml: config/beta.toml parses ($toml_mod)"
else
	echo "toml: skipped (python3 without tomllib/tomli)"
fi

# Templates: every secret must be a *_FILE path, never a value.
if grep -nE '^[A-Z_]*(TOKEN|PASSWORD|SECRET|PEPPER|KEY)=' "$here"/env/*.example | grep -v '_FILE='; then
	err "a template sets a secret value directly"
fi
echo "env templates: no secret values"

caddy_bin=${CADDY:-$(command -v caddy || true)}
if [ -n "$caddy_bin" ]; then
	tmp=$(mktemp -d)
	trap 'rm -rf -- "$tmp"' EXIT
	sed "s#/var/log/caddy/#$tmp/#" "$here/caddy/Caddyfile" >"$tmp/Caddyfile"
	"$caddy_bin" validate --config "$tmp/Caddyfile" --adapter caddyfile >/dev/null 2>"$tmp/validate.log" ||
		err "caddy validate: $(tail -n 3 "$tmp/validate.log")"
	"$caddy_bin" fmt "$here/caddy/Caddyfile" >/dev/null 2>"$tmp/fmt.log" || err "caddy fmt: $(cat "$tmp/fmt.log")"
	echo "caddy: validate and fmt ok ($("$caddy_bin" version | cut -d' ' -f1))"
else
	echo "caddy: skipped (not installed)"
fi

[ "$fail" = 0 ] && echo "all checks passed"
exit "$fail"
