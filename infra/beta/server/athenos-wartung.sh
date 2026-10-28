#!/usr/bin/env bash
# Switch the maintenance page of app.athenos.dev on and off - at Caddy,
# independent of Fenrir and of the app (RUNBOOK 5.3).
#
#   sudo athenos-wartung an <update|fix|wartung> [--bis HH:MM | --minuten N] ["eigener Text"]
#   sudo athenos-wartung aus
#   athenos-wartung status [--json]
#
# an      writes /var/lib/athenos-maintenance/status.json (atomically: a temp
#         file in the same directory, then rename). Caddy's file matcher sees
#         it on the next request: every page shows the maintenance page with
#         503, the API answers 503 ATH-SYS-004. No reload.
#           update | fix | wartung   the reason, picks the page's text
#           --bis HH:MM              back by then (local time, TZ below;
#                                    a time already past means tomorrow)
#           --minuten N              back in N minutes (1-1440)
#           "eigener Text"           an extra line on the page (<= 280 chars)
#         Running `an` again replaces reason, ETA and text.
# aus     removes the file: the page polls, sees "off" and loads the app
#         as soon as it answers healthy.
# status  what is on, since when, until when; whether Caddy knows the flag.
#
# The ETA is entered in ATHENOS_MAINTENANCE_TZ (default Europe/Zurich, the
# server itself runs on UTC); the page shows it in each visitor's time.
# Settings for tests: ATHENOS_MAINTENANCE_DIR, ATHENOS_CADDYFILE, NOW_EPOCH.
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
LIB=${ATHENOS_LIB_DIR:-/usr/local/lib/athenos}
[ -r "$LIB/common.sh" ] || LIB=$here
# shellcheck source=common.sh
. "$LIB/common.sh"

: "${ATHENOS_MAINTENANCE_DIR:=/var/lib/athenos-maintenance}"
: "${ATHENOS_MAINTENANCE_TZ:=Europe/Zurich}"
: "${ATHENOS_CADDYFILE:=/etc/caddy/Caddyfile}"
STATUS="$ATHENOS_MAINTENANCE_DIR/status.json"
MAX_MESSAGE=280

usage() {
	sed -n '4,7p' "$0" | sed 's/^# \{0,1\}//' >&2
	exit 2
}

refuse() {
	log "ERROR: $*"
	exit 2
}

now_epoch() { printf '%s\n' "${NOW_EPOCH:-$(date -u +%s)}"; }

iso_utc() { # epoch -> 2026-10-28T19:15:00Z
	if date -u -d "@0" +%s >/dev/null 2>&1; then
		date -u -d "@$1" +%Y-%m-%dT%H:%M:%SZ
	else
		date -u -r "$1" +%Y-%m-%dT%H:%M:%SZ
	fi
}

local_hm() { # epoch -> HH:MM in the maintenance time zone
	if date -u -d "@0" +%s >/dev/null 2>&1; then
		TZ=$ATHENOS_MAINTENANCE_TZ date -d "@$1" +%H:%M
	else
		TZ=$ATHENOS_MAINTENANCE_TZ date -r "$1" +%H:%M
	fi
}

epoch_of_iso() { # 2026-10-28T19:15:00Z -> epoch
	if date -u -d "@0" +%s >/dev/null 2>&1; then
		date -u -d "$1" +%s
	else
		date -u -j -f '%Y-%m-%dT%H:%M:%SZ' "$1" +%s
	fi
}

# epoch_at_hm HH:MM -> the next moment with that wall-clock time in the
# maintenance time zone (today, or tomorrow when it is past).
epoch_at_hm() {
	local hm=$1 now day candidate
	now=$(now_epoch)
	if date -u -d "@0" +%s >/dev/null 2>&1; then
		day=$(TZ=$ATHENOS_MAINTENANCE_TZ date -d "@$now" +%Y-%m-%d)
		candidate=$(TZ=$ATHENOS_MAINTENANCE_TZ date -d "$day $hm" +%s)
		if [ "$candidate" -le "$now" ]; then
			day=$(TZ=$ATHENOS_MAINTENANCE_TZ date -d "$day + 1 day" +%Y-%m-%d)
			candidate=$(TZ=$ATHENOS_MAINTENANCE_TZ date -d "$day $hm" +%s)
		fi
	else
		day=$(TZ=$ATHENOS_MAINTENANCE_TZ date -r "$now" +%Y-%m-%d)
		candidate=$(TZ=$ATHENOS_MAINTENANCE_TZ date -j -f '%Y-%m-%d %H:%M:%S' "$day $hm:00" +%s)
		if [ "$candidate" -le "$now" ]; then
			candidate=$(TZ=$ATHENOS_MAINTENANCE_TZ date -j -v+1d -f '%Y-%m-%d %H:%M:%S' "$day $hm:00" +%s)
		fi
	fi
	printf '%s\n' "$candidate"
}

