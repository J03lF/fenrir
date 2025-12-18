# Fenrir Unified DB Runtime & StarUML Backlog

High-level goal: allow Fenrir to boot, supervise, observe, and evolve the ticket DB stack without an external container while offering a StarUML-based schema design loop (export/import). Tasks below call out **where** to wire code plus a short rationale so future implementers know where to start.

## 1. Config & Documentation
- [x] **Introduce `db.runtime` switch (`external` vs `embedded`)**  
  - Where: `src/config/model.rs`, `src/config/loading.rs`, `src/config/validation.rs`, `config/default.toml`, `config/dev.toml`, `AGENTS.md`.  
  - Notes: add enums (`DbRuntimeMode`, `EmbeddedEngineKind`), nested settings for `embedded.sqlite` (file path, vacuum schedule) and `embedded.postgres` (data dir, binary path, port range). Validation must ensure required sub-blocks per mode.
- [x] **Expose runtime choice via CLI/docs**  
  - Where: `docs/operations/http_control_plane.md`, `config/README.md`, `TODO.md` summary, `scripts/fenrir-setup.sh`.  
  - Notes: document env overrides (`FENRIR_DB_RUNTIME_MODE`, `FENRIR_DB_EMBEDDED_ENGINE`), mention default dev profile (`embedded-sqlite`).

## 2. Embedded DB Supervisor
- [x] **Create `infra::db::runtime` module**  
  - Where: new dir `src/infra/db/runtime/`.  
  - Notes: implement `DbRuntimeSupervisor` as `ManagedService`; capabilities: prepare data directories under `runtime/db/<engine>`, spawn embedded engine (`sqlite` via in-process connection, later `postgres` via child process), expose connection URI, health probe, and structured log stream.  
  - Status: stub supervisor scaffolded; engine spawn/URI/export still open.
- [x] **Autowire supervisor during boot**  
  - Where: `src/boot/bootstrap.rs`, `src/boot/startup.rs`, `src/services/app.rs`.  
  - Notes: when `mode = embedded`, start supervisor before building adapters, register it in `AppServices::register_runtime_service`, add registry descriptor (`service://db-runtime`) tagged as `core=false`.  
  - Status: registry entry + managed service wiring done; currently starts stub.
- [x] **Supervisor functionality (engine spawn & connector URI)**  
  - Where: `src/infra/db/runtime/`.  
  - Notes: sqlite path + URI live; sealed creds + status/log buffer. Embedded Postgres best-effort: initdb if needed, choose port from range, spawn process, tail stdout/stderr, set URI, health probe TCP; uses `binary_path`/`port_range`. (Further hardening/role creation still possible.)
- [x] **Internal credential sealing**  
  - Where: `src/infra/db/runtime/credentials.rs`.  
  - Notes: generates random creds, seals with SecurityManager/AEAD using `FENRIR_DB_RUNTIME_KEY`, stores at `runtime/db/internal-secrets.json`; decrypts on load. (Relies on env key; rotate by deleting file + restarting with same key.)
- [x] **Expose credentials/URI to connector path**  
  - Where: `src/infra/db/runtime/`, `src/infra/db/manager.rs`, `src/infra/db/adapters/sqlite/`.  
  - Notes: embedded sqlite path+URI wired into manager; sqlite adapter implemented (rusqlite) so DbShell/connector can use embedded DB.

## 3. Adapter Plumbing & Telemetry
- [x] **Inject supervisor URI into adapter builder**  
  - Where: `src/infra/db/manager.rs`, `src/boot/startup.rs`.  
  - Notes: runtime override from supervisor is injected when embedded; adapters built with runtime URI.
- [x] **Diagnostics + log streaming**  
  - Where: `src/infra/db/runtime/`, `src/services/diagnostics.rs`.  
  - Notes: Supervisor records probes/heartbeats under `db-runtime`, hält Log-Puffer (stdout/stderr) und Status (port/pid/health). Logs zusätzlich in Status abrufbar (AppServices).

## 4. Control Plane & CLI Surface
- [ ] **Service registry descriptors + HTTP endpoints**  
  - Where: `src/services/registry.rs`, `src/infra/http/routes.rs`, `src/infra/http/gateway.rs`.  
  - Notes: new control-plane endpoints `/services/db-runtime/{status,start,stop,restart}` mirroring module lifecycle; reuse `ManagedService` plumbing for RBAC/audit.
- [x] **CLI commands (`db runtime *`, `log db-runtime`)**  
  - Where: `src/cli/commands/builtins/db_runtime/`, `src/cli/commands/builtins/log/command.rs`.  
  - Notes: status/logs/control verfügbar via CLI; logs integriert als `log db-runtime`.
- [ ] **`fenrirctl` automation hooks**  
  - Where: `src/bin/fenrirctl.rs`, `docs/control_plane_cli_plan.md`.  
  - Notes: add subcommands mirroring CLI for remote ops tooling.

## 5. StarUML Export Pipeline
- [x] **Schema snapshot service**  
  - Where: `src/services/db_schema/mod.rs` consuming `DbShellService`.  
  - Notes: produces `DatabaseBlueprint` (tables/columns/kind) and exports StarUML `.mdj`.
- [x] **StarUML serializer**  
  - Where: `src/services/db_schema/mod.rs`.  
  - Notes: simple UMLModel with classes/attributes; tables get stereotypes by kind.
- [x] **CLI command `db schema export staruml <file>`**  
  - Where: `src/cli/commands/builtins/db_schema/`.  
  - Notes: `db schema export <file> [engine]` exports current schema to .mdj (engine optional).
