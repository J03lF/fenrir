//! The host's connection to the Fenrir Depot: settings from `FENRIR_DEPOT_*`
//! and the mTLS HTTP client behind `services::depot::DepotSource`.

mod client;
mod config;

pub use client::DepotHttpClient;
pub use config::{default_platform, DepotSettings, ENV_VARS};
