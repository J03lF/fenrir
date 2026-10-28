# Runbook: Athenos-Beta auf app.athenos.dev

Diese Anleitung führt dich vom gemieteten Server bis zur offenen Beta und durch den Betrieb danach. Alle Befehle mit `sudo` laufen auf dem Server, alle mit `infra/beta/…` auf deinem Mac im Fenrir-Checkout.

Kurz zum Aufbau (Details in [README.md](README.md)):
- **Caddy** nimmt HTTPS an und leitet nur an `127.0.0.1:41002` weiter (die Static-Edge des Hosts).
- **athenos.service** ist der Fenrir-Host. Er startet die Module als Kindprozesse.
- **PostgreSQL 16** ist das Paket der Distribution. Es hört nur auf einem Unix-Socket; der Host meldet sich per *peer* als Systembenutzer `athenos` an. Es gibt kein Datenbank-Passwort.

---

## 1. Server mieten

### Grösse

| Teil | RAM | Platte |
|---|---|---|
| Ubuntu 24.04 + Caddy | 0,5 GB | 5 GB |
| PostgreSQL 16 (`shared_buffers` 1 GB) | 1–1,5 GB | Beta: < 1 GB |
| Host + 4 Module | 0,5–1 GB | 5 Releases × ca. 150 MB |
| Athenos AI (Modell 1,1 GB auf der Platte, im Speicher mehr) | 1,5–2,5 GB | 1,1 GB je Modellversion |
| Backups (14 Tage + Dumps vor jedem Deploy) | – | ein paar GB |
| Journal (max. 1 GB) | – | 1 GB |

- **Empfehlung: 4 vCPU, 8 GB RAM, ≥ 80 GB SSD, x86_64.** 4 GB RAM reichen mit dem KI-Modell nicht sicher.
- **Architektur x86_64 (Intel/AMD), nicht ARM:** Die CI baut `x86_64-unknown-linux-gnu`.
- **Betriebssystem: Ubuntu 24.04 LTS.** Die Binaries sind auf `ubuntu-24.04` gebaut und brauchen glibc ≥ 2.39 und OpenSSL 3. Debian 12 ist zu alt.

### Anbieter

| | Hetzner Cloud (DE) | Infomaniak (CH) |
|---|---|---|
| Typ | z. B. CX33/CX32 (4 vCPU geteilt, 8 GB, 80 GB) oder CPX31 (4 vCPU AMD, 8 GB, 160 GB) | VPS Cloud / VPS Lite mit 4 vCPU, 8 GB |
| Preis ca. | 7–17 € / Monat | 15–35 CHF / Monat |
| Standort | Falkenstein oder Nürnberg (Deutschland) | Genf oder Winterthur (Schweiz) |
| Off-site-Backup | Storage Box BX11 (1 TB, ca. 4 € / Monat), anderer Standort wählen | Swiss Backup (S3/rclone) |
| Vertrag zur Auftragsverarbeitung (AVV/DPA) | in der Cloud Console abschliessen | in der Verwaltung abschliessen |

Die Namen und Preise der Server ändern sich. **Prüfe sie beim Bestellen.**

Für die Datenschutzerklärung (Platzhalter `[HOSTER, LAND]`, später in Admin → Einstellungen → `legal_hoster`) schreibst du zum Beispiel:
- „Hetzner Online GmbH, Deutschland (Rechenzentrum Nürnberg)“ oder
- „Infomaniak Network SA, Schweiz“.

Wenn der Off-site-Speicher woanders steht, nennst du ihn mit. Der Website-Server und der Fenrir-Server stehen beim selben Hoster (so steht es im Entwurf der Datenschutzerklärung).

### Beim Bestellen
1. Image **Ubuntu 24.04**. IPv4 und IPv6 an.
2. Deinen **SSH-Public-Key** hinterlegen. Kein Root-Passwort.
3. **Cloud-Firewall des Anbieters** dazu (zweite Schicht vor `ufw`). Eingehend nur 22/tcp, 80/tcp, 443/tcp und 443/udp.
4. Optional die Snapshots/Backups des Anbieters. Sie ersetzen den eigenen Dump nicht und laufen ausserhalb der 14-Tage-Frist: Prüfe deren Aufbewahrung, bevor du sie einschaltest.
5. Den AVV/DPA mit dem Anbieter abschliessen und ablegen.

