//! Header rules shared by the host's HTTP proxies: the control-plane gateway
//! (`/gateway/services/...` on the HTTP port) and the static-site runtime's
//! `/gateway` proxy, which forwards to it.

/// Hop-by-hop headers (RFC 9110, section 7.6.1, plus the legacy `Keep-Alive`
/// and `Proxy-Connection`). They describe a single connection, so a proxy
/// must not forward them in either direction.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// The hop-by-hop headers of one message: the fixed set, every `Proxy-*`
/// header (`Proxy-Authenticate`, `Proxy-Authorization`, ...) and every
/// header that the message's own `Connection` header names.
#[derive(Debug, Default)]
pub(crate) struct HopByHop {
    listed: Vec<String>,
}

impl HopByHop {
    /// `connection_values` are the raw values of all `Connection` headers of
    /// the message, e.g. `keep-alive, x-session-hint`.
    pub(crate) fn new<'a>(connection_values: impl IntoIterator<Item = &'a [u8]>) -> Self {
        let listed = connection_values
            .into_iter()
            .filter_map(|value| std::str::from_utf8(value).ok())
            .flat_map(|value| value.split(','))
            .map(|token| token.trim().to_ascii_lowercase())
            .filter(|token| !token.is_empty())
            .collect();
        Self { listed }
    }

    /// `name` is a lower-case header name, as `HeaderName::as_str` returns it.
    pub(crate) fn contains(&self, name: &str) -> bool {
        HOP_BY_HOP.contains(&name)
            || name.starts_with("proxy-")
            || self.listed.iter().any(|listed| listed == name)
    }
}

// The client-address headers a proxy must not take from an untrusted
// client are `config::CLIENT_ADDRESS_HEADERS` (owned by `[server.edge]`,
// checked through `ClientIpPolicy::is_address_header`).

pub(crate) const FORWARDED_FOR: &str = "x-forwarded-for";

/// Marks a request from the host's own static-site edge (`/gateway` proxy of
/// the static runtime) to its control-plane gateway: that hop set
/// `X-Forwarded-For` from the real peer, so the gateway keeps it. The value
/// is a random secret of this process; the gateway compares it in constant
/// time, strips it before forwarding, and it never appears in a log.
pub(crate) const EDGE_HOP_HEADER: &str = "x-fenrir-edge-hop";

static EDGE_HOP_SECRET: once_cell::sync::Lazy<String> = once_cell::sync::Lazy::new(|| {
    use rand_core::RngCore;
    let mut bytes = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
});

pub(crate) fn edge_hop_secret() -> &'static str {
    EDGE_HOP_SECRET.as_str()
}

/// True when `value` carries this process's edge secret.
pub(crate) fn is_edge_hop(value: Option<&[u8]>) -> bool {
    use subtle::ConstantTimeEq;
    let Some(value) = value else {
        return false;
    };
    let expected = edge_hop_secret().as_bytes();
    value.len() == expected.len() && bool::from(value.ct_eq(expected))
}

/// The `X-Forwarded-For` to send upstream.
///
/// - `trusted` (the request came from the host's own edge or from a module
///   with a valid service token): the existing chain plus the TCP peer as
///   its last entry.
/// - otherwise the caller is the client itself: only its TCP peer, whatever
///   it claimed.
///
/// `None` drops the header (untrusted caller without a known peer).
pub(crate) fn forwarded_for<'a>(
    existing: impl IntoIterator<Item = &'a [u8]>,
    peer: Option<std::net::IpAddr>,
    trusted: bool,
) -> Option<String> {
    let peer = peer.map(|ip| ip.to_canonical().to_string());
    if !trusted {
        return peer;
    }
    let mut chain: Vec<String> = existing
        .into_iter()
        .filter_map(|value| std::str::from_utf8(value).ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|hop| !hop.is_empty())
        .map(str::to_string)
        .collect();
    if let Some(peer) = peer {
        chain.push(peer);
    }
    if chain.is_empty() {
        None
    } else {
        Some(chain.join(", "))
    }
}

