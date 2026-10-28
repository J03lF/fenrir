#!/usr/bin/env bash
# Roll the beta server back to an earlier release, from the Mac.
#
#   infra/beta/rollback.sh [--host HOST] --list
#   infra/beta/rollback.sh [--host HOST] [<release-id>]
#   infra/beta/rollback.sh [--host HOST] [<release-id>] --with-db <dump-on-server>
#
# Without an id: back to the release that was current before the last
# deploy. Only the code is switched (symlink + restart + health check); the
# database stays as it is, which is right as long as the migrations of the
# newer release were additive (the convention: 0002_*.sql and later only add).
# --with-db additionally restores a dump from /var/backups/athenos over the
# live database (restore.sh --swap --force; the current database is kept
# under a new name). The server asks you to type ROLLBACK for that.
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=server/common.sh
. "$here/server/common.sh"
load_env_file "${DEPLOY_ENV_FILE:-$HOME/.config/athenos/deploy.env}"
: "${DEPLOY_HOST:=}"

args=()
list=0
while [ $# -gt 0 ]; do
	case $1 in
	--host)
		DEPLOY_HOST=${2:?--host needs a value}
		shift 2
		;;
	--list)
		list=1
		shift
		;;
	--with-db)
		dump=${2:?--with-db needs the dump path on the server}
		[[ $dump =~ ^/var/backups/athenos/athenos-[0-9]{8}T[0-9]{6}Z[A-Za-z0-9._-]*\.dump$ ]] ||
			die "--with-db expects /var/backups/athenos/athenos-<stamp>[-label].dump"
		args+=(--with-db "$dump")
		shift 2
		;;
	-h | --help)
		sed -n '2,15p' "$0"
		exit 0
		;;
	-*) die "unknown option: $1" ;;
	*)
		[[ $1 =~ ^[A-Za-z0-9._+-]{1,80}$ ]] || die "invalid release id: $1"
		args=("$1" ${args[@]+"${args[@]}"})
		shift
		;;
	esac
done
[ -n "$DEPLOY_HOST" ] || die "no host: set DEPLOY_HOST in deploy.env or pass --host"

tool=/usr/local/lib/athenos/athenos-release.sh
if [ "$list" = 1 ]; then
	ssh -t -- "$DEPLOY_HOST" "sudo $tool list; echo; echo 'database dumps:'; sudo ls -1t /var/backups/athenos | grep '\.dump\$' | head -n 20"
	exit 0
fi
# shellcheck disable=SC2029 # the arguments are meant to expand here
ssh -t -- "$DEPLOY_HOST" "sudo $tool rollback ${args[*]+${args[*]}}"
