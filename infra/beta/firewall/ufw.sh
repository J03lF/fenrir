#!/usr/bin/env bash
# Host firewall of the Athenos beta with ufw (run as root; idempotent).
#
# Inbound:
#   22/tcp   SSH, key-only, rate-limited (6 new connections per 30 s and
#            source). From anywhere by default; set SSH_SOURCES to a list of
#            CIDRs (your home/office) to restrict it, e.g.
#            SSH_SOURCES="203.0.113.7/32 2001:db8::/48" ./ufw.sh
#   443/tcp  only from the Cloudflare ranges (/etc/athenos/cloudflare-ips.txt,
#            rules managed by server/cloudflare-ips.sh, comment cloudflare-443)
# Everything else is closed: 80, 443/udp, 8080, 41000-46000, 50051, 2222,
# 5432 (those bind to 127.0.0.1 anyway). IPv4 and IPv6. Outgoing stays open
# (Mailgun SMTP, apt, Cloudflare range list, off-site backup).
# Mirror the same rules in the Infomaniak firewall (security group) as a
# second layer.
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }
command -v ufw >/dev/null || { echo "ufw missing: apt install ufw" >&2; exit 1; }

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
SSH_PORT=${SSH_PORT:-22}
SSH_SOURCES=${SSH_SOURCES:-}
CF_TOOL=${CF_TOOL:-$here/../server/cloudflare-ips.sh}
[ -x "$CF_TOOL" ] || CF_TOOL=/usr/local/lib/athenos/cloudflare-ips.sh

sed -i 's/^IPV6=.*/IPV6=yes/' /etc/default/ufw
ufw --force reset >/dev/null
ufw default deny incoming
ufw default allow outgoing
ufw default deny routed
if [ -z "$SSH_SOURCES" ]; then
	ufw limit "$SSH_PORT/tcp" comment 'ssh (key-only, rate-limited)'
else
	for src in $SSH_SOURCES; do
		ufw limit proto tcp from "$src" to any port "$SSH_PORT" comment 'ssh (restricted)'
	done
fi
ufw logging low
ufw --force enable
# 443 for Cloudflare only (fetches the list if it is not there yet).
if [ -s /etc/athenos/cloudflare-ips.txt ]; then
	"$CF_TOOL" apply
else
	"$CF_TOOL" update
fi
ufw status verbose
