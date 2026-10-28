#!/usr/bin/env bash
# The admin's way into the beta app past Caddy - while the maintenance page
# is on, or to look at the app without Cloudflare - from the Mac:
#
#   infra/beta/admin-tunnel.sh [--port N] [--host HOST]
#
# Opens an SSH tunnel localhost:<port> -> 127.0.0.1:41002 on the server (the
# host's static edge, the same upstream Caddy uses) and prints the address.
# Nothing is opened to the internet: the edge listens on loopback only, the
# way in is your SSH key. Ctrl-C closes the tunnel.
#
# Shortcut (RUNBOOK 5.4), in your ~/.zshrc:
#   frprodapp() { /opt/fenrir/development/fenrir/infra/beta/admin-tunnel.sh "$@"; }
#
# Defaults: port 41902 (not 41002: a local Fenrir may use that one on the
# Mac), host DEPLOY_HOST from ~/.config/athenos/deploy.env, else `athenos`.
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=server/common.sh
. "$here/server/common.sh"
load_env_file "${DEPLOY_ENV_FILE:-$HOME/.config/athenos/deploy.env}"

port=${ATHENOS_TUNNEL_PORT:-41902}
host=${ATHENOS_TUNNEL_HOST:-${DEPLOY_HOST:-athenos}}
while [ $# -gt 0 ]; do
	case $1 in
	--port)
		port=${2:?--port needs a number}
		shift 2
		;;
	--host)
		host=${2:?--host needs an SSH destination}
		shift 2
		;;
	-h | --help)
		sed -n '2,16p' "$0"
		exit 0
		;;
	*) die "unknown argument: $1" ;;
	esac
done
[[ $port =~ ^[0-9]{4,5}$ ]] && [ "$port" -ge 1024 ] && [ "$port" -le 65535 ] || die "port 1024-65535, not $port"
if command -v lsof >/dev/null 2>&1 && lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
	die "port $port is in use on this Mac (a tunnel already open?): --port <other>"
fi

url="http://localhost:$port/"
cat >&2 <<EOF

  Athenos (beta) through SSH, past Caddy and Cloudflare:

      $url

  - Sign in there separately: the sign-in lives per address, the session of
    app.athenos.dev does not carry over (and is not touched).
  - Use Chrome, Firefox or Safari; http://localhost counts as secure.
  - Links in mails still point to https://app.athenos.dev.
  - Ctrl-C closes the tunnel.

EOF
exec ssh -N -o ExitOnForwardFailure=yes -o ServerAliveInterval=30 \
	-L "127.0.0.1:$port:127.0.0.1:41002" -- "$host"