---

## 2. DNS in Cloudflare

Zone `athenos.dev`:

| Typ | Name | Inhalt | Proxy | TTL |
|---|---|---|---|---|
| A | `app` | IPv4 des Servers | **aus** (graue Wolke, „DNS only“) | 5 min |
| AAAA | `app` | IPv6 des Servers | **aus** | 5 min |
| CAA | `app` | `0 issue "letsencrypt.org"` | – | Auto |
| CAA | `app` | `0 issue "sectigo.com"` (ZeroSSL, Caddys Ausweich-CA) | – | Auto |
| CAA | `app` | `0 iodef "mailto:privacy@athenos.dev"` | – | Auto |

**Warum der Proxy zuerst aus bleibt:**
- Caddy holt das Zertifikat direkt bei Let's Encrypt (HTTP-01/TLS-ALPN). Mit Proxy müsste die Verschlüsselung zweimal stimmen („Full (strict)“).
- Caddy sieht die echte Besucher-IP als TCP-Gegenstelle. Die Rate-Limits und die Login-Sperren hängen daran (SPEC §2). Mit Proxy käme jeder Besuch von einer Cloudflare-Adresse.
- Mit Proxy entschlüsselt Cloudflare jeden Aufruf, auch Passwörter und Tickets. Cloudflare wäre dann ein weiterer Auftragsverarbeiter in der Datenschutzerklärung.
- Event-Streams (Live-Tickets) laufen ohne Zwischenpuffer, und die Fehlersuche ist einfacher.

**Später einschalten geht so:**
1. Im Caddyfile unter `servers` die Cloudflare-Bereiche als `trusted_proxies static …` eintragen und `client_ip_headers CF-Connecting-IP` setzen.
2. In Cloudflare SSL auf „Full (strict)“ stellen.
3. Die Datenschutzerklärung ergänzen.

**Nicht anfassen:**
- die Einträge von Mailgun unter `mail.athenos.dev` (MX, SPF-TXT, DKIM, CNAME für Tracking);
- das Email Routing der Hauptdomain (`privacy@athenos.dev`);
- den DMARC-Eintrag.

Die CAA-Einträge liegen bewusst nur auf `app`, damit sie die Website unter `athenos.dev` nicht betreffen.

**Prüfen:** `dig +short app.athenos.dev A`, `dig +short app.athenos.dev AAAA` und `dig app.athenos.dev CAA`.

---

## 3. Erstinstallation

### 3.1 Admin-Konto auf dem Server
Als `root` per Schlüssel einloggen (nur dieses eine Mal):

```bash
adduser joel                      # Passwort für sudo setzen (Passwortmanager)
usermod -aG sudo joel
install -d -m 700 -o joel -g joel /home/joel/.ssh
cp /root/.ssh/authorized_keys /home/joel/.ssh/ && chown joel:joel /home/joel/.ssh/authorized_keys
hostnamectl set-hostname athenos-beta
```

Dann **in einem zweiten Terminal** `ssh joel@<server>` testen. Erst danach `root` verlassen.

Auf dem Mac in `~/.ssh/config`:
```
Host athenos-beta
  HostName app.athenos.dev
  User joel
  IdentityFile ~/.ssh/id_ed25519
```
Dazu `cp infra/beta/env/deploy.env.example ~/.config/athenos/deploy.env`. Dort steht `DEPLOY_HOST=athenos-beta`.

### 3.2 infra/beta auf den Server kopieren und installieren
Auf dem Mac, im Fenrir-Checkout auf dem Stand, den du ausrollen willst:
```bash
rsync -a infra/beta/ athenos-beta:athenos-beta/
```
Auf dem Server:
```bash
sudo ~/athenos-beta/server/install.sh all --admin-user joel
```

