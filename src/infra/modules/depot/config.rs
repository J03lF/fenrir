//! Depot settings from the environment. Every variable may also be given as
//! `<VAR>_FILE` (a path to a file holding the value, trimmed), like the
//! other secrets of the host; setting both is an error.
//!
//! | variable | meaning |
//! |---|---|
//! | `FENRIR_DEPOT_URL` | `https://depot.athenos.dev:8443`; unset = no depot |
//! | `FENRIR_DEPOT_CHANNEL` | `beta`, `stable` or `prod` (default `prod`): installs this channel or above |
//! | `FENRIR_DEPOT_TRUSTED_KEYS` | release keys, `ed25519:<hex>`, comma or newline separated |
//! | `FENRIR_DEPOT_CLIENT_CERT` | client certificate (PEM) from `depot cert issue … --kind server` |
//! | `FENRIR_DEPOT_CLIENT_KEY` | its private key (PEM); use `_FILE` with a 0600 file |
//! | `FENRIR_DEPOT_CA_CERT` | CA (PEM) of the depot's TLS certificate; unset = the public roots |
//! | `FENRIR_DEPOT_PLATFORM` | override of the build platform (`linux-x86_64`, …) |

use std::str::FromStr;

use depot_protocol::{Channel, Platform, PublicKey, TrustedKeys};
use reqwest::Url;
use semver::Version;

use crate::services::depot::{DepotError, HostPolicy};

const URL: &str = "FENRIR_DEPOT_URL";
const CHANNEL: &str = "FENRIR_DEPOT_CHANNEL";
const TRUSTED_KEYS: &str = "FENRIR_DEPOT_TRUSTED_KEYS";
const CLIENT_CERT: &str = "FENRIR_DEPOT_CLIENT_CERT";
const CLIENT_KEY: &str = "FENRIR_DEPOT_CLIENT_KEY";
const CA_CERT: &str = "FENRIR_DEPOT_CA_CERT";
const PLATFORM: &str = "FENRIR_DEPOT_PLATFORM";

/// Every depot variable (each also as `<VAR>_FILE`).
pub const ENV_VARS: &[&str] = &[
    URL,
    CHANNEL,
    TRUSTED_KEYS,
    CLIENT_CERT,
    CLIENT_KEY,
    CA_CERT,
    PLATFORM,
];

#[derive(Clone)]
pub struct DepotSettings {
    pub url: Url,
    pub channel: Channel,
    pub trusted_keys: Vec<PublicKey>,
    /// Client certificate followed by its key (PEM), for mTLS.
    pub client_identity: Option<Vec<u8>>,
    pub ca_cert: Option<Vec<u8>>,
    pub platform: Platform,
}

impl std::fmt::Debug for DepotSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DepotSettings")
            .field("url", &self.url.as_str())
            .field("channel", &self.channel)
            .field("trusted_keys", &self.trusted_keys)
            .field(
                "client_identity",
                &self.client_identity.as_ref().map(|_| "<set>"),
            )
            .field("ca_cert", &self.ca_cert.as_ref().map(|_| "<set>"))
            .field("platform", &self.platform)
            .finish()
    }
}

/// `linux-x86_64`, `linux-aarch64`, `macos-aarch64`, … of this build.
pub fn default_platform() -> Platform {
    let text = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    Platform::from_str(&text).unwrap_or_else(|_| Platform::any())
}

/// `VAR`, or the content of the file named by `VAR_FILE`.
fn value(
    lookup: &impl Fn(&str) -> Option<String>,
    var: &str,
) -> Result<Option<String>, DepotError> {
    let file_var = format!("{var}_FILE");
    let direct = lookup(var)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let file = lookup(&file_var)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    match (direct, file) {
        (Some(_), Some(_)) => Err(DepotError::Config(format!(
            "set either {var} or {file_var}, not both"
        ))),
        (Some(value), None) => Ok(Some(value)),
        (None, Some(path)) => {
            let content = std::fs::read_to_string(&path).map_err(|err| {
                DepotError::Config(format!("cannot read {file_var} ({path}): {err}"))
            })?;
            let content = content.trim();
            if content.is_empty() {
                return Err(DepotError::Config(format!(
                    "the file named by {file_var} is empty"
                )));
            }
            Ok(Some(content.to_string()))
        }
        (None, None) => Ok(None),
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        None => false,
    }
}

