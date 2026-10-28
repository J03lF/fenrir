#!/usr/bin/env bash
# Restore an Athenos database dump into a FRESH database and check it.
#
#   restore.sh [options] <dump>
#
#   --target-db NAME   database to restore into (default athenos_restore_<UTC stamp>)
#   --force            drop an existing target database first. Also required
#                      together with --swap.
#   --swap             make the restored database the live one: stops
#                      athenos.service, renames the live database to
#                      <live>_before_restore_<stamp> (kept, not dropped),
#                      renames the target to <live>, starts the service and
#                      waits for /health/ready. Needs --force and either --yes
#                      or typing the live database name at the prompt.
#   --compare-db NAME  also compare a per-table checksum of every row with
#                      database NAME (only meaningful when NAME did not
#                      change since the dump, e.g. a restore drill).
#   --yes              no interactive confirmation for --swap.
#
# Checks, in this order, each one fatal: the .sha256 next to the dump, the
# archive table of contents, the restore itself (one transaction, stop on
# the first error), the row count of every table against the manifest
# written at backup time (<dump>.manifest.tsv), and with --compare-db the
# row checksums. Never touches the live database unless --swap --force.
#
# Run as root on the server (admin steps as `postgres`, peer auth). For a
# local drill: run as any user and set PGHOST/PGPORT/PG_ADMIN_USER to a
# throwaway superuser.
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "${ATHENOS_LIB_DIR:-$here}/common.sh"

load_env_file "${BACKUP_ENV_FILE:-/etc/athenos/backup.env}"
: "${PG_BIN_DIR:=/usr/lib/postgresql/16/bin}"
: "${PGHOST:=/var/run/postgresql}"
: "${PGDATABASE:=athenos}"
: "${DB_OWNER:=athenos}"
: "${PG_ADMIN_USER:=postgres}"
: "${ATHENOS_UNIT:=athenos.service}"
: "${HOST_READY_URL:=http://127.0.0.1:8080/health/ready}"
export PGHOST

target=""
force=0
swap=0
compare=""
yes=0
dump=""
while [ $# -gt 0 ]; do
	case $1 in
	--target-db)
		target=${2:?--target-db needs a name}
		shift 2
		;;
	--force)
		force=1
		shift
		;;
	--swap)
		swap=1
		shift
		;;
	--compare-db)
		compare=${2:?--compare-db needs a name}
		shift 2
		;;
	--yes)
		yes=1
		shift
		;;
	-h | --help)
		sed -n '2,30p' "$0"
		exit 0
		;;
	-*) die "unknown option: $1" ;;
	*)
		[ -z "$dump" ] || die "only one dump file"
		dump=$1
		shift
		;;
	esac
done
[ -n "$dump" ] || die "usage: restore.sh [options] <dump>"
[ -f "$dump" ] || die "no such file: $dump"
stamp=$(utc_stamp | tr 'TZ' 'tz')
live=$PGDATABASE
[ -n "$target" ] || target="${live}_restore_${stamp}"
valid_db_name "$target" || die "target database name must match ^[a-z_][a-z0-9_]*$: $target"
valid_db_name "$live" || die "PGDATABASE must be a plain lower-case name: $live"
[ -z "$compare" ] || valid_db_name "$compare" || die "--compare-db must be a plain lower-case name"
[ "$target" != "$live" ] || die "the target must not be the live database ($live); use --swap"
if [ "$swap" = 1 ] && [ "$force" != 1 ]; then
	die "--swap replaces the live database: add --force if you really mean it"
fi

pg_restore_bin="$PG_BIN_DIR/pg_restore"
psql_bin="$PG_BIN_DIR/psql"
[ -x "$pg_restore_bin" ] && [ -x "$psql_bin" ] || die "pg_restore/psql not found in $PG_BIN_DIR"

# As root, admin statements run as the postgres OS user (peer auth).
admin() {
	if [ "$(id -u)" = 0 ]; then
		runuser -u "$PG_ADMIN_USER" -- "$@"
	else
		PGUSER=$PG_ADMIN_USER "$@"
	fi
}
admin_psql() { admin "$psql_bin" -X -q -v ON_ERROR_STOP=1 --no-psqlrc -d postgres "$@"; }
db_psql() {
	local db=$1
	shift
	admin "$psql_bin" -X -q -A -t -v ON_ERROR_STOP=1 --no-psqlrc -d "$db" "$@"
}
db_exists() { [ "$(admin_psql -A -t -c "SELECT 1 FROM pg_database WHERE datname = '$1'")" = 1 ]; }

# 1. checksum
sums="$dump.sha256"
[ -f "$sums" ] || die "missing $sums (refusing to restore an unverified dump)"
(cd -- "$(dirname -- "$dump")" && sha256_check "$(basename -- "$sums")") || die "checksum mismatch for $dump"
log "checksum ok"