Das Skript arbeitet diese Schritte ab. Jeder einzelne lässt sich mit `install.sh <schritt>` wiederholen.

| Schritt | Was passiert |
|---|---|
| `packages` | PostgreSQL 16, Caddy (offizielles Repository; vergleiche den angezeigten Schlüssel-Fingerprint mit der Caddy-Doku), ufw, jq, age, msmtp, unattended-upgrades |
| `user` | Systembenutzer `athenos` ohne Login-Shell |
| `dirs` | alle Ordner mit Rechten, siehe README |
| `postgres` | nur Socket, nur peer; Rolle und DB `athenos` (Besitzer, kein Superuser, Sortierung `C`) |
| `config` | `/etc/athenos/{fenrir,backup,monitor}.env` aus den Vorlagen; Tokens erzeugen; `ports.json` vorbelegen (die Edge muss 41002 sein) |
| `units` | Skripte nach `/usr/local/lib/athenos`, systemd-Units, Journal und logrotate auf 7 Tage, Timer an |
| `caddy` | Caddyfile, `caddy validate`, neu laden. Das Zertifikat kommt, sobald das DNS zeigt |
| `updates` | nächtliche Sicherheitsupdates, Neustart bei Bedarf um 03:30 UTC |
| `firewall` | ufw: 22 begrenzt, 80, 443 tcp+udp; alles andere zu |
| `ssh` | nur Schlüssel, kein root, `AllowUsers joel`. **Die offene Sitzung erst schliessen, wenn ein neuer Login klappt** |

Bestehende Dateien in `/etc/athenos` überschreibt das Skript nicht.

### 3.3 Einstellungen und Geheimnisse
1. **SMTP-Passwort von Mailgun** (Sending → Domain settings → SMTP credentials):
   ```bash
   sudo /usr/local/lib/athenos/secret.sh set smtp-password
   ```
2. **`/etc/athenos/fenrir.env` prüfen:**
   - `SMTP_USER` ist der SMTP-Login von `mail.athenos.dev`.
   - `ATHENOS_QUALITY_MODEL` ist das Modell, das du hochlädst.
   - Für einen geschlossenen Test vorerst `NOTIFY_RECIPIENT_ALLOWLIST`.
   - Kein `AUTH_ENV`, kein `AUTH_DEV_PIN_SINK`, kein `*_DEV_*`.
3. **`/etc/athenos/monitor.env`:** `ALERT_EMAIL_TO` auf deine Adresse setzen.
4. **`/etc/athenos/backup.env`:** das Off-site-Ziel eintragen, siehe 6.3.
5. **Fenrir-Admin** für die SSH-Shell des Hosts setzen (das ist nicht das Athenos-Konto):
   ```bash
   sudo /usr/local/lib/athenos/set-fenrir-admin-password.sh
   ```
6. **Mail-Notweg für Alarme** (falls Athenos selbst nicht läuft): `/etc/msmtprc` mit Rechten `0640 root:athenos` anlegen:
   ```
   defaults
   auth on
   tls on
   tls_trust_file /etc/ssl/certs/ca-certificates.crt
   account mailgun
   host smtp.eu.mailgun.org
   port 587
   from alerts@mail.athenos.dev
   user postmaster@mail.athenos.dev
   passwordeval cat /etc/athenos/secrets/smtp-password
   account default : mailgun
   ```
   Testen mit:
   ```bash
   printf 'Test\n' | sudo -u athenos /usr/local/lib/athenos/alert.sh message "Testalarm"
   ```

### 3.4 KI-Modell hochladen
Auf dem Mac:
```bash
infra/beta/model-upload.sh ~/Documents/personal/modules/models/athenos-quality/0.5.0-beta4
```
Das Skript prüft die `SHA256SUMS` des Pakets vor und nach dem Hochladen und legt es schreibgeschützt nach `/opt/athenos/shared/models/athenos-quality/0.5.0-beta4`.

