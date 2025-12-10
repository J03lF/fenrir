---
description: "Projektregeln & Architekturleitplanken für Tickets-Backend (Rust-Monolith, SSH + CLI, Multi-DB, dynamische Commands, Verschlüsselung)."
alwaysApply: true
version: 2
---

# Zielbild
- Modularer **Rust-Monolith** mit klaren Layern und starker Security-Basis.
- **Transports**: Pflicht-SSH mit eigener Shell; HTTP/gRPC optional per Feature/Config.
- **CLI**: Dynamische Command-Registry, Subshell `db:` für DB-Admin-Flows.
- **Multi-DB**: Adapter für Postgres/MySQL/SQLite/Mongo; Services bleiben engine-agnostisch.
- **Security First**: KDF/AEAD, RBAC, Sessions, Audit; Secrets ausschliesslich über ENV/Secret-Store.
- **Modul- & Plugin-Fähig**: Signierte Module via Registry + Runtime, strenge Trust-Gates.

# Layer & Verantwortungen
- **Domain** (`src/domain`): Reine Business-Logik (Entities, Value Objects, Ports, Fehler). Keine IO- oder Infra-Abhängigkeiten.
- **Security Base Layer** (`src/security`): `SecurityManager` bündelt KDF, AEAD, Session-Handling, RBAC und Audit-Sink. Alle neuen Features (Password-Hashing, Token, Encryption) nutzen ausschließlich diese API; niemals direkt die Crypto-Primitives einbinden.
- **Session Core** (`src/session`): Domain-nahe Session-Modelle (IDs, Builders, Stores) die vom Security-Manager konsumiert werden.
- **Services** (`src/services`): Orchestrieren Use-Cases über Domain-Ports, Security-Manager und Infra-Adapter. Keine direkten DB-Zugriffe.
- **Infra** (`src/infra`): Implementiert Ports (DB-Adapter, SSH/HTTP Server, Logging, Telemetry, Module-Runtime usw.). Abhängig von Config/Features.
- **CLI/Transports** (`src/cli`, `src/infra/ssh`, `src/infra/http`, `src/infra/grpc`): Dünne IO-Schichten, mappen DTO ↔ Services.
- **Boot** (`src/boot`): Fail-fast Initialisierung, Service-Registry, Telemetry, Security-Attach. Liefert strukturierte `BootError` Codes.

# Ordnerstruktur (Auszug)
```
src/
  main.rs
  lib.rs
  boot/
  config/
  domain/
    module/
    db/
  security/
    auth/
    crypto/
    session/
    manager.rs
  session/
  services/
    db_shell/
    module/
    scheduler/
    mod.rs
  infra/
    db/
      adapters/
    logging/
    telemetry/
    ssh/
    http/
    modules/
  cli/
    shell/
    commands/
      builtins/
    completion.rs
  audit/
  prompts/
  protocol/
  utils/
  prelude/
```

# Architekturrichtlinien (Do/Don't)
- ✅ Domain nutzt ausschließlich Traits/Ports, keine direkten Abhängigkeiten auf Infra/CLI/Security.
- ✅ Services injizieren Abhängigkeiten (SecurityManager, Repositories, Registries) über Konstruktoren/Builder.
- ✅ Infra-Adapter wählen Engine/Backend anhand Config & Feature-Flags; Secrets nur per ENV.
- ✅ CLI/Transports bleiben IO-only, mappen Fehler zu stabilen Codes.
- ✅ CLI `modules release-dev-overrides` gibt alle aktiven Sync-Overrides frei und eignet sich für Stop-Skripte (kein manuelles Modul-Listing nötig).
- ✅ CLI `modules stop-all` stoppt alle laufenden Module über den ModuleRuntime – ideal, um vor einem Shutdown aufzuräumen oder nach einem Crash saubere Zustände zu erzwingen.
- ✅ `fenrirctl` (HTTP-Client unter `src/bin/fenrirctl.rs`) nutzt die Control-Plane-Endpunkte (`/modules/runtime/release-dev-overrides`, `/modules/runtime/stop-all`, `/services/actions/stop-all`) – Start-/Stop-Skripte oder Service-Manager interagieren damit ohne einen zweiten Fenrir-Prozess hochzufahren.
- ❌ Keine Secrets/Keys/PII im Repo oder Logs. ❌ Keine DB-spezifischen Typen nach Domain/Services leaken. ❌ Keine globalen Singletons – Dependency Injection erzwingen.

