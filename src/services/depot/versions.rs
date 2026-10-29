//! Between the module manifest's `semver::Version` and the depot's
//! build-numbered [`DepotVersion`] (`X.Y.Z` or `X.Y.Z-N`).
//!
//! Every depot version is valid semver text, so the way to the manifest
//! always works. The way back fails for a semver pre-release tag
//! (`1.2.0-beta1`), which the depot does not know; [`installed`] decides
//! what that means for a module that is already on the host.

use depot_protocol::ModuleVersion as DepotVersion;

use super::error::DepotError;

/// The manifest form of a depot version (`1.2.0-3` stays `1.2.0-3`).
pub fn to_manifest(version: &DepotVersion) -> Result<semver::Version, DepotError> {
    semver::Version::parse(&version.to_string())
        .map_err(|err| DepotError::Protocol(format!("version `{version}`: {err}")))
}

/// The depot form of a manifest version, if it has one.
pub fn from_manifest(version: &semver::Version) -> Result<DepotVersion, String> {
    DepotVersion::parse(&version.to_string()).map_err(|err| err.to_string())
}

/// The depot form of an installed module's version. A module the depot
/// never touches (synchronized from local sources or a dev override) may
/// carry a pre-release tag such as `0.7.0-preview3`; it counts as its core
/// (build 0), which is all its requirement checks need. Any other module
/// with such a version cannot be planned against signed versions.
pub fn installed(
    module: &str,
    version: &semver::Version,
    local_sync: bool,
) -> Result<DepotVersion, String> {
    match from_manifest(version) {
        Ok(parsed) => Ok(parsed),
        Err(_) if local_sync => Ok(DepotVersion::new(
            version.major,
            version.minor,
            version.patch,
            0,
        )),
        Err(err) => Err(format!(
            "installed module {module} has version {version}, which the depot cannot compare ({err}); synchronize it from local sources or reinstall it from the depot"
        )),
    }
}
