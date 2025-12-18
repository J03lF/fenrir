# StarUML Schema Export (dev preview)

## CLI
- `db schema export <file> [engine]`  
  - Example: `db schema export tmp/schema.mdj postgres`
  - Engine optional; falls back auf aktuellen DbShell-Default.

## Output
- Erzeugt eine StarUML `.mdj` mit:
  - UMLModel (`name = db-<engine>`)
  - UMLClass pro Tabelle/View (Stereotype: table/view/materialized_view/index/other)
  - UMLAttribute pro Spalte (Name, Datentyp, Default falls vorhanden)

## Grenzen (Stand jetzt)
- PK/FK für SQLite & Postgres werden modelliert (Stereotype `pk`/`fk` pro Spalte). Indices/constraints darüber hinaus fehlen noch.
- Schema wird über die bestehenden DbShell-Ports gezogen (engine-agnostisch).
- Für umfangreiche Modelle ggf. manuell in StarUML arrangieren (Layout/Diagramm).

## Geplant
- PK/FK-Erkennung pro Engine.
- Optionale Include-Views/Filter.
- Roundtrip-Import (StarUML → Migrationsplan).

