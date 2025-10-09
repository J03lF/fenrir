Configuration quickstart

1) Username/Host/Port
   - Set in `config/default.toml` under `[server.ssh]`:
     - `user` (e.g., `test`)
     - `host` (e.g., `127.0.0.1`)
     - `port` (e.g., `2222`)

2) Password (ENV only)
   - Set `FENRIR_SSH_PASSWORD` in your shell before starting the server:
```
export FENRIR_SSH_PASSWORD='your-secret-password'
```

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
- Permission denied: ensure `FENRIR_SSH_PASSWORD` matches the password you type.

postgresql://<USER>:<PASSWORD>@<HOST>:<PORT>/<DBNAME>
export FENRIR_DB_POSTGRES_URI=postgresql://root:1234@localhost:5432/postgres
export FENRIR_SSH_PASSWORD=test
export FENRIR_HTTP_TOKEN_ADMIN=testa
export FENRIR_HTTP_TOKEN_OPERATOR=testo
export FENRIR_HTTP_TOKEN_VIEWER=testv
export FENRIR_REGISTRY_TOKEN=""
TLS Konfiguration
- HTTP: aktiviere `[server.http.tls]` mit `enabled = true`, setze `cert_path`, `key_path` und sichere `cipher_suites`.
- Optional `reload_interval_seconds` erlaubt automatisches Neuladen von Zertifikaten.
- SSH: nutze `[server.ssh.tls]` um erlaubte Cipher-Suites zu überschreiben und `host_key_reload_seconds` für Key-Rotation zu setzen.
