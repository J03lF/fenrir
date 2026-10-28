mod edge;
mod error;
mod loading;
mod mode;
mod model;
mod validation;

pub use edge::{ClientIpPolicy, EdgeSection, IpCidr, CLIENT_ADDRESS_HEADERS};
pub use error::ConfigError;
pub use loading::{load, validate};
pub use mode::{production_violations, OperatingMode, ENV_OPERATING_MODE};
pub use model::*;

#[cfg(test)]
#[path = "../../tests/unit/config/config_tests.rs"]
mod tests;
