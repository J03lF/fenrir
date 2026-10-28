//! `[server.edge]`: how the host learns the real client address when a
//! reverse proxy (Caddy, a tunnel client) sits in front of it (W9A-01).
//!
//! Without configuration every request's client is its TCP peer, as before.
//! With `client_ip_header` and `trusted_proxies` the host believes that one
//! header, and only from a peer inside `trusted_proxies`.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use serde::{Deserialize, Deserializer};

use super::error::ConfigError;

/// Client-address headers a client may send. The host never forwards any of
/// them from an external caller; the configured `client_ip_header` is
/// stripped as well.
pub const CLIENT_ADDRESS_HEADERS: &[&str] = &[
    "x-forwarded-for",
    "x-real-ip",
    "forwarded",
    "cf-connecting-ip",
    "true-client-ip",
];

#[derive(Debug, Deserialize, Clone, Default)]
pub struct EdgeSection {
    /// Header that carries the client address set by the proxy in front of
    /// the host, e.g. `x-real-ip`. Unset or empty: feature off.
    #[serde(default)]
    pub client_ip_header: Option<String>,
    /// Peers (CIDR or single address) whose `client_ip_header` is believed.
    /// A list in TOML; a comma-separated string from the environment.
    #[serde(default, deserialize_with = "string_or_list")]
    pub trusted_proxies: Vec<String>,
}

impl EdgeSection {
    /// Parses and checks the section; garbage is a startup error.
    pub fn policy(&self) -> Result<ClientIpPolicy, ConfigError> {
        let header = match self
            .client_ip_header
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(name) => {
                if !is_header_token(name) {
                    return Err(ConfigError::InvalidMessage(format!(
                        "server.edge.client_ip_header '{name}' is not a valid header name"
                    )));
                }
                Some(name.to_ascii_lowercase())
            }
            None => None,
        };
        let trusted = self
            .trusted_proxies
            .iter()
            .map(|entry| entry.trim())
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                entry.parse::<IpCidr>().map_err(|reason| {
                    ConfigError::InvalidMessage(format!(
                        "server.edge.trusted_proxies entry '{entry}' is invalid: {reason}"
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if header.is_some() && trusted.is_empty() {
            return Err(ConfigError::Invalid(
                "server.edge.client_ip_header needs at least one server.edge.trusted_proxies entry",
            ));
        }
        Ok(ClientIpPolicy { header, trusted })
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.policy().map(|_| ())
    }
}

/// Accepts `["a", "b"]` (TOML) as well as `"a,b"` (environment).
fn string_or_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Raw::deserialize(deserializer)? {
        Raw::One(value) => split_list(&value),
        Raw::Many(values) => values,
    })
}

/// Splits a comma-separated list, dropping empty entries.
pub(crate) fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// RFC 9110 `token` characters, the alphabet of a header field name.
fn is_header_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// An address block such as `127.0.0.1/32`, `10.0.0.0/8` or `::1/128`. A
/// bare address is a block of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpCidr {
    network: IpAddr,
    prefix: u8,
}

impl IpCidr {
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip.to_canonical()) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => prefix_match(
                u128::from(u32::from(net)),
                u128::from(u32::from(ip)),
                32,
                self.prefix,
            ),
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                prefix_match(u128::from(net), u128::from(ip), 128, self.prefix)
            }
            _ => false,
        }
    }
}

fn prefix_match(net: u128, ip: u128, bits: u8, prefix: u8) -> bool {
    if prefix == 0 {
        return true;
    }
    let shift = u32::from(bits - prefix);
    (net >> shift) == (ip >> shift)
}

impl FromStr for IpCidr {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let (addr, prefix) = match raw.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (raw, None),
        };
        let network = addr
            .trim()
            .parse::<IpAddr>()
            .map_err(|_| "not an IP address".to_string())?
            .to_canonical();
        let max = if network.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(value) => value
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|prefix| *prefix <= max)
                .ok_or_else(|| format!("prefix length must be 0..={max}"))?,
            None => max,
        };
        let cidr = Self { network, prefix };
        if cidr.network != cidr.masked() {
            return Err("host bits are set below the prefix length".to_string());
        }
        Ok(cidr)
    }
}

