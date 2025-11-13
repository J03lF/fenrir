# Identity Broker TLS via Managed Proxy (Variant B)

Fenrir production environments terminate Identity-Service traffic at a dedicated
TLS proxy or load balancer. The proxy presents browser-grade certificates and
forwards plain HTTP to the Rust identity-service pool. This model keeps crypto
operations outside the app while enabling centralized control and scale-out.

## Why the Proxy Layer

- Zertifikats-Lifecycle liegt außerhalb der Anwendung; Rotation automatisierbar.
- Scale-Out: mehrere Identity-Service-Instanzen gesichert hinter einem Load Balancer.
- Security Layer: Proxy kann zusätzliche Checks (WAF, Auth, DDoS-Guard) übernehmen.
- Trennung der Verantwortlichkeiten: App-Team konzentriert sich auf Business-Logik, Plattform/SRE verwaltet TLS und Infrastruktur.

## Proxy Requirements

- Terminate TLS with certificates issued from the company PKI or an approved CA.
- Forward requests to the identity-service upstream(s) via HTTP on an internal network.
- Append `X-Forwarded-Proto` and `X-Forwarded-For` headers for auditing (optional but recommended).
- Enforce mutual TLS if the environment requires additional caller authentication.
- Expose a health check endpoint (e.g. `/healthz`) and integrate it into the load balancer.

> Certificate rotation, OCSP stapling, HSTS and any WAF policies live entirely on the proxy.

## Fenrir Configuration

`config/prod.toml` must reference the HTTPS endpoint that the proxy exposes.
Point Fenrir to the CA bundle (and optional client certificate) so the identity
client validates the proxy during logins and token operations.

```toml
[security.identity.external]
base_url = "https://identity.prod.example.com"
jwks_url = "https://identity.prod.example.com/jwks.json"
auth_token = "env:FENRIR_IDENTITY_TOKEN"
jwks_refresh_seconds = 300
audience = "fenrir-control-plane"

[security.identity.external.tls]
ca_cert_path = "env:FENRIR_IDENTITY_TLS_CA"
# Optional mutual TLS configuration; must point at PEM files containing cert+key.
# client_cert_path = "env:FENRIR_IDENTITY_TLS_CERT"
# client_key_path = "env:FENRIR_IDENTITY_TLS_KEY"
accept_invalid_certs = false
```

Runtime expectations:

- `FENRIR_IDENTITY_TLS_CA` resolves to a PEM bundle with the proxy’s issuing CA.
- If mutual TLS is enabled, provide PEM files that include the full certificate
  and corresponding private key. Both variables must be set together.
- `accept_invalid_certs` is only meant for local testing; Fenrir rejects `true`
  when `security.identity.environment` is `prod`/`production`.

## Identity-Service Deployment

- Start each identity-service instance in HTTP mode behind the proxy.
- Keep instance counts identical to the Fenrir control-plane expectation to
  simplify auditing. Multiple versions may run concurrently; route by hostname
  or dedicated listener as needed.
- Store admin tokens (`FENRIR_IDENTITY_ADMIN_TOKEN`, etc.) in your secret store.
- Enable persistent storage for user/password history so sessions survive restarts.

## Starting Fenrir in Production Mode

```bash
export FENRIR_ENV=prod
export FENRIR_IDENTITY_TOKEN="$(op read op://identity/prod/fenrir-client-token)"
export FENRIR_IDENTITY_TLS_CA=/etc/ssl/certs/identity-proxy.pem
# Optional mutual TLS:
# export FENRIR_IDENTITY_TLS_CERT=/etc/fenrir/tls/fenrir-client.crt
# export FENRIR_IDENTITY_TLS_KEY=/etc/fenrir/tls/fenrir-client.key

cargo run --release -- --check-config   # should print CFG-OK
cargo run --release
```

Fenrir logs a structured `BOOT-IDENTITY-INIT` entry if the HTTPS handshake fails.
Check certificate paths, proxy availability and token values first when triaging.

