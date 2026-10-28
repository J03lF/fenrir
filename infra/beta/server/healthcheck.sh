#!/usr/bin/env bash
# Health check of the Athenos beta server (athenos-healthcheck.timer, every
# 5 minutes; also fine to run by hand).
#
#   healthcheck.sh [--no-alert] [--quiet]
#
# Checks: the units (athenos, postgresql@16-main, caddy), the host
# (/health/ready, /public/status not major_outage), the static edge
# (127.0.0.1:41002/healthz), the public site and API through Caddy
# (https://app.athenos.dev/healthz and the gateway /health), disk usage,
# age of the newest database backup, TLS certificate validity.
#
# Alerts (alert.sh) on a new problem, every ALERT_REPEAT_HOURS while it
# lasts, and once when everything is fine again. Exit code 1 if a check
# failed. With HEALTHCHECK_HEARTBEAT_URL set, an all-green run pings it (dead
# man's switch at an external monitor: no ping = server down).
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "${ATHENOS_LIB_DIR:-$here}/common.sh"

load_env_file "${MONITOR_ENV_FILE:-/etc/athenos/monitor.env}"
load_env_file "${BACKUP_ENV_FILE:-/etc/athenos/backup.env}"
: "${PUBLIC_URL:=https://app.athenos.dev}"
: "${HOST_URL:=http://127.0.0.1:8080}"
: "${EDGE_URL:=http://127.0.0.1:41002}"
: "${API_SERVICE_PATH:=/gateway/services/module%3Aathenos-api%3A%3Aapi-gateway}"
: "${CHECK_UNITS:=athenos.service postgresql@16-main.service caddy.service}"
: "${DISK_PATHS:=/ /var/lib/postgresql /var/backups/athenos /var/lib/athenos}"
: "${DISK_MAX_PERCENT:=85}"
: "${BACKUP_DIR:=/var/backups/athenos}"
: "${BACKUP_MAX_AGE_HOURS:=26}"
: "${CERT_MIN_DAYS:=14}"
: "${ALERT_REPEAT_HOURS:=6}"
: "${MONITOR_STATE_DIR:=/var/lib/athenos/monitor}"

alert=1
quiet=0
for arg in "$@"; do
	case $arg in
	--no-alert) alert=0 ;;
	--quiet) quiet=1 ;;
	*) die "unknown argument: $arg" ;;
	esac
done

problems=()
pass() { [ "$quiet" = 1 ] || printf 'ok    %s\n' "$*"; }
problem() {
	problems+=("$*")
	printf 'FAIL  %s\n' "$*"
}

http_ok() { curl -fsS -m 10 -o /dev/null -- "$1"; }

# units
for unit in $CHECK_UNITS; do
	if systemctl is-active --quiet -- "$unit"; then pass "unit $unit active"; else problem "unit $unit is $(systemctl is-active -- "$unit" 2>/dev/null || true)"; fi
done

# host and edge (loopback)
if http_ok "$HOST_URL/health/ready"; then pass "host ready"; else problem "host /health/ready fails"; fi
if command -v jq >/dev/null 2>&1; then
	overall=$(curl -fsS -m 10 -- "$HOST_URL/public/status" 2>/dev/null | jq -r '.overall // empty' 2>/dev/null || true)
	case $overall in
	operational) pass "host status operational" ;;
	degraded) pass "host status degraded (not alerting; see /public/status)" ;;
	*) problem "host status ${overall:-unreadable}" ;;
	esac
fi
if http_ok "$EDGE_URL/healthz"; then pass "static edge healthz"; else problem "static edge $EDGE_URL/healthz fails"; fi

# public path: DNS, TLS, Caddy, edge, control plane, gateway
if http_ok "$PUBLIC_URL/healthz"; then pass "public $PUBLIC_URL/healthz"; else problem "public $PUBLIC_URL/healthz fails"; fi
api=$(curl -fsS -m 10 -- "$PUBLIC_URL$API_SERVICE_PATH/health" 2>/dev/null || true)
case $api in
*'"status":"healthy"'*) pass "public API health" ;;
*) problem "public API $PUBLIC_URL$API_SERVICE_PATH/health not healthy" ;;
esac

