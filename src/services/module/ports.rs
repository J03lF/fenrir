use std::collections::HashMap;
use std::fs;
use std::io::{self, ErrorKind};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::config::{ModulePortRange, ModulePortStrategy};
use crate::domain::module::{ModuleId, ModuleRuntimeError};
use crate::utils::messages::services::module::ports::logs as port_logs;

#[derive(Debug)]
pub struct ModulePortAllocator {
    strategy: ModulePortStrategy,
    range: ModulePortRange,
    state_path: PathBuf,
    assignments: Mutex<HashMap<ModuleId, u16>>,
}

impl ModulePortAllocator {
    pub fn new(strategy: ModulePortStrategy, range: ModulePortRange, state_path: PathBuf) -> Self {
        let assignments = Self::load_state(&state_path);
        Self {
            strategy,
            range,
            state_path,
            assignments: Mutex::new(assignments),
        }
    }

    pub async fn assigned_port(
        &self,
        module_id: &ModuleId,
    ) -> Result<Option<u16>, ModuleRuntimeError> {
        match self.strategy {
            ModulePortStrategy::Fixed => Ok(None),
            ModulePortStrategy::Dynamic => self.allocate_dynamic(module_id).await,
        }
    }

    pub async fn release(&self, module_id: &ModuleId) {
        if !matches!(self.strategy, ModulePortStrategy::Dynamic) {
            return;
        }

        let mut guard = self.assignments.lock().await;
        if guard.remove(module_id).is_some() {
            if let Err(err) = self.persist_locked(&guard) {
                warn!(
                    module = %module_id,
                    error = %err,
                    "{}",
                    port_logs::STATE_SAVE_FAILED
                );
            } else {
                info!(module = %module_id, "{}", port_logs::PORT_RELEASED);
            }
        }
    }

    async fn allocate_dynamic(
        &self,
        module_id: &ModuleId,
    ) -> Result<Option<u16>, ModuleRuntimeError> {
        let mut guard = self.assignments.lock().await;

        if let Some(&current) = guard.get(module_id) {
            if Self::is_port_available(current) {
                debug!(
                    module = %module_id,
                    port = current,
                    "{}",
                    port_logs::PORT_REUSED
                );
                return Ok(Some(current));
            }

            warn!(
                module = %module_id,
                port = current,
                "{}",
                port_logs::PORT_UNAVAILABLE_RECLAIM
            );
            guard.remove(module_id);
            let _ = self.persist_locked(&guard);
        }

        for candidate in self.range.min..=self.range.max {
            if guard.values().any(|assigned| assigned == &candidate) {
                continue;
            }
            if !Self::is_port_available(candidate) {
                continue;
            }
            guard.insert(module_id.clone(), candidate);
            if let Err(err) = self.persist_locked(&guard) {
                warn!(
                    module = %module_id,
                    port = candidate,
                    error = %err,
                    "{}",
                    port_logs::STATE_SAVE_FAILED
                );
            } else {
                info!(
                    module = %module_id,
                    port = candidate,
                    "{}",
                    port_logs::PORT_ASSIGNED
                );
            }
            return Ok(Some(candidate));
        }

        error!(
            module = %module_id,
            range_min = self.range.min,
            range_max = self.range.max,
            "{}",
            port_logs::PORT_RANGE_EXHAUSTED
        );

        Err(ModuleRuntimeError::NoAvailablePorts {
            range_start: self.range.min,
            range_end: self.range.max,
        })
    }

    fn is_port_available(port: u16) -> bool {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                drop(listener);
                true
            }
            Err(_) => false,
        }
    }

    fn load_state(path: &Path) -> HashMap<ModuleId, u16> {
        if !path.exists() {
            return HashMap::new();
        }
        match fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str::<HashMap<String, u16>>(&contents) {
                Ok(map) => map
                    .into_iter()
                    .filter_map(|(id, port)| {
                        ModuleId::new(id).ok().map(|module_id| (module_id, port))
                    })
                    .collect(),
                Err(err) => {
                    warn!(error = %err, path = %path.display(), "{}", port_logs::STATE_LOAD_FAILED);
                    HashMap::new()
                }
            },
            Err(err) => {
                warn!(error = %err, path = %path.display(), "{}", port_logs::STATE_LOAD_FAILED);
                HashMap::new()
            }
        }
    }

    fn persist_locked(&self, assignments: &HashMap<ModuleId, u16>) -> io::Result<()> {
        if let Some(parent) = self.state_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let serialized: HashMap<&str, u16> = assignments
            .iter()
            .map(|(id, port)| (id.as_str(), *port))
            .collect();
        let json = serde_json::to_string_pretty(&serialized)
            .map_err(|err| io::Error::new(ErrorKind::Other, err))?;
        fs::write(&self.state_path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fenrir-port-allocator-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn reuses_assigned_port_when_available() {
        let range = ModulePortRange {
            min: 45000,
            max: 45010,
        };
        let dir = temp_dir();
        let allocator =
            ModulePortAllocator::new(ModulePortStrategy::Dynamic, range, dir.join("ports.json"));
        let module = ModuleId::new("alpha").unwrap();
        let port = allocator.assigned_port(&module).await.unwrap().unwrap();
        let port_again = allocator.assigned_port(&module).await.unwrap().unwrap();
        assert_eq!(port, port_again);
    }

    #[tokio::test]
    async fn errors_when_no_ports_available() {
        let range = ModulePortRange {
            min: 47000,
            max: 47000,
        };
        let dir = temp_dir();
        let allocator =
            ModulePortAllocator::new(ModulePortStrategy::Dynamic, range, dir.join("ports.json"));
        let first = ModuleId::new("first").unwrap();
        let second = ModuleId::new("second").unwrap();
        allocator.assigned_port(&first).await.unwrap();
        let err = allocator.assigned_port(&second).await.unwrap_err();
        match err {
            ModuleRuntimeError::NoAvailablePorts {
                range_start,
                range_end,
            } => {
                assert_eq!(range_start, 47000);
                assert_eq!(range_end, 47000);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
