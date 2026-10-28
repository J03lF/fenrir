#!/usr/bin/env bash
# Tests of the maintenance switch (no server, no root, no network):
#
#   1. server/athenos-wartung.sh: an/aus/status, atomic writes with mode
#      0644 (Caddy must read the file), the ETA, and every refusal.
#   2. server/athenos-release.sh activate --maintenance: on during the
#      restart, off when healthy, off after a healthy rollback, ON when
#      nothing healthy runs, untouched when it was on already; against a
#      temp /opt/athenos and stubbed systemctl, curl, runuser, journalctl.
#   3. deploy.sh with a fake ssh/rsync: passes --maintenance update unless
#      --no-maintenance, and says so when the page stays on.
#
#   infra/beta/tests/maintenance-test.sh
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
command -v jq >/dev/null || {
	echo "SKIP maintenance: jq missing"
	exit 0
}

tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
passes=0
fails=0
ok() {
	passes=$((passes + 1))
	printf 'ok   %s\n' "$1"
}
bad() {
	fails=$((fails + 1))
	printf 'FAIL %s\n' "$1" >&2
}
check() { # name command...
	local name=$1
	shift
	if "$@"; then ok "$name"; else bad "$name"; fi
}

# --- 1. athenos-wartung.sh ----------------------------------------------------
W="$here/server/athenos-wartung.sh"
export ATHENOS_MAINTENANCE_DIR="$tmp/state" ATHENOS_LOCK_DIR="$tmp" ATHENOS_LIB_DIR="$tmp/nolib"
export ATHENOS_CADDYFILE="$here/caddy/Caddyfile" ATHENOS_MAINTENANCE_TZ=Europe/Zurich
# 2026-10-28 18:00:00 UTC = 19:00 in Zurich (CET).
export NOW_EPOCH=1793210400
status_file="$ATHENOS_MAINTENANCE_DIR/status.json"
wart() { bash "$W" "$@" 2>"$tmp/err"; }
field() { jq -r "$1" "$status_file"; }

check "status while off" test "$(wart status)" = "Wartung: AUS"
check "status --json while off" test "$(wart status --json)" = '{"active":false}'

wart an update --minuten 10 "Neue Suche und schnellere Boards"
check "an writes the status file" test -f "$status_file"
check "the file is valid JSON with the reason" test "$(field .reason)" = update
check "active is true" test "$(field .active)" = true
check "--minuten 10 is now + 10 min" test "$(field .until)" = 2026-10-28T18:10:00Z
check "since is now" test "$(field .since)" = 2026-10-28T18:00:00Z
check "the text is kept" test "$(field .message)" = "Neue Suche und schnellere Boards"
mode=$(stat -c %a "$status_file" 2>/dev/null || stat -f %Lp "$status_file")
check "mode 0644 (Caddy reads it)" test "$mode" = 644
check "no temp file left behind" test "$(find "$ATHENOS_MAINTENANCE_DIR" -name '.status.*' | wc -l | tr -d ' ')" = 0
out=$(wart status)
check "status names reason, ETA in Zurich time and the text" \
	bash -c '[[ $1 == *"AN (update)"* && $1 == *"19:10 Europe/Zurich"* && $1 == *"Neue Suche"* ]]' _ "$out"

NOW_EPOCH=$((NOW_EPOCH + 120)) wart an fix --bis 20:15
check "an again replaces reason and ETA" test "$(field .reason)/$(field .until)" = fix/2026-10-28T19:15:00Z
check "an again keeps since of the running maintenance" test "$(field .since)" = 2026-10-28T18:00:00Z
check "an again without text drops the text" test "$(field .message)" = null

wart an wartung --bis 07:30
check "--bis in the past means tomorrow" test "$(field .until)" = 2026-10-29T06:30:00Z
wart an wartung
check "no ETA is null" test "$(field .until)" = null

msg=$'Zeile eins\nZeile zwei'
wart an wartung "$msg"
check "a text over two lines is allowed" test "$(field .message)" = "$msg"
wart an update -- "--kein Schalter"
check "a text after -- may start with a dash" test "$(field .message)" = "--kein Schalter"

