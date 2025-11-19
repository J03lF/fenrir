# Fenrir Ticketsystem – Modulübersicht

| Modul | Zweck | Kernabhängigkeiten |
| --- | --- | --- |
| `fenrir-api` | Exponiert Ticket-/User-/Workflow-Endpunkte via HTTP/gRPC, mappt DTOs auf Domain-Ports und erzwingt RBAC über den SecurityManager. | SecurityManager, SessionService, Ticket-Domain-Ports, Telemetrie |
| `ticket-domain` | Definiert Entities (Ticket, Comment, Attachment), Value Objects und Use-Case-Services (CRUD, Statuswechsel, SLA). Liefert Repositories/Ports für Datenhaltung. | Db-Ports, Security (Audit), Module Runtime für Events |
| `user-directory` | Synchronisiert Benutzer/Rollen mit Identity-Provider, verwaltet Ownership/Teams und stellt RBAC-Metadaten bereit. | IdentityProvider, SecurityManager, AuditSink |
| `db-adapters-*` | Engine-spezifische Ticket-Repositories inkl. Migrationen für Postgres/MySQL/SQLite/Mongo; kapseln SQL/Query-Dialekte. | DbEngine-Konfiguration, Migration Runner, Ticket-Domain-Ports |
| `notification-hub` | Versendet Ticket-Ereignisse (Created, SLA-violation, Assignment) über Mail, Webhook, Chat; nutzt Scheduler für Retries. | SecurityManager (Secrets, Encryption), SchedulerService, AuditSink |
| `automation-rules` | Konfigurierbare Workflows (Auto-Assign, Eskalationen, Tagging). Bindet sich an Ticket-Service und Scheduler. | Ticket-Domain, SchedulerService, Security/RBAC |
| `reporting-analytics` | Aggregiert KPIs (Resolution Time, SLA-Erfüllung) und stellt APIs/Dashboards bereit; liest Telemetrie- und Audit-Daten. | TelemetryState, Ticket-Domain-Ports, AuditLog |
| `search-indexer` | Baut Volltextsuche (z.B. Elasticsearch/Meilisearch) und liefert Query-Ports für fenrir-api; inkl. Re-Index-Jobs. | Ticket-Domain, External Search Client, SchedulerService |
| `attachment-storage` | Verwaltet Upload/Download, verschlüsselt Assets via SecurityManager::encrypt und speichert Metadaten beim Ticket. | SecurityManager (AEAD/KDF), Storage Backend (S3/FS), Ticket-Domain |
| `audit-compliance-viewer` | Bereitet Audit-Events für Reviewer auf, ermöglicht Export/Retention und RBAC-konforme Einsicht. | AuditLog, SecurityManager (RBAC), Telemetry |
| `integration-*` | Connectors für Slack/Teams, PagerDuty, Jira etc.; jeweils als isoliertes Modul mit eigenen Secrets/Config-Ports. | SecurityManager (Secrets), Module Registry, Ticket/API-Ports |

> Hinweis: Alle Module müssen signiert/registriert werden (siehe `modules.trust`), Secrets ausschließlich über ENV/Secret-Store beziehen und sich an die Layer-Guidelines aus `AGENTS.md` halten.


mindestens 12 module
+ fenrir-web = Webseite vom Ticketsystem kernabhänigkeiten fenrir-api