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
export FENRIR_DB_POSTGRES_URI=postgresql://root:1234@localhost:5432/postgres
export FENRIR_SSH_PASSWORD=test
export FENRIR_HTTP_TOKEN_ADMIN=testa
export FENRIR_HTTP_TOKEN_OPERATOR=testo
export FENRIR_HTTP_TOKEN_VIEWER=testv
export FENRIR_REGISTRY_TOKEN="test12345678910111213141516"
export FENRIR_IDENTITY_TOKEN=test1234567891202

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

`modules.runtime.env_passthrough_prefixes` erlaubt zusätzlich globales Prefix-Passthrough aus Fenrirs Prozess-Umgebung (z. B. `["ATHENE_", "AUTH_"]`). Unabhängig davon leitet Fenrir pro Modul automatisch abgeleitete Prefixes weiter (z. B. für `athene-api`: `ATHENE_API_` und `ATHENE_`; für `auth-service`: `AUTH_SERVICE_` und `AUTH_`), damit neue Modulvariablen ohne Fenrir-Codeänderung verfügbar sind.

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
