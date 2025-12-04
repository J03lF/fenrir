use std::time::SystemTime;

use super::managed::{ServiceControlError, ServiceControlOutcome};
use crate::security::auth::Role;
use crate::security::service::{ServiceRole, ServiceScope};
use crate::security::service_tokens::{DelegatedActor, DelegatedTokenClaims};
use crate::utils::messages::services::types::{
    action_kind, kind as kind_messages, status as status_messages, tag as tag_messages,
};

#[derive(Debug)]
pub struct ServiceActionReport {
    pub id: String,
    pub result: Result<ServiceControlOutcome, ServiceControlError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceActionKind {
    Start,
    Stop,
    Restart,
}

impl ServiceActionKind {
    pub fn as_str(self) -> &'static str {
        self.verb()
    }

    pub fn verb(self) -> &'static str {
        match self {
            ServiceActionKind::Start => action_kind::START,
            ServiceActionKind::Stop => action_kind::STOP,
            ServiceActionKind::Restart => action_kind::RESTART,
        }
    }

    pub fn required_role(self) -> Role {
        match self {
            ServiceActionKind::Start => Role::Operator,
            ServiceActionKind::Stop => Role::Operator,
            ServiceActionKind::Restart => Role::Admin,
        }
    }

    pub fn supports_force(self) -> bool {
        matches!(self, ServiceActionKind::Stop | ServiceActionKind::Restart)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceKind {
    Infrastructure,
    Transport,
    BackgroundJob,
    Cli,
    Security,
    Storage,
    Other,
}

impl ServiceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceKind::Infrastructure => kind_messages::INFRASTRUCTURE,
            ServiceKind::Transport => kind_messages::TRANSPORT,
            ServiceKind::BackgroundJob => kind_messages::BACKGROUND_JOB,
            ServiceKind::Cli => kind_messages::CLI,
            ServiceKind::Security => kind_messages::SECURITY,
            ServiceKind::Storage => kind_messages::STORAGE,
            ServiceKind::Other => kind_messages::OTHER,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ServiceTag {
    Core,
    Platform,
    Auxiliary,
}

impl ServiceTag {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceTag::Core => tag_messages::CORE,
            ServiceTag::Platform => tag_messages::PLATFORM,
            ServiceTag::Auxiliary => tag_messages::AUXILIARY,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceStatus {
    Starting,
    Active,
    Degraded,
    Failed,
    Standby,
    Stopped,
}

impl ServiceStatus {
    pub fn label(&self) -> &'static str {
        match self {
            ServiceStatus::Starting => status_messages::STARTING,
            ServiceStatus::Active => status_messages::ACTIVE,
            ServiceStatus::Degraded => status_messages::DEGRADED,
            ServiceStatus::Failed => status_messages::FAILED,
            ServiceStatus::Standby => status_messages::STANDBY,
            ServiceStatus::Stopped => status_messages::STOPPED,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ServiceIngressProtocol {
    Http,
    Grpc,
}

impl ServiceIngressProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceIngressProtocol::Http => "http",
            ServiceIngressProtocol::Grpc => "grpc",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceIngressAccess {
    Internal,
    Public,
}

impl ServiceIngressAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceIngressAccess::Internal => "internal",
            ServiceIngressAccess::Public => "public",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceRateLimit {
    Default,
    Unlimited,
    CustomPerSecond(u32),
}

impl ServiceRateLimit {
    pub fn limit_per_second(&self) -> Option<u32> {
        match self {
            ServiceRateLimit::Default => None,
            ServiceRateLimit::Unlimited => Some(u32::MAX),
            ServiceRateLimit::CustomPerSecond(value) => Some(*value),
        }
    }

    pub fn is_unlimited(&self) -> bool {
        matches!(self, ServiceRateLimit::Unlimited)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceTenantGuard {
    Any,
    Fixed(String),
    AllowList(Vec<String>),
}

impl ServiceTenantGuard {
    pub fn any() -> Self {
        Self::Any
    }

    pub fn fixed(value: impl Into<String>) -> Self {
        Self::Fixed(value.into())
    }

    pub fn allow_list(values: Vec<String>) -> Self {
        Self::AllowList(values)
    }

    pub fn allows(&self, tenant_id: &str) -> bool {
        match self {
            ServiceTenantGuard::Any => true,
            ServiceTenantGuard::Fixed(value) => value == tenant_id,
            ServiceTenantGuard::AllowList(values) => values.iter().any(|value| value == tenant_id),
        }
    }

    pub fn mode_label(&self) -> &'static str {
        match self {
            ServiceTenantGuard::Any => "any",
            ServiceTenantGuard::Fixed(_) => "fixed",
            ServiceTenantGuard::AllowList(_) => "allow_list",
        }
    }

    pub fn values(&self) -> Vec<String> {
        match self {
            ServiceTenantGuard::Any => Vec::new(),
            ServiceTenantGuard::Fixed(value) => vec![value.clone()],
            ServiceTenantGuard::AllowList(values) => values.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceIngressMetadata {
    pub route_prefix: Option<String>,
    pub health_endpoint: Option<String>,
    pub access: ServiceIngressAccess,
    pub rate_limit: ServiceRateLimit,
    pub protocols: Vec<ServiceIngressProtocol>,
}

impl ServiceIngressMetadata {
    pub fn internal() -> Self {
        Self {
            route_prefix: None,
            health_endpoint: None,
            access: ServiceIngressAccess::Internal,
            rate_limit: ServiceRateLimit::Default,
            protocols: vec![ServiceIngressProtocol::Http],
        }
    }

    pub fn public() -> Self {
        let mut metadata = Self::internal();
        metadata.access = ServiceIngressAccess::Public;
        metadata
    }

    pub fn with_route_prefix(mut self, prefix: impl Into<String>) -> Self {
        let mut value = prefix.into();
        if value.is_empty() {
            self.route_prefix = None;
        } else if value.starts_with('/') {
            self.route_prefix = Some(value);
        } else {
            value.insert(0, '/');
            self.route_prefix = Some(value);
        }
        self
    }

    pub fn with_health_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        let mut value = endpoint.into();
        if value.is_empty() {
            self.health_endpoint = None;
        } else if value.starts_with('/') {
            self.health_endpoint = Some(value);
        } else {
            value.insert(0, '/');
            self.health_endpoint = Some(value);
        }
        self
    }

    pub fn with_protocol(mut self, protocol: ServiceIngressProtocol) -> Self {
        if !self.protocols.iter().any(|existing| *existing == protocol) {
            self.protocols.push(protocol);
        }
        self
    }

    pub fn with_protocols<I>(mut self, protocols: I) -> Self
    where
        I: IntoIterator<Item = ServiceIngressProtocol>,
    {
        let mut unique = Vec::new();
        for protocol in protocols {
            if !unique.iter().any(|existing| *existing == protocol) {
                unique.push(protocol);
            }
        }
        self.protocols = if unique.is_empty() {
            vec![ServiceIngressProtocol::Http]
        } else {
            unique
        };
        self
    }

    pub fn with_rate_limit(mut self, rate_limit: ServiceRateLimit) -> Self {
        self.rate_limit = rate_limit;
        self
    }
}

#[derive(Clone, Debug)]
pub struct ServiceDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub kind: ServiceKind,
    pub critical: bool,
    pub tags: &'static [ServiceTag],
    pub security: Option<ServiceSecurityMetadata>,
    pub ingress: Option<ServiceIngressMetadata>,
}

impl ServiceDescriptor {
    pub const fn new(
        id: &'static str,
        name: &'static str,
        description: &'static str,
        kind: ServiceKind,
    ) -> Self {
        Self {
            id,
            name,
            description,
            kind,
            critical: false,
            tags: &[],
            security: None,
            ingress: None,
        }
    }

    pub fn critical(self) -> Self {
        Self {
            critical: true,
            ..self
        }
    }

    pub fn with_tags(self, tags: &'static [ServiceTag]) -> Self {
        Self { tags, ..self }
    }

    pub fn has_tag(&self, tag: ServiceTag) -> bool {
        self.tags.iter().any(|t| t == &tag)
    }

    pub fn with_security(self, metadata: ServiceSecurityMetadata) -> Self {
        Self {
            security: Some(metadata),
            ..self
        }
    }

    pub fn with_ingress(self, ingress: ServiceIngressMetadata) -> Self {
        Self {
            ingress: Some(ingress),
            ..self
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServiceDescriptorOwned {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: ServiceKind,
    pub critical: bool,
    pub tags: Vec<ServiceTag>,
    pub security: Option<ServiceSecurityMetadata>,
    pub ingress: Option<ServiceIngressMetadata>,
}

impl ServiceDescriptorOwned {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        kind: ServiceKind,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            kind,
            critical: false,
            tags: Vec::new(),
            security: None,
            ingress: None,
        }
    }

    pub fn with_tags(mut self, tags: impl Into<Vec<ServiceTag>>) -> Self {
        self.tags = tags.into();
        self
    }

    pub fn critical(mut self) -> Self {
        self.critical = true;
        self
    }

    pub fn has_tag(&self, tag: ServiceTag) -> bool {
        self.tags.iter().any(|t| t == &tag)
    }

    pub fn with_security(mut self, metadata: ServiceSecurityMetadata) -> Self {
        self.security = Some(metadata);
        self
    }

    pub fn with_ingress(mut self, ingress: ServiceIngressMetadata) -> Self {
        self.ingress = Some(ingress);
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}

impl From<ServiceDescriptor> for ServiceDescriptorOwned {
    fn from(value: ServiceDescriptor) -> Self {
        Self {
            id: value.id.to_string(),
            name: value.name.to_string(),
            description: value.description.to_string(),
            kind: value.kind,
            critical: value.critical,
            tags: value.tags.to_vec(),
            security: value.security.clone(),
            ingress: value.ingress.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceSecurityMetadata {
    pub internal_only: bool,
    pub allowed_roles: Vec<ServiceRole>,
    pub required_scopes: Vec<ServiceScope>,
    pub tenant: ServiceTenantGuard,
}

impl ServiceSecurityMetadata {
    pub fn internal_default() -> Self {
        Self {
            internal_only: true,
            allowed_roles: Self::default_allowed_roles(),
            required_scopes: Vec::new(),
            tenant: ServiceTenantGuard::any(),
        }
    }

    pub fn default_allowed_roles() -> Vec<ServiceRole> {
        vec![ServiceRole::Admin, ServiceRole::Write, ServiceRole::Read]
    }

    pub fn allows_role(&self, role: ServiceRole) -> bool {
        if self.allowed_roles.is_empty() {
            return true;
        }
        self.allowed_roles
            .iter()
            .any(|allowed| role.satisfies(*allowed))
    }

    pub fn allows_claims(&self, claims: &DelegatedTokenClaims) -> bool {
        if self.internal_only {
            match &claims.actor {
                DelegatedActor::Service { role, .. } => {
                    if !self.allows_role(*role) {
                        return false;
                    }
                }
                DelegatedActor::User { .. } => return false,
            }
        }
        if !self.tenant.allows(&claims.tenant_id) {
            return false;
        }
        if !self.required_scopes.is_empty() {
            for required in &self.required_scopes {
                if !claims.scopes.iter().any(|scope| scope == required) {
                    return false;
                }
            }
        }
        true
    }

    pub fn allowed_roles(&self) -> &[ServiceRole] {
        &self.allowed_roles
    }

    pub fn required_scopes(&self) -> &[ServiceScope] {
        &self.required_scopes
    }

    pub fn with_tenant_guard(mut self, guard: ServiceTenantGuard) -> Self {
        self.tenant = guard;
        self
    }
}

#[derive(Clone, Debug)]
pub struct ServiceSnapshot {
    pub descriptor: ServiceDescriptorOwned,
    pub status: ServiceStatus,
    pub since: SystemTime,
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    #[test]
    fn security_allows_roles_and_scopes() {
        let metadata = ServiceSecurityMetadata {
            internal_only: true,
            allowed_roles: vec![ServiceRole::Write],
            required_scopes: vec![ServiceScope::new("tickets:read").expect("scope")],
            tenant: ServiceTenantGuard::any(),
        };
        let claims = DelegatedTokenClaims {
            token_id: "id".to_string(),
            actor: DelegatedActor::Service {
                service_id: "module:test".to_string(),
                role: ServiceRole::Write,
            },
            tenant_id: "tenant".to_string(),
            scopes: vec![ServiceScope::new("tickets:read").expect("scope")],
            issued_at: OffsetDateTime::now_utc(),
            expires_at: OffsetDateTime::now_utc(),
        };
        assert!(metadata.allows_claims(&claims));
        let mut invalid = claims.clone();
        invalid.actor = DelegatedActor::Service {
            service_id: "module:test".to_string(),
            role: ServiceRole::Read,
        };
        assert!(!metadata.allows_claims(&invalid));
    }

    #[test]
    fn tenant_guard_modes() {
        let guard = ServiceTenantGuard::fixed("tenant-a");
        assert!(guard.allows("tenant-a"));
        assert!(!guard.allows("tenant-b"));
        let list = ServiceTenantGuard::allow_list(vec!["x".into(), "y".into()]);
        assert!(list.allows("x"));
        assert!(!list.allows("z"));
    }
}
