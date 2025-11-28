use crate::security::auth::Role;
use whoami;

pub struct PromptSet {
    pub main_cli: String,
    pub main_transport: String,
    pub db_cli: String,
    pub db_transport: String,
}

#[derive(Clone, Debug)]
pub struct PromptContext {
    pub user: String,
    pub host: String,
    pub role: String,
    pub transport: String,
}

impl PromptContext {
    pub fn local_default(app_host: &str) -> Self {
        Self {
            user: whoami::username(),
            host: whoami::fallible::hostname().unwrap_or_else(|_| app_host.to_string()),
            role: std::env::var("FENRIR_CLI_ROLE")
                .unwrap_or_else(|_| Role::Admin.as_str().to_string()),
            transport: "local".to_string(),
        }
    }
}