### 3.5 Erster Deploy
DNS zeigt auf den Server (siehe 2). Dann wie in 5.1 einen Release ausrollen:
```bash
infra/beta/deploy.sh v0.11.0
```
Danach prüfen:
```bash
sudo /usr/local/lib/athenos/healthcheck.sh      # alle Zeilen "ok"
curl -sS https://app.athenos.dev/healthz
```
Im Browser:
- `https://app.athenos.dev` lädt ohne Konsolenfehler.
- Das Schloss zeigt ein gültiges Zertifikat.
- `curl -sI https://app.athenos.dev` zeigt `strict-transport-security`.

---

## 4. Erstes Admin-Konto

Solange es **kein aktives Konto** gibt, schreibt der Core einen Einmal-Token. Er gilt 24 Stunden und wird mit der Bestätigung verbraucht.

1. Token anzeigen (nur auf dem Server, nirgends hinkopieren ausser ins Formular):
   ```bash
   sudo cat /var/lib/athenos/modules-runtime/secrets/athenos-setup-token
   ```
2. `https://app.athenos.dev/setup` öffnen. Token, Name, E-Mail und Passwort eingeben, der Zustimmung zustimmen. Dann den PIN aus der Mail eingeben. Du bist als Plattform-Admin angemeldet.
3. **Notweg**, falls die Seite nicht geht:
   ```bash
   sudo /usr/local/lib/athenos/bootstrap-admin.sh du@example.org "Dein Name"
   ```
4. Ist der Token abgelaufen und noch kein Konto aktiv, startet `sudo systemctl restart athenos` einen neuen.
5. Danach in **Admin → Einstellungen**:
   - `legal_hoster` (zum Beispiel „Hetzner Online GmbH, Deutschland“) und `legal_effective_date`;
   - Registrierungsmodus;
   - Beta-Banner an;
   - Sitzungsdauer.

---

## 5. Deploy

### 5.1 Neue Version ausrollen
1. In jedem Repo ist der Stand gepusht. Den gleichen Tag setzt du in allen sechs Repos (fenrir, athenos, athenos-api, auth-service, notification-service, athenos-web):
   ```bash
   git tag -a v0.11.0 -m v0.11.0 && git push origin v0.11.0
   ```
   - Die CI baut das Linux-Bündel erst nach den Qualitäts-Toren.
   - Sie legt `<modul>-<version>-x86_64-unknown-linux-gnu.tar.gz` (Web: `athenos-web-<version>-static.tar.gz`) und `SHA256SUMS` an den GitHub-Release.
   - Hat ein Repo keinen neuen Tag, setzt du in `deploy.env` `TAG_<KOMPONENTE>=v…` (z. B. `TAG_AUTH_SERVICE=v0.1.0`).
2. Wenn alle CI-Läufe grün sind:
   ```bash
   infra/beta/deploy.sh v0.11.0
   ```
3. Ohne Tag geht es auch: einen manuellen CI-Lauf starten („Run workflow“), die Artefakte herunterladen und `infra/beta/deploy.sh ~/Downloads/athenos-artefakte/` aufrufen. Der Ordner hält die sechs Tarballs mit ihren `SHA256SUMS`, flach oder je Komponente ein Unterordner. `--dry-run` prüft nur.

**Was `deploy.sh` macht:**
1. alle `SHA256SUMS` lokal prüfen;
2. hochladen;
3. auf dem Server erneut prüfen und die Tarballs auf unsichere Pfade prüfen;
4. nach `/opt/athenos/releases/<tag>-<zeit>` entpacken und das Layout prüfen;
5. **DB-Dump vor dem Deploy** (scheitert er, ändert sich nichts);
6. `/opt/athenos/current` atomar umstellen;
7. `athenos` neu starten. Beim Start laufen die Migrationen;
8. bis 4 Minuten auf Gesundheit warten: Host bereit, Edge, API, Status nicht `major_outage`.

Bleibt die neue Version krank, schaltet das Skript **automatisch auf das vorige Release zurück** und startet neu. Die DB bleibt dabei, wie sie ist (die Migrationen sind additiv).