# Security Base Layer & Audit
- `SecurityManager::new(cfg, audit_sink)` validiert KDF/Cipher-Auswahl, erstellt Session-Store und AEAD-Registry.
- Passwort-Hashing, Key-Derivation, Encryption/Decryption laufen ausschließlich über `SecurityManager`.
- Sitzungen (`SessionStore`) werden zentral verwaltet; Services erhalten nur geprüfte Sessions/Rollen via `ensure_role` oder `validate_session`.
- Service-Tokens für Modulkommunikation laufen über `SecurityManager::issue_service_token` und den `[security.service_tokens]`-Block. Fenrir injiziert diese Delegated Tokens als `FENRIR_SERVICE_TOKEN` plus `FENRIR_SERVICE_TOKEN_{ISSUED_AT,EXPIRES_AT,TTL_SECS}` (RFC 3339 + Restlaufzeit in Sekunden) in jeden Modulprozess; Tokens sind kurzlebig und dürfen niemals im Repo landen. Module nutzen die Lease-Daten, um rechtzeitig über `/modules/runtime/tokens` einen Ersatz zu ziehen.
- Audit-Events (RBAC, Session, Control-Plane) laufen über `AuditSink`; fehlgeschlagene Writes werden geloggt, aber dürfen keine Panics auslösen.
- HTTP-Control-Plane Tokens kommen aus `security.http.control_tokens` und müssen Rollen-Prefix `admin|operator|viewer` besitzen.
- Produktions-SSH (`security.identity.provider = external`, `security.identity.environment = prod`, `app.version >= 1.0.0`) authentifiziert ausschließlich via Identity-Broker `POST /sessions/login`; `FENRIR_SSH_PASSWORD` wird ignoriert. Nur Identity-Admins dürfen sich anmelden, CLI-Audits übernehmen danach den Identity-Actor. Passwort-Setups laufen über den Identity-Service (`POST /users/password`) und werden Argon2-gehasht + auditiert.
- Produktions-Identity-Verkehr läuft über einen vorgelagerten TLS-Proxy/Load Balancer; `security.identity.external.tls.ca_cert_path` (optional `client_cert_path`+`client_key_path` für mTLS) müssen gültige PEM-Pfade referenzieren, `accept_invalid_certs` ist in prod verboten.

# Konfiguration (fail-fast)
- Quellen: `config/default.toml` → profile (`config/<env>.toml`, via `FENRIR_CONFIG_ENV`/`FENRIR_ENV`) → `config/local.toml` → `FENRIR_CONFIG_FILE` (explizit) → ENV Overrides `FENRIR__...`.
- Vor dem Laden der Config setzt Fenrir automatisch Variablen aus `secrets/.env` (überschreibbar via `FENRIR_ENV_FILE`); vorhandene Prozess-ENV bleiben unangetastet – ideal für lokale Secrets.
- Validierung beim Laden (`config::load`) und via CLI-Flag `--check-config` (Alias `-check-config`). Erfolgreich: `CFG-OK configuration valid`. Fehler: Exit 1 mit Codes `CFG-MISSING-SECRET`, `CFG-INVALID`, `CFG-MISSING-FILE`, `CFG-INVALID-PROFILE`, `CFG-DESERIALIZE`.
- Schema-Hinweise:
  - `app.name`, `app.version`.
  - `server.enable_http|enable_grpc`, `server.ssh`, `server.http`, optional `server.grpc` (inkl. TLS-Subsektionen).
