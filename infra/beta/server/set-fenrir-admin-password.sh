#!/usr/bin/env bash
# Set (or reset) the password of the Fenrir host admin (the SSH shell on
# 127.0.0.1:2222, not an Athenos account). Run as root, then restart:
#
#   sudo /usr/local/lib/athenos/set-fenrir-admin-password.sh
#   sudo systemctl restart athenos
#
# The host hashes the pending password on its next start and deletes the
# file at once (same mechanism as scripts/fenrir-setup.sh).
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "${ATHENOS_LIB_DIR:-$here}/common.sh"

: "${FENRIR_RUNTIME_DIR:=/var/lib/athenos/runtime}"
: "${SERVICE_USER:=athenos}"
[ "$(id -u)" = 0 ] || die "run as root (sudo)"
[ -t 0 ] || die "needs a terminal"

identity="$FENRIR_RUNTIME_DIR/identity"
install -d -m 0700 -o "$SERVICE_USER" -g "$SERVICE_USER" -- "$identity"

echo "Fenrir admin password (min. 12 characters, upper and lower case, a digit):" >&2
read -r -s -p "New password: " p1
echo >&2
read -r -s -p "Again: " p2
echo >&2
[ "$p1" = "$p2" ] || die "passwords differ"
[ "${#p1}" -ge 12 ] || die "at least 12 characters"

umask 077
tmp=$(mktemp "$identity/.pending.XXXXXX")
printf '%s\n' "$p1" >"$tmp"
unset p1 p2
chown "$SERVICE_USER:$SERVICE_USER" "$tmp"
chmod 0600 "$tmp"
mv -f -- "$tmp" "$identity/.pending-password"
rm -f -- "$identity/.db-password-set"
log "pending password stored; restart athenos to apply it"
