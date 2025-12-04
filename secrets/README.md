Do not commit real secrets. Use environment variables in development and a secret store in production.

Fenrir automatically loads `secrets/.env` (if present) before parsing its configuration. Put your local dev secrets there so you don't have to `export` them manually:

```
# secrets/.env
FENRIR_SSH_PASSWORD=dev-password
FENRIR_HTTP_TOKEN_ADMIN=admin-token
FENRIR_DB_POSTGRES_URI=postgresql://localhost:5432/fenrir
```

- Only variables that are currently unset in the process are applied, so explicit `export` statements continue to win.
- To point at a different file, set `FENRIR_ENV_FILE=/absolute/path/to/.env` before starting Fenrir.