impl IpCidr {
    fn masked(&self) -> IpAddr {
        match self.network {
            IpAddr::V4(net) => {
                let bits = u32::from(net);
                let mask = u32::MAX
                    .checked_shl(u32::from(32 - self.prefix))
                    .unwrap_or(0);
                IpAddr::V4((bits & mask).into())
            }
            IpAddr::V6(net) => {
                let bits = u128::from(net);
                let mask = u128::MAX
                    .checked_shl(u32::from(128 - self.prefix))
                    .unwrap_or(0);
                IpAddr::V6((bits & mask).into())
            }
        }
    }
}

impl fmt::Display for IpCidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.network, self.prefix)
    }
}

/// The resolved `[server.edge]` settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientIpPolicy {
    /// Lower-case header name; `None` = feature off.
    header: Option<String>,
    trusted: Vec<IpCidr>,
}

impl ClientIpPolicy {
    pub fn header(&self) -> Option<&str> {
        self.header.as_deref()
    }

    pub fn trusted_proxies(&self) -> &[IpCidr] {
        &self.trusted
    }

    pub fn is_trusted_peer(&self, peer: IpAddr) -> bool {
        self.trusted.iter().any(|cidr| cidr.contains(peer))
    }

    /// True for every header a client could use to claim an address: the
    /// fixed set plus the configured one. `name` is lower case.
    pub fn is_address_header(&self, name: &str) -> bool {
        CLIENT_ADDRESS_HEADERS.contains(&name) || self.header.as_deref() == Some(name)
    }

