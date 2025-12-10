use std::sync::Arc;

use tracing::{info, warn};

use crate::config::AppConfig;
use crate::domain::module::{ModuleId, ModuleVersion};
use crate::services::ModuleService;
use crate::utils::messages::boot::bootstrap as bootstrap_messages;

pub async fn bootstrap_modules(config: Arc<AppConfig>, service: Arc<ModuleService>) {
    for entry in &config.modules.bootstrap {
        let module_spec = entry.trim();
        if module_spec.is_empty() {
            continue;
        }
        let (module_id, version) = match parse_bootstrap_spec(module_spec) {
            Ok(spec) => spec,
            Err(err) => {
                warn!(
                    module = %module_spec,
                    error = %err,
                    "{}",
                    bootstrap_messages::INVALID_SPEC
                );
                continue;
            }
        };

        match service.install(&module_id, version.as_ref()).await {
            Ok(result) => {
                info!(
                    module = %module_id,
                    version = version
                        .as_ref()
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "latest".to_string()),
                    status = ?result.status,
                    "{}",
                    bootstrap_messages::MODULE_READY
                );
            }
            Err(err) => {
                warn!(
                    module = %module_id,
                    error = %err,
                    "{}",
                    bootstrap_messages::INSTALL_FAILED
                );
            }
        }
    }
}

fn parse_bootstrap_spec(spec: &str) -> Result<(ModuleId, Option<ModuleVersion>), String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err(bootstrap_messages::EMPTY_SPEC.to_string());
    }

    let (id_part, version_part) = match trimmed.split_once('@') {
        Some((_id, version)) if version.trim().is_empty() => {
            return Err(bootstrap_messages::EMPTY_VERSION_SEGMENT.to_string());
        }
        Some((id, version)) => (id, Some(version.trim())),
        None => (trimmed, None),
    };

    let module_id = ModuleId::new(id_part.trim()).map_err(|err| err.to_string())?;
    let version = if let Some(version_raw) = version_part {
        Some(ModuleVersion::parse(version_raw).map_err(|err| err.to_string())?)
    } else {
        None
    };

    Ok((module_id, version))
}

#[cfg(test)]
#[path = "../../tests/unit/boot/bootstrap_tests.rs"]
mod tests;
