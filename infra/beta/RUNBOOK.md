# Runbook: Athenos-Beta auf app.athenos.dev

Diese Anleitung führt dich vom gemieteten Server bis zur offenen Beta und durch den Betrieb danach. Alle Befehle mit `sudo` laufen auf dem Server, alle mit `infra/beta/…` auf deinem Mac im Fenrir-Checkout.

Kurz zum Aufbau (Details in [README.md](README.md)):
- **Ein Server** bei Infomaniak (VPS Cloud, Schweiz) trägt alles: Host, Module, Web-App, PostgreSQL und Athenos AI.
- **Cloudflare** steht als Proxy davor (orange Wolke). Es macht HTTPS für die Besucher, cacht die gehashten statischen Dateien und leitet alles andere an den Server weiter.
- **Caddy** auf dem Server nimmt nur Verbindungen von Cloudflare an (Firewall: 443 nur aus den Cloudflare-Bereichen). Es leitet nur an `127.0.0.1:41002` weiter (die Static-Edge des Hosts) und gibt die echte Besucher-IP aus `CF-Connecting-IP` als `X-Real-IP` weiter.
- **athenos.service** ist der Fenrir-Host. Er startet die Module als Kindprozesse.
- **PostgreSQL 16** ist das Paket der Distribution. Es hört nur auf einem Unix-Socket; der Host meldet sich per *peer* als Systembenutzer `athenos` an. Es gibt kein Datenbank-Passwort.

---

## 1. Der Server: Infomaniak VPS Cloud

**Bestellt: VPS Cloud, Schweiz.**

| Eigenschaft | Wert |
|---|---|
| CPU | 4 vCPU (x86_64) |
| RAM | 8 GB |
| Platte | 256 GB SSD |
| System | Ubuntu 24.04 LTS |

**Warum das reicht:**

| Teil | RAM | Platte |
|---|---|---|
| Ubuntu + Caddy | 0,5 GB | 5 GB |
| PostgreSQL 16 (`shared_buffers` 1 GB) | 1–1,5 GB | Beta: < 1 GB |
| Host + 4 Module + Web-App | 0,5–1 GB | 5 Releases × ca. 150 MB |
| Athenos AI (Modell 1,1 GB auf der Platte, im Speicher mehr) | 1,5–2,5 GB | 1,1 GB je Modellversion |
| Backups (14 Tage + Dumps vor jedem Deploy) | – | ein paar GB |
| Journal (max. 1 GB) | – | 1 GB |

- **Ubuntu 24.04 ist Pflicht.** Die CI baut auf `ubuntu-24.04` für `x86_64-unknown-linux-gnu`. Die Binaries brauchen glibc ≥ 2.39 und OpenSSL 3. Ein älteres System oder ARM geht nicht.
- **Für die Datenschutzerklärung** (Platzhalter `[HOSTER, LAND]`, später in Admin → Einstellungen → `legal_hoster`): **„Infomaniak Network SA, Genf, Schweiz“**.
  - Cloudflare kommt als weiterer Auftragsverarbeiter dazu: Es sieht als Proxy jeden Aufruf.
  - Steht die Off-site-Kopie woanders, nennst du sie mit.

**Beim Bestellen bzw. jetzt in der Infomaniak-Verwaltung:**
1. Image **Ubuntu 24.04**. IPv4 (und IPv6, falls angeboten) an.
2. **SSH-Schlüssel** hinterlegen (dein `~/.ssh/id_ed25519.pub`). Der Login geht per Schlüssel, ohne Passwort.
   - Infomaniak legt dafür den Benutzer `ubuntu` mit `sudo` an.
3. **Firewall der VPS Cloud** (Sicherheitsgruppe) als zweite Schicht vor `ufw`. Eingehend nur:
   - 22/tcp von überall (oder nur von deinen Adressen);
   - 443/tcp aus den Cloudflare-Bereichen (`infra/beta/cloudflare/ips.txt`).
4. Optional die Snapshots von Infomaniak. Sie ersetzen den eigenen Dump nicht und laufen ausserhalb der 14-Tage-Frist: Prüfe ihre Aufbewahrung, bevor du sie einschaltest.
5. Den **Vertrag zur Auftragsverarbeitung (AVV/DPA)** mit Infomaniak abschliessen und ablegen.