/// The `X-Forwarded-For` the control-plane gateway sends to a module.
///
/// A trusted hop (the host's own edge, a module with a service token) keeps
/// its chain and gets its peer appended. For anyone else the chain is the
/// client alone: the TCP peer, or the `[server.edge]` header when the peer
/// is a trusted proxy (W9A-01).
pub(crate) fn gateway_forwarded_for(
    headers: &axum::http::HeaderMap,
    peer: Option<std::net::IpAddr>,
    trusted_hop: bool,
    policy: &crate::config::ClientIpPolicy,
) -> Option<String> {
    use axum::http::HeaderValue;
    let hop = if trusted_hop {
        peer
    } else {
        policy.resolve(
            peer,
            policy
                .header()
                .into_iter()
                .flat_map(|name| headers.get_all(name).iter())
                .map(HeaderValue::as_bytes),
        )
    };
    forwarded_for(
        headers
            .get_all(FORWARDED_FOR)
            .iter()
            .map(HeaderValue::as_bytes),
        hop,
        trusted_hop,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_fixed_proxy_and_connection_listed_headers() {
        let hop = HopByHop::new([
            b"keep-alive, X-Session-Hint".as_slice(),
            b" close ".as_slice(),
        ]);
        for name in [
            "connection",
            "keep-alive",
            "proxy-connection",
            "te",
            "trailer",
            "transfer-encoding",
            "upgrade",
            "proxy-authenticate",
            "proxy-authorization",
            "x-session-hint",
            "close",
        ] {
            assert!(hop.contains(name), "{name}");
        }
        for name in [
            "set-cookie",
            "content-type",
            "content-length",
            "cache-control",
            "x-request-id",
            "trailers",
            "authorization",
        ] {
            assert!(!hop.contains(name), "{name}");
        }
    }

    #[test]
    fn forwarded_for_trusts_only_trusted_hops() {
        let peer = Some("127.0.0.1".parse().unwrap());
        let spoofed: Vec<&[u8]> = vec![b"1.2.3.4"];
        // An untrusted caller gets its peer, never its claim.
        assert_eq!(
            forwarded_for(spoofed.clone(), peer, false),
            Some("127.0.0.1".to_string())
        );
        assert_eq!(forwarded_for(spoofed.clone(), None, false), None);
        // A trusted hop keeps the chain and appends the peer.
        let chain: Vec<&[u8]> = vec![b"10.0.0.7", b"10.0.0.8, 10.0.0.9"];
        assert_eq!(
            forwarded_for(chain, peer, true),
            Some("10.0.0.7, 10.0.0.8, 10.0.0.9, 127.0.0.1".to_string())
        );
        assert_eq!(
            forwarded_for(
                Vec::<&[u8]>::new(),
                Some("::ffff:10.1.2.3".parse().unwrap()),
                true
            ),
            Some("10.1.2.3".to_string())
        );
    }

    #[test]
    fn the_gateway_resolves_external_clients_through_the_edge_policy() {
        use crate::config::{ClientIpPolicy, EdgeSection};
        use axum::http::{HeaderMap, HeaderValue};
        let policy = EdgeSection {
            client_ip_header: Some("x-real-ip".into()),
            trusted_proxies: vec!["127.0.0.1/32".into(), "::1/128".into()],
        }
        .policy()
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.7"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("1.2.3.4"));
        let loopback = Some("127.0.0.1".parse().unwrap());
        // External caller behind the trusted proxy: the header wins, the
        // spoofed chain is gone.
        assert_eq!(
            gateway_forwarded_for(&headers, loopback, false, &policy),
            Some("203.0.113.7".to_string())
        );
        // Untrusted peer: its own address.
        let stranger = Some("192.0.2.1".parse().unwrap());
        assert_eq!(
            gateway_forwarded_for(&headers, stranger, false, &policy),
            Some("192.0.2.1".to_string())
        );
        // Trusted hop: chain plus peer, the edge header plays no role.
        assert_eq!(
            gateway_forwarded_for(&headers, loopback, true, &policy),
            Some("1.2.3.4, 127.0.0.1".to_string())
        );
        // Feature off: the peer, as before.
        assert_eq!(
            gateway_forwarded_for(&headers, loopback, false, &ClientIpPolicy::default()),
            Some("127.0.0.1".to_string())
        );
        // IPv6 proxy and client.
        let mut v6 = HeaderMap::new();
        v6.insert("x-real-ip", HeaderValue::from_static("2001:db8::9"));
        assert_eq!(
            gateway_forwarded_for(&v6, Some("::1".parse().unwrap()), false, &policy),
            Some("2001:db8::9".to_string())
        );
    }

    #[test]
    fn edge_hop_secret_is_checked_exactly() {
        assert!(is_edge_hop(Some(edge_hop_secret().as_bytes())));
        assert!(!is_edge_hop(None));
        assert!(!is_edge_hop(Some(b"guess")));
        let mut wrong = edge_hop_secret().as_bytes().to_vec();
        wrong[0] ^= 1;
        assert!(!is_edge_hop(Some(&wrong)));
    }
}
