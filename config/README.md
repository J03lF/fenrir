Configuration quickstart

1) Username/Host/Port
   - Set in `config/default.toml` under `[server.ssh]`:
     - `user` (e.g., `test`)
     - `host` (e.g., `127.0.0.1`)
     - `port` (e.g., `2222`)

2) Password (ENV only)
   - Set `FENRIR_SSH_PASSWORD` in your shell before starting the server (development & test):
```
export FENRIR_SSH_PASSWORD='your-secret-password'
```
   - Production (`security.identity.provider = "external"`, `security.identity.environment = "prod"`, `app.version >= "1.0.0"`): SSH authentication delegates to the Identity-Broker (`POST /sessions/login`). Provision admin accounts there, set passwords via `identity-service` (`POST /users/password`), and ensure `security.identity.external.base_url`/`jwks_url` use HTTPS with a configured `auth_token`.

3) Validate config
```
cargo check-config
```

4) Start server
```
cargo run
```

5) SSH login
```
ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no \
    ${USER_FROM_CONFIG}@${HOST_FROM_CONFIG} -p ${PORT_FROM_CONFIG}

z.B: ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no test@127.0.0.1 -p 2222
```

Troubleshooting
- Host key changed warning: remove old key and re-scan
```
ssh-keygen -R "[127.0.0.1]:2222"
ssh-keyscan -p 2222 127.0.0.1 >> ~/.ssh/known_hosts
```
- Permission denied: ensure `FENRIR_SSH_PASSWORD` matches the password you type (development) or verify the Identity-Broker credentials and control token in production.

postgresql://<USER>:<PASSWORD>@<HOST>:<PORT>/<DBNAME>
export FENRIR_DB_POSTGRES_URI=postgresql://<USER>:<PASSWORD>@localhost:5432/<DBNAME>
export FENRIR_SSH_PASSWORD=<ssh-password>
export FENRIR_HTTP_TOKEN_ADMIN=<admin-token>
export FENRIR_HTTP_TOKEN_OPERATOR=<operator-token>
export FENRIR_HTTP_TOKEN_VIEWER=<viewer-token>
export FENRIR_REGISTRY_TOKEN=<registry-api-key>
export FENRIR_IDENTITY_TOKEN=<identity-client-token>

# Statt alles per Hand zu exportieren kannst du dieselben Variablen auch in `secrets/.env` legen.
# Fenrir lädt diese Datei automatisch (bzw. eine alternative via `FENRIR_ENV_FILE=/path/to/.env`),
# solange der jeweilige Key noch nicht im Environment gesetzt ist.

DB Runtime Switch
- `db.runtime.mode = "external"|"embedded"` (Default: external).  
- External: nutzt wie bisher `db.connections.*` URIs (z. B. `FENRIR_DB_POSTGRES_URI`).  
- Embedded: wähle Engine via `db.runtime.embedded.engine` (`sqlite`/`postgres`).  
  - Sqlite: `db.runtime.embedded.sqlite.file_path`, optional `vacuum_interval_seconds`.  
  - Postgres: `db.runtime.embedded.postgres.data_dir`, `binary_path`, `port_range`.  
- ENV-Overrides folgen dem Schema `FENRIR__DB__RUNTIME__...` (z. B. `FENRIR__DB__RUNTIME__MODE=embedded`).


export FENRIR_IDENTITY_TLS_CA="/opt/fenrir/development/fenrir/config/certs/test-ca.crt"
export FENRIR_IDENTITY_TLS_CERT="/opt/fenrir/development/fenrir/config/certs/identity-client.crt"
export FENRIR_IDENTITY_TLS_KEY="/opt/fenrir/development/fenrir/config/certs/identity-client.key"

