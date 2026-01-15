use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ReportedServicesPayload {
    #[serde(default)]
    pub services: Vec<ReportedServiceEntry>,
}

#[derive(Debug, Deserialize)]
pub struct ModuleServicesPublishRequest {
    pub module_id: String,
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default)]
    pub signed_at: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub services: Vec<ReportedServiceEntry>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ReportedServiceEntry {
    pub service_id: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub route_prefix: Option<String>,
    #[serde(default)]
    pub health_path: Option<String>,
    #[serde(default)]
    pub internal_only: Option<bool>,
    #[serde(default)]
    pub ingress_access: Option<String>,
    #[serde(default)]
    pub protocols: Vec<String>,
    #[serde(default)]
    pub required_scopes: Vec<String>,
    #[serde(default)]
    pub allowed_roles: Vec<String>,
}
