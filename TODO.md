# Fenrir TODO & Improvement Backlog

## Security & Identity
- Implement dedicated session service with secure storage (in-memory + pluggable persistent backend) and integrate SSH/CLI/HTTP logins with session lifecycle.
- Replace static control-plane tokens with configurable OAuth2/JWT validation and enforce RBAC gates per transport.
- Add rate-limiting and anomaly detection for transport logins (backoff, lockout, audit trail enrichment).

## Configuration & Boot
- Wire TLS certificate rotation hooks into HTTP transport (file watchers + graceful reload).

## Infrastructure Services
- Extend scheduler with cron-like expressions and persistence of job definitions; expose introspection via CLI/HTTP.
- Implement database adapter auto-discovery & readiness checks per configured engine, including pooled connection validation.
- Add pluggable storage backends (S3/MinIO) for attachments and ensure encryption at rest by default.

## Application Services
- Complete Ticket and User service command coverage (update, delete, RBAC checks) and add domain-level validation rules.
- Introduce modular plugin runtime (loader, signature verification, sandboxing) for CLI commands and transport handlers.

## Observability & Operations
- Wire up metrics export (Prometheus/OpenTelemetry) including scheduler/job metrics and per-transport counters.
- Add structured audit sink (file + optional external system) with tamper-evident hashing.
- Provide readiness gates per managed service (including HTTP/NATS/etc.) and surface via `/health/ready`.

## Testing & QA
- Create integration suites for SSH transport (auth flows, db-shell guard rails) and scheduler job orchestration.
- Add property-based tests for domain value objects (tickets, users) and fuzz testing for CLI parsers.
- Configure CI to run `cargo fmt`, `cargo clippy -D warnings`, unit/integration/E2E tests, and security scanners.

## Packaging & Delivery
- Produce Docker images with multi-stage builds, including non-root runtime user and health probes.
- Add deployment manifests (systemd unit, Helm chart) with environment-driven secrets and config maps.
