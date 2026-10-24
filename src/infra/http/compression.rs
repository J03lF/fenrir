//! Response compression at the browser edge (step 5.6): the static-site
//! runtime's files and its `/gateway` proxy (`process.rs`).
//!
//! Rules:
//! - Negotiated on `Accept-Encoding`: Brotli over gzip when the client rates
//!   both equally, nothing without the header (or with `q=0`).
//! - Only text-like types ([`is_compressible`]); never `text/event-stream`,
//!   images (except SVG), fonts, archives. Bodies under [`MIN_SIZE`] stay
//!   as they are.
//! - `Vary: Accept-Encoding` on every response whose representation depends
//!   on it, also when it went out uncompressed.
//! - BREACH: no compression for responses that can carry secrets next to
//!   attacker-controlled input: paths with a token-bearing segment
//!   ([`is_secret_bearing_path`]: `/auth/*`, API keys, invites, sessions,
//!   tokens) and any response with `Set-Cookie`.
//! - Proxied bodies are compressed as a stream ([`CompressedStream`]): each
//!   chunk is encoded as it arrives and the encoder is flushed whenever the
//!   upstream pauses, so nothing waits for the end of the body.

use std::io::Write;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Bytes;
use axum::http::header::{self, HeaderMap, HeaderValue};
use futures::Stream;

/// Smallest body worth compressing.
pub(crate) const MIN_SIZE: usize = 1024;

/// Brotli quality and window for proxied (dynamic) bodies: fast.
const STREAM_BROTLI_QUALITY: u32 = 5;
/// For static files, compressed once and cached: smallest output.
const STATIC_BROTLI_QUALITY: u32 = 11;
const BROTLI_WINDOW: u32 = 22;
const STREAM_GZIP_LEVEL: u32 = 6;
const STATIC_GZIP_LEVEL: u32 = 9;
/// Upstream bytes taken in before the encoder output is passed on, while
/// the upstream keeps delivering.
const STREAM_BATCH: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Encoding {
    Brotli,
    Gzip,
}

impl Encoding {
    pub(crate) fn header_value(self) -> HeaderValue {
        HeaderValue::from_static(self.token())
    }

    pub(crate) fn token(self) -> &'static str {
        match self {
            Encoding::Brotli => "br",
            Encoding::Gzip => "gzip",
        }
    }
}

/// The encoding to use for a request's `Accept-Encoding`, if any.
pub(crate) fn negotiate(headers: &HeaderMap) -> Option<Encoding> {
    let mut brotli: Option<f32> = None;
    let mut gzip: Option<f32> = None;
    let mut wildcard: Option<f32> = None;
    for value in headers.get_all(header::ACCEPT_ENCODING) {
        let Ok(text) = value.to_str() else {
            continue;
        };
        for item in text.split(',') {
            let mut parts = item.split(';');
            let coding = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
            let mut quality = 1.0_f32;
            for param in parts {
                if let Some(q) = param.trim().strip_prefix("q=") {
                    quality = q.trim().parse().unwrap_or(0.0);
                }
            }
            match coding.as_str() {
                "br" => brotli = Some(quality),
                "gzip" | "x-gzip" => gzip = Some(quality),
                "*" => wildcard = Some(quality),
                _ => {}
            }
        }
    }
    let brotli = brotli.or(wildcard).unwrap_or(0.0);
    let gzip = gzip.or(wildcard).unwrap_or(0.0);
    if brotli <= 0.0 && gzip <= 0.0 {
        return None;
    }
    if brotli >= gzip {
        Some(Encoding::Brotli)
    } else {
        Some(Encoding::Gzip)
    }
}

/// Text-like media types that compress well. `text/event-stream` never.
pub(crate) fn is_compressible(content_type: &str) -> bool {
    let media = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if media == "text/event-stream" {
        return false;
    }
    media.starts_with("text/")
        || matches!(
            media.as_str(),
            "application/json"
                | "application/javascript"
                | "application/x-javascript"
                | "application/xml"
                | "application/manifest+json"
                | "application/problem+json"
                | "application/wasm"
                | "image/svg+xml"
                | "image/x-icon"
                | "image/vnd.microsoft.icon"
        )
        || media.ends_with("+json")
        || media.ends_with("+xml")
}

