mod edge;
mod error;
mod loading;
mod model;
mod validation;

pub use edge::{ClientIpPolicy, EdgeSection, IpCidr, CLIENT_ADDRESS_HEADERS};
pub use error::ConfigError;
pub use loading::{load, validate};
pub use model::*;

#[cfg(test)]
#[path = "../../tests/unit/config/config_tests.rs"]
mod tests;
