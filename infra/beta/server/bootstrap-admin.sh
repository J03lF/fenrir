#!/usr/bin/env bash
# Fallback for the first Athenos administrator on the server, when the web
# page https://app.athenos.dev/setup cannot be used. Run as root ON the server:
#
#   sudo /usr/local/lib/athenos/bootstrap-admin.sh <email> "<display name>"
#
# Reads the one-time setup token the core writes while no active user exists
# (SPEC §6), asks for the password twice (hidden), registers with the token
# as api_key and the current legal version (SPEC §5), then asks for the PIN
# from the registration mail. Token and password only go through a 0600 temp
# file to curl, never through argv or the environment of other processes.
set -euo pipefail

email=${1:?usage: bootstrap-admin.sh <email> "<display name>"}
name=${2:?usage: bootstrap-admin.sh <email> "<display name>"}
TOKEN_FILE=${ATHENOS_SETUP_TOKEN_FILE:-/var/lib/athenos/modules-runtime/secrets/athenos-setup-token}
BASE=${ATHENOS_BASE:-http://127.0.0.1:41002/gateway/services/module%3Aathenos-api%3A%3Aapi-gateway/api/v1}

[ -r "$TOKEN_FILE" ] || {
	echo "no setup token at $TOKEN_FILE (is there already an active user? see the core log: journalctl -u athenos | grep setup_token)" >&2
	exit 1
}
command -v jq >/dev/null || {
	echo "jq missing (apt install jq)" >&2
	exit 1
}

legal=${ATHENOS_LEGAL_VERSION:-$(curl -fsS -m 10 "$BASE/public/site" | jq -r '.data.legal.version // empty' || true)}
[ -n "$legal" ] || {
	echo "could not read the legal version from $BASE/public/site; set ATHENOS_LEGAL_VERSION" >&2
	exit 1
}
echo "Legal version to accept: $legal (Nutzungsbedingungen und Datenschutzerklärung, see /nutzungsbedingungen and /datenschutz)"

read -r -s -p "Password for $email: " pw
echo
read -r -s -p "Again: " pw2
echo
[ "$pw" = "$pw2" ] || {
	echo "passwords differ" >&2
	exit 1
}

tmp=$(mktemp)
chmod 600 "$tmp"
trap 'rm -f -- "$tmp"' EXIT
jq -n --arg e "$email" --arg n "$name" --arg p "$pw" --arg l "$legal" --rawfile k "$TOKEN_FILE" \
	'{email: $e, display_name: $n, password: $p, accepted_legal_version: $l, api_key: ($k | rtrimstr("\n"))}' >"$tmp"
unset pw pw2

resp=$(curl -fsS -m 20 -X POST -H 'content-type: application/json' --data-binary @"$tmp" "$BASE/auth/register/request")
cid=$(printf '%s' "$resp" | jq -r '.data.challenge_id // .challenge_id // empty')
[ -n "$cid" ] || {
	echo "no challenge in the answer" >&2
	exit 1
}
echo "A PIN was sent to $email (valid for a few minutes)."
read -r -p "PIN: " pin
jq -n --arg c "$cid" --arg p "$pin" '{challenge_id: $c, pin: $p}' >"$tmp"
curl -fsS -m 20 -X POST -H 'content-type: application/json' --data-binary @"$tmp" "$BASE/auth/register/confirm" >/dev/null
echo "Administrator $email is active. Sign in at https://app.athenos.dev/auth/login"
