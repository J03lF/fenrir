#!/usr/bin/env bash
# Nightly database backup of the Athenos beta (athenos-backup.service).
#
#   backup.sh [--label NAME] [--no-offsite] [--no-rotate]
#
# 1. pg_dump -Fc of $PGDATABASE into $BACKUP_DIR (via a .partial file),
#    file name athenos-<UTC stamp>[-<label>].dump, mode 0600.
# 2. <dump>.sha256 (sha256sum format) and <dump>.manifest.tsv (rows per
#    table, read back from the dump itself, so it always matches the dump).
# 3. Rotation: dumps older than $BACKUP_RETENTION_DAYS (14) days go, the
#    newest $BACKUP_MIN_KEEP (3) always stay.
# 4. Off-site mirror of the same retention set (rsync over SSH or rclone),
#    optionally encrypted with age. Optional heartbeat ping on success.
# Exit code != 0 on any failure; systemd then runs athenos-alert@.service.
#
# Settings: environment, else /etc/athenos/backup.env (KEY=VALUE).
# Connects as the OS user `athenos` over the Unix socket (peer auth).
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "${ATHENOS_LIB_DIR:-$here}/common.sh"

load_env_file "${BACKUP_ENV_FILE:-/etc/athenos/backup.env}"

: "${BACKUP_DIR:=/var/backups/athenos}"
: "${BACKUP_RETENTION_DAYS:=14}"
: "${BACKUP_MIN_KEEP:=3}"
: "${BACKUP_MIN_FREE_MB:=1024}"
: "${PG_BIN_DIR:=/usr/lib/postgresql/16/bin}"
: "${PGDATABASE:=athenos}"
: "${PGHOST:=/var/run/postgresql}"
: "${OFFSITE_MODE:=none}"
export PGDATABASE PGHOST

label=""
offsite=1
rotate=1
while [ $# -gt 0 ]; do
	case $1 in
	--label)
		label=${2:?--label needs a value}
		shift 2
		;;
	--no-offsite)
		offsite=0
		shift
		;;
	--no-rotate)
		rotate=0
		shift
		;;
	-h | --help)
		sed -n '2,20p' "$0"
		exit 0
		;;
	*) die "unknown argument: $1" ;;
	esac
done
[ -z "$label" ] || [[ $label =~ ^[A-Za-z0-9._-]{1,64}$ ]] || die "label may only use A-Z a-z 0-9 . _ -"
[[ $BACKUP_RETENTION_DAYS =~ ^[0-9]+$ ]] && [ "$BACKUP_RETENTION_DAYS" -ge 1 ] || die "BACKUP_RETENTION_DAYS must be >= 1"
[[ $BACKUP_MIN_KEEP =~ ^[0-9]+$ ]] || die "BACKUP_MIN_KEEP must be a number"

pg_dump_bin="$PG_BIN_DIR/pg_dump"
pg_restore_bin="$PG_BIN_DIR/pg_restore"
[ -x "$pg_dump_bin" ] || die "pg_dump not found at $pg_dump_bin (PG_BIN_DIR)"

# manifest_of DUMP -> "<table>\t<rows>" lines, from the COPY blocks of the dump.
manifest_of() {
	"$pg_restore_bin" --data-only --file=- -- "$1" | awk '
		/^COPY .* FROM stdin;$/ { table = $2; rows = 0; inside = 1; next }
		inside && $0 == "\\." { printf "%s\t%d\n", table, rows; inside = 0; next }
		inside { rows++ }
	' | LC_ALL=C sort
}

dump_once() {
	umask 077
	mkdir -p -- "$BACKUP_DIR"
	chmod 0700 "$BACKUP_DIR"

	local free
	free=$(free_mb "$BACKUP_DIR")
	[ "$free" -ge "$BACKUP_MIN_FREE_MB" ] || die "only ${free} MB free in $BACKUP_DIR (need $BACKUP_MIN_FREE_MB)"

	local stamp name
	stamp=$(utc_stamp)
	name="athenos-$stamp${label:+-$label}.dump"
	local final="$BACKUP_DIR/$name" partial="$BACKUP_DIR/.$name.partial"
	[ ! -e "$final" ] || die "$final exists already"

	log "dumping database $PGDATABASE to $final"
	rm -f -- "$partial"
	if ! "$pg_dump_bin" --format=custom --compress=6 --lock-wait-timeout=60000 \
		--file="$partial" --dbname="$PGDATABASE"; then
		rm -f -- "$partial"
		die "pg_dump failed"
	fi
	# The archive must be readable as a whole before it counts as a backup.
	"$pg_restore_bin" --list -- "$partial" >/dev/null || {
		rm -f -- "$partial"
		die "pg_restore cannot read the new dump"
	}
	mv -- "$partial" "$final"
	chmod 0600 "$final"

	(cd -- "$BACKUP_DIR" && printf '%s  %s\n' "$(sha256_of "$name")" "$name" >"$name.sha256")
	manifest_of "$final" >"$final.manifest.tsv"
	chmod 0600 "$final.sha256" "$final.manifest.tsv"

	local tables rows size
	tables=$(wc -l <"$final.manifest.tsv" | tr -d ' ')
	rows=$(awk -F '\t' '{s += $2} END {print s + 0}' "$final.manifest.tsv")
	size=$(wc -c <"$final" | tr -d ' ')
	log "backup ok: $name ($size bytes, $tables tables, $rows rows)"
	printf '%s\n' "$final"
}