Mit `--restore-db-on-failure` spielt es zusätzlich den Dump von vor dem Deploy zurück. Was nach dem Dump geschrieben wurde, ist dann nicht mehr live; `restore.sh` behält die alte DB unter neuem Namen.

Es bleiben 5 Releases liegen, nie das aktuelle und das vorige. Das Log liegt in `/var/log/athenos/deploy.log`.

**Units geändert?** Meldet der Deploy „systemd units differ“:
```bash
sudo /opt/athenos/current/infra/beta/server/install.sh units
```
Danach `sudo systemctl restart athenos`.

---

## 6. Rollback, Backup, Restore

### 6.1 Rollback
```bash
infra/beta/rollback.sh --list            # Releases (* aktuell, p vorig) und die letzten Dumps
infra/beta/rollback.sh                   # zurück auf das vorige Release, DB bleibt
infra/beta/rollback.sh <release-id>      # auf ein bestimmtes Release
infra/beta/rollback.sh <id> --with-db /var/backups/athenos/athenos-<zeit>-predeploy-<id>.dump
```

- `--with-db` ersetzt die Live-DB durch den Dump. Der Server verlangt dafür, dass du `ROLLBACK` tippst.
- Die bisherige DB bleibt als `athenos_before_restore_<zeit>`. Löschen kannst du sie, wenn alles passt:
  ```bash
  sudo -u postgres psql -c 'DROP DATABASE …'
  ```

### 6.2 Backups
- **Jede Nacht um ca. 01:30 UTC:**
  - `pg_dump -Fc` nach `/var/backups/athenos/athenos-<zeit>.dump`;
  - dazu `.sha256` und `.manifest.tsv` (Zeilen pro Tabelle, aus dem Dump selbst gelesen);
  - Rotation nach **14 Tagen**; die neusten 3 bleiben immer;
  - danach die Off-site-Kopie und der Heartbeat.
- **Von Hand:** `sudo systemctl start athenos-backup.service`, dann `journalctl -u athenos-backup -n 20`.
- **Scheitert ein Backup**, kommt eine Alarm-Mail (OnFailure), und der Healthcheck meldet ein Backup, das älter als 26 h ist.

### 6.3 Off-site-Kopie einrichten (Beispiel Hetzner Storage Box)
1. Storage Box bestellen, an einem anderen Standort als der Server. SSH-Zugang aktivieren.
2. Schlüssel nur für Backups anlegen:
   ```bash
   sudo -u athenos ssh-keygen -t ed25519 -N '' -f /etc/athenos/secrets/offsite_ed25519
   ```
   Den Public Key in der Storage Box hinterlegen.
3. Host-Key festhalten:
   ```bash
   ssh-keyscan -p 23 u123456.your-storagebox.de | sudo tee /etc/athenos/offsite_known_hosts
   ```
   Den Fingerprint vergleichst du mit der Hetzner-Konsole.
4. In `/etc/athenos/backup.env` `OFFSITE_MODE=rsync` und die `OFFSITE_RSYNC_*`-Zeilen setzen.
5. Verschlüsseln (empfohlen): `age-keygen -o athenos-backup.key` **auf dem Mac**. Den privaten Schlüssel legst du in den Passwortmanager. Nur den Public Key (`age1…`) schreibst du nach `/etc/athenos/offsite-age-recipients.txt` und setzt `OFFSITE_AGE_RECIPIENTS_FILE`.
6. `sudo systemctl start athenos-backup.service` und prüfen, ob die Dateien drüben liegen.

Die Off-site-Kopie spiegelt den lokalen Bestand. Sie läuft also auch nach 14 Tagen ab, wie es die Datenschutzerklärung verspricht. Ein leerer lokaler Ordner wird nie gespiegelt.

### 6.4 Restore
- **Ausprobieren, ohne die Live-DB zu berühren:**
  ```bash
  sudo /usr/local/lib/athenos/restore.sh /var/backups/athenos/athenos-<zeit>.dump
  ```
  Das spielt in eine **frische** DB `athenos_restore_<zeit>` und prüft:
  1. die Prüfsumme;
  2. das Inhaltsverzeichnis;
  3. den Restore selbst, in einer Transaktion, beim ersten Fehler Abbruch;
  4. die Zeilenzahl jeder Tabelle gegen das Manifest.

  Eine bestehende Ziel-DB überschreibt es nur mit `--force`.