    /// The client of a request that reached the host from outside.
    ///
    /// `header_values` are all values of the configured header. They count
    /// only when the TCP peer is a trusted proxy and there is exactly one
    /// value that is exactly one IP address; otherwise the peer is the client.
    pub fn resolve<'a>(
        &self,
        peer: Option<IpAddr>,
        header_values: impl IntoIterator<Item = &'a [u8]>,
    ) -> Option<IpAddr> {
        let peer = peer.map(|ip| ip.to_canonical());
        if self.header.is_none() {
            return peer;
        }
        let Some(proxy) = peer.filter(|ip| self.is_trusted_peer(*ip)) else {
            return peer;
        };
        let mut values = header_values.into_iter();
        let (Some(value), None) = (values.next(), values.next()) else {
            return Some(proxy);
        };
        std::str::from_utf8(value)
            .ok()
            .and_then(|value| value.trim().parse::<IpAddr>().ok())
            .map_or(Some(proxy), |client| Some(client.to_canonical()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(header: &str, trusted: &[&str]) -> ClientIpPolicy {
        EdgeSection {
            client_ip_header: Some(header.to_string()),
            trusted_proxies: trusted.iter().map(|s| (*s).to_string()).collect(),
        }
        .policy()
        .unwrap()
    }

    fn ip(raw: &str) -> IpAddr {
        raw.parse().unwrap()
    }

    #[test]
    fn cidr_parsing_and_matching() {
        let net: IpCidr = "10.0.0.0/8".parse().unwrap();
        assert!(net.contains(ip("10.200.3.4")));
        assert!(!net.contains(ip("11.0.0.1")));
        assert!(net.contains(ip("::ffff:10.1.1.1")));
        let one: IpCidr = "127.0.0.1".parse().unwrap();
        assert_eq!(one.to_string(), "127.0.0.1/32");
        assert!(one.contains(ip("127.0.0.1")));
        assert!(!one.contains(ip("127.0.0.2")));
        let v6: IpCidr = "2001:db8::/32".parse().unwrap();
        assert!(v6.contains(ip("2001:db8:1::5")));
        assert!(!v6.contains(ip("2001:db9::1")));
        assert!(!v6.contains(ip("10.0.0.1")));
        let all: IpCidr = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains(ip("203.0.113.9")));
        let lo6: IpCidr = "::1/128".parse().unwrap();
        assert!(lo6.contains(ip("::1")));
        for garbage in [
            "",
            "localhost",
            "10.0.0.0/33",
            "::/129",
            "10.0.0.1/8",
            "10.0.0.0/x",
            "1.2.3",
        ] {
            assert!(garbage.parse::<IpCidr>().is_err(), "{garbage}");
        }
    }

    #[test]
    fn section_rejects_garbage_at_startup() {
        let bad_cidr = EdgeSection {
            client_ip_header: Some("x-real-ip".into()),
            trusted_proxies: vec!["127.0.0.1/32".into(), "nonsense".into()],
        };
        let err = bad_cidr.policy().unwrap_err().to_string();
        assert!(err.contains("nonsense"), "{err}");
        let bad_header = EdgeSection {
            client_ip_header: Some("x real ip".into()),
            trusted_proxies: vec!["127.0.0.1".into()],
        };
        assert!(bad_header.policy().is_err());
        let header_without_proxies = EdgeSection {
            client_ip_header: Some("x-real-ip".into()),
            trusted_proxies: Vec::new(),
        };
        assert!(header_without_proxies.policy().is_err());
        // Unset and empty mean "off".
        assert_eq!(
            EdgeSection::default().policy().unwrap(),
            ClientIpPolicy::default()
        );
        let empty = EdgeSection {
            client_ip_header: Some("  ".into()),
            trusted_proxies: Vec::new(),
        };
        assert_eq!(empty.policy().unwrap().header(), None);
    }

    #[test]
    fn header_name_is_case_insensitive() {
        let policy = policy("X-Real-IP", &["127.0.0.1/32"]);
        assert_eq!(policy.header(), Some("x-real-ip"));
        assert!(policy.is_address_header("x-real-ip"));
    }

    #[test]
    fn trusted_peer_with_header_yields_that_ip() {
        let policy = policy("x-real-ip", &["127.0.0.1/32", "::1/128"]);
        let value: &[u8] = b" 203.0.113.7 ";
        assert_eq!(
            policy.resolve(Some(ip("127.0.0.1")), [value]),
            Some(ip("203.0.113.7"))
        );
        let v6: &[u8] = b"2001:db8::42";
        assert_eq!(
            policy.resolve(Some(ip("::1")), [v6]),
            Some(ip("2001:db8::42"))
        );
        // An IPv4-mapped peer and value are canonical.
        let mapped: &[u8] = b"::ffff:198.51.100.2";
        assert_eq!(
            policy.resolve(Some(ip("::ffff:127.0.0.1")), [mapped]),
            Some(ip("198.51.100.2"))
        );
    }

    #[test]
    fn untrusted_peer_keeps_its_own_address() {
        let policy = policy("x-real-ip", &["127.0.0.1/32"]);
        let value: &[u8] = b"203.0.113.7";
        assert_eq!(
            policy.resolve(Some(ip("192.0.2.10")), [value]),
            Some(ip("192.0.2.10"))
        );
        assert_eq!(policy.resolve(None, [value]), None);
    }

    #[test]
    fn garbage_or_multiple_values_fall_back_to_the_peer() {
        let policy = policy("x-real-ip", &["127.0.0.1/32"]);
        let peer = Some(ip("127.0.0.1"));
        for value in [
            b"".as_slice(),
            b"unknown",
            b"203.0.113.7, 198.51.100.1",
            b"203.0.113.7:443",
            b"[2001:db8::1]",
            b"\xff\xfe",
        ] {
            assert_eq!(policy.resolve(peer, [value]), peer, "{value:?}");
        }
        let a: &[u8] = b"203.0.113.7";
        let b: &[u8] = b"198.51.100.1";
        assert_eq!(policy.resolve(peer, [a, b]), peer);
        assert_eq!(policy.resolve(peer, Vec::<&[u8]>::new()), peer);
    }

    #[test]
    fn feature_off_is_todays_behaviour() {
        let policy = ClientIpPolicy::default();
        let value: &[u8] = b"203.0.113.7";
        assert_eq!(
            policy.resolve(Some(ip("127.0.0.1")), [value]),
            Some(ip("127.0.0.1"))
        );
        for name in CLIENT_ADDRESS_HEADERS {
            assert!(policy.is_address_header(name));
        }
        assert!(!policy.is_address_header("x-request-id"));
    }

    #[test]
    fn list_accepts_comma_separated_strings() {
        assert_eq!(
            split_list(" 127.0.0.1/32 ,::1/128,, "),
            vec!["127.0.0.1/32".to_string(), "::1/128".to_string()]
        );
        let section: EdgeSection = toml::from_str(
            "client_ip_header = \"x-real-ip\"\ntrusted_proxies = \"127.0.0.1/32,::1/128\"",
        )
        .unwrap();
        assert_eq!(section.trusted_proxies.len(), 2);
        let section: EdgeSection = toml::from_str("trusted_proxies = [\"127.0.0.1/32\"]").unwrap();
        assert_eq!(section.trusted_proxies, vec!["127.0.0.1/32".to_string()]);
    }
}
