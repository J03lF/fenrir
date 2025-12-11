pub mod attach {
    pub const MODULE_SERVICE_ALREADY_ATTACHED: &str = "module service already attached";
    pub const SECURITY_MANAGER_ALREADY_ATTACHED: &str = "security manager already attached";
    pub const IDENTITY_SERVICE_ALREADY_ATTACHED: &str = "identity service already attached";
    pub const SESSION_SERVICE_ALREADY_ATTACHED: &str = "session service already attached";
    pub const TOKEN_EXCHANGE_ALREADY_ATTACHED: &str = "token exchange service already attached";
}

pub mod logs {
    pub const LOGGING_HANDLE_LOCK_POISONED: &str = "logging handle lock poisoned";
    pub const MANAGED_REGISTRY_LOCK_POISONED: &str = "managed service registry lock poisoned";
}