caddy_knows_flag() {
	[ -r "$ATHENOS_CADDYFILE" ] && grep -q 'athenos-maintenance' -- "$ATHENOS_CADDYFILE"
}

ensure_dir() {
	if [ ! -d "$ATHENOS_MAINTENANCE_DIR" ]; then
		mkdir -p -- "$ATHENOS_MAINTENANCE_DIR" 2>/dev/null || refuse "cannot create $ATHENOS_MAINTENANCE_DIR (run with sudo)"
		chmod 0755 "$ATHENOS_MAINTENANCE_DIR"
	fi
	[ -w "$ATHENOS_MAINTENANCE_DIR" ] || refuse "$ATHENOS_MAINTENANCE_DIR is not writable (run with sudo)"
}

cmd_on() {
	[ $# -ge 1 ] || usage
	local reason=$1
	shift
	case $reason in
	update | fix | wartung) ;;
	*) refuse "unknown reason '$reason' (update, fix or wartung)" ;;
	esac
	local until_epoch="" message="" have_message=0
	while [ $# -gt 0 ]; do
		case $1 in
		--bis)
			[ -z "$until_epoch" ] || refuse "--bis and --minuten only once, and not both"
			[ $# -ge 2 ] || refuse "--bis needs a time (HH:MM)"
			[[ $2 =~ ^([01][0-9]|2[0-3]):[0-5][0-9]$ ]] || refuse "--bis wants HH:MM (00:00-23:59), not '$2'"
			until_epoch=$(epoch_at_hm "$2")
			shift 2
			;;
		--minuten)
			[ -z "$until_epoch" ] || refuse "--bis and --minuten only once, and not both"
			[ $# -ge 2 ] || refuse "--minuten needs a number"
			[[ $2 =~ ^[0-9]{1,4}$ ]] && [ "$((10#$2))" -ge 1 ] && [ "$((10#$2))" -le 1440 ] ||
				refuse "--minuten wants 1-1440, not '$2'"
			until_epoch=$(($(now_epoch) + 10#$2 * 60))
			shift 2
			;;
		--)
			shift
			[ $# -le 1 ] || refuse "only one text (put it in quotes)"
			[ $# -eq 0 ] || {
				message=$1
				have_message=1
				shift
			}
			;;
		-*) refuse "unknown option $1" ;;
		*)
			[ "$have_message" = 0 ] || refuse "only one text (put it in quotes)"
			message=$1
			have_message=1
			shift
			;;
		esac
	done
	if [ "$have_message" = 1 ]; then
		# One or more lines of plain text; the page shows it as text, never as HTML.
		[ "${#message}" -le "$MAX_MESSAGE" ] || refuse "the text has ${#message} characters (at most $MAX_MESSAGE)"
		if printf '%s' "$message" | tr -d '\n' | LC_ALL=C grep -q '[[:cntrl:]]'; then
			refuse "the text holds control characters"
		fi
		message=$(printf '%s' "$message" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')
	fi
	command -v jq >/dev/null 2>&1 || refuse "jq is needed (apt install jq)"
	ensure_dir

	local now since until_iso="" tmp
	now=$(now_epoch)
	since=$(iso_utc "$now")
	# Switched on again while on: keep "since" of the running maintenance.
	if [ -r "$STATUS" ]; then
		since=$(jq -r '.since // empty' -- "$STATUS" 2>/dev/null || true)
		[[ $since =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$ ]] || since=$(iso_utc "$now")
	fi
	[ -z "$until_epoch" ] || until_iso=$(iso_utc "$until_epoch")
	tmp=$(mktemp "$ATHENOS_MAINTENANCE_DIR/.status.XXXXXX")
	jq -n --arg reason "$reason" --arg since "$since" --arg until "$until_iso" --arg message "$message" \
		'{active: true, reason: $reason, since: $since,
		  until: (if $until == "" then null else $until end),
		  message: (if $message == "" then null else $message end)}' >"$tmp"
	chmod 0644 "$tmp"
	mv -f -- "$tmp" "$STATUS"
	log "maintenance ON ($reason)${until_iso:+, back by $(local_hm "$until_epoch") $ATHENOS_MAINTENANCE_TZ}"
	command -v logger >/dev/null 2>&1 && logger -t athenos-wartung "on: $reason${until_iso:+ until $until_iso}" || true
	caddy_knows_flag || warn "$ATHENOS_CADDYFILE does not use $ATHENOS_MAINTENANCE_DIR yet: install the Caddyfile (sudo install.sh caddy), else nothing changes for visitors"
}

