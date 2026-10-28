#!/usr/bin/env bash
# Local proof of backup.sh and restore.sh against a throwaway PostgreSQL 16.
#
#   infra/beta/tests/backup-restore-proof.sh [PG_BIN_DIR]
#
# Starts a private cluster (socket only, no TCP) under $PROOF_DIR, creates the
# same role layout as the server (non-superuser `athenos` owning database
# `athenos`, pg_trgm created by that owner), fills a synthetic schema with
# awkward data, then:
#   1. backup.sh: dump + sha256 + manifest,
#   2. rotation: fake dumps 20 and 15 days old go, 13 days old stays,
#      BACKUP_MIN_KEEP protects the newest ones,
#   3. restore.sh into a fresh database with row counts and row checksums
#      compared to the source,
#   4. refusals: existing target without --force, tampered checksum, --swap
#      without --force,
#   5. --force over the existing target.
# Everything is removed at the end (also on failure).
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
server="$here/../server"
PG_BIN_DIR=${1:-${PG_BIN_DIR:-/opt/homebrew/opt/postgresql@16/bin}}
PROOF_DIR=${PROOF_DIR:-/tmp/w9-infra-pg}
PORT=${PROOF_PORT:-55871}
export PG_BIN_DIR
# shellcheck source=../server/common.sh
. "$server/common.sh"

[ ! -e "$PROOF_DIR" ] || {
	echo "proof: $PROOF_DIR exists; remove it first" >&2
	exit 1
}
mkdir -p "$PROOF_DIR"
data="$PROOF_DIR/data"
sock="$PROOF_DIR/sock"
backups="$PROOF_DIR/backups"
mkdir -p "$sock"

cleanup() {
	"$PG_BIN_DIR/pg_ctl" -D "$data" -m immediate stop >/dev/null 2>&1 || true
	rm -rf -- "$PROOF_DIR"
}
trap cleanup EXIT

step() { printf '\n== %s\n' "$*"; }
ok() { printf 'ok: %s\n' "$*"; }
fail() {
	printf 'FAIL: %s\n' "$*" >&2
	exit 1
}

step "throwaway PostgreSQL 16 on socket $sock (port $PORT, TCP off)"
LC_ALL=C "$PG_BIN_DIR/initdb" -D "$data" -U postgres --auth=trust --locale=C -E UTF8 >/dev/null
LC_ALL=C "$PG_BIN_DIR/pg_ctl" -D "$data" -l "$PROOF_DIR/pg.log" -w \
	-o "-k $sock -p $PORT -c listen_addresses=''" start >/dev/null
export PGHOST="$sock" PGPORT="$PORT"
psql_as() {
	local user=$1 db=$2
	shift 2
	"$PG_BIN_DIR/psql" -X -q -v ON_ERROR_STOP=1 -U "$user" -d "$db" "$@"
}
"$PG_BIN_DIR/psql" -X -A -t -U postgres -d postgres -c 'SELECT version()'

step "server role layout: non-superuser athenos owns database athenos"
psql_as postgres postgres <<'SQL'
CREATE ROLE athenos LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE;
CREATE DATABASE athenos OWNER athenos TEMPLATE template0 ENCODING 'UTF8' LC_COLLATE 'C' LC_CTYPE 'C';
SQL