- **Wirklich zurückspielen:**
  ```bash
  sudo /usr/local/lib/athenos/restore.sh --swap --force /var/backups/athenos/athenos-<zeit>.dump
  ```
  Das Skript:
  1. stoppt `athenos`;
  2. benennt die Live-DB in `athenos_before_restore_<zeit>` um;
  3. setzt die geprüfte Kopie als `athenos` ein;
  4. startet `athenos` und wartet auf `/health/ready`.

  Du musst dafür den DB-Namen tippen.
- **Aus der Off-site-Kopie:**
  1. Die `.dump.age`, `.sha256` und `.manifest.tsv` nach `/var/backups/athenos/` holen.
  2. Auf dem Mac entschlüsseln (`age -d -i athenos-backup.key …`) oder den Schlüssel kurz auf den Server legen und danach löschen.
  3. Dann wie oben.
- **Neuer Server:** Erstinstallation (3), dann Restore mit `--swap --force`, dann Deploy.
  - Die Geheimnisse aus `/etc/athenos/secrets` sicherst du einmalig im Passwortmanager.
  - Ohne sie funktionieren SMTP und die Service-Tokens nicht. Das kannst du neu erzeugen; die Daten selbst brauchen sie nicht.

### 6.5 Restore-Übung (jeden Monat, 10 Minuten)
1. `sudo /usr/local/lib/athenos/restore.sh "$(ls -1 /var/backups/athenos/athenos-*.dump | tail -n 1)"`
2. Die Ausgabe muss mit „restore into athenos_restore_… verified“ enden.
3. Stichprobe:
   ```bash
   sudo -u postgres psql -d athenos_restore_<zeit> -c 'SELECT count(*) FROM athenos_users'
   ```
4. Die Kopie löschen:
   ```bash
   sudo -u postgres psql -c 'DROP DATABASE athenos_restore_<zeit>'
   ```
5. Datum und Ergebnis in deinem Betriebslog notieren.
6. Alle drei Monate einmal aus der Off-site-Kopie üben.

Lokal ohne Server prüft `infra/beta/tests/backup-restore-proof.sh` denselben Ablauf gegen ein Wegwerf-PostgreSQL 16.

---

## 7. Geheimnisse rotieren

Alle Geheimnisse sind Dateien in `/etc/athenos/secrets/`. `sudo /usr/local/lib/athenos/secret.sh list` zeigt Namen, Rechte und Grösse, nie Inhalte.

| Geheimnis | Rotieren | Folge |
|---|---|---|
| `smtp-password` | in Mailgun ein neues SMTP-Passwort erzeugen, `secret.sh set smtp-password`, Neustart | keine |
| `athenos-internal.token`, `notify-service.token` | `secret.sh generate <name> --force`, Neustart | keine (alle Seiten lesen dieselbe Datei) |
| `control-*.token`, `registry.token` | `generate --force`, Neustart | `fenrirctl` braucht den neuen Token |
| `auth-pin-pepper` | `generate --force`, Neustart | offene PIN-Codes werden ungültig (Minuten) |
| `intel-token.key` | `generate --force`, Neustart | offene KI-Vorschläge werden ungültig |
| Fenrir-Admin | `set-fenrir-admin-password.sh`, Neustart | – |
| `offsite_ed25519` | neuen Schlüssel erzeugen, in der Storage Box tauschen | – |
| SSH-Schlüssel deines Kontos | neuen Key in `authorized_keys`, testen, alten entfernen | – |

Neustart heisst immer `sudo systemctl restart athenos`. Nach einem Verdacht auf Kompromittierung rotierst du **alle** Geheimnisse und meldest alle Sitzungen ab (siehe 10).

---

## 8. Updates

