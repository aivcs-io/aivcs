//! The data-mesh HTTP binding — an in-workspace client for the standard
//! collection offering (`/v1/collections/*`).
//!
//! This replaces the `data-mesh-client = { git = … }` dependency. data-mesh is a
//! **deployed workload on the service-mesh**, so the sanctioned way to consume it
//! is to call the endpoint, not to compile a client in from a git edge — the
//! ordering `FR_REMOVE_GIT_BUILD_DEPS_USE_AIVCS` sets is *call the endpoint >
//! local contract > registry crate > (never) git*. This module is that local
//! contract: plain `reqwest` over the mesh, no first-party build-time source.
//!
//! The wire contract is unchanged — same paths, same headers, same status
//! handling — so this is a dependency change, not a behaviour change.

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::{Client as HttpClient, Method, RequestBuilder, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// In-cluster service — the default bind for consumers inside the mesh.
pub const IN_CLUSTER_URL: &str = "http://data-mesh.data-mesh.svc.cluster.local";

/// lornu-ai does not use `*.lornu.ai` for consumer binds (production-endpoints policy).
const BANNED_CONSUMER_DOMAIN: &str = "lornu.ai";

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("API error (status {status}): {message}")]
    Api { status: StatusCode, message: String },
}

/// Result of a `create_document` call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Created {
    pub id: String,
    pub status: String,
}

