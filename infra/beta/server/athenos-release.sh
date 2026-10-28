#!/usr/bin/env bash
# Server side of deploy.sh / rollback.sh (run as root on the server).
#
#   athenos-release.sh deploy <upload-dir> <release-id> [--restore-db-on-failure]
#   athenos-release.sh install <upload-dir> <release-id>
#   athenos-release.sh activate <release-id> [--restore-db-on-failure]
#   athenos-release.sh rollback [<release-id>] [--with-db <dump>] [--yes]
#   athenos-release.sh list | current | prune
#
# install   verifies SHA256SUMS of the upload, checks every tarball (no
#           absolute paths, no "..", no links, one top directory per
#           component), unpacks into /opt/athenos/releases/<id>, checks the
#           per-component FILES.SHA256SUMS and the expected layout.
# activate  takes a database dump (abort if it fails), switches
#           /opt/athenos/current atomically, restarts athenos.service and waits
#           for health. Unhealthy: switches back to the previous release and
#           restarts it. The database is only restored from the pre-deploy
#           dump with --restore-db-on-failure (that discards what was written
#           after the dump; it asks no question, the flag is the consent).
# rollback  switches to the previous (or the given) release; the database
#           stays as it is unless --with-db <dump> (restore.sh --swap --force).
# prune     keeps the newest $KEEP_RELEASES releases, never current/previous.
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
LIB=${ATHENOS_LIB_DIR:-/usr/local/lib/athenos}
[ -r "$LIB/common.sh" ] || LIB=$here
# shellcheck source=common.sh
. "$LIB/common.sh"

: "${ATHENOS_ROOT:=/opt/athenos}"
: "${KEEP_RELEASES:=5}"
: "${SERVICE_USER:=athenos}"
: "${ATHENOS_UNIT:=athenos.service}"
: "${HEALTH_TIMEOUT_SECONDS:=240}"
: "${HOST_URL:=http://127.0.0.1:8080}"
: "${EDGE_URL:=http://127.0.0.1:41002}"
: "${API_SERVICE_PATH:=/gateway/services/module%3Aathenos-api%3A%3Aapi-gateway}"
: "${MODULES_RUNTIME:=/var/lib/athenos/modules-runtime}"
: "${DEPLOY_LOG:=/var/log/athenos-deploy.log}"
RELEASES="$ATHENOS_ROOT/releases"
CURRENT="$ATHENOS_ROOT/current"
PREVIOUS_FILE="$ATHENOS_ROOT/.previous-release"

RUST_MODULES="athenos athenos-api auth-service notification-service"
COMPONENTS="fenrir $RUST_MODULES athenos-web"

[ "$(id -u)" = 0 ] || die "run as root (sudo)"

record() {
	mkdir -p -- "$(dirname -- "$DEPLOY_LOG")"
	printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" >>"$DEPLOY_LOG"
	log "$*"
}

valid_id() { [[ $1 =~ ^[A-Za-z0-9._+-]{1,80}$ ]] && [ "$1" != . ] && [ "$1" != .. ]; }

current_id() {
	local target
	target=$(readlink -- "$CURRENT" 2>/dev/null) || return 1
	basename -- "$target"
}

# tarball_of DIR COMPONENT -> the one tarball of that component
tarball_of() {
	local dir=$1 comp=$2 f found=""
	for f in "$dir"/"$comp"-[0-9]*.tar.gz; do
		[ -e "$f" ] || continue
		[ -z "$found" ] || die "more than one tarball for $comp in $dir"
		found=$f
	done
	[ -n "$found" ] || die "no tarball for $comp in $dir (expected $comp-<version>-*.tar.gz)"
	printf '%s\n' "$found"
}