- `security.kdf` (Algorithmus, Versionierung, Argon2-Parameter), `security.allowed_ciphers`, `security.jwt`, `security.session`, `security.http.control_tokens` (ENV-Resolver).
- Control-Plane-Tokens werden nie im Repo hinterlegt – lokale Automatisierung zieht sie aus sicheren Stores (macOS Keychain via `security find-generic-password`, Vault, o. Ä.) und exportiert sie runtime-only als `FENRIR_CONTROL_TOKEN`/`FENRIR_HTTP_TOKEN_ADMIN`, damit `fenrirctl`/Scripts HTTP-Aufrufe autorisieren können.
  - In `prod`-Umgebungen mit `identity.external` müssen `base_url`/`jwks_url` HTTPS nutzen und `auth_token` gesetzt sein; Verstöße führen zu `CFG-INVALID`.
  - `security.identity.external.tls.ca_cert_path|client_cert_path|client_key_path` verweisen auf PEM-Dateien (oft via `env:`); `client_*` müssen gemeinsam gesetzt werden. `accept_invalid_certs` ist nur für lokale Tests erlaubt.
  - `db.default_engine ∈ {postgres, mysql, sqlite, mongodb}`, `db.connections.<engine>.uri`, optional `pool.max|timeout_ms`.
  - `telemetry.tracing.level`, `telemetry.metrics.enabled/exporter`, `telemetry.health.enabled`, `telemetry.system.enabled/interval_ms`.
  - `audit.enabled`, `audit.buffer_capacity`, `audit.storage.path|retention_hours|persist_interval_seconds`.
  - `cli.prompt_theme`, `modules.registry` (URL, Auth-Token via ENV, TLS-Settings), `modules.storage`, `modules.trust.require_signature|allowed_signers|keyring_path`.
  - `modules.registry.offline_dirs` (lokale Modul-Repositories), `modules.runtime.engine ∈ {process, stub}`, `modules.bootstrap` (Auto-Install-Liste – Default `fenrir-api`).
  - `[modules.runtime.ports]` definiert die Port-Strategie (`dynamic` vergibt einen freien Port aus `range`, `fixed` respektiert Modul-Config). Fenrir persistiert diese Zuteilungen unter `runtime/ports.json`.
  - `[modules.services."<service-id>"]` injiziert pro Service zusätzliche Env-Variablen (`.env`) bzw. Secrets (`.secrets`, ausschließlich `env:`-Referenzen). `.policy` überschreibt `internal_only`, `allowed_roles`, `required_scopes` sowie `tenant.mode = any|fixed|allow_list`; fehlende Secrets blockieren den Modulstart.
- Fehlende Secrets/ENV triggern `BootErrorCode::ConfigMissingSecret`.

# Boot & Diagnostics
- `boot::boot()` liefert `BootContext { config, services, http_server, logging }`.
- Fehler werden als `BootError` mit Codes wie `BOOT-SECURITY-INIT`, `BOOT-DB-ADAPTERS`, `BOOT-MODULE-ATTACH` usw. ausgegeben – Logging immer strukturiert.
- Runtime-Verzeichnis via `FENRIR_RUNTIME_DIR`; Logging/Telemetry Reload-Handles über `infra::logging`.

# DB-Adapter & Migrations
- Trait-basierte Engine-Auswahl (`infra::db::manager`). Adapter unter `infra/db/adapters/{postgres,mysql,sqlite,mongodb}` implementieren Ports.
- `services::DbShellService` nutzt Ports + Security-Guards (Confirmations für destruktive Befehle).
- Migrationen je Engine in `infra/db/migrations/<engine>/`; Runner bleibt idempotent.

