//! HTTPS client for the depot API v1 (reqwest + rustls), with the host's
//! client certificate for mTLS (Caddy requires and verifies it).

use std::time::Duration;

use async_trait::async_trait;
use depot_protocol::api::{
    DistributionDetail, DistributionSummary, ErrorBody, ModuleInfo, SearchResponse,
};
use depot_protocol::{DistributionId, Name, Sha256Digest};
use futures::StreamExt;
use reqwest::{Certificate, Identity, Response, StatusCode, Url};
use serde::de::DeserializeOwned;

use crate::services::depot::blob::BlobWriter;
use crate::services::depot::{DepotError, DepotSource};

use super::config::DepotSettings;

pub struct DepotHttpClient {
    http: reqwest::Client,
    base: Url,
}

/// The whole error chain, so a TLS refusal says why.
fn chain(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(inner) = source {
        let line = inner.to_string();
        if !text.contains(&line) {
            text.push_str(": ");
            text.push_str(&line);
        }
        source = inner.source();
    }
    text
}

impl DepotHttpClient {
    pub fn new(settings: &DepotSettings) -> Result<Self, DepotError> {
        let plain = settings.url.scheme() == "http";
        let mut builder = reqwest::Client::builder()
            .use_rustls_tls()
            .https_only(!plain)
            .connect_timeout(Duration::from_secs(15))
            .user_agent(concat!(
                "fenrir/",
                env!("CARGO_PKG_VERSION"),
                " depot-client"
            ));
        if let Some(pem) = &settings.ca_cert {
            let cert = Certificate::from_pem(pem).map_err(|err| {
                DepotError::Config(format!(
                    "FENRIR_DEPOT_CA_CERT is not a PEM certificate: {err}"
                ))
            })?;
            // Only the configured CA: the depot is not a public site.
            builder = builder
                .tls_built_in_root_certs(false)
                .add_root_certificate(cert);
        }
        if let Some(pem) = &settings.client_identity {
            let identity = Identity::from_pem(pem).map_err(|err| {
                DepotError::Config(format!(
                    "FENRIR_DEPOT_CLIENT_CERT/_KEY are not a PEM certificate and key: {err}"
                ))
            })?;
            builder = builder.identity(identity);
        }
        let http = builder
            .build()
            .map_err(|err| DepotError::Config(format!("depot HTTP client: {}", chain(&err))))?;
        Ok(Self {
            http,
            base: settings.url.clone(),
        })
    }

    fn url(&self, path: &str) -> Result<Url, DepotError> {
        self.base
            .join(path)
            .map_err(|err| DepotError::Protocol(format!("bad depot path {path}: {err}")))
    }

    async fn get(&self, url: Url) -> Result<Response, DepotError> {
        self.http
            .get(url)
            .send()
            .await
            .map_err(|err| DepotError::Unreachable(chain(&err)))
    }

    async fn json<T: DeserializeOwned>(response: Response) -> Result<T, DepotError> {
        response.json().await.map_err(|err| {
            DepotError::Protocol(format!("unexpected answer from the depot: {}", chain(&err)))
        })
    }

    async fn get_json<T: DeserializeOwned>(&self, url: Url) -> Result<T, DepotError> {
        let response = self.get(url).await?;
        if !response.status().is_success() {
            return Err(api_error(response).await);
        }
        Self::json(response).await
    }

    async fn get_optional<T: DeserializeOwned>(&self, url: Url) -> Result<Option<T>, DepotError> {
        let response = self.get(url).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(api_error(response).await);
        }
        Self::json(response).await.map(Some)
    }
}

async fn api_error(response: Response) -> DepotError {
    let status = response.status().as_u16();
    match response.json::<ErrorBody>().await {
        Ok(body) => {
            let mut message = body.error.message;
            for detail in body.error.details {
                message.push_str("\n  - ");
                message.push_str(&detail);
            }
            DepotError::Api {
                status,
                code: body.error.code,
                message,
            }
        }
        Err(_) => DepotError::Api {
            status,
            code: "unknown".to_string(),
            message: "no error body".to_string(),
        },
    }
}

#[async_trait]
impl DepotSource for DepotHttpClient {
    async fn distribution(
        &self,
        id: &DistributionId,
    ) -> Result<Option<DistributionDetail>, DepotError> {
        self.get_optional(self.url(&format!("v1/distributions/{id}"))?)
            .await
    }

    async fn distributions(&self, product: &Name) -> Result<Vec<DistributionSummary>, DepotError> {
        let mut url = self.url("v1/distributions")?;
        url.query_pairs_mut()
            .append_pair("product", product.as_str());
        self.get_json(url).await
    }

    async fn module(&self, name: &Name) -> Result<Option<ModuleInfo>, DepotError> {
        self.get_optional(self.url(&format!("v1/modules/{name}"))?)
            .await
    }

    async fn search(&self, query: &str) -> Result<SearchResponse, DepotError> {
        let mut url = self.url("v1/search")?;
        url.query_pairs_mut().append_pair("q", query);
        self.get_json(url).await
    }

    async fn blob(
        &self,
        digest: &Sha256Digest,
        size: u64,
        sink: &mut BlobWriter,
    ) -> Result<(), DepotError> {
        let response = self.get(self.url(&format!("v1/blobs/{digest}"))?).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(DepotError::NotFound(format!(
                "blob {digest} is not in the depot"
            )));
        }
        if !response.status().is_success() {
            return Err(api_error(response).await);
        }
        if let Some(length) = response.content_length() {
            if length != size {
                return Err(DepotError::Bundle(format!(
                    "blob {digest}: the depot sends {length} bytes, the signed manifest says {size}"
                )));
            }
        }
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| {
                DepotError::Unreachable(format!(
                    "download of {digest} interrupted: {}",
                    chain(&err)
                ))
            })?;
            sink.write(&chunk)?;
        }
        Ok(())
    }
}