# check_tarball FILE TOPDIR: refuse anything that could escape or surprise.
check_tarball() {
	local file=$1 top=$2 bad
	bad=$(tar -tzvf "$file" | awk -v top="$top" '
		{ type = substr($1, 1, 1); name = $NF }
		type != "-" && type != "d" { print "link or special file: " $0; next }
		name ~ /^\// { print "absolute path: " name; next }
		name ~ /(^|\/)\.\.(\/|$)/ { print "parent path: " name; next }
		name != top && name != top "/" && index(name, top "/") != 1 { print "outside " top "/: " name }
	')
	[ -z "$bad" ] || die "unsafe tarball $(basename -- "$file"): $(printf '%s\n' "$bad" | head -n 5)"
}

unpack() { # file top dest
	check_tarball "$1" "$2"
	mkdir -p -- "$3"
	tar -xzf "$1" -C "$3" --strip-components=1 --no-same-owner --no-overwrite-dir
}

verify_layout() {
	local r=$1 m
	[ -x "$r/fenrir" ] && [ -x "$r/fenrir-module-kit" ] || die "fenrir or fenrir-module-kit missing in $r"
	[ -f "$r/config/default.toml" ] && [ -f "$r/config/beta.toml" ] || die "config/default.toml or config/beta.toml missing"
	# The module verifier needs a keyring even without signatures (an older
	# host bundle lacked it and the host refused to start).
	[ -f "$r/config/modules/keyring.json" ] || die "config/modules/keyring.json missing (host bundle too old: package it with ci/package-host.sh of this revision)"
	[ -d "$r/infra/db/migrations/postgres" ] || die "host migrations missing"
	for f in bin/.fenrir-profile config/local.toml config/dev.toml secrets/.env; do
		[ ! -e "$r/$f" ] || die "release contains $f (developer settings must not ship)"
	done
	for m in $RUST_MODULES; do
		[ -x "$r/modules/$m/$m" ] || die "binary modules/$m/$m missing"
		[ "$(head -c 4 -- "$r/modules/$m/$m" | tail -c 3)" = ELF ] || die "modules/$m/$m is not an ELF binary"
	done
	for m in $RUST_MODULES athenos-web; do
		[ -f "$r/modules/$m/.fenrir-meta/manifest.json" ] || die "modules/$m/.fenrir-meta/manifest.json missing"
		[ -f "$r/modules/$m/.fenrir/runtime.toml" ] || die "modules/$m/.fenrir/runtime.toml missing"
	done
	[ -f "$r/modules/athenos-web/dist/browser/index.html" ] || die "athenos-web dist/browser/index.html missing"
	for m in $COMPONENTS; do
		local d="$r"
		[ "$m" = fenrir ] || d="$r/modules/$m"
		[ -f "$d/FILES.SHA256SUMS" ] || die "$m has no FILES.SHA256SUMS"
		(cd -- "$d" && sha256_check FILES.SHA256SUMS) || die "$m: files differ from FILES.SHA256SUMS"
	done
}

cmd_install() {
	local upload=$1 id=$2
	valid_id "$id" || die "invalid release id: $id"
	[ -d "$upload" ] || die "no upload directory $upload"
	local dest="$RELEASES/$id" tmp="$RELEASES/.$id.partial"
	[ ! -e "$dest" ] || die "release $id exists already"
	[ -f "$upload/SHA256SUMS" ] || die "$upload/SHA256SUMS missing"
	(cd -- "$upload" && sha256_check SHA256SUMS) || die "upload does not match SHA256SUMS"
	local comp f
	for comp in $COMPONENTS; do
		f=$(tarball_of "$upload" "$comp")
		grep -q " $(basename -- "$f")\$" "$upload/SHA256SUMS" || die "$(basename -- "$f") is not listed in SHA256SUMS"
	done
	log "upload verified"

	rm -rf -- "$tmp"
	mkdir -p -- "$tmp/modules"
	unpack "$(tarball_of "$upload" fenrir)" fenrir "$tmp"
	for comp in $RUST_MODULES athenos-web; do
		unpack "$(tarball_of "$upload" "$comp")" "$comp" "$tmp/modules/$comp"
	done
	verify_layout "$tmp"

	{
		printf '{\n  "id": "%s",\n  "installed_at": "%s",\n  "tarballs": {\n' "$id" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
		local first=1 line
		while IFS= read -r line; do
			[ -n "$line" ] || continue
			[ $first = 1 ] || printf ',\n'
			first=0
			printf '    "%s": "%s"' "${line##* }" "${line%% *}"
		done <"$upload/SHA256SUMS"
		printf '\n  }\n}\n'
	} >"$tmp/RELEASE.json"

	chown -R root:root -- "$tmp"
	chmod -R go-w -- "$tmp"
	# The host writes <module>/.fenrir/runtime.json before each start.
	for comp in $RUST_MODULES athenos-web; do
		chown "$SERVICE_USER:$SERVICE_USER" -- "$tmp/modules/$comp/.fenrir" "$tmp/modules/$comp/.fenrir-meta"
		chmod 0750 -- "$tmp/modules/$comp/.fenrir" "$tmp/modules/$comp/.fenrir-meta"
	done
	chmod 0755 -- "$tmp"
	# Persistent module runtime (ports, logs, module secrets, runtime state);
	# linked only after the chown/chmod above, which must not reach it.
	install -d -m 0750 -o "$SERVICE_USER" -g "$SERVICE_USER" -- "$MODULES_RUNTIME"
	ln -s -- "$MODULES_RUNTIME" "$tmp/modules/runtime"
	mv -- "$tmp" "$dest"
	record "installed release $id"
}

switch_to() { # release id
	local id=$1
	[ -d "$RELEASES/$id" ] || die "no release $id"
	ln -sfn -- "releases/$id" "$ATHENOS_ROOT/.current.new"
	mv -T -- "$ATHENOS_ROOT/.current.new" "$CURRENT"
}

sync_tools() { # release id: server scripts of that release -> /usr/local/lib/athenos
	local src="$RELEASES/$1/infra/beta/server" f
	[ -d "$src" ] || return 0
	install -d -m 0755 -- /usr/local/lib/athenos
	for f in "$src"/*.sh; do
		install -m 0755 -o root -g root -- "$f" /usr/local/lib/athenos/
	done
	local unit changed=""
	for unit in "$RELEASES/$1"/infra/beta/systemd/*; do
		[ -f "$unit" ] || continue
		cmp -s -- "$unit" "/etc/systemd/system/$(basename -- "$unit")" || changed="$changed $(basename -- "$unit")"
	done
	[ -z "$changed" ] || warn "systemd units differ from the release:$changed (review them, then: sudo /opt/athenos/current/infra/beta/server/install.sh units)"
}

wait_healthy() {
	local deadline=$(($(date -u +%s) + HEALTH_TIMEOUT_SECONDS)) overall
	while [ "$(date -u +%s)" -lt "$deadline" ]; do
		sleep 3
		systemctl is-active --quiet -- "$ATHENOS_UNIT" || continue
		curl -fsS -m 5 -o /dev/null -- "$HOST_URL/health/ready" || continue
		curl -fsS -m 5 -o /dev/null -- "$EDGE_URL/healthz" || continue
		curl -fsS -m 5 -- "$EDGE_URL$API_SERVICE_PATH/health" 2>/dev/null | grep -q '"status":"healthy"' || continue
		overall=$(curl -fsS -m 5 -- "$HOST_URL/public/status" 2>/dev/null | jq -r '.overall // empty' 2>/dev/null || true)
		[ "$overall" = operational ] || [ "$overall" = degraded ] || continue
		log "healthy (host ready, edge, API, status $overall)"
		return 0
	done
	return 1
}

restart_and_wait() {
	systemctl restart -- "$ATHENOS_UNIT"
	wait_healthy
}

predeploy_dump() { # id -> path of the dump on stdout
	local db_exists
	db_exists=$(runuser -u postgres -- psql -X -A -t -d postgres -c "SELECT 1 FROM pg_database WHERE datname = 'athenos'" 2>/dev/null || true)
	if [ "$db_exists" != 1 ]; then
		warn "no database athenos yet (first deploy): no pre-deploy dump"
		return 0
	fi
	local label="predeploy-${1//[^A-Za-z0-9._-]/_}"
	runuser -u "$SERVICE_USER" -- env ATHENOS_LIB_DIR="$LIB" "$LIB/backup.sh" --label "${label:0:64}" --no-offsite ||
		die "pre-deploy dump failed: nothing changed"
}

cmd_activate() {
	local id=$1 restore_on_failure=$2
	[ -d "$RELEASES/$id" ] || die "no release $id"
	local prev
	prev=$(current_id || true)
	[ "$prev" != "$id" ] || die "release $id is already current"

	local dump
	dump=$(predeploy_dump "$id" | tail -n 1)
	[ -z "$dump" ] || record "pre-deploy dump $dump"

	[ -z "$prev" ] || printf '%s\n' "$prev" >"$PREVIOUS_FILE"
	switch_to "$id"
	sync_tools "$id"
	record "switched current to $id (previous: ${prev:-none}); restarting"
	if restart_and_wait; then
		record "deploy of $id healthy"
		cmd_prune
		return 0
	fi

	record "deploy of $id NOT healthy after ${HEALTH_TIMEOUT_SECONDS}s"
	journalctl --no-pager --lines=60 --unit="$ATHENOS_UNIT" >&2 || true
	if [ -z "$prev" ]; then
		die "first deploy failed and there is no previous release; current stays at $id"
	fi
	switch_to "$prev"
	sync_tools "$prev"
	if [ "$restore_on_failure" = 1 ] && [ -n "$dump" ]; then
		warn "restoring the database from $dump (--restore-db-on-failure)"
		systemctl stop -- "$ATHENOS_UNIT"
		"$LIB/restore.sh" --swap --force --yes "$dump" || warn "database restore failed; see above"
	else
		[ -z "$dump" ] || log "database not restored (no --restore-db-on-failure); the dump is $dump"
	fi
	if restart_and_wait; then
		record "rolled back to $prev (healthy)"
	else
		record "rolled back to $prev but it is NOT healthy either"
	fi
	die "deploy of $id failed"
}

cmd_rollback() {
	local target=${1:-} with_db=${2:-} yes=${3:-0}
	local cur
	cur=$(current_id || true)
	if [ -z "$target" ]; then
		[ -r "$PREVIOUS_FILE" ] || die "no previous release recorded; name one (athenos-release.sh list)"
		target=$(cat -- "$PREVIOUS_FILE")
	fi
	valid_id "$target" || die "invalid release id: $target"
	[ -d "$RELEASES/$target" ] || die "no release $target"
	[ "$target" != "$cur" ] || die "$target is already current"
	if [ -n "$with_db" ]; then
		cat >&2 <<EOF

  WARNING: rollback --with-db restores $with_db over the live database.
  Everything written after that dump is no longer live (the current
  database is kept under a new name by restore.sh).

EOF
		if [ "$yes" != 1 ]; then
			read -r -p "Type ROLLBACK to continue: " answer
			[ "$answer" = ROLLBACK ] || die "aborted"
		fi
	fi
	[ -z "$cur" ] || printf '%s\n' "$cur" >"$PREVIOUS_FILE"
	switch_to "$target"
	sync_tools "$target"
	record "rollback: current $target (was ${cur:-none})"
	if [ -n "$with_db" ]; then
		systemctl stop -- "$ATHENOS_UNIT"
		"$LIB/restore.sh" --swap --force --yes "$with_db"
	fi
	if restart_and_wait; then
		record "rollback to $target healthy"
	else
		die "rollback to $target is not healthy; journalctl -u $ATHENOS_UNIT"
	fi
}

cmd_list() {
	local cur prev d
	cur=$(current_id || true)
	prev=$(cat -- "$PREVIOUS_FILE" 2>/dev/null || true)
	for d in $(find "$RELEASES" -mindepth 1 -maxdepth 1 -type d ! -name '.*' -exec basename {} \; | LC_ALL=C sort); do
		local mark="  "
		[ "$d" != "$cur" ] || mark="* "
		[ "$d" != "$prev" ] || [ "$d" = "$cur" ] || mark="p "
		printf '%s%s\n' "$mark" "$d"
	done
	echo "(* current, p previous)"
}

cmd_prune() {
	local cur prev d n=0
	cur=$(current_id || true)
	prev=$(cat -- "$PREVIOUS_FILE" 2>/dev/null || true)
	# newest first by install time
	for d in $(ls -1t -- "$RELEASES" 2>/dev/null); do
		[ -d "$RELEASES/$d" ] || continue
		case $d in .*) continue ;; esac
		n=$((n + 1))
		[ "$n" -gt "$KEEP_RELEASES" ] || continue
		[ "$d" != "$cur" ] && [ "$d" != "$prev" ] || continue
		rm -rf -- "${RELEASES:?}/$d"
		record "pruned release $d"
	done
}

sub=${1:-}
shift || true
case $sub in
deploy)
	upload=${1:?upload dir}
	id=${2:?release id}
	flag=0
	[ "${3:-}" != --restore-db-on-failure ] || flag=1
	with_lock release cmd_install "$upload" "$id"
	with_lock release cmd_activate "$id" "$flag"
	;;
install) with_lock release cmd_install "${1:?upload dir}" "${2:?release id}" ;;
activate)
	flag=0
	[ "${2:-}" != --restore-db-on-failure ] || flag=1
	with_lock release cmd_activate "${1:?release id}" "$flag"
	;;
rollback)
	target=""
	with_db=""
	yes=0
	while [ $# -gt 0 ]; do
		case $1 in
		--with-db)
			with_db=${2:?--with-db needs a dump file}
			shift 2
			;;
		--yes)
			yes=1
			shift
			;;
		-*) die "unknown option $1" ;;
		*)
			target=$1
			shift
			;;
		esac
	done
	with_lock release cmd_rollback "$target" "$with_db" "$yes"
	;;
list) cmd_list ;;
current) current_id || echo "none" ;;
prune) with_lock release cmd_prune ;;
*)
	sed -n '2,24p' "$0"
	exit 1
	;;
esac