impl DepotSettings {
    /// `None` when `FENRIR_DEPOT_URL` is not set (no depot on this host).
    pub fn from_env() -> Result<Option<Self>, DepotError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    pub fn from_lookup(
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Self>, DepotError> {
        let Some(raw_url) = value(&lookup, URL)? else {
            return Ok(None);
        };
        let mut url = Url::parse(&raw_url)
            .map_err(|err| DepotError::Config(format!("{URL} `{raw_url}`: {err}")))?;
        if url.scheme() != "https" && !(url.scheme() == "http" && is_loopback(&url)) {
            return Err(DepotError::Config(format!(
                "{URL} must be https (plain http only for loopback tests)"
            )));
        }
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }
        let channel = match value(&lookup, CHANNEL)? {
            Some(raw) => raw
                .parse::<Channel>()
                .map_err(|err| DepotError::Config(format!("{CHANNEL}: {err}")))?,
            None => Channel::Prod,
        };
        let trusted_keys = value(&lookup, TRUSTED_KEYS)?
            .unwrap_or_default()
            .split([',', '\n'])
            .map(str::trim)
            .filter(|key| !key.is_empty() && !key.starts_with('#'))
            .map(|key| {
                key.parse::<PublicKey>()
                    .map_err(|err| DepotError::Config(format!("{TRUSTED_KEYS}: `{key}`: {err}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if trusted_keys.is_empty() {
            return Err(DepotError::Config(format!(
                "{TRUSTED_KEYS} is empty: without a trusted release key nothing can be verified"
            )));
        }
        let client_identity = match (value(&lookup, CLIENT_CERT)?, value(&lookup, CLIENT_KEY)?) {
            (Some(cert), Some(key)) => {
                let mut pem = cert.into_bytes();
                pem.push(b'\n');
                pem.extend(key.into_bytes());
                pem.push(b'\n');
                Some(pem)
            }
            (None, None) => None,
            _ => {
                return Err(DepotError::Config(format!(
                    "{CLIENT_CERT} and {CLIENT_KEY} go together"
                )))
            }
        };
        let ca_cert = value(&lookup, CA_CERT)?.map(String::into_bytes);
        let platform = match value(&lookup, PLATFORM)? {
            Some(raw) => raw
                .parse::<Platform>()
                .map_err(|err| DepotError::Config(format!("{PLATFORM}: {err}")))?,
            None => default_platform(),
        };
        Ok(Some(Self {
            url,
            channel,
            trusted_keys,
            client_identity,
            ca_cert,
            platform,
        }))
    }

    /// The checks of this host, with `fenrir` = its own version.
    pub fn policy(&self, fenrir: Version) -> HostPolicy {
        HostPolicy {
            trusted: TrustedKeys::new(self.trusted_keys.clone()),
            channel: self.channel,
            platform: self.platform.clone(),
            fenrir,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const KEY: &str = "ed25519:8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c";

    fn settings(vars: &[(&str, &str)]) -> Result<Option<DepotSettings>, DepotError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        DepotSettings::from_lookup(move |name| map.get(name).cloned())
    }

    #[test]
    fn no_url_means_no_depot() {
        assert!(settings(&[]).unwrap().is_none());
    }

    #[test]
    fn a_full_configuration_reads_files_and_defaults_to_prod() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("client.key");
        std::fs::write(
            &key,
            "-----BEGIN PRIVATE KEY-----\nAA\n-----END PRIVATE KEY-----\n",
        )
        .unwrap();
        let keys = dir.path().join("keys");
        std::fs::write(&keys, format!("# release keys\n{KEY}\n")).unwrap();
        let parsed = settings(&[
            ("FENRIR_DEPOT_URL", "https://depot.athenos.dev:8443"),
            ("FENRIR_DEPOT_TRUSTED_KEYS_FILE", keys.to_str().unwrap()),
            (
                "FENRIR_DEPOT_CLIENT_CERT",
                "-----BEGIN CERTIFICATE-----\nAA\n-----END CERTIFICATE-----",
            ),
            ("FENRIR_DEPOT_CLIENT_KEY_FILE", key.to_str().unwrap()),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(parsed.url.as_str(), "https://depot.athenos.dev:8443/");
        assert_eq!(parsed.channel, Channel::Prod);
        assert_eq!(parsed.trusted_keys.len(), 1);
        let identity = String::from_utf8(parsed.client_identity.clone().unwrap()).unwrap();
        assert!(identity.contains("BEGIN CERTIFICATE") && identity.contains("BEGIN PRIVATE KEY"));
        assert!(
            !format!("{parsed:?}").contains("PRIVATE KEY"),
            "never in Debug"
        );
        assert_eq!(parsed.platform, default_platform());
    }

    #[test]
    fn mistakes_are_named() {
        let err = |vars: &[(&str, &str)]| settings(vars).unwrap_err().to_string();
        let base = [("FENRIR_DEPOT_URL", "https://depot.example:8443")];
        assert!(err(&base).contains("FENRIR_DEPOT_TRUSTED_KEYS is empty"));
        let with_key = |extra: (&'static str, &'static str)| {
            vec![base[0], ("FENRIR_DEPOT_TRUSTED_KEYS", KEY), extra]
        };
        assert!(
            err(&with_key(("FENRIR_DEPOT_CHANNEL", "nightly"))).contains("FENRIR_DEPOT_CHANNEL")
        );
        assert!(err(&with_key(("FENRIR_DEPOT_CLIENT_CERT", "pem"))).contains("go together"));
        assert!(err(&with_key((
            "FENRIR_DEPOT_CA_CERT_FILE",
            "/nonexistent/ca.pem"
        )))
        .contains("cannot read FENRIR_DEPOT_CA_CERT_FILE"));
        let both = vec![
            base[0],
            ("FENRIR_DEPOT_TRUSTED_KEYS", KEY),
            ("FENRIR_DEPOT_TRUSTED_KEYS_FILE", "/tmp/x"),
        ];
        assert!(err(&both).contains("not both"));
        let plain = [
            ("FENRIR_DEPOT_URL", "http://depot.example:8470"),
            ("FENRIR_DEPOT_TRUSTED_KEYS", KEY),
        ];
        assert!(err(&plain).contains("must be https"));
        let loopback = [
            ("FENRIR_DEPOT_URL", "http://127.0.0.1:8470"),
            ("FENRIR_DEPOT_TRUSTED_KEYS", KEY),
            ("FENRIR_DEPOT_CHANNEL", "beta"),
        ];
        assert_eq!(settings(&loopback).unwrap().unwrap().channel, Channel::Beta);
    }
}
