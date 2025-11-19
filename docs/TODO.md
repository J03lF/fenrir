# TODO – Tickets-Backend (Monolith)

## Leitprinzipien
- [ ] Kein Hardcode – alles über Config/ENV initialisieren
- [ ] Klare Layering-Struktur (`domain`, `services`, `infra/*`, `cli/*`, `security/*`)
- [ ] Commands dynamisch – jeder Command eigener Ordner
- [ ] Audit & Security First – keine PII in Logs

---

## Phase 0 — Repository & Werkzeuge
- [ ] Repo scaffolding (Cargo.toml, toolchain, .gitignore, etc.)
- [ ] Ordnerstruktur anlegen (config, secrets, keys, migrations, ops, scripts, docs, schemas, tests, benches, src)
- [ ] CI minimal: fmt + clippy

---

## Phase 1 — Konfiguration
- [ ] Config-Layer (`src/config/`)
- [ ] Schema definieren (app, server, security, db, telemetry, audit, cli)
- [ ] Validierung & Fail-fast
- [ ] Secrets nur aus ENV

---

## Phase 2 — Logging, Telemetrie, Health
- [ ] Logging (tracing, level konfigurierbar)
- [ ] Telemetry (Prometheus, Traces)
- [ ] Health-Endpunkte

---

## Phase 3 — Security & Sessions
- [ ] Crypto: AEAD, RNG, KDF
- [ ] Auth: User/Pass, Lockouts
- [ ] Session-Modell (TTL, Storage)
- [ ] Rollen/Policies
- [ ] Audit: Append-only Events

---

## Phase 4 — Datenbanken
- [ ] DB-Abstraktion Trait
- [ ] Adapter: Postgres, MySQL, SQLite, MongoDB
- [ ] Pooling konfigurierbar
- [ ] Migrations-Runner

---

## Phase 5 — Domain & Services
- [ ] Domain: Ticket, User, Comment, Attachment
- [ ] Value Objects: TicketId, UserId, Email, Status, Priority
- [ ] Services: Ticket (CRUD, Search), User (Auth, Roles)

---

## Phase 6 — CLI
- [ ] Shell: Login, Prompt, Dispatcher
- [ ] Builtins:
    - [ ] help
    - [ ] modules
    - [ ] services
    - [ ] user
    - [ ] db-shell (Subprompt `db:` mit Query-Dispatch)
    - [ ] exit
- [ ] Plugin-SDK + Loader

---

## Phase 7 — Transports
- [ ] SSH-Server mit PTY + Shell
- [ ] Optional: HTTP (Health, Metrics, Ticket CRUD)
- [ ] Optional: gRPC

---

## Phase 8 — Observability & Performance
- [ ] Tracing-Spans für kritische Pfade
- [ ] Metrics Requests/Fehler/Latenz
- [ ] Rate-Limits/Backpressure
- [ ] Feature-Flags

---

## Phase 9 — Tests
- [ ] Unit-Tests (Domain, Services)
- [ ] Integration-Tests (Login, Tickets, db-shell)
- [ ] E2E-Tests (SSH, API)
- [ ] Security-Tests (Lockout, RBAC, Logs ohne PII)
- [ ] Benchmarks

---

## Iteration 2 (Next)
- [ ] security/crypto: implement real KDF (Argon2id) and AEAD (XChaCha20-Poly1305)
- [ ] sessions: in-memory store with TTL, sign/verify session tokens
- [ ] RBAC: gates for admin/operator/viewer
- [ ] Postgres adapter: implement connect() and ping(); config for URI
- [ ] CLI: login flow skeleton using authenticator stub
- [ ] db-shell: real SELECT dispatch via adapter; simple table formatter
- [ ] Unit/Integration tests for auth, db adapter
- [ ] Modul-Contract definieren: Ports/Lifecycle damit `fenrir-api` HTTP/gRPC/CLI-Befehle sauber registrieren kann; Dokumentation in `docs/modules_overview.md`

## Iteration 3
- [ ] SSH server: start, PTY, bind CLI shell
- [ ] Migrations runner: apply up migrations from migrations/postgres/
- [ ] Domain & services: Ticket/User minimal use-cases via ports
- [ ] Optional HTTP: health and metrics endpoints
- [ ] E2E tests: CLI over SSH basic flow

## Phase 10 — Migration & Backup
- [ ] Forward/backward Migrations
- [ ] Seed-Daten
- [ ] Backup/Restore-Guides

---

## Phase 11 — Packaging & Deployment
- [ ] Binary optimiert (LTO, strip)
- [ ] Dockerfile (multi-stage)
- [ ] Systemd Unit
- [ ] K8s Deployment + Secrets
- [ ] Versionierung (SemVer + Changelog)

---

## Phase 12 — Dokumentation
- [ ] Architektur-Übersicht
- [ ] Security Model
- [ ] CLI-Handbuch
- [ ] Operations Guide
- [ ] API Schemas gepflegt

---

## Definition of Done (Release)
- [ ] Alle Phasen fertig
- [ ] CI grün
- [ ] Container startbar, Health grün
- [ ] Docs vollständig
- [ ] Changelog + Tag erstellt
