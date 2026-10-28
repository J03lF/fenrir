#!/usr/bin/env bash
# First installation of the Athenos beta server (Ubuntu 24.04 LTS), as root,
# from a copy of infra/beta (or later from /opt/athenos/current/infra/beta):
#
#   sudo ./server/install.sh all --admin-user <name>
#   sudo ./server/install.sh <step> [--admin-user <name>]
#
# Steps, in the order of `all` (each one idempotent):
#   packages  apt: PostgreSQL 16, Caddy (official repo), ufw, jq, curl, rsync,
#             age, msmtp-mta, unattended-upgrades
#   user      system user `athenos` (no login shell, no home login)
#   dirs      /opt/athenos/{releases,shared}, /var/lib/athenos, /var/log/athenos,
#             /var/backups/athenos, /etc/athenos (+ secrets 0700)
#   postgres  conf.d/athenos.conf, pg_hba.conf (socket + peer only), role and
#             database `athenos` (owner, no superuser, collation C)
#   config    /etc/athenos/{fenrir,backup,monitor}.env from the templates (only
#             if missing), generated secrets (only if missing), seeded ports.json
#   units     scripts to /usr/local/lib/athenos, systemd units and drop-ins,
#             journald (7 days), logrotate (7 days); enables the timers and
#             athenos.service (started by the first deploy)
#   cloudflare  fetch the Cloudflare ranges (Caddy trusted proxies, firewall)
#   caddy     /etc/caddy/Caddyfile, validate, reload; needs the Cloudflare
#             Origin CA certificate in /etc/caddy/origin first (RUNBOOK 2.3)
#   updates   unattended security upgrades with automatic reboot at night
#   firewall  ufw: 22 (limited, key-only), 443/tcp from Cloudflare only
#   ssh       sshd: keys only, no root, AllowUsers <admin> (refuses without a key)
#
# Nothing here starts Athenos or touches application data.
set -euo pipefail

BASE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=common.sh
. "$BASE/server/common.sh"

SERVICE_USER=athenos
PG_VERSION=16
PG_CLUSTER=main
ADMIN_USER=${SUDO_USER:-}

[ "$(id -u)" = 0 ] || die "run as root (sudo)"

step=${1:-}
shift || true
force_os=0
while [ $# -gt 0 ]; do
	case $1 in
	--admin-user)
		ADMIN_USER=${2:?--admin-user needs a name}
		shift 2
		;;
	--force-os)
		force_os=1
		shift
		;;
	*) die "unknown argument: $1" ;;
	esac
done

check_os() {
	# The CI builds on ubuntu-24.04 (glibc 2.39, OpenSSL 3): the binaries need
	# a system at least that new.
	. /etc/os-release
	if [ "${ID:-}" != ubuntu ] || [ "${VERSION_ID:-}" != 24.04 ]; then
		[ "$force_os" = 1 ] || die "expected Ubuntu 24.04 LTS, found ${PRETTY_NAME:-unknown} (--force-os to continue)"
		warn "not Ubuntu 24.04 (${PRETTY_NAME:-unknown}); continuing because of --force-os"
	fi
}

step_packages() {
	check_os
	export DEBIAN_FRONTEND=noninteractive
	apt-get update
	apt-get install -y --no-install-recommends \
		"postgresql-$PG_VERSION" postgresql-client-common ufw jq curl rsync openssl \
		age msmtp-mta unattended-upgrades ca-certificates gnupg \
		debian-keyring debian-archive-keyring apt-transport-https
	if ! command -v caddy >/dev/null 2>&1; then
		# Official Caddy repository (https://caddyserver.com/docs/install#debian-ubuntu-raspbian).
		local key=/usr/share/keyrings/caddy-stable-archive-keyring.gpg
		curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor --yes -o "$key"
		chmod 0644 "$key"
		log "Caddy repository key fingerprint (compare with the Caddy install docs):"
		gpg --show-keys --with-fingerprint "$key" >&2 || true
		curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' >/etc/apt/sources.list.d/caddy-stable.list
		chmod 0644 /etc/apt/sources.list.d/caddy-stable.list
		apt-get update
		apt-get install -y caddy
	fi
	caddy version
	psql --version
}

