Configuration quickstart

1) Username/Host/Port
   - Set in `config/default.toml` under `[server.ssh]`:
     - `user` (e.g., `test`)
     - `host` (e.g., `127.0.0.1`)
     - `port` (e.g., `2222`)

2) Password (ENV only)
   - Set `FENRIR_SSH_PASSWORD` in your shell before starting the server (development & test):
```
export FENRIR_SSH_PASSWORD='your-secret-password'
```
   - Production (`security.identity.provider = "external"`, `security.identity.environment = "prod"`, `app.version >= "1.0.0"`): SSH authentication delegates to the Identity-Broker (`POST /sessions/login`). Provision admin accounts there, set passwords via `identity-service` (`POST /users/password`), and ensure `security.identity.external.base_url`/`jwks_url` use HTTPS with a configured `auth_token`.

3) Validate config
```
cargo check-config
```

4) Start server
```
cargo run
```

5) SSH login
```
ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no \
    ${USER_FROM_CONFIG}@${HOST_FROM_CONFIG} -p ${PORT_FROM_CONFIG}

z.B: ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no test@127.0.0.1 -p 2222
```

Troubleshooting
- Host key changed warning: remove old key and re-scan
```
ssh-keygen -R "[127.0.0.1]:2222"
ssh-keyscan -p 2222 127.0.0.1 >> ~/.ssh/known_hosts
```
- Permission denied: ensure `FENRIR_SSH_PASSWORD` matches the password you type (development) or verify the Identity-Broker credentials and control token in production.

postgresql://<USER>:<PASSWORD>@<HOST>:<PORT>/<DBNAME>
export FENRIR_DB_POSTGRES_URI=postgresql://root:1234@localhost:5432/postgres
export FENRIR_SSH_PASSWORD=test
export FENRIR_HTTP_TOKEN_ADMIN=testa
export FENRIR_HTTP_TOKEN_OPERATOR=testo
export FENRIR_HTTP_TOKEN_VIEWER=testv
export FENRIR_REGISTRY_TOKEN="test12345678910111213141516"
export FENRIR_IDENTITY_TOKEN=test1234567891202


export FENRIR_IDENTITY_TLS_CA="/opt/fenrir/development/fenrir/config/certs/test-ca.crt"
export FENRIR_IDENTITY_TLS_CERT="/opt/fenrir/development/fenrir/config/certs/identity-client.crt"
export FENRIR_IDENTITY_TLS_KEY="/opt/fenrir/development/fenrir/config/certs/identity-client.key"

TLS Konfiguration
- HTTP: aktiviere `[server.http.tls]` mit `enabled = true`, setze `cert_path`, `key_path` und sichere `cipher_suites`.
- Optional `reload_interval_seconds` erlaubt automatisches Neuladen von Zertifikaten.
- SSH: nutze `[server.ssh.tls]` um erlaubte Cipher-Suites zu überschreiben und `host_key_reload_seconds` für Key-Rotation zu setzen.

export FENRIR_ENV=dev
export FENRIR_ENV=prod

new service or module what more sense is: Dynamic port manager, that dynmaic change ports if the port is in usw
