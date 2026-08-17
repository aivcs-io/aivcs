//! Single-Turn Cognition & Brains Provider for AIVCS CLI
//!
//! Connects `aivcs` to a configured cognition endpoint.
//! to generate single-turn decisive action steps over data-mesh context.

use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainsReasonRequest {
    pub goal: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainsReasonResponse {
    pub decision: String,
    pub next_action: String,
    #[serde(default)]
    pub explanation: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

pub struct BrainsClient {
    endpoint: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl BrainsClient {
    pub fn new(endpoint_override: Option<&str>) -> Self {
        let base_url = endpoint_override
            .map(|s| s.trim_end_matches('/').to_string())
            .or_else(|| std::env::var("BRAINS_URL").ok())
            .unwrap_or_else(|| "http://127.0.0.1:18080".to_string());

        let token = std::env::var("AIVCS_TOKEN").ok().or_else(|| {
            if let Ok(home) = std::env::var("HOME") {
                let p = PathBuf::from(home).join(".aivcs").join("token");
                fs::read_to_string(p).ok().map(|s| s.trim().to_string())
            } else {
                None
            }
        });

        Self {
            endpoint: base_url,
            token,
            http: reqwest::Client::builder().build().unwrap_or_default(),
        }
    }

    pub async fn reason(
        &self,
        goal: &str,
        task_id: Option<&str>,
        agent: Option<&str>,
        context: Option<serde_json::Value>,
    ) -> Result<BrainsReasonResponse> {
        let url = format!("{}/v1/reason", self.endpoint);
        let req = BrainsReasonRequest {
            goal: goal.to_string(),
            task_id: task_id.map(String::from),
            agent: agent.map(String::from),
            context,
        };

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(ref t) = self.token {
            if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", t)) {
                headers.insert(AUTHORIZATION, val);
            }
        }

        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(&req)
            .send()
            .await
            .context(format!(
                "Failed to send reason request to brains at {}",
                url
            ))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Brains error (HTTP {}): {}", status, body));
        }

        let res: BrainsReasonResponse = resp
            .json()
            .await
            .context("Failed to parse brains response")?;
        Ok(res)
    }
}
