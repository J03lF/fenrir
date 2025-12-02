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
- ❌ Keine Secrets/Keys/PII im Repo oder Logs. ❌ Keine DB-spezifischen Typen nach Domain/Services leaken. ❌ Keine globalen Singletons – Dependency Injection erzwingen.

# Security Base Layer & Audit
- `SecurityManager::new(cfg, audit_sink)` validiert KDF/Cipher-Auswahl, erstellt Session-Store und AEAD-Registry.
- Passwort-Hashing, Key-Derivation, Encryption/Decryption laufen ausschließlich über `SecurityManager`.
- Sitzungen (`SessionStore`) werden zentral verwaltet; Services erhalten nur geprüfte Sessions/Rollen via `ensure_role` oder `validate_session`.
- Audit-Events (RBAC, Session, Control-Plane) laufen über `AuditSink`; fehlgeschlagene Writes werden geloggt, aber dürfen keine Panics auslösen.
- HTTP-Control-Plane Tokens kommen aus `security.http.control_tokens` und müssen Rollen-Prefix `admin|operator|viewer` besitzen.
- Produktions-SSH (`security.identity.provider = external`, `security.identity.environment = prod`, `app.version >= 1.0.0`) authentifiziert ausschließlich via Identity-Broker `POST /sessions/login`; `FENRIR_SSH_PASSWORD` wird ignoriert. Nur Identity-Admins dürfen sich anmelden, CLI-Audits übernehmen danach den Identity-Actor. Passwort-Setups laufen über den Identity-Service (`POST /users/password`) und werden Argon2-gehasht + auditiert.
- Produktions-Identity-Verkehr läuft über einen vorgelagerten TLS-Proxy/Load Balancer; `security.identity.external.tls.ca_cert_path` (optional `client_cert_path`+`client_key_path` für mTLS) müssen gültige PEM-Pfade referenzieren, `accept_invalid_certs` ist in prod verboten.

# Konfiguration (fail-fast)
- Quellen: `config/default.toml` → profile (`config/<env>.toml`, via `FENRIR_CONFIG_ENV`/`FENRIR_ENV`) → `config/local.toml` → `FENRIR_CONFIG_FILE` (explizit) → ENV Overrides `FENRIR__...`.
- Validierung beim Laden (`config::load`) und via CLI-Flag `--check-config` (Alias `-check-config`). Erfolgreich: `CFG-OK configuration valid`. Fehler: Exit 1 mit Codes `CFG-MISSING-SECRET`, `CFG-INVALID`, `CFG-MISSING-FILE`, `CFG-INVALID-PROFILE`, `CFG-DESERIALIZE`.
- Schema-Hinweise:
  - `app.name`, `app.version`.
  - `server.enable_http|enable_grpc`, `server.ssh`, `server.http`, optional `server.grpc` (inkl. TLS-Subsektionen).
  - `security.kdf` (Algorithmus, Versionierung, Argon2-Parameter), `security.allowed_ciphers`, `security.jwt`, `security.session`, `security.http.control_tokens` (ENV-Resolver).
  - In `prod`-Umgebungen mit `identity.external` müssen `base_url`/`jwks_url` HTTPS nutzen und `auth_token` gesetzt sein; Verstöße führen zu `CFG-INVALID`.
  - `security.identity.external.tls.ca_cert_path|client_cert_path|client_key_path` verweisen auf PEM-Dateien (oft via `env:`); `client_*` müssen gemeinsam gesetzt werden. `accept_invalid_certs` ist nur für lokale Tests erlaubt.
  - `db.default_engine ∈ {postgres, mysql, sqlite, mongodb}`, `db.connections.<engine>.uri`, optional `pool.max|timeout_ms`.
  - `telemetry.tracing.level`, `telemetry.metrics.enabled/exporter`, `telemetry.health.enabled`, `telemetry.system.enabled/interval_ms`.
  - `audit.enabled`, `audit.buffer_capacity`, `audit.storage.path|retention_hours|persist_interval_seconds`.
  - `cli.prompt_theme`, `modules.registry` (URL, Auth-Token via ENV, TLS-Settings), `modules.storage`, `modules.trust.require_signature|allowed_signers|keyring_path`.
  - `modules.registry.offline_dirs` (lokale Modul-Repositories), `modules.runtime.engine ∈ {process, stub}`, `modules.bootstrap` (Auto-Install-Liste – Default `fenrir-api`).
  - `[modules.runtime.ports]` definiert die Port-Strategie (`dynamic` vergibt einen freien Port aus `range`, `fixed` respektiert Modul-Config). Fenrir persistiert diese Zuteilungen unter `runtime/ports.json`.
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
- `modules.bootstrap` listet Module, die beim Boot automatisch installiert/aktualisiert werden (Standard: `fenrir-api`).
- Trust Layer (`modules.trust`) erzwingt Signaturen, sofern aktiviert; im Dev-Default (`require_signature = false`, leere Allowlist) dürfen Offline-Artefakte ohne Signatur installiert werden.
- Installationspfade kommen aus Config (`modules.storage.install_dir`).
- Dev-Builds können ohne manuelles Kopieren genutzt werden: `[modules.dev_sources]` mit `base_path` setzen, `synchronize module` packt dann automatisch den Build unter `<base_path>/<module-id>` (Override per `.fenrir-dev.toml` möglich) und markiert das Modul als `local_override`.
- `.fenrir-dev.toml` **oder** `.fenrir/config.toml` im Modul-Repo können `output = ".."` und `[[services]]` definieren (mindestens `id` + `endpoint`, optional `name|description|kind`). `sync module` (bzw. Auto-Detection bei gesetztem `[modules.dev_sources]`) stoppt dann den Modulprozess und registriert die angegebenen Dev-Service-Endpunkte über den `ServiceRegistry`, statt ein Artifact zu packen; `release module` stellt wieder auf Distribution zurück.
- Module laufen dauerhaft ohne manuelles Start/Stop: CLI/HTTP expose keine manuellen Start/Stop-Kommandos mehr. Fenrir startet installierte Module beim Boot automatisch (`module:<id>` taucht im Service-Registry auf) und Distribution-Imports stoppen/aktualisieren/starts Module inklusive echter Progress-Anzeige. `.fenrir-dev.toml`-Services erscheinen separat als `module:<id>::service`.
- Laufende Module erhalten automatisch `FENRIR_MODULE_ID`, `FENRIR_SERVICE_ID`, `FENRIR_SERVICE_URI` sowie – bei dynamischer Portvergabe – `FENRIR_SERVICE_PORT` und `FENRIR_SERVICE_ADDR`. Diese ENV-Variablen ersetzen harte Ports/URIs innerhalb der Module.
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