step_user() {
	if ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
		useradd --system --user-group --home-dir /var/lib/athenos --no-create-home \
			--shell /usr/sbin/nologin --comment "Athenos service" "$SERVICE_USER"
		log "created system user $SERVICE_USER"
	fi
	usermod --shell /usr/sbin/nologin "$SERVICE_USER"
	# The Postgres socket is 0770 postgres:postgres (postgres/athenos.conf);
	# peer auth still decides which role the user may be.
	usermod -aG postgres "$SERVICE_USER"
	passwd --lock "$SERVICE_USER" >/dev/null
}

step_dirs() {
	install -d -m 0755 -o root -g root /opt/athenos /opt/athenos/releases /opt/athenos/shared /opt/athenos/shared/models
	install -d -m 0750 -o "$SERVICE_USER" -g "$SERVICE_USER" /var/lib/athenos \
		/var/lib/athenos/runtime /var/lib/athenos/modules-runtime /var/lib/athenos/modules-cache \
		/var/lib/athenos/monitor /var/log/athenos
	install -d -m 0700 -o "$SERVICE_USER" -g "$SERVICE_USER" /var/lib/athenos/keys /var/lib/athenos/keys/ssh
	install -d -m 0700 -o "$SERVICE_USER" -g "$SERVICE_USER" /var/backups/athenos
	install -d -m 0750 -o root -g "$SERVICE_USER" /etc/athenos
	install -d -m 0700 -o "$SERVICE_USER" -g "$SERVICE_USER" /etc/athenos/secrets
	install -d -m 0755 -o root -g root /usr/local/lib/athenos
	log "directories ready"
}

pg_conf_dir() { printf '/etc/postgresql/%s/%s\n' "$PG_VERSION" "$PG_CLUSTER"; }

step_postgres() {
	local conf
	conf=$(pg_conf_dir)
	[ -d "$conf" ] || die "no PostgreSQL $PG_VERSION cluster '$PG_CLUSTER' ($conf); run the packages step"
	install -d -m 0755 -o postgres -g postgres "$conf/conf.d"
	install -m 0644 -o postgres -g postgres "$BASE/postgres/athenos.conf" "$conf/conf.d/athenos.conf"
	if ! cmp -s "$BASE/postgres/pg_hba.conf" "$conf/pg_hba.conf"; then
		[ -e "$conf/pg_hba.conf.orig" ] || cp -p "$conf/pg_hba.conf" "$conf/pg_hba.conf.orig"
		install -m 0640 -o postgres -g postgres "$BASE/postgres/pg_hba.conf" "$conf/pg_hba.conf"
	fi
	grep -q "^include_dir = 'conf.d'" "$conf/postgresql.conf" || die "$conf/postgresql.conf has no include_dir = 'conf.d'"
	systemctl enable "postgresql@$PG_VERSION-$PG_CLUSTER.service"
	systemctl restart "postgresql@$PG_VERSION-$PG_CLUSTER.service"
	local psql=(runuser -u postgres -- psql -X -q -v ON_ERROR_STOP=1 -d postgres)
	"${psql[@]}" -c "DO \$\$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '$SERVICE_USER') THEN CREATE ROLE $SERVICE_USER LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION; END IF; END \$\$;"
	if [ "$("${psql[@]}" -A -t -c "SELECT 1 FROM pg_database WHERE datname = 'athenos'")" != 1 ]; then
		# Collation C (like the dev cluster): byte order, and no index
		# corruption when a glibc update changes locale rules.
		"${psql[@]}" -c "CREATE DATABASE athenos OWNER $SERVICE_USER TEMPLATE template0 ENCODING 'UTF8' LC_COLLATE 'C' LC_CTYPE 'C'"
		log "created database athenos"
	fi
	"${psql[@]}" -c "REVOKE ALL ON DATABASE athenos FROM PUBLIC"
	log "postgres ready, peer login as $SERVICE_USER: $(runuser -u "$SERVICE_USER" -- psql -X -A -t -d athenos -c 'SELECT current_user')"
}

