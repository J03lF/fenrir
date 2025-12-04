use std::future::Future;
use std::io::{self, Write};
use std::sync::Arc;

use tokio::runtime::{Handle, Runtime};
use tracing::{warn, Instrument};
use whoami;

use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::{CliDependencies, CommandOutput};
use crate::domain::module::{ModuleId, ModuleVersion};
use crate::domain::module::{ModuleRuntimeError, ModuleServiceError, ModuleStorageError};
use crate::services::module::{ModuleDevServices, ModuleService};

pub(super) fn run_module_future<F, Fut, T>(factory: F) -> Result<T, ModuleServiceError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ModuleServiceError>> + Send + 'static,
    T: Send + 'static,
{
    let span = tracing::Span::current();
    if Handle::try_current().is_ok() {
        return std::thread::spawn(move || {
            let _enter = span.enter();
            Runtime::new()
                .map_err(|err| {
                    ModuleServiceError::Storage(ModuleStorageError::Unavailable(format!(
                        "runtime init failed: {err}"
                    )))
                })?
                .block_on(factory().in_current_span())
        })
        .join()
        .unwrap_or_else(|err| {
            Err(ModuleServiceError::Storage(
                ModuleStorageError::Unavailable(format!("blocking thread panicked: {err:?}")),
            ))
        });
    }
    let _enter = span.enter();
    Runtime::new()
        .map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::Unavailable(format!(
                "runtime init failed: {err}"
            )))
        })?
        .block_on(factory().in_current_span())
}

pub(super) fn run_runtime_future<F, Fut, T>(factory: F) -> Result<T, ModuleRuntimeError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ModuleRuntimeError>> + Send + 'static,
    T: Send + 'static,
{
    let span = tracing::Span::current();
    if Handle::try_current().is_ok() {
        return std::thread::spawn(move || {
            let _enter = span.enter();
            Runtime::new()
                .map_err(|err| {
                    ModuleRuntimeError::InvalidState(format!("runtime init failed: {err}"))
                })?
                .block_on(factory().in_current_span())
        })
        .join()
        .unwrap_or_else(|err| {
            Err(ModuleRuntimeError::InvalidState(format!(
                "blocking thread panicked: {err:?}"
            )))
        });
    }
    let _enter = span.enter();
    Runtime::new()
        .map_err(|err| ModuleRuntimeError::InvalidState(format!("runtime init failed: {err}")))?
        .block_on(factory().in_current_span())
}

