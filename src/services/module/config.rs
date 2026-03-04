use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::str::FromStr;

use crate::config::{
    ConfigError, ModuleServiceOverride, ModuleServiceProfile, ModuleServiceTenantMode,
};
use crate::security::service::{ServiceRole, ServiceScope};
use crate::services::types::{ServiceIngressAccess, ServiceRateLimit, ServiceTenantGuard};
use crate::services::{ServiceDescriptorOwned, ServiceIngressMetadata, ServiceSecurityMetadata};

#[derive(Clone, Debug, Default)]
pub struct ModuleServiceOverrides {
    env: HashMap<String, Vec<ModuleEnvVar>>,
    security: HashMap<String, ServiceSecurityOverride>,
    profiles: HashMap<String, ModuleServiceProfileResolved>,
}

impl ModuleServiceOverrides {
    pub fn from_config(
        entries: &HashMap<String, ModuleServiceOverride>,
        profiles: &HashMap<String, ModuleServiceProfile>,
    ) -> Result<Self, ConfigError> {
        let mut env_map: HashMap<String, Vec<ModuleEnvVar>> = HashMap::new();
        let mut security_map: HashMap<String, ServiceSecurityOverride> = HashMap::new();
        let mut profile_map: HashMap<String, ModuleServiceProfileResolved> = HashMap::new();

        for (service_id_raw, cfg) in entries {
            let service_id = service_id_raw.trim();
            if service_id.is_empty() {
                return Err(ConfigError::Invalid(
                    "modules.services keys must not be empty",
                ));
            }
            let mut vars = Vec::new();
            for (key, value) in &cfg.env {
                vars.push(ModuleEnvVar::new(service_id, key, value, false)?);
            }
            for (key, value) in &cfg.secrets {
                vars.push(ModuleEnvVar::new(service_id, key, value, true)?);
            }
            if !vars.is_empty() {
                env_map.insert(service_id.to_string(), vars);
            }

            if let Some(override_policy) =
                ServiceSecurityOverride::from_policy(service_id, &cfg.policy)?
            {
                security_map.insert(service_id.to_string(), override_policy);
            }
        }

        for (profile_name_raw, profile) in profiles {
            let profile_name = profile_name_raw.trim();
            if profile_name.is_empty() {
                return Err(ConfigError::Invalid(
                    "modules.service_profiles keys must not be empty",
                ));
            }
            let resolved = ModuleServiceProfileResolved::from_profile(profile_name, profile)?;
            profile_map.insert(profile_name.to_string(), resolved);
        }

        Ok(Self {
            env: env_map,
            security: security_map,
            profiles: profile_map,
        })
    }

    pub fn env_for(&self, service_id: &str) -> Vec<ModuleEnvVar> {
        let mut collected = Vec::new();
        for candidate in service_candidates(service_id) {
            if let Some(vars) = self.env.get(candidate) {
                collected.extend(vars.iter().cloned());
            }
        }
        collected
    }

    pub fn security_override(&self, service_id: &str) -> Option<ServiceSecurityOverride> {
        for candidate in service_candidates(service_id) {
            if let Some(policy) = self.security.get(candidate) {
                return Some(policy.clone());
            }
        }
        None
    }

    pub fn profile(&self, name: &str) -> Option<ModuleServiceProfileResolved> {
        self.profiles.get(name).cloned()
    }
}

fn service_candidates(service_id: &str) -> Vec<&str> {
    let mut parts = vec![service_id];
    if let Some((module_root, _)) = service_id.split_once("::") {
        parts.push(module_root);
    }
    parts
}

#[derive(Clone, Debug)]
pub struct ModuleEnvVar {
    key: String,
    source: ModuleEnvSource,
    secret: bool,
}

