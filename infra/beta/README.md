# infra/beta – Athenos-Beta-Server als Code

Alles, was den Beta-Server `app.athenos.dev` aufsetzt und betreibt. Die Anleitung Schritt für Schritt steht in **[RUNBOOK.md](RUNBOOK.md)**.

## Aufbau auf dem Server

```
Browser ── Cloudflare (HTTPS, Cache für gehashte Dateien, Proxy)
  └─ 443/tcp nur aus Cloudflare-Bereichen ── Caddy (Origin-CA-Zertifikat, HSTS,
       CF-Connecting-IP → X-Real-IP, Access-Log 7 T)
                                  └─ 127.0.0.1:41002  static edge (athenos-web, Host)
                                       └─ /gateway/* → 127.0.0.1:8080 control plane
                                            └─ athenos-api → athenos, auth-service, notification-service
athenos.service (User athenos) ── Unix-Socket, peer ── postgresql@16-main (Distro-Paket)
```

| Pfad | Inhalt |
|---|---|
| `/opt/athenos/releases/<id>/` | ein Release: Host-Binaries, `config/`, Host-Migrationen, `modules/<modul>/` (gehört root, nur lesbar) |
| `/opt/athenos/current` | Symlink auf das aktive Release (wird atomar umgestellt) |
| `/opt/athenos/shared/models/` | Athenos-AI-Modellpakete (`model-upload.sh`) |
| `/var/lib/athenos/` | Zustand: Host-Runtime, Modul-Runtime (Ports, Modul-Logs, Setup-Token), Monitor-Status |
| `/var/log/athenos/` | DB-Log des Hosts mit Archiv (der Host rotiert, 7 Tage); das Deploy-Log liegt in `/var/log/athenos-deploy.log` |
| `/var/backups/athenos/` | nächtliche `pg_dump`-Dateien mit `.sha256` und `.manifest.tsv` (14 Tage) |
| `/etc/athenos/` | `fenrir.env`, `backup.env`, `monitor.env` (ohne Geheimnisse), `secrets/` (0700, Dateien 0600) |
| `/usr/local/lib/athenos/` | Server-Skripte aus `server/`; `/usr/local/sbin/athenos-wartung` zeigt auf `athenos-wartung.sh` |
| `/usr/local/share/athenos/maintenance/` | die Wartungsseite (`install.sh maintenance`) |
| `/var/lib/athenos-maintenance/` | `status.json`, solange die Wartungsseite an ist (root schreibt, Caddy liest) |

## Dateien

