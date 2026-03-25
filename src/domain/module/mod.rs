mod bundle;
mod errors;
mod id;
mod install;
mod manifest;
mod ports;
mod progress;
mod runtime;
mod search;
mod serde_helpers;
mod summary;
mod version;

pub use bundle::ModuleBundle;
pub use errors::{
    ModuleError, ModuleRegistryError, ModuleResult, ModuleServiceError, ModuleStorageError,
    ModuleVerificationError,
};
pub use id::ModuleId;
pub use install::{
    DistributionTarget, InstalledModule, ModuleInstallResult, ModuleInstallSource,
    ModuleInstallStatus,
};
pub use manifest::{
    ChecksumAlgorithm, ModuleArtifactDescriptor, ModuleChecksum, ModuleDependency,
    ModuleDependencySpec, ModuleManifest, ModuleSignatureDescriptor, SignatureAlgorithm,
};
pub use ports::{ModuleRegistryPort, ModuleStoragePort, ModuleVerifierPort};
pub use progress::{ModuleProgress, ProgressCallback};
pub use runtime::{
    ModuleRuntimeError, ModuleRuntimeInfo, ModuleRuntimeInstanceInfo, ModuleRuntimeKind,
    ModuleRuntimePort, ModuleRuntimeStatus, ModuleStartConfig,
};
pub use search::ModuleSearchQuery;
pub use summary::ModuleSummary;
pub use version::ModuleVersion;
