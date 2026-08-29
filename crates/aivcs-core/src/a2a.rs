//! A2A event emission helpers.
//!
//! The event emitter is deliberately best-effort: callers can notify the A2A
//! transport after durable local state changes without letting transport
//! failures roll back the local commit.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use tokio::time::{sleep, Duration};
use tracing::warn;

/// Default JSON-RPC method used to publish A2A lifecycle events.
pub const DEFAULT_A2A_METHOD: &str = "a2a.events.publish";

/// Event kind emitted after an AIVCS commit is durably recorded.
pub const CODE_COMMITTED_KIND: &str = "CODE_COMMITTED";

/// Payload for a `CODE_COMMITTED` A2A event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeCommittedEvent {
    /// Repository in `owner/name` form.
    pub repo: String,
    /// Branch that received the AIVCS commit.
    pub branch: String,
    /// AIVCS commit hash.
    pub commit_sha: String,
    /// Paths included in the commit context.
    pub changed_paths: Vec<String>,
    /// Agent that authored the commit.
    pub authoring_agent_id: String,
    /// Ephemeral job identifier for the authoring agent.
    pub job_id: Option<String>,
    /// Event creation timestamp.
    pub timestamp: DateTime<Utc>,
    /// Associated AIVCS commit ID, if present. `serde(default)` so historical
    /// payloads emitted before this field existed deserialize cleanly into the
    /// current struct — `skip_serializing_if` alone is one-way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aivcs_commit_id: Option<String>,
}

impl CodeCommittedEvent {
    /// Build a JSON-RPC params object for this event.
    pub fn json_rpc_params(&self) -> serde_json::Value {
        json!({
            "event": {
                "kind": CODE_COMMITTED_KIND,
                "payload": self,
            }
        })
    }
}

/// JSON-RPC request body sent to the A2A transport.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub method: String,
    pub params: serde_json::Value,
    pub id: u64,
}

impl JsonRpcRequest {
    pub fn new(method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0",
            method: method.into(),
            params,
            id: 1,
        }
    }
}

/// Errors returned by A2A transports.
#[derive(Debug, Error)]
pub enum A2aError {
    #[error("A2A HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("A2A transport failed with status {status}: {body}")]
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("A2A transport error: {0}")]
    Transport(String),
}

/// Transport abstraction for emitting JSON-RPC messages.
#[async_trait]
pub trait A2aTransport: Send + Sync {
    async fn send_json_rpc(&self, request: &JsonRpcRequest) -> Result<(), A2aError>;
}

/// HTTP JSON-RPC transport for A2A events.
#[derive(Debug, Clone)]
pub struct HttpJsonRpcTransport {
    endpoint: String,
    client: reqwest::Client,
}

impl HttpJsonRpcTransport {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }
}

#[async_trait]
impl A2aTransport for HttpJsonRpcTransport {
    async fn send_json_rpc(&self, request: &JsonRpcRequest) -> Result<(), A2aError> {
        let response = self
            .client
            .post(&self.endpoint)
            .json(request)
            .send()
            .await?;
        let status = response.status();

        if status.is_success() {
            Ok(())
        } else {
            let body = response.text().await.unwrap_or_default();
            Err(A2aError::Status { status, body })
        }
    }
}

/// Retry policy for best-effort A2A event emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct A2aRetryPolicy {
    /// Total send attempts, including the initial attempt.
    pub max_attempts: u8,
    /// Base exponential backoff delay.
    pub backoff_base_ms: u64,
}

impl Default for A2aRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            backoff_base_ms: 100,
        }
    }
}

/// Emit `CODE_COMMITTED` when `AIVCS_A2A_JSONRPC_URL` is configured.
///
/// No-op when the endpoint env var is unset. Transport failures are logged and
/// retried but never propagated to callers.
///
/// `repo_override` supplies `owner/repo` when the caller knows the target repo
/// (e.g. GitHub Contents API commits from Agent Jobs without a local git remote).
pub async fn maybe_emit_code_committed_from_env(
    branch: &str,
    commit_sha: &str,
    changed_paths: Vec<String>,
    author: &str,
    repo_override: Option<&str>,
    aivcs_commit_id: Option<&str>,
) {
    let Some(endpoint) = std::env::var("AIVCS_A2A_JSONRPC_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return;
    };

    // Validate `repo_override` through the same gate the auto-detect path
    // goes through. An invalid override (e.g. unvalidated `--owner`/`--repo`
    // CLI flags from an upstream caller) silently falls back to detection
    // rather than smuggling shell-metachar / newline / dot-traversal bytes
    // into the emitted A2A event.
    let repo = repo_override
        .filter(|s| crate::git_host::is_forge_project_path(s))
        .map(str::to_string)
        .or_else(crate::git_host::detect_forge_repository)
        .unwrap_or_else(|| "unknown/unknown".to_string());
    let method = std::env::var("AIVCS_A2A_JSONRPC_METHOD")
        .unwrap_or_else(|_| DEFAULT_A2A_METHOD.to_string());
    let authoring_agent_id = std::env::var("AIVCS_AGENT_ID").unwrap_or_else(|_| author.to_string());
    let job_id = std::env::var("AIVCS_JOB_ID")
        .ok()
        .filter(|value| !value.trim().is_empty());

    let event = CodeCommittedEvent {
        repo,
        branch: branch.to_string(),
        commit_sha: commit_sha.to_string(),
        changed_paths,
        authoring_agent_id,
        job_id,
        timestamp: Utc::now(),
        aivcs_commit_id: aivcs_commit_id.map(str::to_string),
    };

    let transport = HttpJsonRpcTransport::new(endpoint);
    emit_code_committed_best_effort(&transport, &method, &event, A2aRetryPolicy::default()).await;
}