TLS Konfiguration
- HTTP: aktiviere `[server.http.tls]` mit `enabled = true`, setze `cert_path`, `key_path` und sichere `cipher_suites`.
- Optional `reload_interval_seconds` erlaubt automatisches Neuladen von Zertifikaten.
- SSH: nutze `[server.ssh.tls]` um erlaubte Cipher-Suites zu überschreiben und `host_key_reload_seconds` für Key-Rotation zu setzen.

export FENRIR_ENV=dev
export FENRIR_ENV=prod

Operating mode (`FENRIR_ENV`, W9A-03)
- `prod` | `production` => production; `dev` | `development` | `local` | `test` => development. Missing, empty or unknown => production (fail-safe).
- The value also picks the profile file: `prod`/`production` -> `config/prod.toml`, `dev`/`development` -> `config/dev.toml`, other names -> `config/<name>.toml` (must exist). `FENRIR_CONFIG_ENV` still overrides the file choice only.
- Production ignores `config/local.toml`; server overrides go into `FENRIR_CONFIG_FILE` or `FENRIR__*` variables.
- Production refuses to start (clear error naming every key) while any of these is active: `modules.registry.tls.accept_invalid_certs`, `modules.runtime.clients.tls.accept_invalid_certs`, `security.identity.external.tls.accept_invalid_certs`, a non-empty `modules.dev_sources.base_path` (also via `FENRIR_DEV_MODULE_PATH`), `security.identity.environment` other than prod, an external identity `base_url` without https, `security.password_policy.min_length` < 12 or `check_common_passwords = false`, an inline control token or one shorter than 32 characters, embedded DB with `auth_method = "trust"`, HTTP/gRPC without TLS on a non-loopback address, a plain `http://` registry URL to a remote host.
- Every module process receives the normalised mode as `FENRIR_ENV=prod` or `FENRIR_ENV=dev` (reserved: a service env override cannot change it).
- The tracked `bin/.fenrir-profile` is a dev profile (it sets `FENRIR_ENV=dev` and `FENRIR_DEV_MODULE_PATH` when they are unset). A server sets `FENRIR_ENV=prod` explicitly in its own environment.

6) Module Runtime Ports
   - Configure `[modules.runtime.ports]` to control how Fenrir assigns module ports.
   - `strategy = "dynamic"` lets Fenrir pick a free port from `[modules.runtime.ports.range]` and persist it.
   - `strategy = "fixed"` keeps the module-provided port (e.g. from its `config.toml`).
   - Example:
```
[modules.runtime.ports]
strategy = "dynamic"

[modules.runtime.ports.range]
min = 41000
max = 46000

[modules.runtime.clients]
timeout_ms = 10000
retries = 2
backoff_ms = 200
health_probe_interval_seconds = 30

[modules.runtime.clients.tls]
# ca_cert_path = "config/certs/control-plane-ca.pem"
# client_cert_path = "config/certs/control-plane-client.crt"
# client_key_path = "config/certs/control-plane-client.key"
accept_invalid_certs = false

[modules.services."module:fenrir-api".env]
# PUBLIC_URL = "https://tickets.local"

[modules.services."module:fenrir-api".secrets]
# API_KEY = "env:FENRIR_API_KEY" # supports FENRIR_API_KEY or FENRIR_API_KEY_FILE

[modules.services."module:fenrir-api".policy]
# internal_only = false
# allowed_roles = ["service-read", "service-write"]
# required_scopes = ["tickets:read"]

[modules.services."module:fenrir-api".policy.tenant]
# mode = "fixed"
# value = "default"

[modules.service_profiles.public_low]
# internal_only = false
# ingress_access = "public"
# allowed_roles = ["service-read", "service-write"]
# required_scopes = ["tickets:read"]
# rate_limit_per_second = 20
```

`[modules.runtime.clients]` steuert Timeouts, Retry-/Backoff-Strategien sowie den Health-Probe-Intervall des ModuleService. Der `.tls`-Block erlaubt optionales mTLS gegenüber der Control-Plane. Mit `[modules.services."<service-id>"]` lassen sich pro Service zusätzliche Env-Variablen (`.env`) und Secrets (`.secrets`, nur `env:...`) injizieren sowie die Security-Policy (`.policy`, inkl. `tenant.mode = any|fixed|allow_list`) überschreiben.

