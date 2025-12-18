# DB Runtime Workflow (embedded)

## Modi
- `db.runtime.mode = "external"`: nutzt `db.connections.*` wie gehabt.
- `db.runtime.mode = "embedded"`: Supervisor startet sqlite/postgres, URI wird intern injiziert.

## Setup / Init
- Optionales Init-Skript: `scripts/db-runtime-init.sh` (legt `runtime/db/` an, seedet sqlite).
- Postgres eingebettet: benötigt `db.runtime.embedded.postgres.binary_path` und Port-Range.

## Starten
- Boot startet Supervisor automatisch in embedded-Mode; URI/Status via:
  - CLI: `db runtime status`, Logs über `log db-runtime`.
  - HTTP: `GET /services/db-runtime/status?tail=N`, `GET /services/db-runtime/logs?tail=N`.
  - `fenrirctl`: `db-runtime-status|logs|start|stop|restart`.

## Schema-Export/Import
- Export: `db schema export <file> [engine]` → StarUML `.mdj`.
- Import: `db schema import <file> [engine] [--dry-run] [--force]` → Parser → Diff → Plan → Apply.
  - `--dry-run` speichert SQL unter `runtime/migrations/generated/<engine>/...`.
  - Audit: `db::schema::import`, Probe: `db-schema-import`.

## Hinweise
- PK/FK-Erkennung: SQLite/Postgres best-effort; weitere Engines ohne PK/FK.
- SQLite-Alter/Drop wird als Kommentar im Plan markiert (erfordert manuellen Rebuild).
- Logs/Tail über Supervisor-Puffer; Health/Diagnostics unter `db-runtime`.

