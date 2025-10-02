# HTTP Control Plane Provisioning & Operations

The HTTP transport exposes administrative health and lifecycle endpoints. Before enabling
`server.enable_http`, provision *role-scoped bearer secrets* via the configuration key
`security.http.control_tokens`.

## Token Layout

Tokens are defined as `<role>:<value>` entries. `value` should reference a secure source:

```toml
[security.http]
control_tokens = [
  "admin:env:FENRIR_HTTP_TOKEN_ADMIN",
  "operator:env:FENRIR_HTTP_TOKEN_OPERATOR",
  "viewer:env:FENRIR_HTTP_TOKEN_VIEWER",
]
```

At runtime, set the environment variables with cryptographically strong random strings
(≥32 bytes, base64 or hex encoded). Secrets must never be stored in Git or plain text config.

### Example provisioning script

```bash
export FENRIR_HTTP_TOKEN_ADMIN="$(openssl rand -hex 32)"
export FENRIR_HTTP_TOKEN_OPERATOR="$(openssl rand -hex 32)"
export FENRIR_HTTP_TOKEN_VIEWER="$(openssl rand -hex 32)"
```

Reload the service once the environment is in place. Fenrir aborts boot when HTTP is enabled
but no tokens are configured.

## Rotation Strategy

1. Generate the replacement token (same length/entropy as initial provisioning).
2. Add the new token to the environment and reload the supervising process to inject it.
3. Update affected clients to start using the new token.
4. Remove the legacy token from the configuration and restart Fenrir again.

Use staggered overlaps (dual tokens) to keep management sessions online during rotation.

## Role Semantics

Role | Capabilities
-----|-------------
viewer | `GET /services`, `GET /metrics`, `GET /health/*`
operator | viewer + `POST /services/:id/start|stop`
admin | operator + `POST /services/:id/restart`

Attempting actions beyond the caller role returns `403 Forbidden` with a structured error payload.

## Operational Checklist

- [ ] Ensure HTTP port is protected by firewall / VPN in addition to bearer tokens.
- [ ] Monitor access logs for unusual token usage (sudden source/IP changes).
- [ ] Rotate tokens quarterly or when personnel changes occur.
- [ ] Store tokens in a secret manager (Vault, AWS Secrets Manager) rather than shell profiles.
- [ ] Schedule regular smoke tests exercising `/health` and `/services` after rotations.