Secret-Resolution für `modules.services.<service>.secrets = "env:VAR"`:
- Fenrir akzeptiert `VAR` **oder** `VAR_FILE` (Pfad auf Datei mit Secret-Inhalt).
- Sind beide gesetzt, wird der Start mit Konfigurationsfehler abgebrochen.
- Leere Werte oder nicht lesbare Secret-Dateien werden als harte Fehler behandelt (fail-closed).

`modules.runtime.env_passthrough_prefixes` erlaubt zusätzlich globales Prefix-Passthrough aus Fenrirs Prozess-Umgebung (z. B. `["ATHENOS_", "AUTH_"]`). Unabhängig davon leitet Fenrir pro Modul automatisch abgeleitete Prefixes weiter (z. B. für `athenos-api`: `ATHENOS_API_` und `ATHENOS_`; für `auth-service`: `AUTH_SERVICE_` und `AUTH_`), damit neue Modulvariablen ohne Fenrir-Codeänderung verfügbar sind.

Runtime Env Injection
- Every managed module process receives:
  - `FENRIR_MODULE_ID` (`ticket-domain`, ...),
  - `FENRIR_SERVICE_ID` (`module:ticket-domain`),
  - `FENRIR_SERVICE_URI` (`service://module:ticket-domain`).
- When dynamic ports are enabled and a port is assigned:
  - `FENRIR_SERVICE_PORT` (TCP port number as string).
  - `FENRIR_SERVICE_ADDR` (`127.0.0.1:<port>`).
Use these instead of hardcoded ports/URLs inside modules.
- Modules also learn about the DB connector endpoint:
  - `FENRIR_DB_CONNECTOR_PROTOCOL` (`ipc` or `tcp`),
  - `FENRIR_DB_CONNECTOR_ENDPOINT` (socket path or host:port),
  - `FENRIR_DB_CONNECTOR_URI` (convenience URI).
- `FENRIR_CONTROL_PLANE_URL` points to the HTTP control-plane (`http[s]://host:port`) for lifecycle- and token APIs.
- Control-plane client settings are propagated via `FENRIR_CONTROL_PLANE_TIMEOUT_MS`, `FENRIR_CONTROL_PLANE_RETRY_ATTEMPTS`, `FENRIR_CONTROL_PLANE_RETRY_BACKOFF_MS` as well as optional mTLS pointers `FENRIR_CONTROL_PLANE_TLS_CA_CERT`, `FENRIR_CONTROL_PLANE_TLS_CLIENT_CERT`, `FENRIR_CONTROL_PLANE_TLS_CLIENT_KEY`, `FENRIR_CONTROL_PLANE_TLS_ACCEPT_INVALID`. The module-kit uses these to configure reqwest clients with consistent timeouts/backoffs.
- All OTEL-related host variables (`OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`, `OTEL_EXPORTER_OTLP_*`, `TRACEPARENT`, `TRACESTATE`) are mirrored into module environments so telemetry flows without bespoke config.

Delegated Service Tokens
- `[security.service_tokens]` controls how long delegated service credentials stay valid (`lifetime_seconds`) and when idle tokens are retired (`idle_timeout_seconds`).
- Fenrir injects these ephemeral tokens into managed modules via `FENRIR_SERVICE_TOKEN`; modules must present them when calling other services through the internal gateway.
- Each issued token is accompanied by `FENRIR_SERVICE_TOKEN_ISSUED_AT`, `FENRIR_SERVICE_TOKEN_EXPIRES_AT` (both RFC 3339 timestamps) as well as `FENRIR_SERVICE_TOKEN_TTL_SECS` so modules can refresh long-running credentials before they expire.
- Keep `cleanup_interval_seconds` low (default 60s) to reclaim stale tokens quickly in development.