refused() { # name args... -> exit 2 and the file unchanged
	local name=$1 before after rc=0
	shift
	before=$(cat "$status_file" 2>/dev/null || true)
	bash "$W" "$@" >/dev/null 2>"$tmp/err" || rc=$?
	after=$(cat "$status_file" 2>/dev/null || true)
	if [ "$rc" = 2 ] && [ "$before" = "$after" ]; then ok "refuses $name"; else bad "refuses $name (rc $rc)"; fi
}
refused "an unknown reason" an release
refused "no reason" an
refused "an hour 24" an update --bis 24:00
refused "a time without leading zero" an update --bis 7:30
refused "a time that is no time" an update --bis heute
refused "zero minutes" an update --minuten 0
refused "1441 minutes" an update --minuten 1441
refused "minutes that are no number" an update --minuten 10m
refused "--bis and --minuten together" an update --bis 20:15 --minuten 5
refused "two texts" an update "eins" "zwei"
refused "a text over 280 characters" an update "$(printf 'x%.0s' $(seq 1 281))"
refused "a control character in the text" an update "$(printf 'mit\033[31mFarbe')"
refused "an unknown option" an update --force
refused "aus with an argument" aus jetzt
refused "an unknown command" neustart
refused "status with an unknown flag" status --all

wart aus
check "aus removes the file" test ! -e "$status_file"
check "aus when off is fine" wart aus
check "status after aus" test "$(wart status)" = "Wartung: AUS"

ATHENOS_CADDYFILE="$tmp/none" wart an update
check "warns when the Caddyfile does not know the switch" grep -q 'does not use' "$tmp/err"
wart aus
check "the Caddyfile of this checkout knows the switch" grep -q '/var/lib/athenos-maintenance' "$here/caddy/Caddyfile"
unset NOW_EPOCH

# --- 2. athenos-release.sh activate --maintenance -----------------------------
stubs="$tmp/stubs"
mkdir -p "$stubs"
root="$tmp/opt"
# curl: healthy when the current release is listed in $tmp/healthy.
cat >"$stubs/curl" <<'SH'
#!/usr/bin/env bash
cur=$(basename "$(readlink "$ATHENOS_ROOT/current")")
grep -qx "$cur" "$STUB_HEALTHY" 2>/dev/null || exit 22
url=${!#}
case $url in
*/health) echo '{"status":"healthy"}' ;;
*/public/status) echo '{"overall":"operational"}' ;;
esac
exit 0
SH
# systemctl: records each restart with the state of the maintenance flag.
cat >"$stubs/systemctl" <<'SH'
#!/usr/bin/env bash
case $1 in
restart)
	m=off
	[ ! -e "$ATHENOS_MAINTENANCE_DIR/status.json" ] || m=on
	echo "restart $(basename "$(readlink "$ATHENOS_ROOT/current")") maintenance=$m" >>"$STUB_LOG"
	;;