impl ModuleEnvVar {
    fn new(service_id: &str, key: &str, value: &str, secret: bool) -> Result<Self, ConfigError> {
        let name = key.trim();
        if name.is_empty() {
            return Err(ConfigError::InvalidMessage(format!(
                "modules.services.{service_id} env keys must not be empty"
            )));
        }
        if name.contains(char::is_whitespace) {
            return Err(ConfigError::InvalidMessage(format!(
                "modules.services.{service_id} env key '{name}' must not contain whitespace"
            )));
        }
        let resolved = if let Some(env_ref) = value.trim().strip_prefix("env:") {
            let var = env_ref.trim();
            if var.is_empty() {
                return Err(ConfigError::InvalidMessage(format!(
                    "modules.services.{service_id}.env.{name} references an empty environment variable"
                )));
            }
            ModuleEnvSource::Env(var.to_string())
        } else if secret {
            return Err(ConfigError::InvalidMessage(format!(
                "modules.services.{service_id}.secrets.{name} must reference env:<VAR>"
            )));
        } else {
            ModuleEnvSource::Literal(value.to_string())
        };
        Ok(Self {
            key: name.to_string(),
            source: resolved,
            secret,
        })
    }

    pub fn resolve_for(
        &self,
        service_id: &str,
    ) -> Result<(String, String), ModuleEnvResolutionError> {
        match &self.source {
            ModuleEnvSource::Literal(value) => Ok((self.key.clone(), value.clone())),
            ModuleEnvSource::Env(var) => resolve_secret_value(service_id, &self.key, var)
                .map(|value| (self.key.clone(), value)),
        }
    }

    pub fn is_secret(&self) -> bool {
        self.secret
    }
}

#[derive(Clone, Debug)]
enum ModuleEnvSource {
    Literal(String),
    Env(String),
}

#[derive(Debug)]
pub enum ModuleEnvResolutionError {
    Missing {
        service_id: String,
        env_var: String,
        key: String,
    },
    Empty {
        service_id: String,
        env_var: String,
        key: String,
    },
    Conflict {
        service_id: String,
        env_var: String,
        file_var: String,
        key: String,
    },
    FileRead {
        service_id: String,
        env_var: String,
        file_var: String,
        key: String,
        path: String,
        reason: String,
    },
}

impl ModuleEnvResolutionError {
    pub fn to_message(&self) -> String {
        match self {
            ModuleEnvResolutionError::Missing {
                service_id,
                env_var,
                key,
            } => format!(
                "service {service_id} requires env {key} but source variable {env_var} is not set"
            ),
            ModuleEnvResolutionError::Empty {
                service_id,
                env_var,
                key,
            } => format!(
                "service {service_id} resolved env {key} from {env_var}, but the value is empty"
            ),
            ModuleEnvResolutionError::Conflict {
                service_id,
                env_var,
                file_var,
                key,
            } => format!(
                "service {service_id} resolved env {key} from both {env_var} and {file_var}; set only one source"
            ),
            ModuleEnvResolutionError::FileRead {
                service_id,
                env_var,
                file_var,
                key,
                path,
                reason,
            } => format!(
                "service {service_id} resolved env {key} from {file_var} (for {env_var}) at '{path}', but reading failed: {reason}"
            ),
        }
    }
}

fn resolve_secret_value(
    service_id: &str,
    key: &str,
    env_var: &str,
) -> Result<String, ModuleEnvResolutionError> {
    let direct = env::var(env_var).ok();
    let file_var = format!("{env_var}_FILE");
    let file_ref = env::var(&file_var).ok();

    let direct_non_empty = direct
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let file_ref_non_empty = file_ref
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    if direct_non_empty.is_some() && file_ref_non_empty.is_some() {
        return Err(ModuleEnvResolutionError::Conflict {
            service_id: service_id.to_string(),
            env_var: env_var.to_string(),
            file_var,
            key: key.to_string(),
        });
    }

    if let Some(value) = direct_non_empty {
        return Ok(value);
    }

    if let Some(path) = file_ref_non_empty {
        let secret_path = PathBuf::from(path.trim());
        let raw = std::fs::read_to_string(&secret_path).map_err(|err| {
            ModuleEnvResolutionError::FileRead {
                service_id: service_id.to_string(),
                env_var: env_var.to_string(),
                file_var: file_var.clone(),
                key: key.to_string(),
                path: secret_path.display().to_string(),
                reason: err.to_string(),
            }
        })?;
        let value = raw.trim();
        if value.is_empty() {
            return Err(ModuleEnvResolutionError::Empty {
                service_id: service_id.to_string(),
                env_var: file_var,
                key: key.to_string(),
            });
        }
        return Ok(value.to_string());
    }

    if matches!(direct, Some(value) if value.trim().is_empty()) {
        return Err(ModuleEnvResolutionError::Empty {
            service_id: service_id.to_string(),
            env_var: env_var.to_string(),
            key: key.to_string(),
        });
    }
    if matches!(file_ref, Some(value) if value.trim().is_empty()) {
        return Err(ModuleEnvResolutionError::Empty {
            service_id: service_id.to_string(),
            env_var: file_var,
            key: key.to_string(),
        });
    }

    Err(ModuleEnvResolutionError::Missing {
        service_id: service_id.to_string(),
        env_var: env_var.to_string(),
        key: key.to_string(),
    })
}