Internal Gateway
- Module HTTP endpoints are mounted on the control-plane server at `/gateway/services/<service-id>/*` (URL-encode `service-id`, e.g. `module%3Afenrir-api`).
- Calls must include `Authorization: Bearer <FENRIR_SERVICE_TOKEN>`; the gateway enforces `internal_only`, allowed roles, and scopes from the service descriptor and rate-limits each service (120 req/s) before proxying to the module.

DB Connector & Scoped Tokens
- `modules.runtime.default_service_scopes` lists the scopes automatically granted to module service tokens (default `["db:read"]`).
- Modules talk to the DB connector exclusively over the provided IPC/TCP endpoint; every request must include a service token with `db:read` or `db:write`.
- The connector proxies SQL statements via Fenrir's DB adapters, so modules never read raw DB credentials.
- Requests now support prepared statements: set `"command": "prepared"`, provide `"params": [{"name": "p1", "value": 42}, ...]` and optionally `"tenant": {"param": "tenant_id", "mode": "inject|require_match"}` to bind the caller tenant.
- Use the bundled `fenrir-module-kit` crate for a ready-made connector client; it reads all `FENRIR_*` env vars, automatically exchanges `db:write` tokens via `POST /modules/runtime/tokens`, and exposes high-level helpers for simple/prepared statements. Eine leere Scope-Liste beim Token-Endpoint bedeutet „Standard-Scopes auffrischen“ und liefert eine neue Basisauthentifizierung für das Modul.
- The control plane exposes lifecycle hooks at `/modules/runtime/:id/{start,stop,restart}` (role `operator`) and quarantines modules after repeated start failures. Quarantine windows and status notes are visible via the Service Registry.

Client IP behind a reverse proxy (`[server.edge]`, W9A-01)
- Default: off. Every client is the TCP peer of the host (as before).
- `client_ip_header` (e.g. `x-real-ip`) plus `trusted_proxies` (CIDRs or single addresses, IPv4/IPv6) switch it on. The header counts only when the TCP peer is inside `trusted_proxies` and the value is exactly one IP address; otherwise the peer is the client.
- The static-site edge and the control-plane gateway strip every client-sent address header (`X-Forwarded-For`, `X-Real-IP`, `Forwarded`, `CF-Connecting-IP`, `True-Client-IP`, the configured header) and send `X-Forwarded-For: <client>` to the modules.
- Env: `FENRIR_SERVER__EDGE__CLIENT_IP_HEADER=x-real-ip`, `FENRIR_SERVER__EDGE__TRUSTED_PROXIES=127.0.0.1/32,::1/128` (comma-separated). The generic `FENRIR__SERVER__EDGE__…` form works too.
- Invalid entries, or a header without trusted proxies, stop the host at startup with a clear error.
- Caddy example: `reverse_proxy 127.0.0.1:41002 { header_up X-Real-IP {client_ip} }` (Caddy replaces any client value).

Log rotation (`[telemetry.logs]`, W9A-13)
- Host logs (`logs/app.log`, `logs/db.log`, or `FENRIR_LOG_PATH` / `FENRIR_DB_LOG_PATH`) and module logs (`<install_dir>/runtime/logs/*.log`) rotate daily (UTC) or at `max_file_mb` (default 50). Archives: `<log dir>/archive/<name>-YYYYMMDD_HHMMSS.log`, deleted after `retention_days` (default 7) and capped at `max_archives` per log (default 30). A line lives at most `retention_days` + 1 day.
- All log and archive files are created 0600; older files are narrowed to 0600 when the host touches them.
- Module processes keep their log descriptor, so their logs are rotated by copy + truncate every `check_interval_seconds` (default 60); lines written during the copy of a 50 MB file can be lost. A restarting module's previous log is archived instead of truncated.