# disk
for path in $DISK_PATHS; do
	[ -e "$path" ] || continue
	used=$(used_percent "$path")
	if [ "$used" -lt "$DISK_MAX_PERCENT" ]; then pass "disk $path ${used}%"; else problem "disk $path ${used}% used (limit ${DISK_MAX_PERCENT}%)"; fi
done

# newest backup
newest=$(find "$BACKUP_DIR" -maxdepth 1 -type f -name 'athenos-*.dump' 2>/dev/null | LC_ALL=C sort | tail -n 1 || true)
if [ -z "$newest" ]; then
	problem "no database backup in $BACKUP_DIR"
else
	age_h=$((($(date -u +%s) - $(mtime_epoch "$newest")) / 3600))
	if [ "$age_h" -le "$BACKUP_MAX_AGE_HOURS" ]; then pass "backup $(basename -- "$newest") (${age_h} h old)"; else problem "newest backup is ${age_h} h old (limit ${BACKUP_MAX_AGE_HOURS} h)"; fi
fi

# certificate (Caddy renews 30 days before expiry; less than CERT_MIN_DAYS = renewal broken)
host=${PUBLIC_URL#https://}
host=${host%%/*}
if [ "$host" != "$PUBLIC_URL" ] && command -v openssl >/dev/null 2>&1; then
	cert=$(openssl s_client -connect "$host:443" -servername "$host" </dev/null 2>/dev/null | openssl x509 2>/dev/null || true)
	if [ -z "$cert" ]; then
		problem "no TLS certificate readable from $host:443"
	elif printf '%s\n' "$cert" | openssl x509 -noout -checkend $((CERT_MIN_DAYS * 86400)) >/dev/null; then
		pass "certificate valid > ${CERT_MIN_DAYS} days ($(printf '%s\n' "$cert" | openssl x509 -noout -enddate | cut -d= -f2))"
	else
		problem "certificate of $host expires within ${CERT_MIN_DAYS} days"
	fi
fi

# alerting with memory: new problem, repeat, recovery
mkdir -p -- "$MONITOR_STATE_DIR"
state="$MONITOR_STATE_DIR/healthcheck.state"
now=$(date -u +%s)
prev_sig=""
prev_alert=0
if [ -r "$state" ]; then
	prev_sig=$(sed -n 1p "$state")
	prev_alert=$(sed -n 2p "$state")
	[[ $prev_alert =~ ^[0-9]+$ ]] || prev_alert=0
fi
if [ ${#problems[@]} -eq 0 ]; then
	sig=ok
else
	sig=$(printf '%s\n' "${problems[@]}" | sed 's/[0-9][0-9]*/N/g' | LC_ALL=C sort | tr '\n' '|')
fi

send=""
if [ "$sig" = ok ]; then
	[ "$prev_sig" = "" ] || [ "$prev_sig" = ok ] || send=recovered
elif [ "$sig" != "$prev_sig" ] || [ $((now - prev_alert)) -ge $((ALERT_REPEAT_HOURS * 3600)) ]; then
	send=problem
fi

if [ -n "$send" ] && [ "$alert" = 1 ]; then
	if [ "$send" = recovered ]; then
		printf 'All checks are green again (%s).\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" |
			"$here/alert.sh" message "recovered" || warn "alert delivery failed"
	else
		printf 'Failed checks (%s):\n\n%s\n\nRun: sudo /usr/local/lib/athenos/healthcheck.sh\n' \
			"$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$(printf -- '- %s\n' "${problems[@]}")" |
			"$here/alert.sh" message "${#problems[@]} health check(s) failed" || warn "alert delivery failed"
	fi
	prev_alert=$now
fi
printf '%s\n%s\n' "$sig" "$prev_alert" >"$state"

if [ ${#problems[@]} -eq 0 ]; then
	if [ -n "${HEALTHCHECK_HEARTBEAT_URL:-}" ]; then
		curl -fsS -m 10 --retry 2 -o /dev/null -- "$HEALTHCHECK_HEARTBEAT_URL" || warn "heartbeat ping failed"
	fi
	[ "$quiet" = 1 ] || echo "all checks passed"
	exit 0
fi
exit 1