# CLI & dynamische Commands
- Registry unter `cli::commands::builtins`; neue Commands als eigener Ordner + `mod.rs`, registriert sich über `register()`.
- Shell-Prompt konfigurierbar via `cli.prompt_theme`. Subshell `db:` wechselt via `\c <engine>`; Guards für DROP/DELETE/ALTER.
- Completion-Engine in `cli/completion.rs`: Tests sichern Alias-/Prefix-Cycling. Keine Debug-Prints im Commit.

# Module-/Plugin-Ebene
- Module-Service (`services::module`) orchestriert Registry (`infra::modules::registry`) und Runtime (`infra::modules::runtime`).
- Registry ist zusammengesetzt: lokale Quellen (`modules.registry.offline_dirs`) werden vor HTTP abgefragt; so lassen sich Arbeitskopien wie `/opt/fenrir/development/fenrir-api` ohne Netzwerk betreiben.
- Runtime wählbar per Config (`modules.runtime.engine`): `process` spawnt Binaries, `stub` nutzt die neue In-Process-Runtime für Tests/CI.
- Module deklarieren produktive Services über einen HTTP-Descriptor (`/.fenrir/services`). Nutzt `fenrir-module-kit::service` für den Payload. Fenrir pollt diesen Endpoint nach dem Start, registriert `module:<id>::service` im ServiceRegistry und schreibt die Zuordnung nach `runtime/services.json`, welches jeder Modulprozess via `FENRIR_SERVICE_SNAPSHOT_PATH` konsumiert. `.fenrir/config.toml`-`[[services]]` bleibt ausschließlich für Dev-Overrides (`sync module`) bzw. Stub-Runtime gedacht.
- Fenrir injiziert zusätzlich zu den `FENRIR_*`-Basiswerten auch Control-Plane-Client-Settings (`FENRIR_CONTROL_PLANE_TIMEOUT_MS|RETRY_ATTEMPTS|RETRY_BACKOFF_MS`, `FENRIR_CONTROL_PLANE_TLS_CA_CERT|TLS_CLIENT_CERT|TLS_CLIENT_KEY|TLS_ACCEPT_INVALID`). Das module-kit konsumiert diese Variablen, baut dadurch einen HTTP-Client mit optionalem mTLS und standardisierten Timeouts/Backoff für Token-Exchange und API-Calls.
- OTEL-Kontext (Service-Name, Resource-Attributes inkl. `fenrir.module_id`, vorhandene `OTEL_EXPORTER_OTLP_*`, `TRACEPARENT`, `TRACESTATE`) wird automatisch weitergereicht – Module senden Traces/Metrics ohne eigene Config weiter.
- `modules.bootstrap` listet Module, die beim Boot automatisch installiert/aktualisiert werden (Standard: `fenrir-api`).
- Trust Layer (`modules.trust`) erzwingt Signaturen, sofern aktiviert; im Dev-Default (`require_signature = false`, leere Allowlist) dürfen Offline-Artefakte ohne Signatur installiert werden.
- Installationspfade kommen aus Config (`modules.storage.install_dir`).
- Dev-Builds können ohne manuelles Kopieren genutzt werden: `[modules.dev_sources]` mit `base_path` setzen, `synchronize module` packt dann automatisch den Build unter `<base_path>/<module-id>` (Override per `.fenrir-dev.toml` möglich) und markiert das Modul als `local_override`.
- `.fenrir-dev.toml` **oder** `.fenrir/config.toml` im Modul-Repo können `output = ".."` und `[[services]]` definieren (mindestens `id` + `endpoint`, optional `name|description|kind|internal_only|allowed_roles|required_scopes|route_prefix|health_endpoint|access|protocols|rate_limit_per_second|disable_rate_limit`). `sync module` (bzw. Auto-Detection bei gesetztem `[modules.dev_sources]`) stoppt dann den Modulprozess und registriert die angegebenen Dev-Service-Endpunkte über den `ServiceRegistry`, statt ein Artifact zu packen; `release module` stellt wieder auf Distribution zurück.
- `sync module` legt vor dem Überschreiben eines Distribution-Artefakts automatisch ein Backup unter `<install_dir>/.fenrir-backups/<module-id>/` an. Dafür wird das aktuell installierte Modulverzeichnis erneut als `.tar.gz` gepackt und mit Manifest/Checksummen hinterlegt – keine Kopie aus `.fenrir-meta`. `release module` spielt dieses Bundle ohne Registry-Download zurück und löscht das Backup erst nach erfolgreicher Wiederherstellung; ein gezielter Registry-Fallback greift nur, wenn das Backup fehlt oder sich nicht entpacken lässt.
- `[dev.run]` in `.fenrir-dev.toml` beschreibt den lokalen Dev-Befehl (String oder Array), optionales Working-Dir und zusätzliche Env-Variablen. Standardmäßig startet `sync module` danach automatisch den Fenrir-internen Dev-Agent (`fenrir --dev-agent-config …`), injiziert `FENRIR_*`/Service-Tokens und protokolliert den Befehl inklusive Log-Pfad. Setze `auto_start = false`, wenn Fenrir nur die Umgebung vorbereiten soll – dann laufen weiterhin Env-Exports unter `.fenrir/dev-env-*.sh` plus `.fenrir/dev.env`, und du startest den Prozess selbst (z. B. `cargo run`). `release module` stoppt den Agenten, löscht `.fenrir/dev.env` und die Export-Skripte und hebt den Override wieder auf.
- Module laufen weiterhin automatisch hoch/runter; ServiceRegistry führt `module:<id>` + `module:<id>::service` Einträge mit Status/Uptime. CLI `modules services [module]` zeigt Ports, Health-Notes, `service://`-Routen sowie Dev-/Declared-Endpunkte auf Basis dieser Registry.
- Operatoren können Lifecycle überschreiben: CLI `modules start|stop|restart <id>` bzw. Control-Plane `POST /modules/runtime/:id/{start|stop|restart}` triggern den ModuleService. Aufrufe verlangen mindestens Rolle `operator`, respektieren Quarantänen und aktualisieren Registry/Audit automatisch.
- Laufende Module erhalten automatisch `FENRIR_MODULE_ID`, `FENRIR_SERVICE_ID`, `FENRIR_SERVICE_URI` sowie – bei dynamischer Portvergabe – `FENRIR_SERVICE_PORT` und `FENRIR_SERVICE_ADDR`. Diese ENV-Variablen ersetzen harte Ports/URIs innerhalb der Module.
- `FENRIR_CONTROL_PLANE_URL` liefert die HTTP-Basis (`http[s]://host:port`) für interne Steuer-APIs (`/modules/runtime/*`, `/modules/runtime/tokens`).
- Der interne Gateway mountet Modul-Services ausschließlich intern über `/gateway/services/<service-id>/*` (HTTP/JSON) sowie `/gateway/grpc/<service-id>/*` (HTTP/2/gRPC). `service-id` muss URL-encoded sein (`module%3Afenrir-api`). Services deklarieren via Descriptor bzw. `.fenrir-dev` unterstützte Protokolle, optionale `route_prefix` und `health_endpoint` sowie individuelle Rate-Limits (`rate_limit_per_second` oder `disable_rate_limit`, Default 120 req/s). Für `ingress.access = internal` bleibt `Authorization: Bearer <FENRIR_SERVICE_TOKEN>` Pflicht; der SecurityManager validiert Tokens und erzwingt Rollen/Scopes aus dem `ServiceSecurityMetadata`. Bei `ingress.access = public` dürfen Requests ohne Service-Token erfolgen – Gateway injiziert dann `X-Fenrir-Actor = public`/`X-Fenrir-Tenant = public`, akzeptiert optional bereitgestellte Tokens, protokolliert jede Anfrage über AuditEvents und behält Rate-Limits/Tracing bei. Gateway ergänzt weiterhin `X-Fenrir-Gateway-Service`, `X-Fenrir-Actor`, `X-Fenrir-Tenant` sowie `X-Fenrir-Scopes` für Downstream-Logging/Audit.
- DB-Zugriffe passieren ausschließlich über den internen Connector (`ipc`/`tcp` Endpoint via `FENRIR_DB_CONNECTOR_*`). Service-Tokens enthalten standardmäßig die Scopes aus `modules.runtime.default_service_scopes` (Default `db:read`); Schreibkommandos verlangen zusätzliche Scopes wie `db:write`. Module speichern niemals DB-URIs oder Credentials.
- Der ModuleHealthMonitor pingt `ingress.health_endpoint` in festen Intervallen (`modules.runtime.clients.health_probe_interval_seconds`) und setzt Registry-Status (`Active` vs. `Degraded`) samt Notizen – CLI/HTTP spiegeln den Heartbeat ohne zusätzliche Checks.
- Module nutzen das neue Crate `fenrir-module-kit` (im Repo enthalten) für Env-Parsen, Connector-Client (IPC/TCP + JSON-Protokoll) und Token-Exchange (`/modules/runtime/tokens`). Der Client erkennt `DbConnectorIntent::Write`, fordert bei Bedarf `db:write`-Tokens an und unterstützt vorbereitete Statements samt Tenant-Bindings.
- Connector-Requests akzeptieren `command = "prepared"` mit `params`-Array sowie optionaler `tenant`-Policy (`inject` überschreibt den Parameter mit dem Token-Tenant, `require_match` erzwingt gleiches Tenant-Id). Statements werden serverseitig validiert, vorbereitet und über die DbShell-Ports ausgeführt.
- Control-Plane stellt Lifecycle-APIs bereit: `POST /modules/runtime/:id/{start|stop|restart}` (Role Operator). Module mit wiederholten Startfehlern (3x in 120s) werden automatisch für 5 Minuten quarantined; ServiceRegistry setzt Status `Failed` mit Vermerk `quarantined until ...`. Solange Quarantäne aktiv ist blocken `start/ensure` sofort mit `ModuleRuntimeError::Quarantined`.
- Modul-Katalog: Überblick der benötigten Ticketsystem-Module inklusive Abhängigkeiten in `docs/modules_overview.md` pflegen und bei Änderungen an Erweiterungspunkten (z. B. Ports, Signaturanforderungen) mitziehen.

