use std::fs;
use std::path::Path;
use std::sync::Arc;

use reqwest::{Certificate, Identity, Url};

use super::authority::IdentityAuthority;
use super::errors::IdentityError;
use super::external::{ExternalIdentityProvider, ExternalIdentityTlsOptions};
use super::store::IdentityUserRecord;
use super::types::{IdentityClaims, IdentityUserProfile, IssueTokenRequest, IssuedToken};
use crate::config::{AppConfig, IdentityExternalTlsResolved, IdentityProviderKind};
use crate::security::manager::AuditSink;
use crate::services::db_shell::DbShellService;
use crate::utils::messages::security::identity as identity_messages;

pub trait IdentityProvider: Send + Sync {
    fn issue_token(&self, request: IssueTokenRequest) -> Result<IssuedToken, IdentityError>;
    fn list_users(&self) -> Result<Vec<IdentityUserRecord>, IdentityError>;
    fn verify(&self, token: &str) -> Result<IdentityClaims, IdentityError>;
    fn authenticate_user(
        &self,
        user_id: &str,
        password: &str,
    ) -> Result<IdentityUserProfile, IdentityError>;

    /// Set password for a user (for embedded provider first-time setup)
    /// Returns Err for external providers that don't support local password management
    fn set_user_password(
        &self,
        user_id: &str,
        password_hash: &str,
        role: crate::security::auth::Role,
    ) -> Result<(), IdentityError>;

    /// Check if user has a password set (for first-time setup detection)
    fn is_password_set(&self, user_id: &str) -> Result<bool, IdentityError>;

    /// Take pending password from setup script (if any)
    /// Returns Some(password) and deletes the pending file, None if no pending password
    fn take_pending_password(&self) -> Option<String>;
}

/// Build identity provider (file storage only - for backward compatibility)
pub fn build_identity_provider(
    cfg: &AppConfig,
    runtime_dir: &Path,
    audit: Arc<dyn AuditSink>,
) -> Result<Arc<dyn IdentityProvider>, IdentityError> {
    build_identity_provider_with_db(cfg, runtime_dir, None, audit)
}

/// Build identity provider with optional database support
/// Uses storage backend from config: `security.identity.embedded.storage`
pub fn build_identity_provider_with_db(
    cfg: &AppConfig,
    runtime_dir: &Path,
    db_shell: Option<Arc<DbShellService>>,
    audit: Arc<dyn AuditSink>,
) -> Result<Arc<dyn IdentityProvider>, IdentityError> {
    match cfg.security.identity.provider {
        IdentityProviderKind::Embedded => {
            let provider = IdentityAuthority::bootstrap_with_storage(
                cfg,
                runtime_dir,
                db_shell,
                Arc::clone(&audit),
            )?;
            Ok(Arc::new(provider))
        }
        IdentityProviderKind::External => {
            let identity_cfg = &cfg.security.identity;
            let base_url = parse_url(
                identity_cfg
                    .external
                    .base_url
                    .as_deref()
                    .expect("validated base_url must be present"),
            )?;
            let jwks_url = if let Some(url) = &identity_cfg.external.jwks_url {
                parse_url(url)?
            } else {
                base_url.join("jwks.json").map_err(|err| {
                    IdentityError::Invalid(identity_messages::derive_jwks_url_failed(
                        &err.to_string(),
                    ))
                })?
            };
            let auth_token = identity_cfg
                .resolve_external_auth_token()
                .map_err(|err| IdentityError::Invalid(format!("{err}")))?;
            let tls_resolved = identity_cfg
                .resolve_external_tls()
                .map_err(|err| IdentityError::Invalid(format!("{err}")))?;
            let tls_options = build_identity_tls_options(&tls_resolved)?;
            let provider = ExternalIdentityProvider::new(
                base_url,
                jwks_url,
                auth_token,
                identity_cfg.environment.clone(),
                identity_cfg.instance_id.clone(),
                identity_cfg.external.audience.clone(),
                cfg.app.version.clone(),
                identity_cfg.external.jwks_refresh_seconds,
                tls_options,
                audit,
            )?;
            Ok(Arc::new(provider))
        }
    }
}

fn build_identity_tls_options(
    tls: &IdentityExternalTlsResolved,
) -> Result<ExternalIdentityTlsOptions, IdentityError> {
    let mut options = ExternalIdentityTlsOptions::default();

    if let Some(ca_path) = &tls.ca_cert_path {
        let pem = fs::read(ca_path).map_err(|err| {
            IdentityError::Invalid(identity_messages::read_tls_ca_failed(ca_path, &err))
        })?;
        let cert = Certificate::from_pem(&pem).map_err(|err| {
            IdentityError::Invalid(identity_messages::tls_ca_invalid_pem(&err.to_string()))
        })?;
        options.set_ca_certificate(cert);
    }

    match (&tls.client_cert_path, &tls.client_key_path) {
        (Some(cert_path), Some(key_path)) => {
            let cert_pem = fs::read(cert_path).map_err(|err| {
                IdentityError::Invalid(identity_messages::read_client_cert_failed(cert_path, &err))
            })?;
            let key_pem = fs::read(key_path).map_err(|err| {
                IdentityError::Invalid(identity_messages::read_client_key_failed(key_path, &err))
            })?;
            let mut identity_pem = Vec::with_capacity(cert_pem.len() + key_pem.len() + 1);
            identity_pem.extend_from_slice(&cert_pem);
            if !identity_pem.ends_with(b"\n") {
                identity_pem.push(b'\n');
            }
            identity_pem.extend_from_slice(&key_pem);
            let identity = Identity::from_pem(&identity_pem).map_err(|err| {
                IdentityError::Invalid(identity_messages::client_identity_invalid_pem(
                    &err.to_string(),
                ))
            })?;
            options.set_client_identity(identity);
        }
        (None, None) => {}
        _ => {
            return Err(IdentityError::Invalid(
                identity_messages::identity_client_tls_incomplete().into(),
            ))
        }
    }

    if tls.accept_invalid_certs {
        options.set_accept_invalid_certs(true);
    }

    Ok(options)
}

fn parse_url(value: &str) -> Result<Url, IdentityError> {
    Url::parse(value).map_err(|err| {
        IdentityError::Invalid(identity_messages::invalid_identity_url(
            value,
            &err.to_string(),
        ))
    })
}