step "synthetic schema and data (as athenos)"
psql_as athenos athenos <<'SQL'
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE TYPE ticket_state AS ENUM ('open', 'in_progress', 'done');
CREATE TABLE athenos_users (
  id uuid PRIMARY KEY,
  email text NOT NULL UNIQUE,
  display_name text,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE tickets (
  id bigserial PRIMARY KEY,
  owner uuid REFERENCES athenos_users(id),
  title text NOT NULL,
  body text,
  state ticket_state NOT NULL DEFAULT 'open',
  meta jsonb NOT NULL DEFAULT '{}'::jsonb,
  attachment bytea,
  due date
);
CREATE INDEX tickets_title_trgm ON tickets USING gin (title gin_trgm_ops);
CREATE TABLE empty_table (id int PRIMARY KEY);
INSERT INTO athenos_users (id, email, display_name)
SELECT md5('u' || g)::uuid, 'person' || g || '@example.org',
       CASE WHEN g % 7 = 0 THEN NULL ELSE 'Person ' || g || E' äöü \U0001F600' END
FROM generate_series(1, 250) g;
INSERT INTO tickets (owner, title, body, state, meta, attachment, due)
SELECT md5('u' || (1 + g % 250))::uuid,
       'Ticket ' || g || CASE WHEN g % 5 = 0 THEN E' with\ttab' ELSE '' END,
       CASE WHEN g % 3 = 0 THEN E'line one\nline two\\ and a backslash' WHEN g % 11 = 0 THEN NULL ELSE repeat('x', g % 300) END,
       (ARRAY['open','in_progress','done']::ticket_state[])[1 + g % 3],
       jsonb_build_object('n', g, 'tags', jsonb_build_array('a', 'b'), 'quote', E'"ß"'),
       CASE WHEN g % 4 = 0 THEN decode(md5(g::text), 'hex') END,
       DATE '2026-01-01' + g
FROM generate_series(1, 5000) g;
SQL
psql_as athenos athenos -A -t -c "SELECT 'source rows: users=' || (SELECT count(*) FROM athenos_users) || ' tickets=' || (SELECT count(*) FROM tickets)"

common_env=(BACKUP_ENV_FILE=/nonexistent BACKUP_DIR="$backups" BACKUP_RETENTION_DAYS=14 BACKUP_MIN_KEEP=3
	BACKUP_MIN_FREE_MB=10 PGDATABASE=athenos ATHENOS_LOCK_DIR="$PROOF_DIR" OFFSITE_MODE=none)

step "1. backup.sh as athenos"
dump=$(env "${common_env[@]}" PGUSER=athenos bash "$server/backup.sh")
[ -f "$dump" ] || fail "no dump file reported"
ls -l "$backups"
cat "$dump.manifest.tsv"
grep -q $'public.tickets\t5000' "$dump.manifest.tsv" || fail "manifest lacks tickets=5000"
grep -q $'public.empty_table\t0' "$dump.manifest.tsv" || fail "manifest lacks the empty table"
[ "$(stat -f %Lp "$dump" 2>/dev/null || stat -c %a "$dump")" = 600 ] || fail "dump is not 0600"
ok "dump, checksum and manifest written (0600)"

step "2. rotation with fake old dumps (BACKUP_MIN_KEEP=1, so the age decides)"
stamp_days_ago() { date -u -v-"$1"d +%Y%m%dT%H%M%SZ 2>/dev/null || date -u -d "$1 days ago" +%Y%m%dT%H%M%SZ; }
fake_dump() { # dir days -> path of a copy of $dump with side files, N days old by name
	local f
	f="$1/athenos-$(stamp_days_ago "$2").dump"
	cp "$dump" "$f"
	(cd "$1" && printf '%s  %s\n' "$(sha256_of "$(basename "$f")")" "$(basename "$f")" >"$(basename "$f").sha256")
	cp "$dump.manifest.tsv" "$f.manifest.tsv"
	printf '%s\n' "$f"
}
f20=$(fake_dump "$backups" 20)
f15=$(fake_dump "$backups" 15)
f13=$(fake_dump "$backups" 13)
echo "before:"
ls "$backups" | sed 's/^/  /'
sleep 1
second=$(env "${common_env[@]}" BACKUP_MIN_KEEP=1 PGUSER=athenos bash "$server/backup.sh" --label predeploy-proof)
echo "after:"
ls "$backups" | sed 's/^/  /'
for f in "$f20" "$f15"; do
	for g in "$f" "$f.sha256" "$f.manifest.tsv"; do
		[ ! -e "$g" ] || fail "$g should have been rotated out"
	done
done
[ -f "$f13" ] && [ -f "$f13.sha256" ] || fail "13 day old dump was rotated"
[ -f "$dump" ] && [ -f "$second" ] || fail "fresh dumps missing"
ok "20 and 15 day old dumps rotated with their side files; 13 day old and the fresh ones kept"

step "2b. BACKUP_MIN_KEEP keeps the newest dumps even when they are all old"
keepdir="$PROOF_DIR/keep"
mkdir -p "$keepdir"
for d in 40 30 25 20; do fake_dump "$keepdir" "$d" >/dev/null; done
env "${common_env[@]}" BACKUP_DIR="$keepdir" BACKUP_MIN_KEEP=3 PGUSER=athenos bash "$server/backup.sh" >/dev/null
left=$(find "$keepdir" -name 'athenos-*.dump' | wc -l | tr -d ' ')
[ "$left" = 3 ] || fail "BACKUP_MIN_KEEP=3 should leave 3 dumps, found $left"
ok "min-keep: the new dump plus the 20 and 25 day old ones kept, 30 and 40 removed"

step "3. restore into a fresh database, compared with the source"
env BACKUP_ENV_FILE=/nonexistent PGDATABASE=athenos PG_ADMIN_USER=postgres \
	bash "$server/restore.sh" --target-db athenos_restore_proof --compare-db athenos "$dump"
psql_as postgres athenos_restore_proof -A -t -c "SELECT 'restored rows: users=' || (SELECT count(*) FROM athenos_users) || ' tickets=' || (SELECT count(*) FROM tickets) || ', owner=' || (SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = current_database()) || ', tickets owner=' || (SELECT tableowner FROM pg_tables WHERE tablename = 'tickets') || ', trgm=' || (SELECT count(*) FROM pg_extension WHERE extname = 'pg_trgm')"
psql_as athenos athenos_restore_proof -A -t -c "SELECT 'similarity search works: ' || count(*) FROM tickets WHERE title % 'Ticket 4242'"
ok "restore verified (row counts against the manifest, row checksums against the source)"

step "4a. refuses an existing target without --force"
if env BACKUP_ENV_FILE=/nonexistent PGDATABASE=athenos PG_ADMIN_USER=postgres \
	bash "$server/restore.sh" --target-db athenos_restore_proof "$dump" 2>"$PROOF_DIR/refuse.log"; then
	fail "restore overwrote an existing database without --force"
fi
grep -q 'without --force' "$PROOF_DIR/refuse.log" || fail "unexpected refusal: $(cat "$PROOF_DIR/refuse.log")"
ok "$(grep -o 'database .* without --force' "$PROOF_DIR/refuse.log")"

step "4b. refuses a dump whose checksum does not match"
bad="$backups/athenos-20000101T000000Z-tampered.dump"
cp "$dump" "$bad"
printf '%s  %s\n' "0000000000000000000000000000000000000000000000000000000000000000" "$(basename "$bad")" >"$bad.sha256"
cp "$dump.manifest.tsv" "$bad.manifest.tsv"
if env BACKUP_ENV_FILE=/nonexistent PGDATABASE=athenos PG_ADMIN_USER=postgres \
	bash "$server/restore.sh" --target-db athenos_restore_bad "$bad" 2>"$PROOF_DIR/bad.log"; then
	fail "restore accepted a tampered dump"
fi
grep -q 'checksum mismatch' "$PROOF_DIR/bad.log" || fail "unexpected: $(cat "$PROOF_DIR/bad.log")"
ok "checksum mismatch refused, no database created: $(psql_as postgres postgres -A -t -c "SELECT count(*) FROM pg_database WHERE datname = 'athenos_restore_bad'") rows"

step "4c. refuses --swap without --force"
if env BACKUP_ENV_FILE=/nonexistent PGDATABASE=athenos PG_ADMIN_USER=postgres \
	bash "$server/restore.sh" --swap "$dump" 2>"$PROOF_DIR/swap.log"; then
	fail "--swap ran without --force"
fi
grep -q 'add --force' "$PROOF_DIR/swap.log" || fail "unexpected: $(cat "$PROOF_DIR/swap.log")"
ok "--swap without --force refused"

step "5. --force replaces the existing target"
psql_as postgres athenos_restore_proof -c "DELETE FROM tickets WHERE id <= 100"
env BACKUP_ENV_FILE=/nonexistent PGDATABASE=athenos PG_ADMIN_USER=postgres \
	bash "$server/restore.sh" --force --target-db athenos_restore_proof --compare-db athenos "$dump"
ok "forced restore verified again"

step "source untouched"
psql_as athenos athenos -A -t -c "SELECT 'source rows: users=' || (SELECT count(*) FROM athenos_users) || ' tickets=' || (SELECT count(*) FROM tickets)"

printf '\nPROOF PASSED\n'
