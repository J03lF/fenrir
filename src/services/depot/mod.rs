//! Installing signed distributions from the Fenrir Depot.
//!
//! `install distribution athenos-0.13.0` resolves and verifies the signed
//! manifest (and what it includes) against the host's trusted release keys,
//! checks channel, yank, platform and requirements with the host's own
//! Fenrir version, plans per module (old → new, pins, overrides, restarts,
//! migrations), downloads and checks every bundle, and only then backs up,
//! installs with the module runtime, waits for health and records the
//! result, rolling back on failure. See `artifacts/quality/PLAN_REPOSITORY.md`
//! §6 and §10 in the development tree.

pub mod blob;
pub mod bundle;
mod error;
pub mod host;
pub mod installer;
pub mod plan;
pub mod ports;
pub mod state;
pub mod verify;
pub mod versions;

pub use error::DepotError;
pub use host::ModuleServiceHost;
pub use installer::{
    execute, prepare_distribution, prepare_module, prepare_rollback, update_state, Depot, Intent,
    Outcome, Prepared,
};
pub use ports::{DepotSource, HostModule, ModuleHost};
pub use state::{HostState, StateStore};
pub use verify::{DistributionRequest, HostPolicy};

#[cfg(test)]
#[path = "../../../tests/unit/services/depot/depot_tests.rs"]
mod tests;
