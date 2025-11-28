use super::error::ConfigError;
use super::model::{
    GrpcConfig, HttpConfig, IdentityExternalTlsSection, ModuleRegistrySection, SshConfig,
};

pub(super) fn validate_http_tls(http: &HttpConfig) -> Result<(), ConfigError> {
    let tls = &http.tls;
    if tls.enabled {
        let cert = tls
            .cert_path
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or(ConfigError::Invalid(
                "server.http.tls.cert_path must be set when TLS is enabled",
            ))?;
        let key = tls
            .key_path
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or(ConfigError::Invalid(
                "server.http.tls.key_path must be set when TLS is enabled",
            ))?;
        if cert == key {
            return Err(ConfigError::Invalid(
                "server.http.tls.cert_path and key_path must differ",
            ));
        }
        if tls.cipher_suites.is_empty() {
            return Err(ConfigError::Invalid(
                "server.http.tls.cipher_suites must not be empty when TLS is enabled",
            ));
        }
        if let Some(interval) = tls.reload_interval_seconds {
            if interval == 0 {
                return Err(ConfigError::Invalid(
                    "server.http.tls.reload_interval_seconds must be > 0",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_grpc_tls(grpc: &GrpcConfig) -> Result<(), ConfigError> {
    let tls = &grpc.tls;
    if tls.enabled {
        let cert = tls
            .cert_path
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or(ConfigError::Invalid(
                "server.grpc.tls.cert_path must be set when TLS is enabled",
            ))?;
        let key = tls
            .key_path
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or(ConfigError::Invalid(
                "server.grpc.tls.key_path must be set when TLS is enabled",
            ))?;
        if cert == key {
            return Err(ConfigError::Invalid(
                "server.grpc.tls.cert_path and key_path must differ",
            ));
        }
        if tls.cipher_suites.is_empty() {
            return Err(ConfigError::Invalid(
                "server.grpc.tls.cipher_suites must not be empty when TLS is enabled",
            ));
        }
        if tls
            .cipher_suites
            .iter()
            .any(|suite| suite.trim().is_empty())
        {
            return Err(ConfigError::Invalid(
                "server.grpc.tls.cipher_suites must not contain empty entries",
            ));
        }
        if let Some(ca) = tls.client_ca_path.as_ref() {
            if ca.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "server.grpc.tls.client_ca_path must not be empty when set",
                ));
            }
        }
        if let Some(interval) = tls.reload_interval_seconds {
            if interval == 0 {
                return Err(ConfigError::Invalid(
                    "server.grpc.tls.reload_interval_seconds must be > 0",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_ssh_tls(ssh: &SshConfig) -> Result<(), ConfigError> {
    if let Some(interval) = ssh.tls.host_key_reload_seconds {
        if interval == 0 {
            return Err(ConfigError::Invalid(
                "server.ssh.tls.host_key_reload_seconds must be > 0",
            ));
        }
    }
    if ssh
        .tls
        .allowed_ciphers
        .iter()
        .any(|cipher| cipher.trim().is_empty())
    {
        return Err(ConfigError::Invalid(
            "server.ssh.tls.allowed_ciphers must not contain empty entries",
        ));
    }
    Ok(())
}

pub(super) fn validate_identity_tls(
    tls: &IdentityExternalTlsSection,
    environment: &str,
) -> Result<(), ConfigError> {
    if let Some(ca) = tls.ca_cert_path.as_ref() {
        if ca.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "security.identity.external.tls.ca_cert_path must not be empty when set",
            ));
        }
    }

    match (
        tls.client_cert_path.as_ref().map(|s| s.trim()),
        tls.client_key_path.as_ref().map(|s| s.trim()),
    ) {
        (Some(cert), Some(key)) => {
            if cert.is_empty() || key.is_empty() {
                return Err(ConfigError::Invalid(
                    "security.identity.external.tls.client_cert_path and client_key_path must not be empty",
                ));
            }
        }
        (None, None) => {}
        _ => {
            return Err(ConfigError::Invalid(
                "security.identity.external.tls.client_cert_path and client_key_path must be provided together",
            ));
        }
    }

    if matches!(environment, "prod" | "production") && tls.accept_invalid_certs {
        return Err(ConfigError::Invalid(
            "security.identity.external.tls.accept_invalid_certs must be false in production",
        ));
    }

    Ok(())
}

pub(super) fn validate_module_registry_tls(cfg: &ModuleRegistrySection) -> Result<(), ConfigError> {
    let tls = &cfg.tls;

    if let Some(ca) = tls.ca_cert_path.as_ref() {
        if ca.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "modules.registry.tls.ca_cert_path must not be empty",
            ));
        }
    }

    match (tls.client_cert_path.as_ref(), tls.client_key_path.as_ref()) {
        (Some(cert), Some(key)) => {
            if cert.trim().is_empty() || key.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "modules.registry.tls.client_cert_path and client_key_path must not be empty",
                ));
            }
        }
        (None, None) => {}
        _ => {
            return Err(ConfigError::Invalid(
                "modules.registry.tls.client_cert_path and client_key_path must be provided together",
            ));
        }
    }

    Ok(())
}