#[derive(Clone, Debug, Default)]
pub struct ServiceSecurityOverride {
    pub internal_only: Option<bool>,
    pub allowed_roles: Option<Vec<ServiceRole>>,
    pub required_scopes: Option<Vec<ServiceScope>>,
    pub tenant_guard: Option<ServiceTenantGuard>,
}

impl ServiceSecurityOverride {
    fn from_policy(
        service_id: &str,
        policy: &crate::config::ModuleServicePolicyOverride,
    ) -> Result<Option<Self>, ConfigError> {
        let mut override_policy = ServiceSecurityOverride::default();
        let mut has_override = false;

        if let Some(value) = policy.internal_only {
            override_policy.internal_only = Some(value);
            has_override = true;
        }

        if !policy.allowed_roles.is_empty() {
            let mut roles = Vec::new();
            for role in &policy.allowed_roles {
                let parsed = ServiceRole::from_str(role).map_err(|_| {
                    ConfigError::InvalidMessage(format!(
                        "modules.services.{service_id}.policy.allowed_roles contains unknown role '{role}'"
                    ))
                })?;
                roles.push(parsed);
            }
            override_policy.allowed_roles = Some(roles);
            has_override = true;
        }

        if !policy.required_scopes.is_empty() {
            let mut scopes = Vec::new();
            for scope in &policy.required_scopes {
                let parsed = ServiceScope::new(scope).map_err(|err| {
                    ConfigError::InvalidMessage(format!(
                        "modules.services.{service_id}.policy.required_scopes '{scope}' invalid: {err}"
                    ))
                })?;
                scopes.push(parsed);
            }
            override_policy.required_scopes = Some(scopes);
            has_override = true;
        }

        if let Some(tenant) = &policy.tenant {
            let guard = match tenant.mode {
                ModuleServiceTenantMode::Any => ServiceTenantGuard::any(),
                ModuleServiceTenantMode::Fixed => {
                    let value = tenant
                        .value
                        .as_ref()
                        .map(|v| v.trim().to_string())
                        .filter(|v| !v.is_empty())
                        .expect("tenant value validated");
                    ServiceTenantGuard::fixed(value)
                }
                ModuleServiceTenantMode::AllowList => {
                    ServiceTenantGuard::allow_list(tenant.allow.clone())
                }
            };
            override_policy.tenant_guard = Some(guard);
            has_override = true;
        }

        if has_override {
            Ok(Some(override_policy))
        } else {
            Ok(None)
        }
    }

    pub fn apply(&self, mut base: ServiceSecurityMetadata) -> ServiceSecurityMetadata {
        if let Some(value) = self.internal_only {
            base.internal_only = value;
        }
        if let Some(roles) = &self.allowed_roles {
            base.allowed_roles = roles.clone();
        }
        if let Some(scopes) = &self.required_scopes {
            base.required_scopes = scopes.clone();
        }
        if let Some(guard) = &self.tenant_guard {
            base.tenant = guard.clone();
        }
        base
    }
}

#[derive(Clone, Debug, Default)]
pub struct ModuleServiceProfileResolved {
    pub internal_only: Option<bool>,
    pub allowed_roles: Option<Vec<ServiceRole>>,
    pub required_scopes: Option<Vec<ServiceScope>>,
    pub tenant_guard: Option<ServiceTenantGuard>,
    pub ingress_access: Option<ServiceIngressAccess>,
    pub rate_limit: Option<ServiceRateLimit>,
}