/// Path segments of responses that can carry secrets (tokens, PINs, keys,
/// invite codes, session lists). Compared case-insensitively on the
/// percent-decoded path.
const SECRET_SEGMENTS: &[&str] = &[
    "auth",
    "api-keys",
    "api_keys",
    "apikeys",
    "invites",
    "invite",
    "invitations",
    "sessions",
    "tokens",
    "token",
    "password",
    "secrets",
];

/// True when the (gateway) path addresses a token-bearing endpoint and its
/// responses must not be compressed (BREACH).
pub(crate) fn is_secret_bearing_path(path: &str) -> bool {
    let decoded = urlencoding::decode(path)
        .map(|decoded| decoded.into_owned())
        .unwrap_or_else(|_| path.to_string());
    decoded
        .split(['/', '\\'])
        .map(|segment| segment.to_ascii_lowercase())
        .any(|segment| SECRET_SEGMENTS.contains(&segment.as_str()))
}

/// Adds `Accept-Encoding` to `Vary` unless it is there already.
pub(crate) fn add_vary(headers: &mut HeaderMap) {
    let present = headers.get_all(header::VARY).iter().any(|value| {
        value.to_str().is_ok_and(|text| {
            text.split(',').any(|item| {
                item.trim().eq_ignore_ascii_case("accept-encoding") || item.trim() == "*"
            })
        })
    });
    if !present {
        headers.append(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
}

/// Compresses a whole static file (best ratio; the result is cached).
pub(crate) fn compress_static(bytes: &[u8], encoding: Encoding) -> std::io::Result<Vec<u8>> {
    let mut encoder = Encoder::new(encoding, true);
    encoder.write(bytes)?;
    encoder.finish()
}

enum Encoder {
    Brotli(Box<brotli::CompressorWriter<Vec<u8>>>),
    Gzip(flate2::write::GzEncoder<Vec<u8>>),
}

impl Encoder {
    fn new(encoding: Encoding, best: bool) -> Self {
        match encoding {
            Encoding::Brotli => {
                let quality = if best {
                    STATIC_BROTLI_QUALITY
                } else {
                    STREAM_BROTLI_QUALITY
                };
                Encoder::Brotli(Box::new(brotli::CompressorWriter::new(
                    Vec::new(),
                    16 * 1024,
                    quality,
                    BROTLI_WINDOW,
                )))
            }
            Encoding::Gzip => {
                let level = if best {
                    STATIC_GZIP_LEVEL
                } else {
                    STREAM_GZIP_LEVEL
                };
                Encoder::Gzip(flate2::write::GzEncoder::new(
                    Vec::new(),
                    flate2::Compression::new(level),
                ))
            }
        }
    }

    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            Encoder::Brotli(writer) => writer.write_all(bytes),
            Encoder::Gzip(writer) => writer.write_all(bytes),
        }
    }

    /// Flushes what was written so far (a decodable prefix) and takes the
    /// output produced since the last call.
    fn flush_output(&mut self) -> std::io::Result<Vec<u8>> {
        match self {
            Encoder::Brotli(writer) => {
                writer.flush()?;
                Ok(std::mem::take(writer.get_mut()))
            }
            Encoder::Gzip(writer) => {
                writer.flush()?;
                Ok(std::mem::take(writer.get_mut()))
            }
        }
    }

    fn finish(self) -> std::io::Result<Vec<u8>> {
        match self {
            Encoder::Brotli(writer) => Ok(writer.into_inner()),
            Encoder::Gzip(writer) => writer.finish(),
        }
    }
}

/// A body stream, compressed on the fly.
///
/// Input is encoded as it arrives. Output goes out when the upstream pauses
/// (the encoder is flushed, so the client can decode everything received so
/// far) or after [`STREAM_BATCH`] bytes of input, and at the end. Upstream
/// errors end the stream with that error.
pub(crate) struct CompressedStream<S> {
    inner: Pin<Box<S>>,
    encoder: Option<Encoder>,
    pending_input: usize,
    done: bool,
}