pub(super) fn run_module_call<F, Fut, T>(
    service: Arc<ModuleService>,
    factory: F,
) -> Result<T, ModuleServiceError>
where
    F: FnOnce(Arc<ModuleService>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ModuleServiceError>> + Send + 'static,
    T: Send + 'static,
{
    run_module_future(move || {
        let service = Arc::clone(&service);
        factory(service)
    })
}

pub(super) fn run_module_runtime_call<F, Fut, T>(
    service: Arc<ModuleService>,
    factory: F,
) -> Result<T, ModuleRuntimeError>
where
    F: FnOnce(Arc<ModuleService>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ModuleRuntimeError>> + Send + 'static,
    T: Send + 'static,
{
    let span = tracing::Span::current();
    if Handle::try_current().is_ok() {
        return std::thread::spawn(move || {
            let _enter = span.enter();
            Runtime::new()
                .map_err(|err| {
                    ModuleRuntimeError::InvalidState(format!("runtime init failed: {err}"))
                })?
                .block_on(
                    async move {
                        let service = Arc::clone(&service);
                        factory(service).await
                    }
                    .in_current_span(),
                )
        })
        .join()
        .unwrap_or_else(|err| {
            Err(ModuleRuntimeError::InvalidState(format!(
                "blocking thread panicked: {err:?}"
            )))
        });
    }
    let _enter = span.enter();
    Runtime::new()
        .map_err(|err| ModuleRuntimeError::InvalidState(format!("runtime init failed: {err}")))?
        .block_on(
            async move {
                let service = Arc::clone(&service);
                factory(service).await
            }
            .in_current_span(),
        )
}

pub(super) struct ModulesCommandCtx<'a> {
    deps: &'a CliDependencies,
    service: Arc<ModuleService>,
}

impl<'a> ModulesCommandCtx<'a> {
    pub(super) fn new(deps: &'a CliDependencies, service: Arc<ModuleService>) -> Self {
        Self { deps, service }
    }

    pub(super) fn deps(&self) -> &'a CliDependencies {
        self.deps
    }

    pub(super) fn service(&self) -> Arc<ModuleService> {
        Arc::clone(&self.service)
    }

    pub(super) fn deps_clone(&self) -> CliDependencies {
        self.deps.clone()
    }

    pub(super) fn output_sink(&self) -> Option<Arc<dyn CommandOutput>> {
        self.deps.output()
    }

    pub(super) fn module_call<F, Fut, T>(&self, factory: F) -> Result<T, ModuleServiceError>
    where
        F: FnOnce(Arc<ModuleService>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, ModuleServiceError>> + Send + 'static,
        T: Send + 'static,
    {
        let service = self.service();
        run_module_future(move || factory(service))
    }

    pub(super) fn runtime_call<F, Fut, T>(&self, factory: F) -> Result<T, ModuleRuntimeError>
    where
        F: FnOnce(Arc<ModuleService>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, ModuleRuntimeError>> + Send + 'static,
        T: Send + 'static,
    {
        let service = self.service();
        run_runtime_future(move || factory(service))
    }

    pub(super) fn record_audit(
        &self,
        action: &str,
        module: &ModuleId,
        version: Option<&ModuleVersion>,
        outcome: AuditOutcome,
        metadata: AuditMetadata,
    ) {
        let mut metadata = metadata.insert("transport", "cli").insert(
            "command",
            format!("modules {}", action.split("::").last().unwrap_or(action)),
        );

        if let Some(version) = version {
            metadata = metadata.insert("version", version.to_string());
        }

        let target = module.as_str().to_string();
        let event = AuditEvent::builder()
            .actor(module_cli_actor(self.deps.session_actor()))
            .action(action)
            .target(target)
            .outcome(outcome)
            .metadata(metadata)
            .build();

        match event {
            Ok(event) => {
                if let Err(err) = self.deps.services.record_audit(event) {
                    warn!(error = %err, "module audit append failed");
                }
            }
            Err(err) => warn!(error = %err, "module audit build failed"),
        }
    }
}

pub(super) fn module_cli_actor(actor: Option<&AuditActor>) -> AuditActor {
    if let Some(actor) = actor {
        return actor.clone();
    }
    AuditActor::User {
        user_id: format!("cli::{}", whoami::username()),
        role: "operator".to_string(),
    }
}

pub(super) fn record_module_audit(
    deps: &CliDependencies,
    action: &str,
    module: &ModuleId,
    version: Option<&ModuleVersion>,
    outcome: AuditOutcome,
    metadata: AuditMetadata,
) {
    let mut metadata = metadata.insert("transport", "cli").insert(
        "command",
        format!("modules {}", action.split("::").last().unwrap_or(action)),
    );

    if let Some(version) = version {
        metadata = metadata.insert("version", version.to_string());
    }

    let target = module.as_str().to_string();
    let actor = module_cli_actor(deps.session_actor());
    let event = AuditEvent::builder()
        .actor(actor)
        .action(action)
        .target(target)
        .outcome(outcome)
        .metadata(metadata)
        .build();

    match event {
        Ok(event) => {
            if let Err(err) = deps.services.record_audit(event) {
                warn!(error = %err, "module audit append failed");
            }
        }
        Err(err) => warn!(error = %err, "module audit build failed"),
    }
}

pub(super) fn dev_services_metadata(dev_services: &ModuleDevServices) -> AuditMetadata {
    let service_ids = dev_services
        .services
        .iter()
        .map(|svc| svc.service_id.clone())
        .collect::<Vec<_>>()
        .join(",");
    let mut metadata = AuditMetadata::default()
        .insert("mode", "dev_services")
        .insert("service_ids", service_ids);
    if let Some(run) = &dev_services.run {
        metadata = metadata
            .insert("dev_agent_command", run.command.clone())
            .insert("dev_agent_workdir", run.workdir.display().to_string());
    } else {
        metadata = metadata.insert("dev_agent_command", "not_configured");
    }
    metadata
}

pub(super) fn module_error_metadata(
    error_code: impl Into<String>,
    message: impl Into<String>,
) -> AuditMetadata {
    AuditMetadata::default()
        .insert("error_code", error_code)
        .insert("error", message)
}

pub(super) struct OwnedStreamedWriter {
    sink: Arc<dyn CommandOutput>,
}

impl OwnedStreamedWriter {
    pub(super) fn new(sink: Arc<dyn CommandOutput>) -> Self {
        Self { sink }
    }
}

impl Write for OwnedStreamedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sink.push(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
