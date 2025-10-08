# Control Plane & CLI Expansion Backlog (AI Reference)

## 1. Module Ecosystem Revamp
- **Current gap**: `modules` builtin is placeholder; no lifecycle for external packages.
- **Target**: Implement signed, versioned module distribution pipeline.
- **Key tasks**:
  - Define module manifest format (name, semantic version, Fenrir compatibility, signing info, dependencies).
  - Implement registry interaction (local cache + remote repository with authenticated fetch, offline fallback).
  - CLI subcommands: `module search <pattern>`, `module info <name>`, `module install <name[@ver]>`, `module update`, `module remove`.
  - Add integrity checks (signature verification, hash validation) before activation.
  - Extend runtime plugin loader (`cli/plugins_runtime`) to load/update modules atomically; coordinate with audit log.
  - Ensure module activation updates service registry + audit entry.

## 3. Module Distribution Workflow
- **Goal**: Remote repository (git-based or artifact store) for publishing modules.
- **Key tasks**:
  - Define repository layout (index, manifests, signed artifacts).
  - Build `module publish` CLI flow for maintainers (package, sign, push).
  - Add server-side validation service (optional) to vet uploaded modules (lint, compatibility checks, attestation).
  - Wire backend bootstrap to detect installed modules and register them before transports start.

## 5. Admin Panel Extensions
- Dynamic panels for modules/plugins (status, version, available updates, load/unload actions).
- Real-time cards for background jobs (next run, last result, manual trigger with RBAC guards).
- Persistent notifications tray fed by SSE (audit, alerts, module updates).
- Theme configurator linked to `config/cli.prompt.theme` with live preview.

## 6. Audit & Telemetry Hardening
- Include CLI command name, arguments (redacted), and result codes automatically.
- Stream service registry changes (status transitions) over SSE alongside audit events.
- Add retention/archival strategy for audit log (spill to disk/DB when capacity exceeded).

## 7. Security & Trust
- Implement module signature verification pipeline (Ed25519 or X.509-based) before installation.
- Define trust policy (allowlist, required signer roles) and expose via CLI + UI.
- Audit events for module lifecycle (download, install, enable/disable, remove).

## 8. Testing & Tooling
- Add integration tests for CLI module commands (mock registry) and SSE stream (assert incremental updates).
- Provide fixtures for module manifests and signed artifacts.
- Stress-test audit broadcast under load (ensure backpressure + no panics).

## 9. Migration Path / Compatibility
- Introduce deprecation warnings for old commands and document upgrade path.
- Supply automated migration script to rename saved CLI aliases or scripts.
- Ensure config schema captures module repo endpoints + credentials (fail-fast validation).

## 10. Documentation Targets
- Author admin handbook covering module lifecycle, CLI verbs, SSE monitoring.
- Developer guide for building modules (SDK usage, packaging, signing, publishing).
- Update roadmap to include UI mockups and RBAC matrix for new commands.

_Note: Work items intentionally structured for iterative implementation; keep audit trails and RBAC enforcement central to every new surface._