cmd_off() {
	[ $# -eq 0 ] || refuse "aus takes no arguments"
	if [ ! -e "$STATUS" ]; then
		log "maintenance is already off"
		return 0
	fi
	[ -w "$ATHENOS_MAINTENANCE_DIR" ] || refuse "$ATHENOS_MAINTENANCE_DIR is not writable (run with sudo)"
	rm -f -- "$STATUS"
	log "maintenance OFF: the page loads the app again as soon as it answers"
	command -v logger >/dev/null 2>&1 && logger -t athenos-wartung "off" || true
}

cmd_status() {
	local json=0
	case ${1:-} in
	"") ;;
	--json) json=1 ;;
	*) refuse "status takes only --json" ;;
	esac
	[ $# -le 1 ] || refuse "status takes only --json"
	if [ ! -r "$STATUS" ]; then
		if [ "$json" = 1 ]; then echo '{"active":false}'; else echo "Wartung: AUS"; fi
		caddy_knows_flag || [ "$json" = 1 ] || echo "Hinweis: $ATHENOS_CADDYFILE kennt den Schalter noch nicht (install.sh caddy)."
		return 0
	fi
	if [ "$json" = 1 ]; then
		cat -- "$STATUS"
		return 0
	fi
	command -v jq >/dev/null 2>&1 || {
		cat -- "$STATUS"
		return 0
	}
	local reason since until message
	reason=$(jq -r '.reason // "?"' -- "$STATUS")
	since=$(jq -r '.since // empty' -- "$STATUS")
	until=$(jq -r '.until // empty' -- "$STATUS")
	message=$(jq -r '.message // empty' -- "$STATUS")
	printf 'Wartung: AN (%s)\n' "$reason"
	[ -z "$since" ] || printf '  seit:  %s %s\n' "$(local_hm "$(epoch_of_iso "$since")")" "$ATHENOS_MAINTENANCE_TZ"
	if [ -n "$until" ]; then
		local u
		u=$(epoch_of_iso "$until")
		if [ "$u" -le "$(now_epoch)" ]; then
			printf '  bis:   %s %s (vorbei; die Seite sagt "dauert etwas länger")\n' "$(local_hm "$u")" "$ATHENOS_MAINTENANCE_TZ"
		else
			printf '  bis:   %s %s\n' "$(local_hm "$u")" "$ATHENOS_MAINTENANCE_TZ"
		fi
	else
		printf '  bis:   offen\n'
	fi
	[ -z "$message" ] || printf '  Text:  %s\n' "$message"
	caddy_knows_flag || echo "Hinweis: $ATHENOS_CADDYFILE kennt den Schalter noch nicht (install.sh caddy): Besucher sehen nichts davon."
}

# Each command in a subshell: a refusal (exit 2) ends it, and with_lock still
# releases the lock (its mkdir fallback on macOS runs the command in-line).
in_subshell() { ("$@"); }

sub=${1:-}
shift || true
case $sub in
an | on) with_lock maintenance in_subshell cmd_on "$@" ;;
aus | off) with_lock maintenance in_subshell cmd_off "$@" ;;
status) cmd_status "$@" ;;
-h | --help | help | "") usage ;;
*) refuse "unknown command '$sub' (an, aus, status)" ;;
esac