# Telemetry & Logging
- `infra::logging::init_tracing` setzt strukturiertes Logging; Level per Config.
- `infra::telemetry::init` aktiviert Tracing, Metrics (optional Exporter) und Health-Probes (`/live`, `/ready`) sobald Boot komplett.
- Keine PII in Logs; Fehler mit stabilen Codes.

# Tests & Qualität
- Unit-Tests für Domain/Security/Services ohne IO (Mocks).
- Integration unter `tests/integration/` (Config, Login, Modules, DB-Shell, SSH).
- E2E unter `tests/e2e/` für CLI/SSH-Flows.
- Security-Tests: Lockout, RBAC, Audit-Vollständigkeit, Session-Expiry.
- Tooling: `cargo fmt`, `cargo clippy -D warnings`, `cargo test`. Benches für Hot Paths (Parsing, KDF, Query).

# Arbeitsanweisungen & Hygiene
- Neue Features müssen Security-Manager einbinden (kein Direktzugriff auf Crypto/Session-Interna).
- Bei Änderungen an Config-Schema, Security-Flows, Telemetry oder Layer-Grenzen **sofort** AGENTS.md aktualisieren.
- Keine fremden Workspace-Änderungen rückgängig machen; Sandbox/Secrets respektieren.
- Bei relevanten Neuerungen (neue Services, Config-Felder, Safety-Gates) Abschluss-Schritt: `AGENTS.md` ergänzen + Hinweis im PR/Ticket.