> Fussnote: Andere Anbieter gehen genauso, wenn sie Ubuntu 24.04 auf x86_64 mit 8 GB RAM bieten (z. B. Hetzner Cloud in Deutschland). Dann ändern sich nur der Hoster-Text und die Cloud-Firewall.

---

## 2. Cloudflare vor app.athenos.dev

Cloudflare macht drei Dinge:
- DNS;
- HTTPS für die Besucher;
- den Cache für die gehashten Dateien der Web-App (`chunk-*.js`, `styles-*.css`, Fonts).

Alles andere geht an den Server weiter, verschlüsselt mit „Full (strict)“.

### 2.1 DNS
Zone `athenos.dev`:

| Typ | Name | Inhalt | Proxy |
|---|---|---|---|
| A | `app` | IPv4 des Servers | **an** (orange Wolke) |
| AAAA | `app` | IPv6 des Servers (falls vorhanden) | **an** |

**Nicht anfassen:**
- die Einträge von Mailgun unter `mail.athenos.dev` (MX, SPF-TXT, DKIM, CNAME für Tracking; diese bleiben „DNS only“);
- das Email Routing der Hauptdomain (`privacy@athenos.dev`);
- den DMARC-Eintrag.

**CAA:** Cloudflare stellt das Besucher-Zertifikat selbst aus (Universal SSL) und trägt die nötigen CAA-Werte selbst ein. Setz keine eigenen CAA-Einträge auf `app`, sonst kann die Erneuerung scheitern. Das Origin-Zertifikat (2.3) braucht kein CAA.

### 2.2 SSL/TLS-Einstellungen (Cloudflare → SSL/TLS)
- **Verschlüsselungsmodus: Full (strict).** Cloudflare prüft das Zertifikat des Servers.
- **Edge Certificates:**
  - **Always Use HTTPS: an.** Port 80 ist am Server zu.
  - **Minimum TLS Version: 1.2.**
  - **TLS 1.3: an.**
  - **HSTS: aus lassen.** Caddy setzt `Strict-Transport-Security: max-age=31536000; includeSubDomains`, und Cloudflare reicht den Header unverändert durch. Schaltest du HSTS auch in Cloudflare ein, kommt der Header doppelt und vielleicht mit verschiedenen Werten. Also nur an einer Stelle; hier bei Caddy, damit es mit der Konfiguration im Repo versioniert ist.
- **Automatic HTTPS Rewrites:** egal (die App lädt nur eigene https-Ressourcen).

### 2.3 Origin-Zertifikat für Caddy
**Entscheid: Cloudflare Origin CA**, nicht ACME.
- Ein Origin-CA-Zertifikat gilt bis 15 Jahre, Caddy braucht kein Plugin, und es kommt nichts über Port 80 herein.
- ACME mit DNS-Challenge bräuchte einen eigenen Caddy-Build mit dem Cloudflare-DNS-Modul und einen API-Token auf dem Server.
- Nachteil: Nur Cloudflare vertraut diesem Zertifikat. Wer den Proxy ausschaltet, muss auf ACME wechseln.

**So gehst du vor:**
1. Cloudflare → SSL/TLS → **Origin Server → Create Certificate**:
   - Key type ECC;
   - Hostname `app.athenos.dev`;
   - Gültigkeit 15 Jahre.
2. Auf dem Server:
   ```bash
   sudo install -d -m 0750 -o root -g caddy /etc/caddy/origin
   sudo nano /etc/caddy/origin/app.athenos.dev.pem   # "Origin Certificate" einfügen
   sudo nano /etc/caddy/origin/app.athenos.dev.key   # "Private Key" einfügen
   sudo chown root:caddy /etc/caddy/origin/app.athenos.dev.*
   sudo chmod 0644 /etc/caddy/origin/app.athenos.dev.pem
   sudo chmod 0640 /etc/caddy/origin/app.athenos.dev.key
   ```
   Den privaten Schlüssel legst du zusätzlich in den Passwortmanager. Cloudflare zeigt ihn nur einmal.