seed_ports() {
	# Fixed module ports (the host keeps assignments from ports.json): the
	# static edge must be 41002, Caddy proxies there.
	local ports=/var/lib/athenos/modules-runtime/ports.json
	[ ! -e "$ports" ] || return 0
	cat >"$ports" <<'JSON'
{
  "athenos": 41000,
  "athenos-api": 41001,
  "athenos-web": 41002,
  "auth-service": 41004,
  "notification-service": 41005
}
JSON
	chown "$SERVICE_USER:$SERVICE_USER" "$ports"
	chmod 0640 "$ports"
	log "seeded $ports"
}

install_template() { # template dest
	if [ -e "$2" ]; then
		log "$2 exists, kept"
		return 0
	fi
	install -m 0640 -o root -g "$SERVICE_USER" "$1" "$2"
	log "installed $2 (review it)"
}

step_config() {
	install_template "$BASE/env/fenrir.env.example" /etc/athenos/fenrir.env
	install_template "$BASE/env/backup.env.example" /etc/athenos/backup.env
	install_template "$BASE/env/monitor.env.example" /etc/athenos/monitor.env
	local s
	for s in control-admin.token control-operator.token control-viewer.token registry.token \
		athenos-internal.token notify-service.token auth-pin-pepper intel-token.key; do
		SECRETS_DIR=/etc/athenos/secrets "$BASE/server/secret.sh" generate "$s"
	done
	[ -e /etc/athenos/secrets/smtp-password ] ||
		warn "no SMTP password yet: sudo /usr/local/lib/athenos/secret.sh set smtp-password"
	seed_ports
}

step_units() {
	local f
	for f in "$BASE"/server/*.sh; do
		install -m 0755 -o root -g root "$f" /usr/local/lib/athenos/
	done
	for f in "$BASE"/systemd/*.service "$BASE"/systemd/*.timer; do
		install -m 0644 -o root -g root "$f" /etc/systemd/system/
	done
	local unit
	for unit in "postgresql@$PG_VERSION-$PG_CLUSTER.service" caddy.service; do
		install -d -m 0755 "/etc/systemd/system/$unit.d"
		install -m 0644 "$BASE/systemd/dropins/alert-on-failure.conf" "/etc/systemd/system/$unit.d/athenos-alert.conf"
	done
	install -d -m 0755 /etc/systemd/journald.conf.d
	install -m 0644 "$BASE/journald/athenos.conf" /etc/systemd/journald.conf.d/athenos.conf
	systemctl restart systemd-journald
	install -m 0644 "$BASE/logrotate/athenos" /etc/logrotate.d/athenos
	# Distro logs with IP addresses or statements: 7 days as well.
	if [ -f /etc/logrotate.d/postgresql-common ]; then
		sed -i -e 's/^\(\s*\)weekly/\1daily/' -e 's/^\(\s*\)rotate [0-9]*/\1rotate 7/' /etc/logrotate.d/postgresql-common
	fi
	if [ -f /etc/logrotate.d/rsyslog ]; then
		sed -i -e 's/^\(\s*\)weekly/\1daily/' -e 's/^\(\s*\)rotate [0-9]*/\1rotate 7/' /etc/logrotate.d/rsyslog
	fi
	systemctl daemon-reload
	systemctl enable athenos.service
	systemctl enable --now athenos-backup.timer athenos-healthcheck.timer athenos-cloudflare-ips.timer
	log "units installed; athenos.service starts with the first deploy"
}

step_cloudflare() {
	"$BASE/server/cloudflare-ips.sh" update
}

