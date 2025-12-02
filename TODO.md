# Fenrir Module/Service Integration Roadmap

## Zielbild
- Module-Services werden als First-Class-Citizens in Fenrir betrieben: dynamische Ports, zentrale Registry, internes Service-Gateway, Telemetrie, RBAC und Secrets aus Fenrir.
- Externe Entry-Points laufen nur über fenrir-api (Gateway/BFF). Interne Services kommunizieren über `service://module-id::service-id` via Registry/Ingress.
- Keine festen Ports oder DB-Creds im Modul; Connector + RBAC/Scopes regeln Zugriff.

## To-Dos
- Service Registry erweitern
  - Port-Strategien (dynamic/fixed) + Persistenz im Runtime-State
  - Service-Metadaten: kind (http/grpc/worker), route_prefix, health_endpoint, auth/rate-limit flags
  - Lifecycle-API: start/stop/restart Service (graceful, backoff, quarantine)
  - Service-Discovery: `service://` Auflösung mit RBAC-Policy
- Ingress/Gateway
  - HTTP/gRPC-Ingress mountet Modul-Routen per Registry (prefix, auth, rate-limit)
  - Internal-only Flag (Service-Rolle erforderlich), Public Routen laufen durch fenrir-api
  - Tracing/Audit/Rate-Limits zentral
- DB/Secrets/Env
  - DB-Connector (IPC/gRPC) statt DB-Creds im Modul; Scoped Tokens/tenant-binding
  - Secrets/Config-Injection pro Service; Start blockt bei fehlenden Secrets
  - Env-Injection: `FENRIR_SERVICE_PORT`, `service://` URIs, OTEL/traceparent
- Rollen & Scopes
  - Service-Rollen: `service-read|service-write|service-admin`
  - Delegated Tokens: actor=user|service, tenant_id, scopes (`attachments:read`, `tickets:read`, `notifications:send`)
  - Per-Service Policy: akzeptierte Rollen/Scopes, internal_only
  - Tenant-Enforcement in Services (kein Cross-Tenant)
- Kommunikation & Clients
  - Internal Clients mit mTLS/JWT aus SecurityManager
  - Timeout/Retry/Backoff Defaults, Health-Probes, Quarantäne
  - Discovery via Registry; kein hardcoded Host/Port
- Telemetrie & Logs
  - Health-Checks in Registry; Status in CLI/SSH
  - OTEL-Injection (traces/metrics), Logs zentral abrufbar `modules logs`
- CLI/UX
  - Konsistente Task-Blocks + Tables (install/release/sync/uninstall)
  - `modules services` Übersicht mit Ports, Status, health, route info
  - Async-Ausführung für lange Tasks (keine Freezes, Streaming)
- Code-Struktur & Qualität
  - `mod.rs` nur als Entry, Logik in Untermods/Files; keine “God Files”
  - Tests/Mocks für Registry/Gateway/Connector; klare Traits/Interfaces
  - Lesbare, modulare Aufteilung je Schicht (Domain/Security/Services/Infra)

## Nächste Schritte (priorisiert)
- [x] 1. Port-Strategie + Registry-Persistenz + Env-Injection (dynamic ports).
- [ ] 2. Service-Rollen/Scopes + internal_only Policy im Gateway; delegated tokens.
- [ ] 3. Ingress-Mounting für Module (HTTP) mit Auth/Rate-Limits.
- [ ] 4. DB-Connector-Prototyp (IPC/gRPC) + Scoped Tokens.
- [ ] 5. Lifecycle-API (start/stop/restart) und Health/Quarantäne.
- [ ] 6. CLI: `modules services` Anzeige und Start/Stop-Commands über Registry.

## Hinweise
- Konfig-Pfad: `config/default.toml` → `config/<env>.toml` (FENRIR_ENV/FENRIR_CONFIG_ENV) → `config/local.toml` → `FENRIR_CONFIG_FILE` → ENV Overrides.
- Security: interne Services akzeptieren nur Service-Rollen oder delegierte Tokens mit Scopes + tenant_id.
- Keine festen Ports, keine DB-Creds im Modul-Code; alles über Registry/Connector/Env-Injection.

## Code-Struktur & Qualität
- `mod.rs` nur als Entry, Logik in Untermods/Files; keine “God Files”
- Lesbare, modulare Aufteilung je Schicht (Domain/Security/Services/Infra)
- Weitere Coding Sytles sind im AGENTS.md
