use std::sync::Arc;

use crate::config::AppConfig;
use crate::infra::http::HttpServer;
use crate::infra::logging::ReloadHandle;
use crate::services::AppServices;

pub struct BootContext {
    pub config: Arc<AppConfig>,
    pub services: Arc<AppServices>,
    pub http_server: Arc<HttpServer>,
    pub logging: ReloadHandle,
}
