#!/usr/bin/env bash
# Host firewall of the Athenos beta with ufw (run as root; idempotent).
#
# Open: 22/tcp (rate-limited: 6 new connections per 30 s and source),
# 80/tcp (ACME + redirect), 443/tcp and 443/udp (HTTP/3). Everything else
# inbound is closed, including 8080, 41000-46000, 50051, 2222 and 5432,
# which bind to 127.0.0.1 anyway. IPv4 and IPv6. Outgoing stays open
# (Mailgun SMTP, ACME, apt, off-site backup).
#
# Add the Hetzner Cloud Firewall (same rules) in front as a second layer.
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }
command -v ufw >/dev/null || { echo "ufw missing: apt install ufw" >&2; exit 1; }

SSH_PORT=${SSH_PORT:-22}

sed -i 's/^IPV6=.*/IPV6=yes/' /etc/default/ufw
ufw --force reset >/dev/null
ufw default deny incoming
ufw default allow outgoing
ufw default deny routed
ufw limit "$SSH_PORT/tcp" comment 'ssh (rate-limited)'
ufw allow 80/tcp comment 'http: ACME and redirect to https'
ufw allow 443/tcp comment 'https'
ufw allow 443/udp comment 'https over HTTP/3'
ufw logging low
ufw --force enable
ufw status verbose