step_caddy() {
	install -d -m 0750 -o caddy -g caddy /var/log/caddy
	install -d -m 0750 -o root -g caddy /etc/caddy/origin
	local cert=/etc/caddy/origin/app.athenos.dev.pem key=/etc/caddy/origin/app.athenos.dev.key
	if [ ! -s "$cert" ] || [ ! -s "$key" ]; then
		warn "no Cloudflare Origin CA certificate yet ($cert, $key): Caddyfile not installed."
		warn "Create it in Cloudflare (RUNBOOK 2.3), then run: sudo $BASE/server/install.sh caddy"
		return 0
	fi
	chown root:caddy "$cert" "$key"
	chmod 0644 "$cert"
	chmod 0640 "$key"
	[ -s /etc/caddy/cloudflare-trusted-proxies.caddy ] || "$BASE/server/cloudflare-ips.sh" update
	[ -e /etc/caddy/Caddyfile.orig ] || [ ! -e /etc/caddy/Caddyfile ] || cp -p /etc/caddy/Caddyfile /etc/caddy/Caddyfile.orig
	install -m 0644 -o root -g root "$BASE/caddy/Caddyfile" /etc/caddy/Caddyfile
	# `caddy validate` opens the log files. Run as root it created the access
	# log root:root, and the caddy service could not open it on the reload
	# (permission denied). So: the log belongs to caddy (an earlier run's
	# root-owned file too), and the validation runs as the caddy user, with
	# the same rights as the service.
	local access_log=/var/log/caddy/app.athenos.dev.access.log
	[ -e "$access_log" ] || install -m 0640 -o caddy -g caddy /dev/null "$access_log"
	find /var/log/caddy -maxdepth 1 -type f -name 'app.athenos.dev.access*' ! -user caddy -exec chown caddy:caddy {} +
	runuser -u caddy -- caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
	systemctl enable caddy
	systemctl reload-or-restart caddy
}

step_updates() {
	cat >/etc/apt/apt.conf.d/20auto-upgrades <<'EOF'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
APT::Periodic::AutocleanInterval "7";
EOF
	cat >/etc/apt/apt.conf.d/52athenos-unattended-upgrades <<'EOF'
// Athenos beta: security updates every night, reboot at 03:30 UTC if a
// kernel or libc update needs it (all units come back by themselves).
Unattended-Upgrade::Automatic-Reboot "true";
Unattended-Upgrade::Automatic-Reboot-Time "03:30";
Unattended-Upgrade::Remove-Unused-Kernel-Packages "true";
Unattended-Upgrade::Remove-Unused-Dependencies "true";
EOF
	systemctl enable --now unattended-upgrades.service
	unattended-upgrade --dry-run >/dev/null 2>&1 || warn "unattended-upgrade dry run reported a problem"
}

step_firewall() { "$BASE/firewall/ufw.sh"; }

step_ssh() {
	[ -n "$ADMIN_USER" ] || die "--admin-user <name> is required for the ssh step"
	id -u "$ADMIN_USER" >/dev/null 2>&1 || die "no such user: $ADMIN_USER"
	local home keys
	home=$(getent passwd "$ADMIN_USER" | cut -d: -f6)
	keys="$home/.ssh/authorized_keys"
	[ -s "$keys" ] || die "$keys is empty: add your public key first, or you lock yourself out"
	sed "s/ADMIN_USER/$ADMIN_USER/" "$BASE/ssh/10-athenos-hardening.conf" >/etc/ssh/sshd_config.d/10-athenos-hardening.conf
	chmod 0644 /etc/ssh/sshd_config.d/10-athenos-hardening.conf
	sshd -t || die "sshd -t failed; fix /etc/ssh/sshd_config.d/10-athenos-hardening.conf"
	systemctl reload ssh
	log "sshd hardened. Keep this session open and test a new login before closing it."
}

case $step in
all)
	for s in packages user dirs postgres config units cloudflare caddy updates firewall ssh; do
		log "=== $s"
		"step_$s"
	done
	;;
packages | user | dirs | postgres | config | units | cloudflare | caddy | updates | firewall | ssh) "step_$step" ;;
*)
	sed -n '2,26p' "$0"
	exit 1
	;;
esac
