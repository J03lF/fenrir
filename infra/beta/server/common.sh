#!/usr/bin/env bash
# Shared helpers for the Athenos beta server scripts (sourced, not run).
# Installed to /usr/local/lib/athenos/common.sh. Works with GNU (server) and
# BSD (macOS, for the local backup/restore proof) userlands.

# shellcheck disable=SC2034 # variables are used by the sourcing scripts
ATHENOS_LIB_DIR=${ATHENOS_LIB_DIR:-/usr/local/lib/athenos}

log() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" >&2; }
warn() { log "WARN: $*"; }
die() {
	log "ERROR: $*"
	exit 1
}

# load_env_file FILE: read KEY=VALUE lines (systemd EnvironmentFile syntax
# without quotes) and export the keys that are not set yet. Never evaluates
# the file as shell code.
load_env_file() {
	local file=$1 line key value
	[ -r "$file" ] || return 0
	while IFS= read -r line || [ -n "$line" ]; do
		case $line in '' | '#'*) continue ;; esac
		key=${line%%=*}
		value=${line#*=}
		[[ $key =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || continue
		if [ -z "${!key+x}" ]; then
			export "$key=$value"
		fi
	done <"$file"
}

# sha256_of FILE -> prints the hex digest.
gnu_sha256sum() { sha256sum --version 2>/dev/null | grep -q GNU; }

sha256_of() {
	if gnu_sha256sum; then
		sha256sum -- "$1" | awk '{print $1}'
	else
		shasum -a 256 -- "$1" | awk '{print $1}'
	fi
}

# sha256_check SUMS_FILE (run in the directory the sums refer to).
sha256_check() {
	if gnu_sha256sum; then
		sha256sum --check --strict --quiet -- "$1"
	else
		shasum -a 256 --check --strict --quiet -- "$1"
	fi
}

# utc_stamp -> 20261028T013000Z
utc_stamp() { date -u +%Y%m%dT%H%M%SZ; }

# stamp_to_epoch 20261028T013000Z -> seconds since 1970 (GNU or BSD date).
stamp_to_epoch() {
	local s=$1
	[[ $s =~ ^[0-9]{8}T[0-9]{6}Z$ ]] || return 1
	if date -u -d '@0' +%s >/dev/null 2>&1; then
		date -u -d "${s:0:4}-${s:4:2}-${s:6:2} ${s:9:2}:${s:11:2}:${s:13:2}" +%s
	else
		date -u -j -f '%Y%m%dT%H%M%SZ' "$s" +%s
	fi
}

# mtime_epoch FILE
mtime_epoch() {
	if stat -c %Y -- "$1" >/dev/null 2>&1; then
		stat -c %Y -- "$1"
	else
		stat -f %m -- "$1"
	fi
}

# with_lock NAME CMD...: run CMD while holding an exclusive lock.
with_lock() {
	local name=$1
	shift
	local dir=${ATHENOS_LOCK_DIR:-/run/lock}
	[ -w "$dir" ] || dir=${TMPDIR:-/tmp}
	local lock="$dir/athenos-$name.lock"
	if command -v flock >/dev/null 2>&1; then
		(
			flock -n 9 || die "another $name run holds $lock"
			"$@"
		) 9>"$lock"
	else
		mkdir "$lock.d" 2>/dev/null || die "another $name run holds $lock.d"
		local rc=0
		"$@" || rc=$?
		rmdir "$lock.d"
		return "$rc"
	fi
}

# valid_db_name NAME: lower-case Postgres identifier without quoting needs.
valid_db_name() { [[ $1 =~ ^[a-z_][a-z0-9_]{0,62}$ ]]; }

# free_mb DIR -> free megabytes on the filesystem of DIR.
free_mb() { df -Pk -- "$1" | awk 'NR==2 {printf "%d\n", $4/1024}'; }

# used_percent DIR -> used percent on the filesystem of DIR.
used_percent() { df -Pk -- "$1" | awk 'NR==2 {gsub("%","",$5); print $5}'; }