# 2. table of contents
"$pg_restore_bin" --list -- "$dump" >/dev/null || die "pg_restore cannot read $dump"
log "archive readable"

# 3. fresh target database
if db_exists "$target"; then
	[ "$force" = 1 ] || die "database $target exists; refusing to overwrite it without --force"
	log "dropping existing $target (--force)"
	admin_psql -c "DROP DATABASE \"$target\" WITH (FORCE)"
fi
collate=C
ctype=C
if db_exists "$live"; then
	collate=$(db_psql postgres -c "SELECT datcollate FROM pg_database WHERE datname = '$live'")
	ctype=$(db_psql postgres -c "SELECT datctype FROM pg_database WHERE datname = '$live'")
fi
log "creating $target (owner $DB_OWNER, collate $collate)"
admin_psql -c "CREATE DATABASE \"$target\" OWNER \"$DB_OWNER\" TEMPLATE template0 ENCODING 'UTF8' LC_COLLATE '$collate' LC_CTYPE '$ctype'"

# 4. restore: one transaction, first error aborts, objects owned by DB_OWNER
log "restoring $dump into $target"
if ! admin "$pg_restore_bin" --dbname="$target" --single-transaction --exit-on-error \
	--no-owner --role="$DB_OWNER" --no-privileges -- "$dump"; then
	die "restore failed; $target is left for inspection (drop it with --force on the next run)"
fi

# 5. row counts against the manifest
manifest="$dump.manifest.tsv"
[ -f "$manifest" ] || die "missing $manifest"
checked=0
failed=0
while IFS=$'\t' read -r table rows; do
	[ -n "$table" ] || continue
	got=$(db_psql "$target" -c "SELECT count(*) FROM $table")
	if [ "$got" != "$rows" ]; then
		warn "row count of $table: expected $rows, restored $got"
		failed=$((failed + 1))
	fi
	checked=$((checked + 1))
done <"$manifest"
[ "$failed" = 0 ] || die "$failed of $checked tables differ from the manifest"
log "row counts ok ($checked tables)"

# 6. optional row checksums against another database
if [ -n "$compare" ]; then
	db_exists "$compare" || die "--compare-db $compare does not exist"
	sumsql() {
		printf "SELECT md5(coalesce(string_agg(h, '' ORDER BY h), '')) FROM (SELECT md5(t::text) AS h FROM %s t) s" "$1"
	}
	diffs=0
	while IFS=$'\t' read -r table _; do
		[ -n "$table" ] || continue
		a=$(db_psql "$target" -c "$(sumsql "$table")")
		b=$(db_psql "$compare" -c "$(sumsql "$table")")
		if [ "$a" != "$b" ]; then
			warn "checksum of $table differs from $compare"
			diffs=$((diffs + 1))
		fi
	done <"$manifest"
	[ "$diffs" = 0 ] || die "$diffs tables differ from $compare"
	log "row checksums equal to $compare ($checked tables)"
fi

log "restore into $target verified"
[ "$swap" = 1 ] || {
	log "live database untouched. To use it: restore.sh --swap --force ... (or drop it: DROP DATABASE $target)"
	exit 0
}

# 7. swap (explicit, destructive for the live name; the old data is kept)
[ "$(id -u)" = 0 ] || die "--swap must run as root (it stops and starts $ATHENOS_UNIT)"
old="${live}_before_restore_${stamp}"
cat >&2 <<EOF

  WARNING: --swap replaces the live database "$live" with "$target".
  Everything written since the dump is no longer live (it stays in "$old").
  $ATHENOS_UNIT is stopped during the switch.

EOF
if [ "$yes" != 1 ]; then
	read -r -p "Type the live database name ($live) to continue: " answer
	[ "$answer" = "$live" ] || die "aborted"
fi
log "stopping $ATHENOS_UNIT"
systemctl stop "$ATHENOS_UNIT"
if db_exists "$live"; then
	admin_psql -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname IN ('$live', '$target') AND pid <> pg_backend_pid()" >/dev/null
	admin_psql -c "ALTER DATABASE \"$live\" RENAME TO \"$old\""
	log "live database kept as $old"
fi
admin_psql -c "ALTER DATABASE \"$target\" RENAME TO \"$live\""
log "starting $ATHENOS_UNIT"
systemctl start "$ATHENOS_UNIT"
for _ in $(seq 1 90); do
	if curl -fsS -m 3 -o /dev/null -- "$HOST_READY_URL"; then
		log "host ready on the restored database. Drop $old once you are sure."
		exit 0
	fi
	sleep 2
done
die "host not ready after the swap; check journalctl -u $ATHENOS_UNIT. The previous data is in $old"
