use std::collections::HashSet;
use std::io::{self, Write};

use crate::cli::commands::table::Table;
use crate::domain::module::{
    ModuleManifest, ModuleRegistryError, ModuleRuntimeError, ModuleServiceError,
    ModuleStorageError, ModuleVerificationError,
};
use crate::services::module::{DistributionAction, DistributionPlanEntry};
use crate::utils::messages::cli::builtins::modules as msg_modules;
use crate::utils::system_time_to_rfc3339;

pub(super) fn render_manifest(out: &mut dyn Write, manifest: &ModuleManifest) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        msg_modules::manifest_view::header(&manifest.id.to_string())
    )?;
    writeln!(out, "{}", msg_modules::manifest_view::UNDERLINE)?;
    writeln!(
        out,
        "{}",
        msg_modules::manifest_view::version(&manifest.version.to_string())
    )?;

    if let Some(title) = &manifest.title {
        writeln!(out, "{}", msg_modules::manifest_view::title(title))?;
    }
    if let Some(desc) = &manifest.description {
        writeln!(out, "{}", msg_modules::manifest_view::description(desc))?;
    }
    if let Some(license) = &manifest.license {
        writeln!(out, "{}", msg_modules::manifest_view::license(license))?;
    }
    if !manifest.authors.is_empty() {
        let authors = manifest.authors.join(", ");
        writeln!(out, "{}", msg_modules::manifest_view::authors(&authors))?;
    }
    if let Some(req) = &manifest.fenrir_version {
        let req_str = req.to_string();
        writeln!(
            out,
            "{}",
            msg_modules::manifest_view::fenrir_version(&req_str)
        )?;
    }
    if !manifest.tags.is_empty() {
        let tags = manifest.tags.join(",");
        writeln!(out, "{}", msg_modules::manifest_view::tags(&tags))?;
    }

    writeln!(
        out,
        "{}",
        msg_modules::manifest_view::download(&manifest.artifact.download_url)
    )?;
    writeln!(
        out,
        "{}",
        msg_modules::manifest_view::checksum(&manifest.artifact.checksum.hash)
    )?;
    writeln!(
        out,
        "{}",
        msg_modules::manifest_view::signature_key(&manifest.signature.key_id)
    )?;

    Ok(())
}

pub(super) fn render_distribution_plan(
    out: &mut dyn Write,
    plan: &[DistributionPlanEntry],
    synchronized: &HashSet<String>,
) -> io::Result<()> {
    writeln!(out, "{}", msg_modules::distribution_plan_view::TITLE)?;
    let mut table = Table::new(
        msg_modules::distribution_plan_view::HEADERS
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    );

    for entry in plan {
        let is_sync = synchronized.contains(entry.module_id.as_str())
            && matches!(entry.action, DistributionAction::Update);
        let action = if is_sync {
            msg_modules::distribution_plan_view::SKIP_LABEL
        } else {
            match entry.action {
                DistributionAction::Install => msg_modules::distribution_plan_view::INSTALL_LABEL,
                DistributionAction::Update => msg_modules::distribution_plan_view::UPDATE_LABEL,
                DistributionAction::AlreadyCurrent => {
                    msg_modules::distribution_plan_view::CURRENT_LABEL
                }
            }
        };
        let current = entry
            .current_version
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_else(|| msg_modules::distribution_plan_view::NO_CURRENT_VERSION.to_string());
        let note = if is_sync {
            msg_modules::distribution_plan_view::SKIP_SYNC_NOTE.to_string()
        } else {
            String::new()
        };
        table.add_row(vec![
            action.to_string(),
            entry.module_id.to_string(),
            current,
            entry.target_version.to_string(),
            note,
        ]);
    }

    table.render(out, "  ")?;
    writeln!(out)?;
    out.flush()
}

pub(super) fn render_service_error(
    out: &mut dyn Write,
    context: &str,
    err: &ModuleServiceError,
) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        msg_modules::error_wrappers::formatted(
            context,
            &module_error_message(err),
            module_error_code(err)
        )
    )
}

pub(super) fn module_error_code(err: &ModuleServiceError) -> &'static str {
    match err {
        ModuleServiceError::Registry(_) => "registry_error",
        ModuleServiceError::Storage(_) => "storage_error",
        ModuleServiceError::Verification(_) => "verification_error",
    }
}

