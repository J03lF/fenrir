## 1. Module Simplicity Initiative
- Ziel: Modul-Autor:innen sollen nur noch Business-Code schreiben. Fenrir-spezifische Tasks (Token-Refresh, Connector, Service-Aufrufe, Health/Logging) laufen in einem offiziellen Toolkit.
- Deliverables:
  1. **`fenrir-service-kit` SDK (Rust + optional Node/TS bindings)**  
     - `ServiceContext::current()` liefert env, Logger, Telemetrie-Hooks.  
     - `GatewayClient::send("module:notification-hub", payload)` kapselt Service-Discovery, Tokens, Audit.  
     - `DbClient::query("SELECT …")` rotiert automatisch Tokens (`service_token_refresh`) und nutzt den Connector über IPC/TCP ohne dass das Modul URIs kennen muss.  
     - Hintergrund-Worker hält Tokens frisch (konfigurierbarer Lead). Ergebnis ins Fenrir-Audit.
     - Static-/UI-Module liefern `.fenrir/runtime.toml` mit `[runtime] mode = "process"|"static_site"|"auto"` sowie `[runtime.static_site] asset_roots|entrypoint|index_file`. `mode = "static_site"` erzwingt den eingebetteten Static-Server, `auto` fällt nur dann auf die Heuristiken (`dist*/`, `apps/*/dist/browser`, …) zurück, wenn kein Binary gefunden wird.
  2. **Runtime Gateway** ✅  
     - Fenrir stellt einen lokalen HTTP-Endpunkt `FENRIR_GATEWAY_ENDPOINT` bereit. Payload enthält `{target, verb, path?, body?, headers?, query?, timeout_ms?}`.  
     - Gateway löst das Service-URI (`service://…`), hängt passende Tokens dran und protokolliert Request/Response inkl. `audit_id`. Retries/Backoff nutzen die Module-Client-Settings, Fehler liefern strukturierte JSON-Antworten.  
     - Module benötigen dadurch keinen direkten Zugriff auf Control-Plane oder Tokens; der service-kit `GatewayClient` ruft nur noch `await call(...)`.
  3. **`fenrir modules scaffold <module-id>`** ✅  
     - Erzeugt Cargo/Node/Angular-Gerüst inkl. Health-Endpoint, Logging, `.fenrir-dev.toml`, Gateway/DB-Client Setup.  
     - Erstellt CI-Hooks (fmt/clippy/test) und Beispiel-Handler (z.B. EmailCommand).  
     - Option `--runtime angular` erzeugt front-end stub (siehe Angular-Idee unten).
  4. **Bootstrap Agent** ✅  
     - Beim Modulstart führt Fenrir `fenrir-module-kit init` aus, schreibt die kombinierte Laufzeit-Metadatei nach `<module>/.fenrir/runtime.json` und startet erst danach den Business-Binary.  
     - Bei Crash/Restart erzeugt Fenrir direkt ein `module::service-token-exchange`-Audit mit `reason = service_token_refresh`, ohne dass Modulcode `/modules/runtime/tokens` aufrufen muss.
  5. **Docs & Samples**  
     - Update `docs/modules_overview.md` + neue Cookbook-Sektion „Send email via notification-hub in 10 Zeilen“.  
     - Beispielmodule (Rust + TypeScript) demonstrieren Gateway + DbClient + Token-Auto-Rotation.
  6. **Modul-Ökosystem anpassen**  
     - Alle bestehenden Module (z.B. `modules/notification-hub`, `modules/fenrir-api`) auf das neue Kit refactoren, sodass der Business-Code nur noch aus `gateway::send`, `db_client.query` usw. besteht (ca. 10 Zeilen pro Flow).  
     - Das `fenrir-service-kit` liegt außerhalb des Fenrir-Monolithen (eigenes Repo unter `modules/fenrir-service-kit/`). Fenrir bindet es nur als Git-Dependency ein.  
     - Einheitliche Struktur:
       ```
       modules/
         fenrir-service-kit/     # SDK + Gateway-Client
         notification-hub/       # Rust-Modul, nutzt das Kit
         fenrir-api/             # Rust-HTTP-Facade für Operator-Frontends
         fenrir-web/             # Angular UI, konsumiert fenrir-api via Gateway
       ```
       das alles liegt in /opt/fenrir/development/ 
     - Ziel: Keine duplizierten Helper im Core; jedes Modul importiert das Kit aus `modules/fenrir-service-kit`.

## 6. Angular Control-Plane UI ✅
- `modules/fenrir-api` ist ein Rust-Modul (`module:fenrir-api::api-gateway`). Es nutzt `FENRIR_GATEWAY_ENDPOINT`, ruft Notification Hub & Co. auf und stellt REST-Endpunkte (`/api/v1/...`) für alle Operator-Frontends bereit.
- `modules/fenrir-web` ist ein eigenständiges Angular-App-Repo. Es lädt `public/fenrir.config.js`, ruft ausschließlich `/gateway/services/module%3Afenrir-api%3A%3Aapi-gateway/*` auf und kann als `static_site`-Modul ohne Node-Prozess gestartet/synchronisiert werden (`npm run build` → `dist/fenrir-web`).