impl<S> CompressedStream<S> {
    pub(crate) fn new(inner: S, encoding: Encoding) -> Self {
        Self {
            inner: Box::pin(inner),
            encoder: Some(Encoder::new(encoding, false)),
            pending_input: 0,
            done: false,
        }
    }
}

impl<S> Stream for CompressedStream<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>>,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        if this.done {
            return Poll::Ready(None);
        }
        loop {
            match this.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    let Some(encoder) = this.encoder.as_mut() else {
                        return Poll::Ready(None);
                    };
                    if let Err(err) = encoder.write(&chunk) {
                        this.done = true;
                        return Poll::Ready(Some(Err(err)));
                    }
                    this.pending_input = this.pending_input.saturating_add(chunk.len());
                    if this.pending_input >= STREAM_BATCH {
                        this.pending_input = 0;
                        match encoder.flush_output() {
                            Ok(out) if !out.is_empty() => {
                                return Poll::Ready(Some(Ok(Bytes::from(out))))
                            }
                            Ok(_) => {}
                            Err(err) => {
                                this.done = true;
                                return Poll::Ready(Some(Err(err)));
                            }
                        }
                    }
                }
                Poll::Ready(Some(Err(err))) => {
                    this.done = true;
                    return Poll::Ready(Some(Err(err)));
                }
                Poll::Ready(None) => {
                    this.done = true;
                    let Some(encoder) = this.encoder.take() else {
                        return Poll::Ready(None);
                    };
                    return match encoder.finish() {
                        Ok(out) if out.is_empty() => Poll::Ready(None),
                        Ok(out) => Poll::Ready(Some(Ok(Bytes::from(out)))),
                        Err(err) => Poll::Ready(Some(Err(err))),
                    };
                }
                Poll::Pending => {
                    // The upstream pauses: pass on what we have.
                    if this.pending_input == 0 {
                        return Poll::Pending;
                    }
                    this.pending_input = 0;
                    let Some(encoder) = this.encoder.as_mut() else {
                        return Poll::Pending;
                    };
                    return match encoder.flush_output() {
                        Ok(out) if out.is_empty() => Poll::Pending,
                        Ok(out) => Poll::Ready(Some(Ok(Bytes::from(out)))),
                        Err(err) => {
                            this.done = true;
                            Poll::Ready(Some(Err(err)))
                        }
                    };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::io::Read;

    fn accept(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_str(value).unwrap(),
        );
        headers
    }

    #[test]
    fn negotiation_prefers_brotli_and_honours_q() {
        assert_eq!(negotiate(&HeaderMap::new()), None);
        assert_eq!(
            negotiate(&accept("gzip, deflate, br, zstd")),
            Some(Encoding::Brotli)
        );
        assert_eq!(negotiate(&accept("gzip")), Some(Encoding::Gzip));
        assert_eq!(negotiate(&accept("br;q=0.5, gzip")), Some(Encoding::Gzip));
        assert_eq!(negotiate(&accept("br;q=0, gzip;q=0")), None);
        assert_eq!(negotiate(&accept("identity")), None);
        assert_eq!(negotiate(&accept("*")), Some(Encoding::Brotli));
        assert_eq!(negotiate(&accept("deflate")), None);
    }

    #[test]
    fn only_text_like_types_compress() {
        for media in [
            "application/json",
            "application/json; charset=utf-8",
            "text/html; charset=utf-8",
            "text/css",
            "application/javascript; charset=utf-8",
            "image/svg+xml",
            "application/problem+json",
        ] {
            assert!(is_compressible(media), "{media}");
        }
        for media in [
            "text/event-stream",
            "TEXT/EVENT-STREAM; charset=utf-8",
            "image/png",
            "image/jpeg",
            "font/woff2",
            "font/woff",
            "application/zip",
            "application/gzip",
            "application/octet-stream",
        ] {
            assert!(!is_compressible(media), "{media}");
        }
    }

    #[test]
    fn secret_bearing_paths() {
        for path in [
            "/gateway/services/module%3Aathene-api%3A%3Aapi-gateway/api/v1/auth/login/confirm",
            "/gateway/services/module%3Aathene-api%3A%3Aapi-gateway/auth/refresh",
            "/gateway/services/x/api/v1/AUTH/password/reset",
            "/gateway/services/x/api/v1/admin/api-keys",
            "/gateway/services/x/api/v1/workspaces/1/invites",
            "/gateway/services/x/api/v1/invite/abc",
            "/gateway/services/x/api%2Fv1%2Fauth%2Fme",
        ] {
            assert!(is_secret_bearing_path(path), "{path}");
        }
        for path in [
            "/gateway/services/x/api/v1/work/tickets",
            "/gateway/services/x/api/v1/dashboard/overview",
            "/gateway/services/x/api/v1/work/tickets/author-list",
        ] {
            assert!(!is_secret_bearing_path(path), "{path}");
        }
    }

    #[test]
    fn vary_is_added_once() {
        let mut headers = HeaderMap::new();
        add_vary(&mut headers);
        add_vary(&mut headers);
        assert_eq!(headers.get_all(header::VARY).iter().count(), 1);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::VARY,
            HeaderValue::from_static("Origin, Accept-Encoding"),
        );
        add_vary(&mut headers);
        assert_eq!(headers.get_all(header::VARY).iter().count(), 1);
    }

    fn decode(encoding: Encoding, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        match encoding {
            Encoding::Brotli => {
                brotli::Decompressor::new(bytes, 4096)
                    .read_to_end(&mut out)
                    .unwrap();
            }
            Encoding::Gzip => {
                flate2::read::GzDecoder::new(bytes)
                    .read_to_end(&mut out)
                    .unwrap();
            }
        }
        out
    }

    fn sample(len: usize) -> Vec<u8> {
        let mut body = Vec::with_capacity(len);
        let mut n = 0u64;
        while body.len() < len {
            n = n
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            body.extend_from_slice(
                format!("{{\"id\":{},\"title\":\"ticket {}\"}},", n >> 40, n % 977).as_bytes(),
            );
        }
        body.truncate(len);
        body
    }

    #[test]
    fn static_compression_round_trips() {
        let body = sample(200_000);
        for encoding in [Encoding::Brotli, Encoding::Gzip] {
            let packed = compress_static(&body, encoding).unwrap();
            assert!(packed.len() < body.len() / 2, "{encoding:?}");
            assert_eq!(decode(encoding, &packed), body);
        }
    }

    #[tokio::test]
    async fn streams_round_trip_and_flush_when_the_upstream_pauses() {
        for encoding in [Encoding::Brotli, Encoding::Gzip] {
            let body = sample(300_000);
            let (tx, rx) = futures::channel::mpsc::unbounded::<Result<Bytes, std::io::Error>>();
            let mut stream = CompressedStream::new(rx, encoding);
            // First part, then the upstream pauses: its compressed form must
            // come out and decode to exactly that part.
            tx.unbounded_send(Ok(Bytes::copy_from_slice(&body[..1000])))
                .unwrap();
            let first = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
                .await
                .expect("output while the upstream pauses")
                .unwrap()
                .unwrap();
            let mut received = first.to_vec();
            for part in body[1000..].chunks(7000) {
                tx.unbounded_send(Ok(Bytes::copy_from_slice(part))).unwrap();
            }
            drop(tx);
            while let Some(chunk) = stream.next().await {
                received.extend_from_slice(&chunk.unwrap());
            }
            assert!(received.len() < body.len() / 2);
            assert_eq!(decode(encoding, &received), body, "{encoding:?}");
            // The early part on its own already decodes to the first 1000 B.
            let mut prefix = Vec::new();
            match encoding {
                Encoding::Brotli => {
                    let mut reader = brotli::Decompressor::new(&first[..], 4096);
                    let mut buf = [0u8; 4096];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => prefix.extend_from_slice(&buf[..n]),
                        }
                    }
                }
                Encoding::Gzip => {
                    let mut reader = flate2::read::GzDecoder::new(&first[..]);
                    let mut buf = [0u8; 4096];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => prefix.extend_from_slice(&buf[..n]),
                        }
                    }
                }
            }
            assert_eq!(prefix, body[..1000].to_vec(), "{encoding:?}");
        }
    }
}
