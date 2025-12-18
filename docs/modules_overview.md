# Fenrir Ticketsystem – Modulübersicht

| Modul | Zweck                                                                                                                                                          | Kernabhängigkeiten |
| --- |----------------------------------------------------------------------------------------------------------------------------------------------------------------| --- |
| `fenrir-api` | HTTP-Facade für Operator-Frontends. Nutzt den Runtime-Gateway (`FENRIR_GATEWAY_ENDPOINT`), ruft interne Module (Notification Hub, Module-Lifecycle, Diagnostics) auf und stellt REST-Handler (`/api/v1/...`) bereit. | Runtime-Gateway, Notification-Hub, Module-Registry, Audit |
| `ticket-domain` | Definiert Entities (Ticket, Comment, Attachment), Value Objects und Use-Case-Services (CRUD, Statuswechsel, SLA). Liefert Repositories/Ports für Datenhaltung. | Db-Ports, Security (Audit), Module Runtime für Events |
| `user-directory` | Synchronisiert Benutzer/Rollen mit Identity-Provider, verwaltet Ownership/Teams und stellt RBAC-Metadaten bereit.                                              | IdentityProvider, SecurityManager, AuditSink |
| `db-adapters-*` | Engine-spezifische Ticket-Repositories inkl. Migrationen für Postgres/MySQL/SQLite/Mongo; kapseln SQL/Query-Dialekte.                                          | DbEngine-Konfiguration, Migration Runner, Ticket-Domain-Ports |
| `notification-hub` | Versendet Ticket-Ereignisse (Created, SLA-violation, Assignment) über Mail, Webhook, Chat; nutzt Scheduler für Retries.                                        | SecurityManager (Secrets, Encryption), SchedulerService, AuditSink |
| `automation-rules` | Konfigurierbare Workflows (Auto-Assign, Eskalationen, Tagging). Bindet sich an Ticket-Service und Scheduler.                                                   | Ticket-Domain, SchedulerService, Security/RBAC |
| `reporting-analytics` | Aggregiert KPIs (Resolution Time, SLA-Erfüllung) und stellt APIs/Dashboards bereit; liest Telemetrie- und Audit-Daten.                                         | TelemetryState, Ticket-Domain-Ports, AuditLog |
| `search-indexer` | Baut Volltextsuche (z.B. Elasticsearch/Meilisearch) und liefert Query-Ports für fenrir-api; inkl. Re-Index-Jobs.                                               | Ticket-Domain, External Search Client, SchedulerService |
| `attachment-storage` | Verwaltet Upload/Download, verschlüsselt Assets via SecurityManager::encrypt und speichert Metadaten beim Ticket.                                              | SecurityManager (AEAD/KDF), Storage Backend (S3/FS), Ticket-Domain |
| `audit-compliance-viewer` | Bereitet Audit-Events für Reviewer auf, ermöglicht Export/Retention und RBAC-konforme Einsicht.                                                                | AuditLog, SecurityManager (RBAC), Telemetry |
| `integration-*` | Connectors für Slack/Teams, PagerDuty, Jira etc.; jeweils als isoliertes Modul mit eigenen Secrets/Config-Ports.                                               | SecurityManager (Secrets), Module Registry, Ticket/API-Ports |
| `fenrir-web` | Angular-Frontend (Static Site). Lädt `public/fenrir.config.js`, spricht ausschließlich via `/gateway/services/module%3Afenrir-api%3A%3Aapi-gateway/*` mit `fenrir-api` und wird über `.fenrir/runtime.toml` als `static_site` gehostet. | Fenrir API (HTTP), Static-Site Runtime |

> Hinweis: Alle Module müssen signiert/registriert werden (siehe `modules.trust`), Secrets ausschließlich über ENV/Secret-Store beziehen und sich an die Layer-Guidelines aus `AGENTS.md` halten.

## Runtime-Profile & Static Assets

- Jedes Modul-Artefakt kann eine `.fenrir/runtime.toml` ausliefern. Die Datei definiert den Laufzeitmodus (`[runtime] mode = "process" | "static_site" | "auto"`) sowie optionale Hinweise für das Static Hosting (`[runtime.static_site] asset_roots = ["dist/fenrir-web", "projects/app/dist"], entrypoint = "dist/fenrir-web/index.html", index_file = "index.html"`).
- `mode = "static_site"` erzwingt den eingebetteten HTTP-Server des Module-Runtimes; `auto` fällt zurück auf die heuristische Suche nach `dist*/`, `build/`, `public/`, `projects/*/dist/browser` sobald kein Binary gefunden wird.
- Der Static-Server dient `/.fenrir/services` plus `/live|/ready|/healthz`, streamt Assets aus dem gefundenen `asset_root` und protokolliert die Latenzen über `ServiceDiagnostics` (`module-runtime-static/<module-id>`). `/.fenrir/services` lädt – sofern vorhanden – `dist/**/.fenrir/services.json` oder `fenrir-services.json`; andernfalls erzeugt Fenrir einen Default-Eintrag `module:<id>::static` (public, `route_prefix = "/"`). Damit funktionieren `fenrir-web` & Co. auch nach einem Stop/Start konsistent ohne Wrapper-Binary und tauchen automatisch in `list services` auf.
- Zusätzlich proxen Static-Site-Server alle Requests unter `/gateway/*` automatisch zur Fenrir-Control-Plane. Web-Module können so relative Pfade (`/gateway/services/...`) verwenden, ohne Ports oder Hosts vorab zu konfigurieren.
- Beim Proxying normalisiert das Gateway deklarierte `route_prefix`-Werte (z. B. `/api/v1`) und entfernt doppelt vorkommende Segmente, falls der Client denselben Prefix bereits mitsendet. Empfehlung: Frontends adressieren nur den Service-Tail (`/auth/email-code`), der Prefix wird bei Bedarf automatisch ergänzt.

## Runtime-Gateway

- Fenrir injiziert für jedes Modul `FENRIR_GATEWAY_ENDPOINT`. Das ist ein lokaler HTTP-Endpunkt (`http://127.0.0.1:<ep>/call`), der Payloads der Form

```json
{
  "target": "service://module:notification-hub::public",
  "verb": "POST",
  "path": "/send",
  "body": { "ticket_id": "...", "template": "sla_warning" },
  "headers": { "x-trace-id": "..." },
  "query": { "tenant": "default" },
  "timeout_ms": 5000
}
```

  entgegennimmt.
- `target` muss eine gültige `service://`-URI oder ein `module:<id>[::suffix]` sein. Fenrir löst die Ingress-Zuweisung, hängt das aktuelle Service-Token an (inkl. automatischer Rotation), nutzt dieselben Retry-/Backoff-Einstellungen wie die Module-Clients und liefert eine normalisierte Antwort

```json
{
  "status": 200,
  "audit_id": "gw-9f0b6829",
  "headers": { "content-type": "application/json" },
  "body": { "format": "json", "value": { "result": "queued" } }
}
```

- Fehlgeschlagene Aufrufe enthalten ebenfalls einen `audit_id`-Wert plus Fehlertext und HTTP-Status (`4xx/5xx`). Die Gateway-Schicht schreibt Telemetrie unter `module-runtime-gateway/<module-id>`; Module benötigen damit weder eigene Gateway-Clients noch eine manuelle Token-Verwaltung.
