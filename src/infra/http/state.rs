use std::sync::Arc;

use crate::security::auth::ControlPlaneAuthorizer;
use crate::services::{AppServices, ServiceRegistry};

#[derive(Clone)]
pub(super) struct HttpInfo {
    pub(super) app_name: String,
    pub(super) app_version: String,
    pub(super) host: String,
    pub(super) port: u16,
}

#[derive(Clone)]
pub(super) struct HttpState {
    pub(super) registry: Arc<ServiceRegistry>,
    pub(super) services: Arc<AppServices>,
    pub(super) auth: Arc<ControlPlaneAuthorizer>,
    pub(super) info: HttpInfo,
}