- **Betriebssystem:** `unattended-upgrades` spielt die Sicherheitsupdates jede Nacht ein und startet bei Bedarf um 03:30 UTC neu. Alle Dienste kommen von selbst wieder.
- **Einmal im Monat** von Hand:
  ```bash
  sudo apt update && sudo apt upgrade
  ```
  Damit kommt auch **Caddy** aus dem eigenen Repository. Danach `sudo systemctl status caddy athenos`.
- **PostgreSQL:** Kleine Versionen (16.x) kommen mit den Updates; dabei startet `athenos` mit neu. Ein Sprung auf eine neue Hauptversion ist in der Beta nicht geplant (`pg_upgradecluster` nur nach einem frischen Dump).
- **Athenos:** per Deploy (5). Fehlerhafte Versionen nimmst du mit Rollback (6.1) zurück.

---

## 9. Überwachung

- **Lokal:** `athenos-healthcheck.timer` prüft alle 5 Minuten:
  - die Units `athenos`, `postgresql@16-main` und `caddy`;
  - `http://127.0.0.1:8080/health/ready` und `/public/status`;
  - die Edge (`127.0.0.1:41002/healthz`);
  - von aussen `https://app.athenos.dev/healthz` und die API-Gesundheit über das Gateway;
  - die Platte (< 85 %);
  - das Alter des letzten Backups (< 26 h);
  - das Zertifikat (> 14 Tage).
- **Alarm-Mails:** bei einem neuen Problem, alle 6 h, solange es andauert, und einmal, wenn alles wieder grün ist.
  - Zuerst geht die Mail über den eigenen notification-service, sonst über `sendmail`/msmtp (3.3).
  - Fällt `athenos`, das Backup, Postgres oder Caddy aus, schickt `athenos-alert@` den Status und die letzten Logzeilen.
- **Von Hand:** `sudo /usr/local/lib/athenos/healthcheck.sh` und `journalctl -u athenos -f`.
- **Von aussen (empfohlen, gratis):** Fällt der ganze Server aus, kann er selbst nicht mehr warnen.
  - **Better Stack Uptime** (Free) oder **UptimeRobot** (Free, 5-Minuten-Intervall): ein HTTPS-Monitor auf `https://app.athenos.dev/gateway/services/module%3Aathenos-api%3A%3Aapi-gateway/health` mit dem Stichwort `healthy`, dazu einer auf `https://app.athenos.dev/healthz`.
  - **Heartbeats** (healthchecks.io oder Better Stack): `BACKUP_HEARTBEAT_URL` in `backup.env` (Erwartung: einmal täglich) und `HEALTHCHECK_HEARTBEAT_URL` in `monitor.env` (Erwartung: alle 5 Minuten). Bleibt der Ping aus, warnt dich der Dienst.
  - Die Dienste sehen nur die öffentlichen Health-Adressen, keine Personendaten.

---

## 10. Notfall und Datenpanne

### Sofort
1. **Eindämmen:**
   - `sudo systemctl stop athenos` (oder nur `caddy`, wenn du intern weiter prüfen willst);
   - bei laufendem Angriff zusätzlich in der Cloud-Firewall alles ausser deiner IP sperren.
2. **Spuren sichern, nichts löschen:**
   ```bash
   sudo journalctl --since "-3 days" > ~/incident-journal.txt
   ```
   Dazu das Caddy-Access-Log (`/var/log/caddy/`), `/var/log/athenos/` und einen frischen DB-Dump. Die Logs laufen nach 7 Tagen ab: Kopiere sie zuerst.
3. **Einordnen:**
   - Welche Daten (Konten, E-Mail-Adressen, Tickets, Anhänge)?
   - Wie viele Personen?
   - Seit wann?
   - Ist es behoben?
   - Hohes Risiko für die Betroffenen?
4. **Beheben:**
   - die Lücke schliessen (Deploy/Rollback);
   - **alle Geheimnisse rotieren** (7);
   - alle Sitzungen beenden (Konten deaktivieren und wieder aktivieren oder die Sitzungsdauer kurz auf 1 Tag senken);
   - bei Passwort-Verdacht ein Passwort-Reset für alle.