- [x] **Docs & tests**  
  - Where: `docs/db_schema_export.md`, `tests/unit/services/db_schema.rs`.  
  - Notes: document CLI usage/limits; unit test covers sqlite export happy-path.

## 6. StarUML Import & Migration Generation
- [x] **StarUML parser & diff engine**  
  - Where: `src/services/db_schema/staruml_parser.rs`, `src/services/db_schema/diff.rs`.  
  - Notes: parses `.mdj`, baut Blueprint, diff erzeugt `SchemaMigrationPlan` (Add/Alter/Drop/PK/FK best-effort). Import-CLI noch stub.
- [x] **Migration synthesizer**  
  - Where: `src/infra/db/migrations/planner.rs`.  
  - Notes: translates `SchemaMigrationPlan` to SQL (pg/sqlite), supports `--dry-run` (saved under `runtime/migrations/generated/<engine>/`), guards drops via `--force`.
- [x] **CLI `db schema import staruml <file>`**  
  - Where: `src/cli/commands/builtins/db_schema/import.rs`.  
  - Notes: parse → diff → plan → dry-run or apply; audit event `db::schema::import`, diagnostics probe `db-schema-import`.
- [x] **Audit/telemetry hooks**  
  - Where: `src/cli/commands/builtins/db_schema/import.rs`.  
  - Notes: records audit (system actor) and diagnostics probe on import/dry-run.

## 7. Tooling, Scripts & Samples
- [x] **`db-test/` consolidation**  
  - Where: `db-test/docker-compose.yml`, `scripts/db-runtime-init.sh`, README.  
  - Notes: docker compose optional; init script seeds embedded runtime dir (sqlite).
- [x] **Developer workflow docs**  
  - Where: `docs/db_runtime_workflow.md`.  
  - Notes: tutorials for switching modes, streaming logs, exporting/importing StarUML.
- [x] **Update `fenrir-module-kit`**  
  - Where: `src/bin/fenrir-module-kit.rs`.  
  - Notes: exports embedded runtime env (`FENRIR_DB_RUNTIME_MODE|URI`) into runtime metadata for modules.

## 8. Testing & Quality Gates
- [x] **Unit tests**  
  - Where: `tests/unit/infra/db_runtime.rs`, `tests/unit/services/db_schema.rs`.  
  - Notes: cover supervisor start/stop stub + schema export/import pieces.
- [x] **Integration/E2E**  
  - Where: `tests/integration/db_runtime.rs`, `tests/e2e/cli_db_runtime.rs`.  
  - Notes: embedded runtime status/logs, CLI registration smoke.
- [ ] **Telemetry regression checks**  
  - Where: `tests/integration/telemetry.rs`.  
  - Notes: ensure `status service db-runtime` surfaces latency/p95/error%, log tail accessible.

## 9. Resilience & Lifecycle Hardening
- [x] **Snapshot & backup hooks**  
  - Where: `src/infra/db/runtime/snapshots.rs`, CLI `db runtime backup/restore`.  
  - Notes: sqlite backups/restore implemented; postgres backup/restore not implemented (errors).  
- [x] **Restore orchestration**  
  - Where: `src/cli/commands/builtins/db_runtime/command.rs`.  
  - Notes: CLI `db runtime restore <file>` (embedded only).  
- [ ] **Drift/health monitors**  
  - Where: Scheduler job (`src/services/scheduler/jobs/db_runtime_watchdog.rs`).  
  - Notes: recurring checks for disk usage, WAL lag, corruption flags; escalate via Audit + `ServiceDiagnostics`.
- [ ] **Multi-instance readiness**  
  - Where: `src/infra/db/runtime/mod.rs`.  
  - Notes: design supervisor to support future HA (leader election, passive replicas). Define config stubs (`db.runtime.embedded.postgres.replication`) so the migration later is additive.

## 10. Governance & Approval Workflow
- [ ] **Schema-change approval queue**  
  - Where: Scheduler job `db-schema-review`, persistence under `runtime/db/schema_queue.json`.  
  - Notes: StarUML imports first create a pending review item; operators approve/deny via CLI (`db schema review list|approve|reject`). Only approved plans apply migrations.
- [ ] **Audit enrichment**  
  - Where: `src/audit/event.rs`, `src/audit/log.rs`.  
  - Notes: include review IDs, reviewer, rationale in events `db::schema::review` / `db::schema::apply`. Attach generated SQL diff to audit metadata (capped/hashed).
- [ ] **Policy hooks**  
  - Where: `src/security/manager.rs`, new RBAC scopes (`db:schema:review`, `db:schema:apply`).  
  - Notes: ensure only scoped roles can approve/apply imports; CLI enforces `ensure_role`.
- [ ] **Notification/Telemetry**  
  - Where: `src/services/diagnostics.rs`, optional Notifications via `notification-hub`.  
  - Notes: send alert when pending reviews exceed SLA; record metrics `db-schema-review-lag`.

---

Nice-to-haves after MVP:
- Support multiple embedded engines concurrently (e.g., Ship sqlite for metadata + embedded Postgres for prod-like testing) by running multiple supervisor instances under different IDs.
- Watch a `runtime/db/schema.inbox/` directory for `.mdj` drops and auto-create review tasks via Scheduler job (`db-schema-watchdog`).
- Offer optional StarUML template bundle (`docs/staruml/fenrir-template.mdj`) preloaded with stereotypes + color scheme for DB models.