impl ModuleServiceProfileResolved {
    fn from_profile(
        profile_name: &str,
        profile: &ModuleServiceProfile,
    ) -> Result<Self, ConfigError> {
        let mut resolved = ModuleServiceProfileResolved::default();
        if let Some(value) = profile.internal_only {
            resolved.internal_only = Some(value);
        }
        if !profile.allowed_roles.is_empty() {
            let mut roles = Vec::new();
            for role in &profile.allowed_roles {
                let parsed = ServiceRole::from_str(role).map_err(|_| {
                    ConfigError::InvalidMessage(format!(
                        "modules.service_profiles.{profile_name}.allowed_roles contains unknown role '{role}'"
                    ))
                })?;
                roles.push(parsed);
            }
            resolved.allowed_roles = Some(roles);
        }
        if !profile.required_scopes.is_empty() {
            let mut scopes = Vec::new();
            for scope in &profile.required_scopes {
                let parsed = ServiceScope::new(scope).map_err(|err| {
                    ConfigError::InvalidMessage(format!(
                        "modules.service_profiles.{profile_name}.required_scopes '{scope}' invalid: {err}"
                    ))
                })?;
                scopes.push(parsed);
            }
            resolved.required_scopes = Some(scopes);
        }
        if let Some(tenant) = &profile.tenant {
            let guard = match tenant.mode {
                ModuleServiceTenantMode::Any => ServiceTenantGuard::any(),
                ModuleServiceTenantMode::Fixed => {
                    let value = tenant
                        .value
                        .as_ref()
                        .map(|v| v.trim().to_string())
                        .filter(|v| !v.is_empty())
                        .expect("tenant value validated");
                    ServiceTenantGuard::fixed(value)
                }
                ModuleServiceTenantMode::AllowList => {
                    ServiceTenantGuard::allow_list(tenant.allow.clone())
                }
            };
            resolved.tenant_guard = Some(guard);
        }
        if let Some(access) = &profile.ingress_access {
            let value = access.trim().to_ascii_lowercase();
            let parsed = match value.as_str() {
                "public" => ServiceIngressAccess::Public,
                "internal" => ServiceIngressAccess::Internal,
                _ => {
                    return Err(ConfigError::InvalidMessage(format!(
                        "modules.service_profiles.{profile_name}.ingress_access '{access}' invalid (expected 'public' or 'internal')"
                    )))
                }
            };
            resolved.ingress_access = Some(parsed);
        }
        if profile.disable_rate_limit && profile.rate_limit_per_second.is_some() {
            return Err(ConfigError::InvalidMessage(format!(
                "modules.service_profiles.{profile_name} cannot set both disable_rate_limit and rate_limit_per_second"
            )));
        }
        if profile.disable_rate_limit {
            resolved.rate_limit = Some(ServiceRateLimit::Unlimited);
        } else if let Some(limit) = profile.rate_limit_per_second {
            if limit == 0 {
                return Err(ConfigError::InvalidMessage(format!(
                    "modules.service_profiles.{profile_name}.rate_limit_per_second must be > 0"
                )));
            }
            resolved.rate_limit = Some(ServiceRateLimit::CustomPerSecond(limit));
        }
        Ok(resolved)
    }

    pub fn apply(&self, mut descriptor: ServiceDescriptorOwned) -> ServiceDescriptorOwned {
        let mut security = descriptor
            .security
            .clone()
            .unwrap_or_else(ServiceSecurityMetadata::internal_default);
        if let Some(value) = self.internal_only {
            security.internal_only = value;
        }
        if let Some(roles) = &self.allowed_roles {
            security.allowed_roles = roles.clone();
        }
        if let Some(scopes) = &self.required_scopes {
            security.required_scopes = scopes.clone();
        }
        if let Some(guard) = &self.tenant_guard {
            security.tenant = guard.clone();
        }
        descriptor.security = Some(security);

        let mut ingress = descriptor
            .ingress
            .clone()
            .unwrap_or_else(ServiceIngressMetadata::internal);
        if let Some(access) = self.ingress_access {
            ingress.access = access;
        } else if let Some(internal_only) = self.internal_only {
            ingress.access = if internal_only {
                ServiceIngressAccess::Internal
            } else {
                ServiceIngressAccess::Public
            };
        }
        if let Some(rate_limit) = self.rate_limit {
            ingress.rate_limit = rate_limit;
        }
        descriptor.ingress = Some(ingress);
        descriptor
    }
}
