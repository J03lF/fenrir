use crate::cli::commands::registry::CliDependencies;
use crate::security::identity::{IdentityError, IdentityProvider};
use crate::utils::messages::cli::builtins::user::identity as user_identity_messages;
use std::io::{self, Write};
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::task::block_in_place;

pub(super) fn call_identity<T, F>(identity: Arc<dyn IdentityProvider>, func: F) -> io::Result<T>
where
    T: Send + 'static,
    F: FnOnce(Arc<dyn IdentityProvider>) -> Result<T, IdentityError> + Send + 'static,
{
    if Handle::try_current().is_ok() {
        block_in_place(|| func(identity)).map_err(|err| io::Error::other(err.to_string()))
    } else {
        func(identity).map_err(|err| io::Error::other(err.to_string()))
    }
}

pub(super) fn identity_or_notify(
    deps: &CliDependencies,
    out: &mut dyn Write,
) -> io::Result<Option<Arc<dyn IdentityProvider>>> {
    match deps.services.identity() {
        Some(identity) => Ok(Some(identity)),
        None => {
            writeln!(out, "{}", user_identity_messages::SERVICE_UNAVAILABLE)?;
            Ok(None)
        }
    }
}

pub(super) fn with_identity<T, F>(
    deps: &CliDependencies,
    out: &mut dyn Write,
    func: F,
) -> io::Result<Option<T>>
where
    T: Send + 'static,
    F: FnOnce(Arc<dyn IdentityProvider>) -> Result<T, IdentityError> + Send + 'static,
{
    let Some(identity) = identity_or_notify(deps, out)? else {
        return Ok(None);
    };
    call_identity(identity, func).map(Some)
}
