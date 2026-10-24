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

#[cfg(test)]
mod tests {
    use super::HopByHop;

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
}
