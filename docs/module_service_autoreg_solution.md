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
  - SDK helper `ServiceTokenProvider::for_scope("notifications:send")`.
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
- **Fenrir-API** (`/opt/fenrir/development/modules/fenrir-api`)  
  - Route `POST /api/v1/auth/email-code` nimmt `{ email }`, fordert Service-Token `notifications:send`, ruft Notification Hub via SDK-Client, liefert Resultat an Webclient.  
  - Meldet ServiceDescriptor über `/.fenrir/services` (`service_id = "api-gateway"`, `kind = "transport"`, `route_prefix = "/api/v1"`, `scopes = ["tickets:read","notifications:send"]`).
- **Notification Hub** (`/opt/fenrir/development/modules/notification-hub`)  
  - Deklariert Service `email-outbound` (`kind = "notification"`, `internal_only = true`).  
  - Endpoint `POST /internal/notify/email` verarbeitet Anfragen, nutzt DbConnector (z. B. Template lookup) und acked result; Descriptor kommt ebenfalls aus `/.fenrir/services`.
- **Flow**  
  1. Webclient call → Fenrir API (service token from Gateway).  
  2. Fenrir API `service://module:notification-hub::email-outbound`.  
  3. Gateway enforces `notifications:send`.  
  4. Notification Hub sendet Email, antwortet success, Fenrir API forwarded success.

Siehe auch die aktualisierten Beispielcodes in den Modul-Repos für Referenz.