/// One write inside `Client::execute_transaction` (Phase 1 forge atomic publish).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TransactionOp {
    CreateIdempotent {
        collection: String,
        idempotency_key: String,
        data: Value,
    },
    Create {
        collection: String,
        data: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransactionResult {
    pub status: String,
    pub results: Vec<TransactionOpEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransactionOpEntry {
    pub collection: String,
    pub id: String,
    pub status: String,
}

/// Connection + identity for the data-mesh offering.
///
/// The base URL and tenant identity come from the environment — never hardcoded —
/// so the deployment controls which mesh endpoint and tenant a consumer binds to.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub base_url: String,
    pub tenant_id: String,
    pub tenant_role: String,
    pub cf_client_id: Option<String>,
    pub cf_client_secret: Option<String>,
    /// Optional `Authorization: Bearer` token, for consumers whose mesh binding
    /// authenticates with a token rather than (or in addition to) CF-Access.
    pub bearer_token: Option<String>,
}

fn read_secret_file(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `*.lornu.ai` is the OSS/community org and must not be used for lornu-ai
/// consumer binds. Returns an actionable error naming the right endpoints.
fn validate_consumer_url(url: &str) -> Result<()> {
    if url.to_ascii_lowercase().contains(BANNED_CONSUMER_DOMAIN) {
        return Err(Error::Config(format!(
            "data-mesh consumers must not bind to lornu.ai URLs ({url}). \
             Use {IN_CLUSTER_URL} (in-cluster) or set DATA_MESH_URL to a reachable data-mesh."
        )));
    }
    Ok(())
}

impl ClientConfig {
    /// Build from the environment.
    ///
    /// - `DATA_MESH_URL` (or legacy `DATAMESH_URL`) — base URL; defaults to the
    ///   in-cluster service.
    /// - `DATA_MESH_TENANT_ID` — required.
    /// - `DATA_MESH_TENANT_ROLE` — defaults to `builder`.
    /// - Cloudflare Access creds from
    ///   `/var/run/secrets/data-mesh/client-{id,secret}`, else
    ///   `CF_ACCESS_CLIENT_ID` / `CF_ACCESS_CLIENT_SECRET`.
    pub fn from_env() -> Result<Self> {
        let base_url = std::env::var("DATA_MESH_URL")
            .ok()
            .or_else(|| std::env::var("DATAMESH_URL").ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| IN_CLUSTER_URL.to_string());
        validate_consumer_url(&base_url)?;

        let tenant_id = std::env::var("DATA_MESH_TENANT_ID")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                Error::Config("DATA_MESH_TENANT_ID environment variable is required".to_string())
            })?;

        let tenant_role =
            std::env::var("DATA_MESH_TENANT_ROLE").unwrap_or_else(|_| "builder".to_string());

        let cf_client_id = read_secret_file("/var/run/secrets/data-mesh/client-id")
            .or_else(|| std::env::var("CF_ACCESS_CLIENT_ID").ok());
        let cf_client_secret = read_secret_file("/var/run/secrets/data-mesh/client-secret")
            .or_else(|| std::env::var("CF_ACCESS_CLIENT_SECRET").ok());

        // Optional bearer: DATA_MESH_TOKEN (or legacy DATAMESH_API_TOKEN).
        let bearer_token = std::env::var("DATA_MESH_TOKEN")
            .ok()
            .or_else(|| std::env::var("DATAMESH_API_TOKEN").ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(Self {
            base_url,
            tenant_id,
            tenant_role,
            cf_client_id,
            cf_client_secret,
            bearer_token,
        })
    }
}

/// Client for the data-mesh standard collection offering.
#[derive(Debug, Clone)]
pub struct Client {
    config: ClientConfig,
    http: HttpClient,
}

impl Client {
    pub fn new(config: ClientConfig) -> Self {
        Self {
            config,
            http: HttpClient::new(),
        }
    }

    /// Construct from [`ClientConfig::from_env`].
    pub fn from_env() -> Result<Self> {
        Ok(Self::new(ClientConfig::from_env()?))
    }

    fn prepare_request(&self, method: Method, path: &str) -> RequestBuilder {
        let url = format!("{}{}", self.config.base_url.trim_end_matches('/'), path);
        let mut req = self
            .http
            .request(method, url)
            .header("x-tenant-id", &self.config.tenant_id)
            .header("x-tenant-role", &self.config.tenant_role);
        if let Some(ref id) = self.config.cf_client_id {
            req = req.header("CF-Access-Client-Id", id);
        }
        if let Some(ref secret) = self.config.cf_client_secret {
            req = req.header("CF-Access-Client-Secret", secret);
        }
        if let Some(ref token) = self.config.bearer_token {
            req = req.bearer_auth(token);
        }
        req
    }

    async fn handle_response<R: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<R> {
        let status = response.status();
        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            return Err(Error::Api { status, message });
        }
        Ok(response.json::<R>().await?)
    }

    /// The mesh endpoint this client is bound to.
    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    /// The tenant this client acts as.
    pub fn tenant_id(&self) -> &str {
        &self.config.tenant_id
    }

    // ── Standard collection offering ─────────────────────────────────────────

    /// Create a document in `collection`. `data` is the opaque app payload; the
    /// offering assigns and returns the id.
    pub async fn create_document(&self, collection: &str, data: &Value) -> Result<Created> {
        let path = format!("/v1/collections/{}", encode_segment(collection));
        let resp = self
            .prepare_request(Method::POST, &path)
            .json(data)
            .send()
            .await?;
        self.handle_response(resp).await
    }

    /// Fetch one document by `collection` + `id`. Returns `Ok(None)` on 404.
    pub async fn get_document(&self, collection: &str, id: &str) -> Result<Option<Value>> {
        let path = format!(
            "/v1/collections/{}/{}",
            encode_segment(collection),
            encode_segment(id)
        );
        let resp = self.prepare_request(Method::GET, &path).send().await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let doc: Value = self.handle_response(resp).await?;
        Ok(Some(doc))
    }

    /// List documents in `collection`, newest first, bounded by `limit`.
    pub async fn list_documents(&self, collection: &str, limit: Option<u32>) -> Result<Vec<Value>> {
        let mut path = format!("/v1/collections/{}", encode_segment(collection));
        if let Some(l) = limit {
            path.push_str(&format!("?limit={l}"));
        }
        let resp = self.prepare_request(Method::GET, &path).send().await?;
        let body: Value = self.handle_response(resp).await?;
        Ok(documents_of(body))
    }

    /// Create a document **idempotently** under `idempotency_key`: re-delivery of
    /// the same key for the same (tenant, collection) dedups to a single row and
    /// returns the same id (`status = "exists"`), instead of inserting a
    /// duplicate. Use this for exactly-once writes (e.g. evidence).
    pub async fn create_document_idempotent(
        &self,
        collection: &str,
        idempotency_key: &str,
        data: &Value,
    ) -> Result<Created> {
        let path = format!("/v1/collections/{}", encode_segment(collection));
        let resp = self
            .prepare_request(Method::POST, &path)
            .header("Idempotency-Key", idempotency_key)
            .json(data)
            .send()
            .await?;
        self.handle_response(resp).await
    }

    /// Query `collection` for documents whose `data.<field>` equals `value`,
    /// newest first, bounded by `limit`. This is how a consumer fetches by a
    /// domain key (e.g. `field = "envelope_id"`) rather than the server-assigned
    /// document id.
    pub async fn query_documents(
        &self,
        collection: &str,
        field: &str,
        value: &str,
        limit: Option<u32>,
    ) -> Result<Vec<Value>> {
        let mut path = format!(
            "/v1/collections/{}?field={}&value={}",
            encode_segment(collection),
            encode_segment(field),
            encode_segment(value)
        );
        if let Some(l) = limit {
            path.push_str(&format!("&limit={l}"));
        }
        let resp = self.prepare_request(Method::GET, &path).send().await?;
        let body: Value = self.handle_response(resp).await?;
        Ok(documents_of(body))
    }

    /// Execute multiple collection writes in one SurrealDB transaction (forge CAS Phase 1).
    pub async fn execute_transaction(&self, ops: &[TransactionOp]) -> Result<TransactionResult> {
        if ops.is_empty() {
            return Err(Error::Config(
                "transaction must contain at least one op".into(),
            ));
        }
        let path = "/v1/transactions";
        let resp = self
            .prepare_request(Method::POST, path)
            .json(&serde_json::json!({ "ops": ops }))
            .send()
            .await?;
        self.handle_response(resp).await
    }
}

