use std::error::Error as StdError;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use anyhow::Error as AnyError;
use async_trait::async_trait;
use futures::future::BoxFuture;
use tokio::runtime::{Handle, Runtime};
use tokio::task;

use crate::utils::messages::services::managed::{
    errors as managed_errors, outcomes as outcome_messages,
};

#[async_trait]
pub trait ManagedService: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    async fn start(self: Arc<Self>) -> anyhow::Result<bool>;
    async fn stop(self: Arc<Self>, force: bool) -> anyhow::Result<bool>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceControlOutcome {
    Started,
    AlreadyRunning,
    Stopped,
    AlreadyStopped,
    Restarted,
}

impl ServiceControlOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceControlOutcome::Started => outcome_messages::STARTED,
            ServiceControlOutcome::AlreadyRunning => outcome_messages::ALREADY_RUNNING,
            ServiceControlOutcome::Stopped => outcome_messages::STOPPED,
            ServiceControlOutcome::AlreadyStopped => outcome_messages::ALREADY_STOPPED,
            ServiceControlOutcome::Restarted => outcome_messages::RESTARTED,
        }
    }
}

#[derive(Debug)]
pub enum ServiceControlError {
    UnknownService(String),
    NotControllable(String),
    ForceRequired(String),
    CoreLocked(String),
    OperationFailed { id: String, source: AnyError },
}

impl fmt::Display for ServiceControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceControlError::UnknownService(id) => {
                write!(f, "{}", managed_errors::unknown_service(id))
            }
            ServiceControlError::NotControllable(id) => {
                write!(f, "{}", managed_errors::not_controllable(id))
            }
            ServiceControlError::ForceRequired(id) => {
                write!(f, "{}", managed_errors::force_required(id))
            }
            ServiceControlError::CoreLocked(id) => {
                write!(f, "{}", managed_errors::core_locked(id))
            }
            ServiceControlError::OperationFailed { id, source } => {
                write!(f, "{}", managed_errors::operation_failed(id, source))
            }
        }
    }
}

impl StdError for ServiceControlError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            ServiceControlError::OperationFailed { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

pub struct ClosureManagedService {
    id: &'static str,
    start: Arc<dyn Fn() -> BoxFuture<'static, anyhow::Result<bool>> + Send + Sync>,
    stop: Arc<dyn Fn(bool) -> BoxFuture<'static, anyhow::Result<bool>> + Send + Sync>,
}

impl ClosureManagedService {
    pub fn new<FStart, FStop>(id: &'static str, start: FStart, stop: FStop) -> Arc<Self>
    where
        FStart: Fn() -> BoxFuture<'static, anyhow::Result<bool>> + Send + Sync + 'static,
        FStop: Fn(bool) -> BoxFuture<'static, anyhow::Result<bool>> + Send + Sync + 'static,
    {
        Arc::new(Self {
            id,
            start: Arc::new(start),
            stop: Arc::new(stop),
        })
    }
}

#[async_trait]
impl ManagedService for ClosureManagedService {
    fn id(&self) -> &'static str {
        self.id
    }

    async fn start(self: Arc<Self>) -> anyhow::Result<bool> {
        (self.start)().await
    }

    async fn stop(self: Arc<Self>, force: bool) -> anyhow::Result<bool> {
        (self.stop)(force).await
    }
}

pub fn block_on_managed<F, T>(future: F) -> T
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    match Handle::try_current() {
        Ok(handle) => task::block_in_place(|| handle.block_on(future)),
        Err(_) => {
            let runtime = Runtime::new().expect("tokio runtime for managed service");
            runtime.block_on(future)
        }
    }
}