3. Danach `sudo ~/athenos-beta/server/install.sh caddy`. Der Healthcheck prüft das Ablaufdatum dieses Zertifikats auf `127.0.0.1:443`.

**Optional, zusätzliche Härtung: Authenticated Origin Pulls.** Dann bekommt nur Cloudflare (mit seinem Client-Zertifikat) eine TLS-Verbindung, selbst wenn die Firewall einmal offen wäre.
1. Das Cloudflare-Origin-Pull-CA-Zertifikat nach `/etc/caddy/origin/cloudflare-origin-pull-ca.pem` legen. Es steht in der Cloudflare-Doku „Authenticated Origin Pulls“ (zonenweit).
2. In Cloudflare SSL/TLS → Origin Server → **Authenticated Origin Pulls** einschalten.
3. Im Caddyfile den Block `client_auth` auskommentieren, dann `sudo systemctl reload caddy`.
4. Prüfen: `curl -sS https://app.athenos.dev/healthz` geht weiter. Ein direkter Aufruf der Server-IP mit `curl -k --resolve app.athenos.dev:443:<IP> https://app.athenos.dev/` scheitert, und das soll er.

### 2.4 Cache-Regeln (Cloudflare → Caching → Cache Rules)
Die Edge des Hosts sendet `Cache-Control: public, max-age=31536000, immutable` nur für die gehashten Build-Dateien. Für alles andere (HTML, `index.html`, i18n-JSON, Icons) sendet sie `no-cache`. Cloudflare soll diesen Angaben folgen und nie etwas Persönliches cachen.

Reihenfolge der Regeln:

| # | Name | Wenn | Dann |
|---|---|---|---|
| 1 | API nie cachen | URI-Pfad beginnt mit `/gateway/` **oder** beginnt mit `/api/` | **Bypass cache** |
| 2 | Angemeldete Anfragen | Header `Authorization` vorhanden **oder** Cookie enthält `athenos_` | **Bypass cache** |
| 3 | HTML nie cachen | URI-Pfad ist `/` **oder** URI Path Extension ist `html` **oder** URI Path Extension ist leer (SPA-Routen wie `/app/…`) | **Bypass cache** |
| 4 | Statische Dateien | URI Path Extension ist eine von `js`, `css`, `woff2`, `woff`, `png`, `svg`, `ico`, `webp` | **Eligible for cache**; Edge TTL: „Use cache-control header if present, bypass cache if not“; Browser TTL: „Respect origin“ |

Regel 4 cacht nur, was die Edge als `immutable` markiert (die gehashten Dateien). Für Dateien mit `no-cache` (ungehashte Skripte, Icons) fragt Cloudflare jedes Mal beim Server nach. Die Regeln brauchen keine regulären Ausdrücke und gehen deshalb im Free-Plan.

Danach:
- Caching → Configuration → **Purge Everything** einmal nach jedem Deploy, der `index.html` ändert. Nötig ist es nicht, weil `index.html` nie gecacht wird; es schadet aber nicht.
- Die Antwort-Header prüfen: `curl -sI https://app.athenos.dev/` muss `cf-cache-status: BYPASS` (oder `DYNAMIC`) zeigen. Ein Chunk aus `index.html` zeigt beim zweiten Aufruf `HIT`. `/gateway/...` zeigt nie `HIT`.

Weitere Einstellungen:
- **Rocket Loader, Auto Minify, Email Obfuscation: aus.** Sie verändern HTML und Skripte und verletzen damit die CSP.
- **Speed → Optimization → Early Hints:** egal.
- **Network → WebSockets:** an (schadet nicht).
- **Network → HTTP/3:** an (zwischen Browser und Cloudflare).