# rotate_local: delete dumps (and their side files) older than the retention,
# never the newest $BACKUP_MIN_KEEP.
rotate_local() {
	local now cutoff keep=$BACKUP_MIN_KEEP
	now=$(date -u +%s)
	cutoff=$((now - BACKUP_RETENTION_DAYS * 86400))
	local -a dumps=()
	local f
	while IFS= read -r f; do dumps+=("$f"); done < <(
		find "$BACKUP_DIR" -maxdepth 1 -type f -name 'athenos-*.dump' -exec basename {} \; | LC_ALL=C sort -r
	)
	local i=0 stamp epoch
	for f in "${dumps[@]+"${dumps[@]}"}"; do
		i=$((i + 1))
		[ "$i" -gt "$keep" ] || continue
		stamp=${f#athenos-}
		stamp=${stamp:0:16}
		if ! epoch=$(stamp_to_epoch "$stamp"); then
			warn "skipping $f: no time stamp in the name"
			continue
		fi
		if [ "$epoch" -lt "$cutoff" ]; then
			log "rotating out $f"
			rm -f -- "$BACKUP_DIR/$f" "$BACKUP_DIR/$f.sha256" "$BACKUP_DIR/$f.manifest.tsv" \
				"$BACKUP_DIR/offsite/$f.age"
		fi
	done
	# Encrypted copies whose dump is gone (e.g. rotated by an older run).
	if [ -d "$BACKUP_DIR/offsite" ]; then
		for f in "$BACKUP_DIR"/offsite/athenos-*.dump.age; do
			[ -e "$f" ] || continue
			local base
			base=$(basename -- "$f" .age)
			[ -e "$BACKUP_DIR/$base" ] || rm -f -- "$f"
		done
	fi
}

# offsite_source -> directory to mirror (plain dumps, or their age copies).
offsite_source() {
	if [ -n "${OFFSITE_AGE_RECIPIENTS_FILE:-}" ]; then
		command -v age >/dev/null 2>&1 || die "OFFSITE_AGE_RECIPIENTS_FILE is set but age is not installed"
		local dir="$BACKUP_DIR/offsite" f base
		mkdir -p -- "$dir"
		chmod 0700 "$dir"
		for f in "$BACKUP_DIR"/athenos-*.dump; do
			[ -e "$f" ] || continue
			base=$(basename -- "$f")
			if [ ! -e "$dir/$base.age" ]; then
				age --encrypt --recipients-file "$OFFSITE_AGE_RECIPIENTS_FILE" \
					--output "$dir/.$base.age.partial" -- "$f"
				mv -- "$dir/.$base.age.partial" "$dir/$base.age"
			fi
			cp -p -- "$f.sha256" "$f.manifest.tsv" "$dir/"
		done
		# side files of rotated dumps
		for f in "$dir"/athenos-*.dump.sha256 "$dir"/athenos-*.dump.manifest.tsv; do
			[ -e "$f" ] || continue
			base=$(basename -- "$f")
			base=${base%.sha256}
			base=${base%.manifest.tsv}
			[ -e "$BACKUP_DIR/$base" ] || rm -f -- "$f"
		done
		printf '%s\n' "$dir"
	else
		printf '%s\n' "$BACKUP_DIR"
	fi
}

# offsite_sync: mirror the retention set, so off-site copies also expire
# after the retention (privacy policy: backups 14 days).
offsite_sync() {
	[ "$OFFSITE_MODE" != none ] || return 0
	local src count
	src=$(offsite_source)
	count=$(find "$src" -maxdepth 1 -type f -name 'athenos-*.dump*' ! -name '*.sha256' ! -name '*.tsv' | wc -l | tr -d ' ')
	# Never mirror an empty set: that would wipe the off-site copies.
	[ "$count" -ge 1 ] || die "no dump to mirror in $src; off-site left untouched"
	case $OFFSITE_MODE in
	rsync)
		: "${OFFSITE_RSYNC_TARGET:?OFFSITE_RSYNC_TARGET (user@host:path) is required}"
		: "${OFFSITE_RSYNC_SSH_KEY:?OFFSITE_RSYNC_SSH_KEY is required}"
		: "${OFFSITE_RSYNC_KNOWN_HOSTS:=/etc/athenos/offsite_known_hosts}"
		: "${OFFSITE_RSYNC_SSH_PORT:=23}"
		log "mirroring $count dump(s) to $OFFSITE_RSYNC_TARGET"
		rsync -rt --delete --max-delete=60 --chmod=D700,F600 \
			--include='athenos-*' --exclude='*' \
			-e "ssh -i $OFFSITE_RSYNC_SSH_KEY -p $OFFSITE_RSYNC_SSH_PORT -o BatchMode=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=$OFFSITE_RSYNC_KNOWN_HOSTS" \
			-- "$src"/ "$OFFSITE_RSYNC_TARGET"/
		;;
	rclone)
		: "${OFFSITE_RCLONE_REMOTE:?OFFSITE_RCLONE_REMOTE (remote:path) is required}"
		command -v rclone >/dev/null 2>&1 || die "rclone is not installed"
		log "mirroring $count dump(s) to $OFFSITE_RCLONE_REMOTE"
		rclone sync --max-delete 60 --include 'athenos-*' -- "$src" "$OFFSITE_RCLONE_REMOTE"
		;;
	*) die "OFFSITE_MODE must be none, rsync or rclone (is: $OFFSITE_MODE)" ;;
	esac
	log "off-site mirror ok"
}

run() {
	local dump
	dump=$(dump_once)
	if [ "$rotate" = 1 ]; then rotate_local; fi
	if [ "$offsite" = 1 ]; then offsite_sync; fi
	if [ -n "${BACKUP_HEARTBEAT_URL:-}" ] && [ "$offsite" = 1 ]; then
		curl -fsS -m 10 --retry 3 -o /dev/null -- "$BACKUP_HEARTBEAT_URL" ||
			warn "heartbeat ping failed"
	fi
	# The only line on stdout: the path of the new dump (deploy uses it).
	printf '%s\n' "$dump"
}

with_lock backup run
