use std::fmt;

/// Why a depot operation stopped. Every variant carries a message that is
/// shown to the operator as is, so it says what to do where it can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepotError {
    /// The host's depot settings are missing or invalid.
    Config(String),
    /// No connection, TLS refused, timeout.
    Unreachable(String),
    /// The depot answered with an error body.
    Api {
        status: u16,
        code: String,
        message: String,
    },
    /// The depot answered something that is not the protocol.
    Protocol(String),
    /// The distribution, module or version does not exist (or is withheld).
    NotFound(String),
    /// A signed document did not verify (signature, unknown key, a summary
    /// that lies, an include that is not the referenced manifest).
    Verification(String),
    /// A downloaded or cached bundle is not what the signed manifest says
    /// (SHA-256, size, archive layout).
    Bundle(String),
    /// Verified, but this host must not install it (channel, yank,
    /// platform, requirements, pins).
    Refused(String),
    /// Local files: cache, backups, state.
    Io(String),
    /// Installing or starting modules failed; `rolled_back` says whether
    /// the previous modules run again.
    Install { message: String, rolled_back: bool },
}

impl DepotError {
    /// Stable code for audit metadata and the CLI.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Config(_) => "depot_config",
            Self::Unreachable(_) => "depot_unreachable",
            Self::Api { .. } => "depot_api",
            Self::Protocol(_) => "depot_protocol",
            Self::NotFound(_) => "depot_not_found",
            Self::Verification(_) => "depot_verification",
            Self::Bundle(_) => "depot_bundle",
            Self::Refused(_) => "depot_refused",
            Self::Io(_) => "depot_io",
            Self::Install { .. } => "depot_install",
        }
    }

    pub(crate) fn io(context: impl fmt::Display, err: impl fmt::Display) -> Self {
        Self::Io(format!("{context}: {err}"))
    }
}

impl fmt::Display for DepotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(message)
            | Self::Protocol(message)
            | Self::NotFound(message)
            | Self::Refused(message)
            | Self::Io(message) => f.write_str(message),
            Self::Unreachable(message) => write!(f, "cannot reach the depot: {message}"),
            Self::Api {
                status,
                code,
                message,
            } => write!(f, "the depot answered {status} ({code}): {message}"),
            Self::Verification(message) => write!(f, "verification failed: {message}"),
            Self::Bundle(message) => write!(f, "bundle rejected: {message}"),
            Self::Install {
                message,
                rolled_back,
            } => {
                if *rolled_back {
                    write!(f, "{message}; rolled back, the previous modules run again")
                } else {
                    write!(
                        f,
                        "{message}; the automatic rollback did not complete, check `list modules`"
                    )
                }
            }
        }
    }
}

impl std::error::Error for DepotError {}