### 2.5 Echte Besucher-IP und die Cloudflare-Liste
- **Caddy** glaubt `CF-Connecting-IP` nur, wenn die Verbindung aus einem Cloudflare-Bereich kommt (`trusted_proxies`). Es gibt die Adresse als `X-Real-IP` an den Host weiter. Der Host glaubt `X-Real-IP` nur von `127.0.0.1` (SPEC §2).
- **Die Bereiche stehen in einer einzigen Liste:** `/etc/athenos/cloudflare-ips.txt`, aus https://www.cloudflare.com/ips-v4 und `ips-v6`.
  - `/usr/local/lib/athenos/cloudflare-ips.sh update` holt sie, prüft sie und schreibt daraus `/etc/caddy/cloudflare-trusted-proxies.caddy` (Caddy lädt neu) und die ufw-Regeln „443/tcp von Cloudflare“. Dabei kommen neue Bereiche zuerst dazu, alte gehen danach.
  - `athenos-cloudflare-ips.timer` macht das jeden Montag. Scheitert es, kommt eine Alarm-Mail, und die alte Liste bleibt.
  - Eine Kopie zum Start ohne Netz liegt in `infra/beta/cloudflare/ips.txt` (`cloudflare-ips.sh update --offline`).
  - Ändert Cloudflare die Bereiche, passt du auch die Firewall bei Infomaniak an (`cloudflare-ips.sh show`).

---

## 3. Erstinstallation

### 3.1 Admin-Konto auf dem Server
Infomaniak gibt dir den Benutzer `ubuntu` mit deinem Schlüssel. Lege dein eigenes Konto an (oder bleib bei `ubuntu` und setze unten `--admin-user ubuntu`):

```bash
ssh ubuntu@<server-ip>
sudo adduser joel                      # Passwort für sudo setzen (Passwortmanager)
sudo usermod -aG sudo joel
sudo install -d -m 700 -o joel -g joel /home/joel/.ssh
sudo cp ~/.ssh/authorized_keys /home/joel/.ssh/ && sudo chown joel:joel /home/joel/.ssh/authorized_keys
sudo hostnamectl set-hostname athenos-beta
```

Dann **in einem zweiten Terminal** `ssh joel@<server-ip>` testen.

Auf dem Mac in `~/.ssh/config`. Nimm die IP, nicht `app.athenos.dev`: Der Name zeigt auf Cloudflare, und Cloudflare leitet kein SSH weiter.
```
Host athenos-beta
  HostName <server-ip>
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
| `cloudflare` | Cloudflare-Bereiche holen (für Caddy und die Firewall, siehe 2.5) |
| `caddy` | Caddyfile, `caddy validate`, neu laden. Fehlt das Origin-Zertifikat (2.3), überspringt der Schritt das Caddyfile und sagt, was zu tun ist |
| `updates` | nächtliche Sicherheitsupdates, Neustart bei Bedarf um 03:30 UTC |
| `firewall` | ufw: 22 begrenzt (nur Schlüssel), 443/tcp nur aus den Cloudflare-Bereichen; alles andere zu, auch 80. Mit `SSH_SOURCES="<cidr> …"` nur von deinen Adressen |
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
Cloudflare ist eingerichtet (2.1 bis 2.4), das Origin-Zertifikat liegt bei Caddy (`install.sh caddy` lief ohne Warnung). Dann wie in 5.1 einen Release ausrollen:
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
- Das Schloss zeigt ein gültiges Zertifikat (von Cloudflare).
- `curl -sI https://app.athenos.dev` zeigt `strict-transport-security` **genau einmal**, `server: cloudflare` und `cf-cache-status`.
- Im Caddy-Access-Log (`sudo tail -n 3 /var/log/caddy/app.athenos.dev.access.log`) steht bei `client_ip` deine eigene IP, bei `remote_ip` eine Cloudflare-Adresse.

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
   - `legal_hoster` („Infomaniak Network SA, Genf, Schweiz“) und `legal_effective_date`;
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

Es bleiben 5 Releases liegen, nie das aktuelle und das vorige. Das Log liegt in `/var/log/athenos-deploy.log`.

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

### 6.3 Off-site-Kopie einrichten
Die Kopie gehört an einen anderen Ort als der Server. Zwei Wege:

**A) Infomaniak Swiss Backup (S3, rclone)**: bleibt in der Schweiz beim selben Auftragsverarbeiter, aber in einem anderen Rechenzentrum.
1. In der Infomaniak-Verwaltung Swiss Backup → ein Gerät vom Typ „Cloud-Speicher (S3)“ anlegen; die Zugangsdaten notieren.
2. `sudo apt install rclone`. Dann die rclone-Konfiguration nur für den Dienst anlegen:
   ```bash
   sudo -u athenos rclone config --config /etc/athenos/secrets/rclone.conf
   ```
   Typ `s3`, Provider „Other“, Endpoint und Schlüssel von Swiss Backup, Remote-Name `swissbackup`.
3. In `/etc/athenos/backup.env`:
   - `OFFSITE_MODE=rclone`;
   - `OFFSITE_RCLONE_REMOTE=swissbackup:athenos-db`;
   - `RCLONE_CONFIG=/etc/athenos/secrets/rclone.conf`.

**B) rsync über SSH** (z. B. eine Storage Box bei einem anderen Anbieter):
1. Schlüssel nur für Backups anlegen:
   ```bash
   sudo -u athenos ssh-keygen -t ed25519 -N '' -f /etc/athenos/secrets/offsite_ed25519
   ```
   Den Public Key beim Ziel hinterlegen.
2. Host-Key festhalten:
   ```bash
   ssh-keyscan -p 23 <ziel-host> | sudo tee /etc/athenos/offsite_known_hosts
   ```
   Den Fingerprint vergleichst du mit der Anzeige des Anbieters.
3. In `/etc/athenos/backup.env` `OFFSITE_MODE=rsync` und die `OFFSITE_RSYNC_*`-Zeilen setzen. Den Anbieter nennst du in der Datenschutzerklärung.

**Für beide:**
- Verschlüsseln (empfohlen): `age-keygen -o athenos-backup.key` **auf dem Mac**. Den privaten Schlüssel legst du in den Passwortmanager. Nur den Public Key (`age1…`) schreibst du nach `/etc/athenos/offsite-age-recipients.txt` und setzt `OFFSITE_AGE_RECIPIENTS_FILE`.
- `sudo systemctl start athenos-backup.service` und prüfen, ob die Dateien drüben liegen.

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

- **Cloudflare-Bereiche:** jeden Montag automatisch (2.5).
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
  - die Zertifikate (> 14 Tage): das von Cloudflare für die Besucher und das Origin-Zertifikat von Caddy (`127.0.0.1:443`).
- **Alarm-Mails:** bei einem neuen Problem, alle 6 h, solange es andauert, und einmal, wenn alles wieder grün ist.
  - Zuerst geht die Mail über den eigenen notification-service, sonst über `sendmail`/msmtp (3.3).
  - Fällt `athenos`, das Backup, Postgres, Caddy oder die wöchentliche Aktualisierung der Cloudflare-Liste aus, schickt `athenos-alert@` den Status und die letzten Logzeilen.
- **Von Hand:** `sudo /usr/local/lib/athenos/healthcheck.sh` und `journalctl -u athenos -f`.
- **Von aussen (empfohlen, gratis):** Fällt der ganze Server aus, kann er selbst nicht mehr warnen.
  - **Better Stack Uptime** (Free) oder **UptimeRobot** (Free, 5-Minuten-Intervall): ein HTTPS-Monitor auf `https://app.athenos.dev/gateway/services/module%3Aathenos-api%3A%3Aapi-gateway/health` mit dem Stichwort `healthy`, dazu einer auf `https://app.athenos.dev/healthz`.
  - **Heartbeats** (healthchecks.io oder Better Stack): `BACKUP_HEARTBEAT_URL` in `backup.env` (Erwartung: einmal täglich) und `HEALTHCHECK_HEARTBEAT_URL` in `monitor.env` (Erwartung: alle 5 Minuten). Bleibt der Ping aus, warnt dich der Dienst.
  - Die Dienste sehen nur die öffentlichen Health-Adressen, keine Personendaten.
  - Die externen Monitore laufen über Cloudflare. Sie erkennen also auch einen Ausfall zwischen Cloudflare und dem Server (z. B. eine veraltete Cloudflare-Liste in der Firewall).

---

## 10. Notfall und Datenpanne

