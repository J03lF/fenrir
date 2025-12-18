# Module Service Autoregistration – Lösungsansatz

Dieser Plan konkretisiert, wie Service-Definitionen aus dem Modulcode heraus erfolgen, wie Ports/Ingress dynamisch registriert werden und wie Module über `service://module-id::service-id` miteinander sprechen. Er baut auf `docs/module_service_autoreg_plan.md` auf und ergänzt die Umsetzungsschritte inkl. Pfadhinweisen.

## Architekturziele
- **Deklarative Services aus dem Code** – Module melden ihre Services (id, scope, ingress, health) zur Laufzeit, nicht mehr via `.fenrir/config.toml`.
- **Runtime-gebundene Registry** – `ServiceRegistry` bekommt Live-Metadaten (Ports, Status, Tokens) direkt vom Modulprozess.
- **Gateway-Orchestrierung** – HTTP/gRPC Ingress mountet Servicepfade dynamisch, respektiert RBAC/Scopes und rate-limits.
- **First-Class Inter-Service Calls** – SDK abstrahiert `service://`-Auflösung, Token-Exchange und HTTP/gRPC Calls.
- **Security & Config Hygiene** – Secrets/DB-Creds bleiben ausschließlich bei Fenrir; Module sehen nur Connector-URIs und Delegated Tokens.

## Service Registry Erweiterung
1. **SDK Hooks** (`module-kit/src/service.rs`):  
   - `ModuleServiceDescriptor::builder()` plus `ModuleReportedServices` erzeugen das JSON für `/.fenrir/services`.  
   - Module deklarieren Services direkt im Code (siehe Demos) – keine `.fenrir/config.toml`-Einträge mehr.
2. **Runtime API & Snapshot** (`src/services/module/runtime.rs`):  
   - ModuleRuntime pollt `http://127.0.0.1:<port>/.fenrir/services`, registriert die gelieferten Services und schreibt `runtime/services.json`.  
   - Allen Modulen wird `FENRIR_SERVICE_SNAPSHOT_PATH` gesetzt; der Snapshot dient als lokaler Resolver für `service://module:<id>::<service>`.
3. **Registry Filtering** (`src/services/module/service.rs`, `src/services/types.rs`):  
   - `module:<id>` Basis-Einträge wandern in `HiddenModuleEntry`; CLI blendet sie aus.  
   - `modules services` (`src/cli/commands/builtins/modules/handlers.rs`) zeigt nur echte Services + deren Live-Port.

## Ingress / Gateway
- **HTTP** (`src/infra/http/routes/modules.rs`): dynamisch generierte `Router`-Instanzen per ServiceDescriptor (`route_prefix`, `internal_only`).  
- **gRPC** (`src/infra/grpc/runtime.rs` – neu): registriert Services mit `tonic::transport::Server`, extrahiert Metadata → Audit.  
- **Routing Info** persistiert im Registry-Snapshot (`audit/modules/services.json`) für Diagnose & CLI.

## DB / Secrets / Env
- **Connector-only Access** (`module-kit/src/connector.rs`): ServiceCode nutzt `DbConnectorClient` (IPC/TCP) mit `DbConnectorIntent::{Read,Write}`.  
- **Secret Projection** (`src/boot/config.rs` + `src/services/module/runtime.rs`): Start blockt mit `BootErrorCode::ConfigMissingSecret`, falls Secrets/ENV nicht verfügbar.  
- **Env Contract** (bereits via `ModuleEnvironment`):  
  - `FENRIR_SERVICE_PORT`, `FENRIR_SERVICE_URI` – echte Ports statt statischer Config.  
  - `FENRIR_CONTROL_PLANE_URL` + optional TLS-Pfade für Token-Exchange.  
  - `FENRIR_DB_CONNECTOR_*` – Endpoint + protocol (ipc/tcp).  
  - `FENRIR_SERVICE_SNAPSHOT_PATH` – JSON mit allen `service://`-Zuordnungen (wird vom Runtime-Service gepflegt).
- **Docs** (`docs/modules_overview.md`, `AGENTS.md`) ergänzen neue Felder unter `modules.runtime`, `modules.service_defaults`.

## Rollen & Scopes
- **Policy Schema** (`src/services/module/types.rs`):  
  - `ServiceSecurityMetadata { internal_only, roles: Vec<Role>, scopes: Vec<Scope> }`.  
  - Default aus `[modules.runtime.default_service_scopes]`, pro Service overridebar.  
- **Token Flow** (`src/security/manager.rs`, `module-kit/src/tokens.rs`):  
  - `issue_service_token(service_id, scopes)` via Control-Plane API `/modules/runtime/tokens`.  
  - Fenrir liefert mit `FENRIR_SERVICE_TOKEN_{ISSUED_AT,EXPIRES_AT,TTL_SECS}` die Lease-Daten des aktuell injizierten Tokens, so dass Clients vor Ablauf rotieren können.  
  - Das module-kit stellt mit `ServiceTokenProvider` einen zentralen Refresh-Helper bereit (`ModuleEnvironment::token_provider()`), der das Standard-Token automatisch erneuert und bei Bedarf zusätzliche Scopes (z. B. `db:write`) per API anfordert.
- **Gateway Enforcement** (`src/infra/http/gateway.rs`):  
  - Requests erhalten `X-Fenrir-Actor`, `X-Fenrir-Scopes`, `X-Fenrir-Tenant`.  
  - Policies prüfen `ensure_scope` + `ensure_role`.

## Kommunikation & Clients
1. **Service Discovery** (`module-kit/src/env.rs`, Modulcode):  
   - Module lesen `FENRIR_SERVICE_SNAPSHOT_PATH` und können `service://module:notification-hub::email-outbound` lokal auflösen.  
   - Overrides (z. B. `NOTIFICATION_HUB_BASE_URL`) bleiben optional für Dev-Shortcuts.