### Meldepflichten (aus RECHT_BETA.md)
- **Schweiz (revDSG Art. 24):** Bei voraussichtlich **hohem Risiko** meldest du die Panne **so rasch als möglich** dem **EDÖB** (Online-Meldeformular auf edoeb.admin.ch). Betroffene informierst du, wenn es zu ihrem Schutz nötig ist.
- **EU (DSGVO Art. 33/34):** Betrifft die Panne Personen in der EU und besteht ein Risiko, meldest du sie **innerhalb von 72 Stunden** der zuständigen Aufsichtsbehörde. Bei hohem Risiko informierst du auch die Betroffenen ohne unnötige Verzögerung. Ist die Frist nicht zu halten, meldest du trotzdem und begründest die Verspätung.
- **Immer dokumentieren**, auch ohne Meldung:
  - was passiert ist;
  - welche Daten betroffen sind;
  - welche Folgen sie haben;
  - welche Massnahmen du getroffen hast;
  - wann du was entschieden hast.
- **Mitteilung an die Betroffenen:**
  - was passiert ist;
  - welche Daten betroffen sind;
  - was du getan hast;
  - was sie tun sollen (z. B. das Passwort ändern);
  - Kontakt `privacy@athenos.dev`.

  Versand über **Admin → Ankündigung per Mail** (geht an alle aktiven Konten).
- **Auftragsverarbeiter informieren**, wenn sie betroffen sind oder helfen müssen: Hoster, Mailgun, gegebenenfalls Cloudflare.

---

## 11. Checkliste vor der Öffnung für fremde Personen

**Server und Netz**
- [ ] Ubuntu 24.04, `install.sh all` ohne Fehler, Login nur per Schlüssel, root gesperrt
- [ ] `sudo ufw status`: nur 22 (LIMIT), 80, 443/tcp, 443/udp; die Cloud-Firewall gleich
- [ ] von aussen `nmap -Pn -p 1-65535 app.athenos.dev`: nur 22, 80, 443 offen
- [ ] DNS A/AAAA/CAA gesetzt, Proxy aus; die Mailgun-Einträge unverändert

**Dienst**
- [ ] `https://app.athenos.dev` mit gültigem Zertifikat, HSTS-Header, HTTP leitet auf HTTPS um
- [ ] `/gateway/services/module%3Aathenos%3A%3Acore/...` gibt 404 (nur das API-Gateway ist offen)
- [ ] `FENRIR_ENV=prod`; keine Dev-Variablen in `/etc/athenos/fenrir.env`; Swagger `/api/docs` nicht erreichbar
- [ ] Mails kommen an (Registrierung, Reset); Allowlist entfernt, wenn es offen sein soll
- [ ] Die echte Besucher-IP erscheint im Log; Rate-Limits greifen pro Person (zwei Geräte testen)

**Konto und Recht**
- [ ] Erster Admin angelegt; der Setup-Token ist verbraucht (Datei weg)
- [ ] `legal_hoster` und `legal_effective_date` gesetzt; Impressum, Datenschutz und Nutzungsbedingungen erreichbar (DE/EN); Beta-Banner an
- [ ] AVV/DPA mit Hoster und Mailgun (und Cloudflare) abgelegt; `privacy@athenos.dev` funktioniert
- [ ] Datenschutz-Texte von einer Fachperson gegengelesen (RECHT_BETA.md)

**Backup und Betrieb**
- [ ] Das erste nächtliche Backup ist da; die Off-site-Kopie liegt drüben (verschlüsselt)
- [ ] Restore-Übung (6.5) einmal erfolgreich durchgespielt
- [ ] Alarm-Mail getestet (`alert.sh message Test`); der externe Uptime-Monitor und die Heartbeats sind eingerichtet
- [ ] Geheimnisse und der age-Schlüssel liegen im Passwortmanager
- [ ] Ein Rollback einmal geübt (`deploy.sh` mit einer Version, dann `rollback.sh`)
- [ ] Die Sicherheitsprüfung von aussen (W9-S) ist abgeschlossen