fn module_error_message(err: &ModuleServiceError) -> String {
    match err {
        ModuleServiceError::Registry(inner) => match inner {
            ModuleRegistryError::Unavailable(msg) => {
                msg_modules::service_errors::registry_unavailable(msg)
            }
            ModuleRegistryError::NotFound { module } => {
                msg_modules::service_errors::registry_not_found(module)
            }
            ModuleRegistryError::Protocol(msg) => {
                msg_modules::service_errors::registry_protocol(msg)
            }
        },
        ModuleServiceError::Storage(inner) => match inner {
            ModuleStorageError::Unavailable(msg) | ModuleStorageError::Io(msg) => {
                msg_modules::service_errors::storage_unavailable(msg)
            }
            ModuleStorageError::InvalidState(msg) => {
                msg_modules::service_errors::storage_invalid_state(msg)
            }
        },
        ModuleServiceError::Verification(inner) => match inner {
            ModuleVerificationError::Signature(msg) | ModuleVerificationError::Checksum(msg) => {
                msg_modules::service_errors::verification_message(msg)
            }
            ModuleVerificationError::Unsupported => {
                msg_modules::service_errors::VERIFICATION_UNSUPPORTED.to_string()
            }
        },
    }
}

pub(super) fn render_runtime_error(
    out: &mut dyn Write,
    context: &str,
    err: &ModuleRuntimeError,
) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        msg_modules::error_wrappers::formatted(
            context,
            &runtime_error_message(err),
            runtime_error_code(err)
        )
    )
}

pub(super) fn runtime_error_code(err: &ModuleRuntimeError) -> &'static str {
    match err {
        ModuleRuntimeError::NotInstalled { .. } => "not_installed",
        ModuleRuntimeError::AlreadyRunning { .. } => "already_running",
        ModuleRuntimeError::NotRunning { .. } => "not_running",
        ModuleRuntimeError::StartFailed { .. } => "start_failed",
        ModuleRuntimeError::StopFailed { .. } => "stop_failed",
        ModuleRuntimeError::PortInUse { .. } => "port_in_use",
        ModuleRuntimeError::NoAvailablePorts { .. } => "no_available_ports",
        ModuleRuntimeError::InvalidState(_) => "invalid_state",
        ModuleRuntimeError::Io(_) => "io_error",
        ModuleRuntimeError::Quarantined { .. } => "quarantined",
        ModuleRuntimeError::EnvUnavailable { .. } => "env_unavailable",
    }
}

fn runtime_error_message(err: &ModuleRuntimeError) -> String {
    match err {
        ModuleRuntimeError::NotInstalled { module_id } => {
            msg_modules::runtime_errors::not_installed(module_id)
        }
        ModuleRuntimeError::AlreadyRunning { module_id } => {
            msg_modules::runtime_errors::already_running(module_id)
        }
        ModuleRuntimeError::NotRunning { module_id } => {
            msg_modules::runtime_errors::not_running(module_id)
        }
        ModuleRuntimeError::StartFailed { module_id, reason } => {
            msg_modules::runtime_errors::start_failed(module_id, reason)
        }
        ModuleRuntimeError::StopFailed { module_id, reason } => {
            msg_modules::runtime_errors::stop_failed(module_id, reason)
        }
        ModuleRuntimeError::PortInUse { port } => msg_modules::runtime_errors::port_in_use(*port),
        ModuleRuntimeError::NoAvailablePorts {
            range_start,
            range_end,
        } => msg_modules::runtime_errors::no_available_ports(*range_start, *range_end),
        ModuleRuntimeError::InvalidState(msg) => msg_modules::runtime_errors::invalid_state(msg),
        ModuleRuntimeError::Io(msg) => msg_modules::runtime_errors::io_error(msg),
        ModuleRuntimeError::Quarantined {
            module_id,
            resume_at,
        } => {
            let until =
                system_time_to_rfc3339(*resume_at).unwrap_or_else(|| format!("{:?}", resume_at));
            msg_modules::runtime_errors::quarantined(module_id, &until)
        }
        ModuleRuntimeError::EnvUnavailable { module_id } => {
            msg_modules::runtime_errors::env_unavailable(module_id)
        }
    }
}