/// The offering wraps list/query results in `{ "documents": [...] }`.
pub(crate) fn documents_of(body: Value) -> Vec<Value> {
    body.get("documents")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default()
}

/// Percent-encode a single path segment so ids/collection names with special
/// characters can't break out of the path.
pub(crate) fn encode_segment(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_lornu_ai_consumer_urls() {
        assert!(validate_consumer_url("https://data-fabric.lornu.ai").is_err());
        assert!(validate_consumer_url(IN_CLUSTER_URL).is_ok());
    }

    #[test]
    fn encode_segment_escapes_path_breakers() {
        assert_eq!(encode_segment("assessment"), "assessment");
        assert_eq!(encode_segment("a/b"), "a%2Fb");
        assert_eq!(encode_segment("id with space"), "id%20with%20space");
        assert_eq!(encode_segment("x?y#z"), "x%3Fy%23z");
    }

    #[test]
    fn from_env_requires_tenant_id() {
        // With no DATA_MESH_TENANT_ID set in this test process, from_env errors.
        // (Guard against a poisoned env by only asserting the error variant.)
        if std::env::var("DATA_MESH_TENANT_ID").is_err() {
            match ClientConfig::from_env() {
                Err(Error::Config(msg)) => assert!(msg.contains("DATA_MESH_TENANT_ID")),
                other => panic!("expected Config error, got {other:?}"),
            }
        }
    }

    #[test]
    fn documents_of_unwraps_the_envelope_and_tolerates_absence() {
        let body = serde_json::json!({ "documents": [{ "id": "a" }, { "id": "b" }] });
        assert_eq!(documents_of(body).len(), 2);
        // A response without the envelope yields an empty list rather than panicking.
        assert!(documents_of(serde_json::json!({ "unexpected": 1 })).is_empty());
    }

    #[test]
    fn in_cluster_url_is_the_mesh_service_not_a_public_hostname() {
        // The default bind must stay a cluster-internal Service: data-mesh is a
        // system of record and is never internet-exposed.
        assert!(IN_CLUSTER_URL.ends_with(".svc.cluster.local"));
        assert!(validate_consumer_url(IN_CLUSTER_URL).is_ok());
    }
}
