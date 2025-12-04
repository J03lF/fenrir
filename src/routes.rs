use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use rand::{thread_rng, Rng};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{error, info};
use uuid::Uuid;

use crate::manifest::{ServiceDescriptor, ServiceManifest};
use crate::notification::{NotificationDispatchResult, NotificationHubClient};

#[derive(Clone)]
pub struct AppState {
    manifest: Arc<ServiceManifest>,
    descriptor: Arc<ServiceDescriptor>,
    notification_client: NotificationHubClient,
}

impl AppState {
    pub fn new(
        manifest: Arc<ServiceManifest>,
        descriptor: Arc<ServiceDescriptor>,
        notification_client: NotificationHubClient,
    ) -> Self {
        Self {
            manifest,
            descriptor,
            notification_client,
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/health", get(health))
        .route("/.fenrir/services", get(reported_services))
        .route("/api/v1/services/manifest", get(service_manifest))
        .route("/api/v1/auth/email-code", post(send_email_code))
        .with_state(state)
}

async fn root(State(state): State<AppState>) -> Json<Value> {
    let descriptor = (*state.descriptor).clone();
    Json(json!({
        "service": descriptor,
        "status": "running",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "healthy",
        "service_id": state.manifest.service_id().to_string(),
        "module_id": state.manifest.module_id().to_string(),
        "version": env!("CARGO_PKG_VERSION")
    }))
}

async fn reported_services(State(state): State<AppState>) -> Json<ModuleReportedServicesWrapper> {
    let payload = state
        .manifest
        .reported_services(state.descriptor.as_ref());
    Json(ModuleReportedServicesWrapper(payload))
}

async fn service_manifest(State(state): State<AppState>) -> Json<ServiceDescriptor> {
    Json((*state.descriptor).clone())
}

async fn send_email_code(
    State(state): State<AppState>,
    Json(request): Json<SendCodeRequest>,
) -> Result<Json<SendCodeResponse>, (StatusCode, Json<ErrorResponse>)> {
    let request_id = Uuid::new_v4();
    let verification_code = generate_verification_code();
    let issuer_service = state.manifest.service_uri();

    info!(
        request_id = %request_id,
        email = %request.email,
        "dispatching verification code"
    );

    match state
        .notification_client
        .send_email_code(
            request_id,
            &request.email,
            &verification_code,
            &issuer_service,
        )
        .await
    {
        Ok(dispatch) => Ok(Json(SendCodeResponse {
            request_id,
            status: "sent".to_string(),
            delivered_via: dispatch.endpoint,
            expires_in_seconds: 300,
        })),
        Err(err) => {
            error!(
                request_id = %request_id,
                error = %err,
                "failed to dispatch verification code"
            );
            Err((
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    request_id,
                    message: "notification hub unavailable".to_string(),
                }),
            ))
        }
    }
}

fn generate_verification_code() -> String {
    let mut rng = thread_rng();
    format!("{:06}", rng.gen_range(0..1_000_000))
}

#[derive(Debug, Deserialize)]
struct SendCodeRequest {
    email: String,
}

#[derive(Debug, Serialize)]
struct SendCodeResponse {
    request_id: Uuid,
    status: String,
    delivered_via: String,
    expires_in_seconds: u64,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    request_id: Uuid,
    message: String,
}

#[derive(Serialize)]
struct ModuleReportedServicesWrapper(
    #[serde(with = "crate::routes::module_services_serde")] ModuleReportedServices,
);

mod module_services_serde {
    use fenrir_module_kit::service::ModuleReportedServices;
    use serde::{Serialize, Serializer};

    pub fn serialize<S>(value: &ModuleReportedServices, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.serialize(serializer)
    }
}
