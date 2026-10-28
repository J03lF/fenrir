#!/usr/bin/env bash
# Manage the secret files of the Athenos beta (run as root).
#
#   secret.sh generate <name> [--force]   random 48-byte token (base64url)
#   secret.sh set <name>                  read the value from the terminal (hidden)
#   secret.sh list                        names, sizes, modes (never values)
#
# Files live in /etc/athenos/secrets (0700 athenos:athenos), each 0600
# athenos:athenos. Names: [a-z0-9._-]. After a change:
#   sudo systemctl restart athenos
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "${ATHENOS_LIB_DIR:-$here}/common.sh"

: "${SECRETS_DIR:=/etc/athenos/secrets}"
: "${SERVICE_USER:=athenos}"

[ "$(id -u)" = 0 ] || die "run as root (sudo)"
cmd=${1:-}
name=${2:-}

ensure_dir() {
	install -d -m 0700 -o "$SERVICE_USER" -g "$SERVICE_USER" -- "$SECRETS_DIR"
}
check_name() {
	[[ $name =~ ^[a-z0-9][a-z0-9._-]{0,63}$ ]] || die "secret name must match [a-z0-9._-]: '$name'"
}
write_secret() { # value on stdin
	local dest="$SECRETS_DIR/$name" tmp
	tmp=$(mktemp "$SECRETS_DIR/.$name.XXXXXX")
	cat >"$tmp"
	[ -s "$tmp" ] || {
		rm -f -- "$tmp"
		die "empty value, nothing written"
	}
	chown "$SERVICE_USER:$SERVICE_USER" "$tmp"
	chmod 0600 "$tmp"
	mv -f -- "$tmp" "$dest"
	log "wrote $dest ($(wc -c <"$dest" | tr -d ' ') bytes)"
}

case $cmd in
generate)
	check_name
	ensure_dir
	if [ -e "$SECRETS_DIR/$name" ] && [ "${3:-}" != --force ]; then
		log "$SECRETS_DIR/$name exists, kept (use --force to rotate)"
		exit 0
	fi
	umask 077
	openssl rand -base64 48 | tr '+/' '-_' | tr -d '=\n' | write_secret
	;;
set)
	check_name
	ensure_dir
	[ -t 0 ] || die "set needs a terminal (the value is read hidden)"
	read -r -s -p "Value for $name: " v1
	echo >&2
	read -r -s -p "Again: " v2
	echo >&2
	[ "$v1" = "$v2" ] || die "values differ"
	umask 077
	printf '%s' "$v1" | write_secret
	unset v1 v2
	;;
list)
	ensure_dir
	find "$SECRETS_DIR" -maxdepth 1 -type f ! -name '.*' -exec stat -c '%a %U:%G %s bytes  %n' {} \;
	;;
*) die "usage: secret.sh generate <name> [--force] | set <name> | list" ;;
esac
