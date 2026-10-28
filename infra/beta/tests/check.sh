#!/usr/bin/env bash
# Static checks of infra/beta (no server needed):
#   bash -n for every script, TOML parse of config/beta.toml, the env
#   templates hold no secret values, `caddy validate` when caddy is on PATH
#   (or CADDY=/path/to/caddy), with the server files (log dir, Cloudflare
#   snippet, origin certificate) replaced by temporary stand-ins.
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

# The keyring the host bundle ships (config/modules/keyring.json): valid,
# version 1, no keys (signatures are off on the beta server).
python3 -c 'import json, sys; k = json.load(open(sys.argv[1])); sys.exit(0 if k.get("version") == 1 and k.get("keys") == [] else 1)' \
	"$here/config/keyring.json" || err "config/keyring.json is not an empty version-1 keyring"
grep -q 'infra/beta/config/keyring.json "$pkg/config/modules/keyring.json"' "$here/ci/package-host.sh" ||
	err "ci/package-host.sh does not package config/modules/keyring.json"
echo "keyring: config/keyring.json is empty and packaged"

# Templates: every secret must be a *_FILE path, never a value.
if grep -nE '^[A-Z_]*(TOKEN|PASSWORD|SECRET|PEPPER|KEY)=' "$here"/env/*.example | grep -v '_FILE='; then
	err "a template sets a secret value directly"
fi
echo "env templates: no secret values"

caddy_bin=${CADDY:-$(command -v caddy || true)}
if [ -n "$caddy_bin" ]; then
	tmp=$(mktemp -d)
	trap 'rm -rf -- "$tmp"' EXIT
	# Server files replaced by stand-ins: the Cloudflare snippet rendered
	# from the shipped snapshot, a throwaway self-signed origin certificate.
	{
		printf 'trusted_proxies static'
		grep -v '^#' "$here/cloudflare/ips.txt" | tr '\n' ' ' | sed 's/ *$//; s/^/ /'
		echo
	} >"$tmp/cf.caddy"
	openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
		-subj /CN=app.athenos.dev -keyout "$tmp/key.pem" -out "$tmp/cert.pem" >/dev/null 2>&1
	cp "$tmp/cert.pem" "$tmp/ca.pem"
	sed -e "s#/var/log/caddy/#$tmp/#" -e "s#/etc/caddy/cloudflare-trusted-proxies.caddy#$tmp/cf.caddy#" \
		-e "s#/etc/caddy/origin/app.athenos.dev.pem#$tmp/cert.pem#" -e "s#/etc/caddy/origin/app.athenos.dev.key#$tmp/key.pem#" \
		-e "s#/etc/caddy/origin/cloudflare-origin-pull-ca.pem#$tmp/ca.pem#" "$here/caddy/Caddyfile" >"$tmp/Caddyfile"
	"$caddy_bin" validate --config "$tmp/Caddyfile" --adapter caddyfile >/dev/null 2>"$tmp/validate.log" ||
		err "caddy validate: $(tail -n 3 "$tmp/validate.log")"
	# The optional Authenticated Origin Pulls block must be valid too.
	perl -pe 'if (/^\s*# client_auth \{/ .. /^\s*# \}/) { s/^(\s*)# ?/$1/ }' "$tmp/Caddyfile" >"$tmp/Caddyfile.aop"
	grep -q '^[[:space:]]*mode require_and_verify' "$tmp/Caddyfile.aop" || err "could not enable the client_auth block for the check"
	"$caddy_bin" validate --config "$tmp/Caddyfile.aop" --adapter caddyfile >/dev/null 2>"$tmp/validate-aop.log" ||
		err "caddy validate (with client_auth): $(tail -n 3 "$tmp/validate-aop.log")"
	"$caddy_bin" fmt "$here/caddy/Caddyfile" >/dev/null 2>"$tmp/fmt.log" || err "caddy fmt: $(cat "$tmp/fmt.log")"
	echo "caddy: validate (also with client_auth) and fmt ok ($("$caddy_bin" version | cut -d' ' -f1))"
else
	echo "caddy: skipped (not installed)"
fi

# The console path for the first admin, against a stand-in of the API.
bash "$here/tests/bootstrap-admin-test.sh" >"${TMPDIR:-/tmp}/bootstrap-admin-test.$$.log" 2>&1 ||
	err "bootstrap-admin: $(tail -n 5 "${TMPDIR:-/tmp}/bootstrap-admin-test.$$.log")"
tail -n 1 "${TMPDIR:-/tmp}/bootstrap-admin-test.$$.log"
rm -f "${TMPDIR:-/tmp}/bootstrap-admin-test.$$.log"

[ "$fail" = 0 ] && echo "all checks passed"
exit "$fail"