2. **HTTP Client Wrapper** (`module-kit/src/clients/http.rs`):  
   - baut `reqwest` Client mit mTLS/Timeouts aus `ControlPlaneEnvironment`.  
   - injiziert Delegated Token per `Authorization: Bearer`.
3. **Usage Pattern**:  
   - Fenrir-API Handler ruft `NotificationHubClient::send_email(email, code)` → intern `call_service("service://module:notification-hub::email-outbound")`.  
   - Notification-Hub validiert Scope `notifications:send` bevor es externe SMTP/API benutzt.

## Umsetzungsschritte
| Schritt | Dateien | Kurzbeschreibung |
| --- | --- | --- |
| SDK-Bausteine | `module-kit/src/{service.rs,clients/http.rs,tokens.rs}` | API für Registration, Discovery, Token Handling |
| Runtime API | `src/infra/http/routes.rs`, `src/services/module/runtime.rs` | POST `/services`, persistenter Snapshot |
| Registry/CLI | `src/services/module/service.rs`, `src/cli/commands/builtins/modules/handlers.rs` | Sichtbare Services + dynamische Ports |
| Gateway | `src/infra/http/gateway.rs`, `src/infra/grpc/mod.rs` | Router-Mounting, policy enforcement |
| Docs | `docs/modules_overview.md`, `AGENTS.md`, `module-kit/README.md` | Flows, ENV, scopes |

## Demo-Szenario (implementiert in separaten Modul-Repos)
- **Dev-Aufstellung** (`/opt/fenrir/development/modules/`)  
  - Enthält `fenrir-web`, `fenrir-api` und `notification-hub`. `sync module <id>` nutzt diesen gemeinsamen Pfad, Dev-Overrides leben in denselben Repos.  
  - Alle drei Module exportieren `/.fenrir/services` und lesen `FENRIR_SERVICE_SNAPSHOT_PATH`, um die `service://`-Routen lokal aufzulösen.
- **Fenrir-Web** (`/opt/fenrir/development/modules/fenrir-web`)  
  - Angular Static-Site (`mode = "static_site"`). Nutzt das Gateway (`/gateway/services/module%3Afenrir-api%3A%3Aapi-gateway/*`) für Auth-Flows.  
  - UI-Screens: (1) Formular zum Anfordern eines Login-PINs, (2) Eingabeseite für empfangene PINs inkl. TTL-Anzeige. Die PIN wird nie lokal persistiert; Web ruft ausschließlich die API.
- **Fenrir-API** (`/opt/fenrir/development/modules/fenrir-api`)  
  - `POST /api/v1/auth/email-code` nimmt `{ email }`, generiert einen 6-stelligen PIN, erzeugt einen `verification_code_id` und schreibt den Datensatz via `DbConnectorClient` (`DbConnectorIntent::Write`) nach `verification_codes` (`id`, `email`, `pin_hash`, `issued_at`, `expires_at`, `status`).  
  - Die TTL ist konfigurabel (Default 10 Minuten) und wird bereits beim Insert geprüft; abgelaufene Codes beantwortet das API mit `422 PIN_EXPIRED`.  
  - Meldet ServiceDescriptor über `/.fenrir/services` (`service_id = "api-gateway"`, `kind = "transport"`, `route_prefix = "/api/v1"`, `scopes = ["tickets:read","notifications:send"]`).  
  - `POST /api/v1/auth/email-code/verify` erwartet `{ verification_code_id, pin }`, lädt per `DbConnectorClient` (`DbConnectorIntent::Read`) den Datensatz, vergleicht den Hash via `SecurityManager::verifier`, markiert erfolgreiche PINs als `consumed` und erstellt danach die Session.
- **Notification Hub** (`/opt/fenrir/development/modules/notification-hub`)  
  - Deklariert Service `email-outbound` (`kind = "notification"`, `internal_only = true`).  
  - Endpoint `POST /internal/notify/email` verarbeitet Anfragen, nutzt DbConnector (z. B. Template lookup) und acked result; Descriptor kommt ebenfalls aus `/.fenrir/services`.  
  - Erwartet Payload `{ email, pin_preview, expires_at }`, loggt Audit `notifications::email-pin-dispatched` inkl. TTL und schreibt optionale Dispatch-Metadaten für spätere Auswertungen.
- **Flow**  
  1. Webclient (fenrir-web) ruft `POST /api/v1/auth/email-code` über das Gateway auf.  
  2. Fenrir API erzeugt PIN + TTL, persistiert sie im `verification_codes`-Table und ruft `service://module:notification-hub::email-outbound`, damit der Dispatch die Informationen in die E-Mail schreibt.  
  3. Gateway erzwingt `notifications:send`; Notification Hub sendet die E-Mail, bestätigt Versand und liefert TTL-Notizen zurück.  
  4. Fenrir API reicht `verification_code_id` an das Frontend weiter; fenrir-web zeigt Countdown und erlaubt die PIN-Eingabe.  
  5. Bei der Eingabe ruft fenrir-web `POST /api/v1/auth/email-code/verify`, die API liest den Datensatz erneut, prüft Hash + TTL + Status und markiert den Code als verbraucht; gültige PINs resultieren in einer Session/Token-Antwort, ungültige in strukturierten Fehlern (`PIN_INVALID`, `PIN_EXPIRED`, `PIN_CONSUMED`).  
  6. Optionale Audits (`identity::pin-issued`, `identity::pin-verified`) werden geschrieben, um PIN-Lebenszyklen nachvollziehen zu können.

Siehe auch die aktualisierten Beispielcodes in den Modul-Repos für Referenz.
