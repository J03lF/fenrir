//! Depot commands; without the `depot` feature they only say so.

#[cfg(feature = "depot")]
mod enabled;
#[cfg(feature = "depot")]
pub(crate) use enabled::{
    depot_configured, forget_module, install_distribution, install_module, pin, rollback,
    search_distributions, search_modules, status, unpin,
};

#[cfg(not(feature = "depot"))]
mod disabled;
#[cfg(not(feature = "depot"))]
pub(crate) use disabled::{
    depot_configured, forget_module, install_distribution, install_module, pin, rollback,
    search_distributions, search_modules, status, unpin,
};
