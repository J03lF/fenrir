#!/usr/bin/env bash
# Send an alert mail to the admin.
#
#   alert.sh unit <unit-name>          (athenos-alert@<unit>.service)
#   alert.sh message <subject> [file]  (body from file or stdin)
#
# Delivery, first that works:
#   1. the local notification-service (POST /api/v1/notifications/send with
#      the service token; port from the host's ports.json),
#   2. sendmail (e.g. msmtp-mta configured for Mailgun), see RUNBOOK.md,
#   3. nothing left: the alert is still in the journal (priority crit).
# Settings: /etc/athenos/monitor.env (ALERT_EMAIL_TO is required for mail).
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "${ATHENOS_LIB_DIR:-$here}/common.sh"

load_env_file "${MONITOR_ENV_FILE:-/etc/athenos/monitor.env}"
load_env_file "${FENRIR_ENV_FILE_PATH:-/etc/athenos/fenrir.env}"
: "${ALERT_EMAIL_TO:=}"
: "${ALERT_EMAIL_FROM:=athenos-alert@$(hostname -f 2>/dev/null || hostname)}"
: "${ALERT_VIA_NOTIFICATION:=true}"
: "${ALERT_PORTS_FILE:=/var/lib/athenos/modules-runtime/ports.json}"
: "${NOTIFY_INTERNAL_SERVICE_AUTH_TOKEN_FILE:=}"
host_label=$(hostname 2>/dev/null || echo server)

mode=${1:-}
case $mode in
unit)
	unit=${2:?unit name}
	subject="[Athenos $host_label] $unit failed"
	body=$(
		printf 'The unit %s entered the failed state at %s.\n\n' "$unit" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
		systemctl status --no-pager --lines=0 -- "$unit" 2>&1 || true
		printf '\nLast log lines:\n'
		journalctl --no-pager --output=short-iso --lines=40 --unit="$unit" 2>&1 || true
	)
	;;
message)
	subject="[Athenos $host_label] ${2:?subject}"
	if [ -n "${3:-}" ]; then body=$(cat -- "$3"); else body=$(cat); fi
	;;
*) die "usage: alert.sh unit <unit> | alert.sh message <subject> [file]" ;;
esac

logger -p user.crit -t athenos-alert -- "$subject" 2>/dev/null || true
log "alert: $subject"
[ -n "$ALERT_EMAIL_TO" ] || {
	warn "ALERT_EMAIL_TO is not set: alert only in the journal"
	exit 0
}

tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
chmod 0700 "$tmp"

via_notification() {
	[ "$ALERT_VIA_NOTIFICATION" = true ] || return 1
	command -v jq >/dev/null 2>&1 || return 1
	[ -r "$ALERT_PORTS_FILE" ] && [ -r "$NOTIFY_INTERNAL_SERVICE_AUTH_TOKEN_FILE" ] || return 1
	local port
	port=$(jq -r '."notification-service" // empty' "$ALERT_PORTS_FILE")
	[[ $port =~ ^[0-9]+$ ]] || return 1
	# Token only in a 0600 header file, never in argv.
	printf 'x-fenrir-service-token: Bearer %s\n' "$(tr -d '\r\n' <"$NOTIFY_INTERNAL_SERVICE_AUTH_TOKEN_FILE")" >"$tmp/headers"
	jq -n --arg to "$ALERT_EMAIL_TO" --arg subject "$subject" --arg body "$body" \
		'{to: $to, subject: $subject, body_text: $body, body_html: ("<pre>" + ($body | @html) + "</pre>")}' >"$tmp/body.json"
	curl -fsS -m 20 -o /dev/null -X POST -H @"$tmp/headers" -H 'content-type: application/json' \
		--data-binary @"$tmp/body.json" "http://127.0.0.1:$port/api/v1/notifications/send"
}

via_sendmail() {
	local sm
	if command -v sendmail >/dev/null 2>&1; then
		sm=$(command -v sendmail)
	elif [ -x /usr/sbin/sendmail ]; then
		sm=/usr/sbin/sendmail
	else
		return 1
	fi
	{
		printf 'From: %s\nTo: %s\nSubject: %s\nContent-Type: text/plain; charset=UTF-8\n\n' \
			"$ALERT_EMAIL_FROM" "$ALERT_EMAIL_TO" "$subject"
		printf '%s\n' "$body"
	} | "$sm" -t -oi
}

if via_notification; then
	log "alert mailed through notification-service"
elif via_sendmail; then
	log "alert mailed through sendmail"
else
	warn "no mail path worked; the alert is only in the journal"
	exit 1
fi
