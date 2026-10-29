//! `ModuleHost` on top of the real module runtime (`ModuleService`) and the
//! boot-time migration runner.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use crate::domain::module::{ModuleBundle, ModuleId, ModuleManifest, ModuleRuntimeStatus};
use crate::infra::db::migrations::{
    apply_module_migrations, discover_module_migrations, list_sql_files_in, parse_migrations_dir,
};
use crate::services::{DbShellService, ModuleService};

use super::ports::{HostModule, ModuleHost};

pub struct ModuleServiceHost {
    service: Arc<ModuleService>,
    db_shell: Arc<DbShellService>,
    /// `runtime.base_path`: where the migration runner keeps its state.
    runtime_dir: PathBuf,
    /// `modules.storage.install_dir`.
    install_dir: PathBuf,
}

impl ModuleServiceHost {
    pub fn new(
        service: Arc<ModuleService>,
        db_shell: Arc<DbShellService>,
        runtime_dir: PathBuf,
        install_dir: PathBuf,
    ) -> Self {
        Self {
            service,
            db_shell,
            runtime_dir,
            install_dir,
        }
    }
}

fn module_id(name: &str) -> Result<ModuleId, String> {
    ModuleId::new(name).map_err(|err| err.to_string())
}

fn installed_migrations(module_dir: &Path) -> Vec<String> {
    let Ok(runtime) = std::fs::read_to_string(module_dir.join(".fenrir/runtime.toml")) else {
        return Vec::new();
    };
    parse_migrations_dir(&runtime)
        .and_then(|dir| list_sql_files_in(&module_dir.join(dir)).ok())
        .unwrap_or_default()
}

#[async_trait]
impl ModuleHost for ModuleServiceHost {
    async fn modules(&self) -> Result<Vec<HostModule>, String> {
        let installed = self
            .service
            .list_installed()
            .await
            .map_err(|err| err.to_string())?;
        let dev: HashSet<String> = self
            .service
            .dev_override_modules()
            .await
            .into_iter()
            .map(|id| id.to_string())
            .collect();
        let mut modules = Vec::new();
        for module in installed {
            let Ok(id) = module.manifest.module_id() else {
                continue;
            };
            let instances = self
                .service
                .runtime_instances(&id)
                .await
                .map(|instances| instances.len())
                .unwrap_or(0);
            let running = matches!(
                self.service
                    .runtime_status(&id)
                    .await
                    .map(|info| info.status),
                Ok(ModuleRuntimeStatus::Running)
            );
            modules.push(HostModule {
                name: id.to_string(),
                version: module.manifest.version.clone(),
                sha256: module.manifest.artifact.checksum.hash.clone(),
                local_sync: module.source.is_synchronized() || dev.contains(id.as_str()),
                running,
                instances,
                migrations: installed_migrations(Path::new(&module.path)),
            });
        }
        Ok(modules)
    }

    async fn backup(&self, module: &str, archive: &Path) -> Result<Option<ModuleManifest>, String> {
        let id = module_id(module)?;
        let Some((manifest, bytes)) = self
            .service
            .package_installed_module(&id)
            .await
            .map_err(|err| err.to_string())?
        else {
            return Ok(None);
        };
        tokio::fs::write(archive, bytes)
            .await
            .map_err(|err| format!("{}: {err}", archive.display()))?;
        Ok(Some(manifest))
    }

    async fn stop(&self, module: &str) -> Result<(), String> {
        self.service.stop_for_replacement(&module_id(module)?).await;
        Ok(())
    }

    async fn activate(&self, bundle: ModuleBundle) -> Result<(), String> {
        self.service
            .activate_verified_bundle(bundle)
            .await
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    async fn remove(&self, module: &str) -> Result<(), String> {
        self.service
            .uninstall(&module_id(module)?)
            .await
            .map_err(|err| err.to_string())
    }

    async fn apply_migrations(&self, modules: &[String]) -> Result<Vec<String>, String> {
        let wanted: HashSet<&str> = modules.iter().map(String::as_str).collect();
        let sources: Vec<_> = discover_module_migrations(&self.install_dir)
            .into_iter()
            .filter(|source| wanted.contains(source.module_id.as_str()))
            .collect();
        let report = apply_module_migrations(
            self.runtime_dir.clone(),
            Arc::clone(&self.db_shell),
            self.db_shell.default_engine(),
            &sources,
        )
        .await
        .map_err(|err| format!("module migrations failed: {err:#}"))?;
        Ok(report.applied().to_vec())
    }

    async fn start(&self, modules: &[String]) -> Result<(), String> {
        // Dependencies first, within the set being started; the rest of the
        // runtime is left as it is.
        let installed = self
            .service
            .list_installed()
            .await
            .map_err(|err| err.to_string())?;
        let wanted: BTreeSet<&str> = modules.iter().map(String::as_str).collect();
        let mut order: Vec<String> = Vec::new();
        let mut placed: HashSet<String> = HashSet::new();
        while placed.len() < wanted.len() {
            let before = placed.len();
            for name in &wanted {
                if placed.contains(*name) {
                    continue;
                }
                let deps_ready = installed
                    .iter()
                    .find(|module| module.manifest.id == *name)
                    .map(|module| {
                        module
                            .manifest
                            .dependencies
                            .iter()
                            .all(|dep| !wanted.contains(dep.id()) || placed.contains(dep.id()))
                    })
                    .unwrap_or(true);
                if deps_ready {
                    placed.insert((*name).to_string());
                    order.push((*name).to_string());
                }
            }
            if placed.len() == before {
                // A cycle: start the rest in name order.
                for name in &wanted {
                    if placed.insert((*name).to_string()) {
                        order.push((*name).to_string());
                    }
                }
            }
        }
        for name in order {
            let id = module_id(&name)?;
            self.service
                .ensure_running(&id)
                .await
                .map_err(|err| format!("{name} did not start: {err}"))?;
        }
        Ok(())
    }

    async fn rolling_restart(&self, module: &str) -> Result<(), String> {
        let report = self
            .service
            .rolling_restart(&module_id(module)?)
            .await
            .map_err(|err| err.to_string())?;
        if report.health_verified {
            Ok(())
        } else {
            Err(format!("{module}: {}", report.note))
        }
    }

    async fn wait_healthy(&self, module: &str) -> Result<(), String> {
        self.service
            .wait_until_healthy(&module_id(module)?)
            .await
            .map(|_| ())
            .map_err(|err| format!("{module} is not healthy: {err}"))
    }
}
