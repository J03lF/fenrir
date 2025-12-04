# Module Service Auto-Registration & Intra-Module IPC Plan

## Ausgangslage
- Dev-/Declared-Services werden aktuell nur via `.fenrir-dev.toml`/`.fenrir/config.toml` geparst (`src/services/module/dev.rs`), es gibt keine eigentlichen Binding Points im Modulcode.
- `ModuleService::module_service_descriptor` registriert für jedes Modul einen generischen `module:<id>`-Eintrag (`src/services/module/service.rs:267`), wodurch `modules services` Hauptmodule als Services wiedergibt.
- Env-Injection liefert zwar Ports/Tokens (`src/services/module/runtime.rs:394ff`), aber Module bieten keine Codestellen, um eigene Services zu deklarieren oder Ports an Fenrir zurückzumelden.
- `ServiceRegistry`-Notes behalten statische Endpoints aus Configs, obwohl Ports dynamisch vergeben werden (`src/services/module/dev.rs:229` etc.).

## Ziele
1. **Code-basierte Service-Definitionen in Modulen**  
   - API (z. B. über `fenrir-module-kit`) um Services mit `id`, `kind`, `ingress`, `security`, optionalem `health_endpoint` zu deklarieren.
   - Deklarationen liefern Ports/Protokolle dynamisch aus dem Modulprozess heraus.

2. **Automatisches Registrieren/Lebenszyklus**  
   - Module teilen Service-Metadaten inkl. lokalem Port mit dem Runtime-Prozess; ModuleService persistiert + aktualisiert Events.
   - Health/Status kommen aus aktiven Checks oder Modul-Signalen (z. B. HTTP Callback).

3. **Inter-Service RPC (`service://module-id::service-id`)**  
   - Module sollen andere Services via Gateway ansprechen (z. B. Fenrir-API → Notification-Hub).  
   - SDK-Helfer (client + token exchange) kapselt Service Discovery + HTTP/gRPC Calls.

4. **`modules services` / `list services` aufräumen**  
   - Standard-Einträge `module:<id>` ausblenden oder in separaten Abschnitt verschieben.  
   - Anzeigen von dynamisch zugewiesenen Ports / Ingress-URLs.

## Arbeitspakete
1. **SDK-Erweiterung (`module-kit/`)**
   - `registry`-Modul hinzufügen: `ServiceDescriptorBuilder`, `register_service()` → sendet JSON an Control-Plane (`/modules/runtime/services`).
   - HTTP/Gateway-Client (nutzt bereits vorhandenen Control-Plane Client + neue `service://` helper).
   - Dokumentierte Traits/Helper für Notification-Hub etc.

2. **Control-Plane API + Runtime**
   - Neue Route `POST /modules/runtime/:id/services` (Axum) → `src/infra/http/routes.rs`.
   - ModuleRuntime speichert gemeldete Services (z. B. `runtime/services.json`) und re-registriert beim Restart.
   - ServiceDescriptor aus API übernimmt `health_endpoint` → ModuleHealthMonitor kann zielgerichtet prüfen.

3. **ServiceRegistry/CLI Anpassungen**
   - `ServiceRegistry::register` filtern: Module-Basisservices (`module:<id>`) optional aus CLI verstecken.
   - `modules services` (`src/cli/commands/builtins/modules/handlers.rs`) zeigt Ports/Ingress aus Runtime-Snapshot, nicht aus statischen Notes.

4. **Gateway Discovery**
   - `service://` Resolver über ModuleService (`resolve_ingress_target`) für modul-interne Clients aufbereiten.
   - SDK-Funktion `call_service(service_uri, request)` (HTTP + gRPC) inkl. Token Handling.

5. **Konfiguration/Docs**
   - Beispiel in `module-kit/README` + `docs/modules_overview.md`.
   - Update `AGENTS.md`/`config/README.md` mit neuem Flow.

## Offene Fragen
- Transport-Protokolle pro Service (HTTP/gRPC) → Flag im SDK-Build?
- AuthN zwischen Modulen: Default `service-write`? Optionale, moduldefinierte Scopes?
- Eventuelle Hot Reload/Watch notwendig?

## Dev-Agent Status
- `[dev.run]` Blöcke in `.fenrir-dev.toml` liefern jetzt den gewünschten Dev-Befehl (Array oder Shell-String), Working-Dir und optional zusätzliche ENV-Werte.
- `[dev.run]` Blöcke in `.fenrir-dev.toml` liefern jetzt den gewünschten Dev-Befehl (Array oder Shell-String), Working-Dir und optional zusätzliche ENV-Werte. Über `auto_start = false` lässt sich der Agent nur zur Env-Generierung verwenden; der Entwickler startet dann selbst `cargo run`.
- `sync module` erzeugt neben den bisherigen `dev-env-<service>.sh` Skripten auch ein neutrales `.fenrir/dev.env` (key=value). Fenrir setzt `FENRIR_DEV_ENV_FILE` in den Exporten, und die Module laden diese Datei automatisch, sodass `cargo run` ohne `source`-Schritt funktioniert. `release module` räumt `.fenrir/dev.env`, die `dev-env-*.sh` Files sowie den `dev-agent/` Ordner auf.
- `sync module` registriert dev Services, schreibt `runtime/dev-agent/config.json` und startet den eingebauten Supervisor (`fenrir --dev-agent-config …`).
- `release module` stoppt den Supervisor automatisch und entfernt den Override.
- CLI zeigt nach dem Sync den aktiven Dev-Agent samt Log-Pfad an; Env-Files bleiben als Fallback bestehen.