| Datei | Zweck |
|---|---|
| `RUNBOOK.md` | Betriebsanleitung (Deutsch): Server mieten, DNS, Installation, Deploy, Rollback, Backup, Monitoring, Notfall, Checkliste |
| `deploy.sh` | vom Mac: Release-Tag oder lokalen Artefakt-Ordner prüfen, hochladen, ausrollen (mit Wartungsseite während des Neustarts, `--no-maintenance`), bei Fehler automatisch zurück |
| `admin-tunnel.sh` | vom Mac (`frprodapp`): SSH-Tunnel `localhost:41902` → Edge des Servers, am Wartungsmodus vorbei (RUNBOOK 5.4) |
| `maintenance/www/` | die statische Wartungsseite (DE/EN, hell/dunkel, pollt und lädt die App von selbst), Impressum-Kopie, Geist, Favicon; Caddy liefert sie aus `/usr/local/share/athenos/maintenance` |
| `server/athenos-wartung.sh` | `sudo athenos-wartung an <update\|fix\|wartung> [--bis HH:MM \| --minuten N] ["Text"]`, `aus`, `status` (RUNBOOK 5.3) |
| `rollback.sh` | vom Mac: auf ein früheres Release zurück (optional mit DB-Dump) |
| `model-upload.sh` | vom Mac: KI-Modellpaket prüfen und hochladen |
| `config/beta.toml` | Host-Konfiguration des Servers: mit `FENRIR_ENV=prod` (Produktionsprüfungen) und `FENRIR_CONFIG_ENV=beta` statt `prod.toml`; Distro-Postgres über Socket, eingebettete Identität, absolute Pfade |
| `env/*.example` | Vorlagen für `/etc/athenos/*.env` und `~/.config/athenos/deploy.env` |
| `systemd/` | `athenos.service` (gehärtet), Backup-, Healthcheck- und Alarm-Units, Drop-in für Postgres/Caddy |
| `caddy/Caddyfile` | Origin von `app.athenos.dev` hinter Cloudflare: Wartungsseite per Schalterdatei (geplant) und bei 502/503/504 (ungeplant), Origin-CA-Zertifikat (optional Authenticated Origin Pulls), HSTS, echte IP aus `CF-Connecting-IP` nur von Cloudflare, nur `/gateway/services/<api-gateway>` und die Web-App, Log mit geschwärzten Tokens |
| `firewall/ufw.sh`, `firewall/nftables.conf` | eingehend nur 22 (begrenzt, nur Schlüssel) und 443/tcp aus den Cloudflare-Bereichen |
| `cloudflare/ips.txt`, `server/cloudflare-ips.sh` | die Cloudflare-Bereiche: eine Liste für Caddy (`trusted_proxies`) und die Firewall; wöchentlich aktualisiert (`athenos-cloudflare-ips.timer`) |
| `postgres/` | `conf.d/athenos.conf` (nur Socket) und `pg_hba.conf` (nur peer) |
| `journald/`, `logrotate/` | Logs 7 Tage |
| `ssh/` | sshd: nur Schlüssel, kein root |
| `server/install.sh` | Erstinstallation (idempotent, in Schritten) |
| `server/athenos-release.sh` | Server-Seite von Deploy/Rollback: prüfen, entpacken, Dump, Umschalten, Health, Rückfall, Aufräumen (5 Releases) |
| `server/backup.sh`, `server/restore.sh` | Backup (Rotation 14 T, Off-site, Heartbeat) und geprüftes Zurückspielen in eine frische DB |
| `server/healthcheck.sh`, `server/alert.sh` | Überwachung alle 5 min und Alarm-Mail |
| `server/secret.sh`, `server/set-fenrir-admin-password.sh`, `server/bootstrap-admin.sh` | Geheimnisse, Fenrir-Admin, erster Athenos-Admin (nur über die Konsole, siehe RUNBOOK §4) |
| `ci/package-host.sh` | Release-Bündel des Hosts, `fenrir-<version>-linux-x86_64.tar.gz` (von `.github/workflows/release.yml`) |
| `tests/check.sh` | statische Prüfung (bash -n, TOML, Keyring, Vorlagen ohne Geheimnisse, `caddy validate`, Wartungsseite ohne fremde Quellen), dazu `tests/bootstrap-admin-test.sh`, `tests/maintenance-test.sh` und `infra/release/tests/release-test.sh` (release.sh in einer Sandbox, Workflow-Prüfung) |
| `tests/maintenance-test.sh` | `athenos-wartung.sh` (Schreiben, Status, Ablehnungen), `athenos-release.sh activate --maintenance` gegen Attrappen (an beim Neustart, aus wenn gesund, an wenn nichts gesund läuft) und `deploy.sh` mit einem falschen ssh |
| `tests/bootstrap-admin-test.sh` | `bootstrap-admin.sh` gegen eine Attrappe der API: verweigert bei aktivem Konto, nie Token oder Passwort in Ausgabe oder argv, Normalfall mit falschem PIN und neuem Code |
| `tests/config-check.sh` | `fenrir --check-config` im Produktionsmodus mit `beta.toml` und der Env-Vorlage; prüft auch, dass Dev-Einstellungen und das Dev-Profil abgelehnt werden |
| `tests/backup-restore-proof.sh` | Backup → Rotation → Restore → Vergleich gegen ein Wegwerf-PostgreSQL 16 |
