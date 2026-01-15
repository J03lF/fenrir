pub mod backup;
pub mod db_connector;
pub mod db_schema;
pub mod db_shell;
pub mod diagnostics;
pub mod jobs;
pub mod module;
pub mod scheduler;
pub mod security;
pub mod token_exchange;

mod app;
mod managed;
mod registry;
mod types;

pub use app::AppServices;
pub use backup::{BackupError, BackupResult, BackupService, BackupStatus};
pub use db_connector::{DbConnectorEndpoint, DbConnectorService};
pub use db_shell::DbShellService;
pub use diagnostics::{ServiceDiagnostics, ServiceMetricSnapshot};
pub use jobs::{JobLogError, JobLogSnapshot};
pub use managed::{block_on_managed, ManagedService, ServiceControlError, ServiceControlOutcome};
pub use module::{
    ModuleClientSettings, ModuleHealthHttpClient, ModulePortAllocator, ModuleService,
    ModuleServiceInit, ModuleServiceOverrides, ModuleTokenLease,
};
pub use registry::ServiceRegistry;
pub use scheduler::SchedulerService;
pub use security::SessionService;
pub use token_exchange::{TokenExchangeError, TokenExchangeService};
pub use types::{
    ServiceActionKind, ServiceActionReport, ServiceDescriptor, ServiceDescriptorOwned,
    ServiceIngressAccess, ServiceIngressMetadata, ServiceIngressProtocol, ServiceKind,
    ServiceRateLimit, ServiceSecurityMetadata, ServiceSnapshot, ServiceStatus, ServiceTag,
    ServiceTenantGuard,
};
