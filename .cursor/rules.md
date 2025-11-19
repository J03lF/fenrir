# Cursor Rules for Fenrir

## Purpose
- Keep Fenrir’s Rust monolith secure, layered, and module-friendly. Follow AGENTS.md and docs/modules_overview.md.

## Coding Style
- Rust 2021, run `cargo fmt` and `cargo clippy -D warnings` before commits; prefer total ordering of imports (std, third-party, crate).
- Avoid panics in non-test code; propagate typed errors (`thiserror`) or `anyhow` at boundaries.
- Keep functions small, name things explicitly, avoid implicit unwrap/expect except in tests.
- Tests: unit on pure logic (domain/security/services via mocks), integration under `tests/integration/`, no network in unit tests.

## Layer Boundaries
- Domain: pure logic and Ports/Traits only, no IO/config/security/infra types.
- Services: orchestrate Domain Ports + SecurityManager; no direct DB/IO.
- Infra: implement Ports (DB/adapters, telemetry, logging, transports, modules). Choose backend by config/feature flags.
- CLI/Transports: IO-only; map DTOs/errors to stable codes; no business logic.
- Security: crypto/session/RBAC only via `SecurityManager`; never use primitives directly.

## Security & Secrets
- No secrets/keys/PII in repo or logs. Secrets only from ENV/secret store; validate non-empty.
- Use `SecurityManager` for KDF/AEAD/passwords/sessions; audit RBAC decisions (`ensure_role` etc.).
- Respect production SSH rules (identity broker, ignore FENRIR_SSH_PASSWORD in prod).

## Config & Boot
- Config load order: default → profile (`FENRIR_CONFIG_ENV`/`FENRIR_ENV`) → local → `FENRIR_CONFIG_FILE` → `FENRIR__...` overrides.
- Fail-fast validation; use stable error codes (CFG-*, BOOT-*). No direct file/network reads in domain/services.

## Modules
- Module registry/runtime via `services::module` + `infra::modules::*`. Trust gates per `modules.trust`; signatures required in prod.
- Keep `docs/modules_overview.md` updated when adding/extending module contracts or ports.
- New commands in CLI go under `src/cli/commands/builtins/<name>/` with `register()`; no hardcoded registry mutations elsewhere.

## Database & Migrations
- Use engine-agnostic Ports; adapter-specific code stays under `infra/db/adapters/<engine>/`.
- Migrations per engine under `infra/db/migrations/<engine>/`; runner must be idempotent and guarded.

## Logging/Telemetry
- Use structured tracing; no PII; include stable codes. Telemetry counters/gauges via `infra::telemetry`. Health/metrics only when boot completed.

## Clean Code Hygiene
- Prefer dependency injection over globals; avoid singletons beyond existing registries.
- Comment only where intent or invariants are non-obvious; keep code self-explanatory.
- Keep public APIs minimal; hide helper functions with `pub(crate)`/`fn` as appropriate.

## Checklist Before Merge
- cargo fmt, cargo clippy -D warnings, cargo test (or targeted suites if slow).
- Update AGENTS.md when touching config schema, security flows, telemetry, or layer rules. Update docs/modules_overview.md if module contract changes.