/// Emit a `CODE_COMMITTED` event. Transport failures are logged and swallowed.
pub async fn emit_code_committed_best_effort<T: A2aTransport>(
    transport: &T,
    method: &str,
    event: &CodeCommittedEvent,
    retry: A2aRetryPolicy,
) {
    let attempts = retry.max_attempts.max(1);
    let request = JsonRpcRequest::new(method, event.json_rpc_params());

    for attempt in 1..=attempts {
        match transport.send_json_rpc(&request).await {
            Ok(()) => return,
            Err(error) if attempt == attempts => {
                warn!(
                    error = %error,
                    attempts = attempts,
                    event_kind = CODE_COMMITTED_KIND,
                    commit_sha = %event.commit_sha,
                    "failed to emit A2A event after retries"
                );
            }
            Err(error) => {
                warn!(
                    error = %error,
                    attempt = attempt,
                    event_kind = CODE_COMMITTED_KIND,
                    commit_sha = %event.commit_sha,
                    "failed to emit A2A event; retrying"
                );

                let delay = retry.backoff_base_ms * 2u64.pow(u32::from(attempt - 1));
                sleep(Duration::from_millis(delay)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingTransport {
        attempts_before_success: usize,
        attempts: AtomicUsize,
        requests: Mutex<Vec<JsonRpcRequest>>,
    }

    #[async_trait]
    impl A2aTransport for RecordingTransport {
        async fn send_json_rpc(&self, request: &JsonRpcRequest) -> Result<(), A2aError> {
            self.requests.lock().unwrap().push(request.clone());
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt <= self.attempts_before_success {
                Err(A2aError::Transport("temporary failure".to_string()))
            } else {
                Ok(())
            }
        }
    }

    fn example_event() -> CodeCommittedEvent {
        CodeCommittedEvent {
            repo: "stevedores-org/aivcs".to_string(),
            branch: "develop".to_string(),
            commit_sha: "abc123".to_string(),
            changed_paths: vec!["state.json".to_string()],
            authoring_agent_id: "builder-agent".to_string(),
            job_id: Some("job-123".to_string()),
            timestamp: Utc::now(),
            aivcs_commit_id: Some("aivcs-hash-123".to_string()),
        }
    }

    #[test]
    fn code_committed_event_serializes_expected_fields() {
        let event = example_event();
        let params = event.json_rpc_params();

        assert_eq!(params["event"]["kind"], CODE_COMMITTED_KIND);
        assert_eq!(params["event"]["payload"]["repo"], "stevedores-org/aivcs");
        assert_eq!(params["event"]["payload"]["branch"], "develop");
        assert_eq!(params["event"]["payload"]["commit_sha"], "abc123");
        assert_eq!(
            params["event"]["payload"]["aivcs_commit_id"],
            "aivcs-hash-123"
        );
        assert_eq!(
            params["event"]["payload"]["changed_paths"],
            json!(["state.json"])
        );
        assert_eq!(
            params["event"]["payload"]["authoring_agent_id"],
            "builder-agent"
        );
        assert_eq!(params["event"]["payload"]["job_id"], "job-123");
        assert!(params["event"]["payload"]["timestamp"].is_string());
    }

    #[tokio::test(start_paused = true)]
    async fn best_effort_emit_retries_then_succeeds() {
        let transport = RecordingTransport {
            attempts_before_success: 2,
            ..RecordingTransport::default()
        };

        emit_code_committed_best_effort(
            &transport,
            DEFAULT_A2A_METHOD,
            &example_event(),
            A2aRetryPolicy {
                max_attempts: 3,
                backoff_base_ms: 10,
            },
        )
        .await;

        assert_eq!(transport.attempts.load(Ordering::SeqCst), 3);
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].jsonrpc, "2.0");
        assert_eq!(requests[0].method, DEFAULT_A2A_METHOD);
    }

    #[tokio::test(start_paused = true)]
    async fn best_effort_emit_swallows_final_failure() {
        let transport = RecordingTransport {
            attempts_before_success: usize::MAX,
            ..RecordingTransport::default()
        };

        emit_code_committed_best_effort(
            &transport,
            DEFAULT_A2A_METHOD,
            &example_event(),
            A2aRetryPolicy {
                max_attempts: 2,
                backoff_base_ms: 10,
            },
        )
        .await;

        assert_eq!(transport.attempts.load(Ordering::SeqCst), 2);
    }
}