### Sofort
1. **Eindämmen:**
   - `sudo systemctl stop athenos` (oder nur `caddy`, wenn du intern weiter prüfen willst);
   - bei laufendem Angriff zusätzlich in Cloudflare Security → **„Under Attack“-Modus** einschalten oder eine WAF-Regel „Block“ setzen;
   - im Notfall in der Infomaniak-Firewall 443 schliessen (die Seite ist dann aus).
2. **Spuren sichern, nichts löschen:**
   ```bash
   sudo journalctl --since "-3 days" > ~/incident-journal.txt
   ```
   Dazu das Caddy-Access-Log (`/var/log/caddy/`), `/var/log/athenos/` und einen frischen DB-Dump. Die Logs laufen nach 7 Tagen ab: Kopiere sie zuerst. Die Cloudflare-Ereignisse (Security → Events) zeigen blockierte und auffällige Anfragen.
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
- **Auftragsverarbeiter informieren**, wenn sie betroffen sind oder helfen müssen: Infomaniak, Cloudflare, Mailgun.

---

## 11. Checkliste vor der Öffnung für fremde Personen

**Server und Netz**
- [ ] Ubuntu 24.04, `install.sh all` ohne Fehler, Login nur per Schlüssel, root gesperrt
- [ ] `sudo ufw status`: 22 (LIMIT) und 443/tcp nur aus den Cloudflare-Bereichen; die Infomaniak-Firewall gleich
- [ ] von aussen `nmap -Pn -p 1-65535 <server-ip>`: nur 22 offen (443 nur für Cloudflare)
- [ ] `curl -sk --resolve app.athenos.dev:443:<server-ip> https://app.athenos.dev/healthz` von zu Hause: keine Antwort (Firewall)
- [ ] DNS A/AAAA mit Proxy an; SSL „Full (strict)“, Always Use HTTPS, HSTS in Cloudflare aus; die Cache-Regeln 2.4 stehen; die Mailgun-Einträge unverändert
- [ ] `athenos-cloudflare-ips.timer` aktiv, `/etc/athenos/cloudflare-ips.txt` aktuell

**Dienst**
- [ ] `https://app.athenos.dev` mit gültigem Zertifikat, HSTS-Header genau einmal, HTTP leitet auf HTTPS um (Cloudflare)
- [ ] `/gateway/...` zeigt nie `cf-cache-status: HIT`; ein Chunk zeigt beim zweiten Abruf `HIT`
- [ ] `/gateway/services/module%3Aathenos%3A%3Acore/...` gibt 404 (nur das API-Gateway ist offen)
- [ ] `FENRIR_ENV=prod`; keine Dev-Variablen in `/etc/athenos/fenrir.env`; Swagger `/api/docs` nicht erreichbar
- [ ] Mails kommen an (Registrierung, Reset); Allowlist entfernt, wenn es offen sein soll
- [ ] Die echte Besucher-IP erscheint im Log; Rate-Limits greifen pro Person (zwei Geräte testen)

**Konto und Recht**
- [ ] Erster Admin angelegt; der Setup-Token ist verbraucht (Datei weg)
- [ ] `legal_hoster` und `legal_effective_date` gesetzt; Impressum, Datenschutz und Nutzungsbedingungen erreichbar (DE/EN); Beta-Banner an
- [ ] AVV/DPA mit Infomaniak, Cloudflare und Mailgun abgelegt; Cloudflare in der Datenschutzerklärung genannt; `privacy@athenos.dev` funktioniert
- [ ] Datenschutz-Texte von einer Fachperson gegengelesen (RECHT_BETA.md)

**Backup und Betrieb**
- [ ] Das erste nächtliche Backup ist da; die Off-site-Kopie liegt drüben (verschlüsselt)
- [ ] Restore-Übung (6.5) einmal erfolgreich durchgespielt
- [ ] Alarm-Mail getestet (`alert.sh message Test`); der externe Uptime-Monitor und die Heartbeats sind eingerichtet
- [ ] Geheimnisse und der age-Schlüssel liegen im Passwortmanager
- [ ] Ein Rollback einmal geübt (`deploy.sh` mit einer Version, dann `rollback.sh`)
- [ ] Die Sicherheitsprüfung von aussen (W9-S) ist abgeschlossen