esac
exit 0
SH
printf '#!/bin/sh\nexit 1\n' >"$stubs/runuser"
printf '#!/bin/sh\nexit 0\n' >"$stubs/journalctl"
printf '#!/bin/sh\nexit 0\n' >"$stubs/sleep"
chmod +x "$stubs"/*

setup_releases() {
	rm -rf -- "$root" "$ATHENOS_MAINTENANCE_DIR"
	mkdir -p "$root/releases/r1" "$root/releases/r2" "$ATHENOS_MAINTENANCE_DIR"
	ln -s releases/r1 "$root/current"
	: >"$tmp/stub.log"
	: >"$tmp/deploy.log"
}
activate() { # healthy-releases... -- args...
	local healthy=()
	while [ "$1" != -- ]; do
		healthy+=("$1")
		shift
	done
	shift
	printf '%s\n' "${healthy[@]+"${healthy[@]}"}" >"$tmp/healthy"
	# A lock directory per run (without flock, a refused run on macOS leaves
	# its mkdir lock behind).
	env PATH="$stubs:$PATH" ATHENOS_LOCK_DIR="$(mktemp -d "$tmp/lock.XXXXXX")" ATHENOS_RELEASE_TEST=1 ATHENOS_ROOT="$root" TOOLS_DIR="$tmp/tools" \
		DEPLOY_LOG="$tmp/deploy.log" HEALTH_TIMEOUT_SECONDS=1 STUB_HEALTHY="$tmp/healthy" \
		STUB_LOG="$tmp/stub.log" bash "$here/server/athenos-release.sh" activate "$@" >"$tmp/out" 2>&1 || { rc=$?; [ -z "${DEBUG_TEST:-}" ] || cat "$tmp/out" >&2; return $rc; }
}

setup_releases
check "healthy deploy succeeds" activate r2 -- r2 --maintenance update
check "the page was on during the restart" grep -qx 'restart r2 maintenance=on' "$tmp/stub.log"
check "and is off afterwards" test ! -e "$status_file"
check "the deploy log says on and off" bash -c 'grep -q "maintenance page on (update)" "$1" && grep -q "maintenance page off (release r2 healthy)" "$1"' _ "$tmp/deploy.log"

setup_releases
rc=0
activate r1 -- r2 --maintenance update || rc=$?
check "unhealthy deploy fails" test "$rc" != 0
check "rolled back to r1" test "$(readlink "$root/current")" = releases/r1
check "off after the healthy rollback" test ! -e "$status_file"
check "the log names the healthy rollback" grep -q "maintenance page off (rolled back to r1, healthy)" "$tmp/deploy.log"

setup_releases
rc=0
activate -- r2 --maintenance update || rc=$?
check "nothing healthy: fails" test "$rc" != 0
check "nothing healthy: the page stays on" test -f "$status_file"
check "nothing healthy: the output says so" grep -q "maintenance page stays ON" "$tmp/out"

setup_releases
bash "$W" an fix --minuten 30 "Von Hand" 2>/dev/null
check "switched on by hand: deploy succeeds" activate r2 -- r2 --maintenance update
check "switched on by hand: still on" test -f "$status_file"
check "switched on by hand: reason and text untouched" test "$(field .reason)/$(field .message)" = "fix/Von Hand"
check "switched on by hand: logged as left alone" grep -q "already on: left as it is" "$tmp/deploy.log"

setup_releases
check "without --maintenance: deploy succeeds" activate r2 -- r2
check "without --maintenance: never on" grep -qx 'restart r2 maintenance=off' "$tmp/stub.log"

setup_releases
rc=0
activate r2 -- r2 --maintenance reboot || rc=$?
check "refuses an unknown maintenance reason" test "$rc" != 0
check "and changes nothing" test "$(readlink "$root/current")" = releases/r1

# --- 3. deploy.sh with a fake ssh ---------------------------------------------
art="$tmp/artifacts"
mkdir -p "$art"
for comp in fenrir athenos athenos-api auth-service notification-service athenos-web; do
	(cd "$tmp" && mkdir -p "pkg/$comp" && echo "$comp" >"pkg/$comp/x" && tar -czf "$art/$comp-0.0.1-x86_64-unknown-linux-gnu.tar.gz" -C pkg "$comp")
done
(cd "$art" && for f in *.tar.gz; do printf '%s  %s\n' "$(shasum -a 256 "$f" 2>/dev/null | cut -d' ' -f1 || sha256sum "$f" | cut -d' ' -f1)" "$f"; done >SHA256SUMS)
fake="$tmp/fake"
mkdir -p "$fake"
# ssh: logs the remote command; the deploy command exits $FAKE_DEPLOY_RC,
# `athenos-wartung.sh status --json` prints $FAKE_STATE.
cat >"$fake/ssh" <<'SH'
#!/usr/bin/env bash
cmd=${!#}
echo "$cmd" >>"$FAKE_LOG"
case $cmd in
*athenos-release.sh*deploy*) exit "${FAKE_DEPLOY_RC:-0}" ;;
*athenos-wartung.sh*status*) printf '%s\n' "${FAKE_STATE:-}" ;;
esac
exit 0
SH
printf '#!/bin/sh\nexit 0\n' >"$fake/rsync"
chmod +x "$fake"/*
deploy_with() { # deploy.sh options... (env in FAKE_*)
	: >"$tmp/fake.log"
	env PATH="$fake:$PATH" FAKE_LOG="$tmp/fake.log" DEPLOY_ENV_FILE="$tmp/none.env" \
		bash "$here/deploy.sh" --host fake-host "$@" "$art" >"$tmp/deploy.out" 2>&1
}

check "deploy.sh: a good deploy succeeds" deploy_with
check "deploy.sh: passes --maintenance update by default" grep -q "athenos-release.sh' deploy .* --maintenance update" "$tmp/fake.log"
check "deploy.sh: uploads the switch with the release tools" grep -q athenos-wartung.sh "$tmp/deploy.out"
check "deploy.sh: --no-maintenance" deploy_with --no-maintenance
check "deploy.sh: --no-maintenance passes nothing" bash -c '! grep -q -- "--maintenance" "$1"' _ "$tmp/fake.log"

rc=0
FAKE_DEPLOY_RC=1 FAKE_STATE='{"active": true, "reason": "update"}' deploy_with || rc=$?
check "deploy.sh: a failed deploy fails" test "$rc" = 1
check "deploy.sh: says the page is still on" grep -q "STILL ON" "$tmp/deploy.out"
check "deploy.sh: and how to switch it off" grep -q "sudo athenos-wartung aus" "$tmp/deploy.out"
rc=0
FAKE_DEPLOY_RC=1 FAKE_STATE='{"active":false}' deploy_with || rc=$?
check "deploy.sh: after a healthy rollback it says the page is off" grep -q "maintenance page is off" "$tmp/deploy.out"

echo "maintenance: $passes passed, $fails failed"
[ "$fails" = 0 ]
