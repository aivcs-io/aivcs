//! `aivcs login` — authenticate to the AIVCS forge access service.
//!
//! Off-cluster callers use the stable authenticated edge. Kubernetes workloads
//! may opt into Service DNS. Neither mode requires `kubectl port-forward`.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const DEFAULT_CONTEXT: &str = "aivcs-core";
const DEFAULT_NAMESPACE: &str = "aivcs-repo";
const DEFAULT_SERVICE: &str = "aivcsd-lite";
const DEFAULT_SECRET: &str = "aivcsd-lite-token";
const DEFAULT_SECRET_KEY: &str = "token";
const DEFAULT_SERVICE_PORT: u16 = 80;
pub const EDGE_FORGE_URL: &str = "https://aivcsd.aivcs.io";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForgeSessionConfig {
    pub forge_url: String,
    pub kube_context: String,
    pub kube_namespace: String,
    pub kube_service: String,
    pub login_method: String,
    pub logged_in_at: String,
}

#[derive(Debug, Clone)]
pub struct LoginOptions {
    pub url: Option<String>,
    pub in_cluster: bool,
    pub context: String,
    pub namespace: String,
    pub service: String,
    pub secret: String,
    pub secret_key: String,
    pub port: u16,
    pub token_file: PathBuf,
    pub config_file: PathBuf,
}

impl Default for LoginOptions {
    fn default() -> Self {
        Self {
            url: None,
            in_cluster: false,
            context: DEFAULT_CONTEXT.to_string(),
            namespace: DEFAULT_NAMESPACE.to_string(),
            service: DEFAULT_SERVICE.to_string(),
            secret: DEFAULT_SECRET.to_string(),
            secret_key: DEFAULT_SECRET_KEY.to_string(),
            port: DEFAULT_SERVICE_PORT,
            token_file: aivcs_home().join("token"),
            config_file: aivcs_home().join("config.json"),
        }
    }
}

pub fn aivcs_home() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".aivcs")
    } else {
        PathBuf::from("/tmp/.aivcs")
    }
}

pub fn forge_service_url(service: &str, namespace: &str, port: u16) -> String {
    if port == 80 {
        format!("http://{service}.{namespace}.svc.cluster.local")
    } else {
        format!("http://{service}.{namespace}.svc.cluster.local:{port}")
    }
}

fn login_target(opts: &LoginOptions) -> Result<(String, &'static str)> {
    if let Some(url) = opts
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        let normalized = url.trim_end_matches('/').to_string();
        if !(normalized.starts_with("https://")
            || normalized.starts_with("http://127.0.0.1")
            || normalized.starts_with("http://localhost")
            || normalized.ends_with(".svc.cluster.local"))
        {
            return Err(anyhow!(
                "forge URL must use HTTPS; HTTP is allowed only for loopback or Kubernetes Service DNS"
            ));
        }
        return Ok((normalized, "explicit"));
    }

    if opts.in_cluster {
        return Ok((
            forge_service_url(&opts.service, &opts.namespace, opts.port),
            "cluster_dns",
        ));
    }

    Ok((EDGE_FORGE_URL.to_string(), "edge_service"))
}

pub fn load_forge_config() -> Option<ForgeSessionConfig> {
    let path = aivcs_home().join("config.json");
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn resolve_forge_url_from_config() -> Option<String> {
    std::env::var("AIVCS_FORGE_URL")
        .ok()
        .or_else(|| std::env::var("AIVCS_URL").ok())
        .or_else(|| load_forge_config().map(|c| c.forge_url))
}

pub async fn run_login(opts: LoginOptions) -> Result<()> {
    fs::create_dir_all(aivcs_home()).context("create ~/.aivcs")?;

    let (forge_url, login_method) = login_target(&opts)?;
    let token = resolve_token(&opts)?;

    probe_forge(&forge_url, &token).await.with_context(|| {
        format!(
            "forge access service unreachable at {forge_url}. \
             Off-cluster callers use {EDGE_FORGE_URL}; workloads use `aivcs login --in-cluster`."
        )
    })?;

    fs::write(&opts.token_file, format!("{token}\n"))
        .with_context(|| format!("write token to {}", opts.token_file.display()))?;

    let config = ForgeSessionConfig {
        forge_url: forge_url.clone(),
        kube_context: opts.context.clone(),
        kube_namespace: opts.namespace.clone(),
        kube_service: opts.service.clone(),
        login_method: login_method.to_string(),
        logged_in_at: chrono::Utc::now().to_rfc3339(),
    };
    let config_json = serde_json::to_string_pretty(&config)?;
    fs::write(&opts.config_file, config_json)
        .with_context(|| format!("write {}", opts.config_file.display()))?;

    println!("Logged in to forge at {forge_url}");
    println!("  method:    {login_method}");
    if opts.in_cluster {
        println!("  context:   {}", opts.context);
        println!("  namespace: {}", opts.namespace);
        println!("  service:   {}", opts.service);
    }
    println!("  token:     {}", opts.token_file.display());
    println!("  config:    {}", opts.config_file.display());
    println!();
    println!("Publish/fetch/clone now use the AIVCS access service — no port-forward required.");
    Ok(())
}

pub async fn run_status() -> Result<()> {
    let config_path = aivcs_home().join("config.json");
    let token_path = aivcs_home().join("token");

    if !config_path.exists() {
        println!("Not logged in. Run: aivcs login");
        return Ok(());
    }

    let config: ForgeSessionConfig =
        serde_json::from_str(&fs::read_to_string(&config_path).context("read forge config")?)?;

    println!("Forge session");
    println!("  url:       {}", config.forge_url);
    println!("  context:   {}", config.kube_context);
    println!("  namespace: {}", config.kube_namespace);
    println!("  service:   {}", config.kube_service);
    println!("  method:    {}", config.login_method);
    println!("  logged in: {}", config.logged_in_at);

    let token = fs::read_to_string(&token_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    match token {
        Some(t) => match probe_forge(&config.forge_url, &t).await {
            Ok(()) => println!("  health:    ok"),
            Err(e) => println!("  health:    failed — {e:#}"),
        },
        None => println!("  health:    no token at {}", token_path.display()),
    }
    Ok(())
}

fn resolve_token(opts: &LoginOptions) -> Result<String> {
    if opts.token_file.is_file() {
        let existing = fs::read_to_string(&opts.token_file)
            .with_context(|| format!("read {}", opts.token_file.display()))?;
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    if let Ok(from_env) = std::env::var("AIVCS_TOKEN") {
        if !from_env.trim().is_empty() {
            return Ok(from_env.trim().to_string());
        }
    }

    kubectl_secret_token(
        &opts.context,
        &opts.namespace,
        &opts.secret,
        &opts.secret_key,
    )
}

fn kubectl_secret_token(context: &str, namespace: &str, secret: &str, key: &str) -> Result<String> {
    let output = Command::new("kubectl")
        .args([
            "--context",
            context,
            "-n",
            namespace,
            "get",
            "secret",
            secret,
            "-o",
            &format!("jsonpath={{.data.{key}}}"),
        ])
        .output()
        .context("kubectl not found or failed to run")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "kubectl get secret {secret} failed: {stderr}. \
             Place a bearer token at ~/.aivcs/token or set AIVCS_TOKEN."
        ));
    }

    let b64 = String::from_utf8(output.stdout)
        .context("secret jsonpath output")?
        .trim()
        .to_string();
    if b64.is_empty() {
        return Err(anyhow!("secret {secret} key {key} is empty"));
    }

    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .context("decode secret token")?;
    String::from_utf8(bytes).context("secret token utf-8")
}

async fn probe_forge(url: &str, token: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    let health_url = format!("{}/healthz", url.trim_end_matches('/'));
    let mut last_failure = String::new();

    for attempt in 1..=3u32 {
        match client
            .get(&health_url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(resp) => {
                let status = resp.status();
                let retryable = matches!(status.as_u16(), 429 | 502 | 503 | 504);
                let body = resp.text().await.unwrap_or_default();
                last_failure = format!("HTTP {status}: {body}");
                if !retryable || attempt == 3 {
                    break;
                }
            }
            Err(error) => {
                last_failure = error.to_string();
                if attempt == 3 {
                    break;
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(250 * 2u64.pow(attempt - 1))).await;
    }

    Err(anyhow!(
        "GET {health_url} failed after 3 attempts: {last_failure}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge_remote::IN_CLUSTER_FORGE_URL;

    #[test]
    fn forge_service_url_default_port_omits_suffix() {
        assert_eq!(
            forge_service_url("aivcsd-lite", "aivcs-repo", 80),
            "http://aivcsd-lite.aivcs-repo.svc.cluster.local"
        );
    }

    #[test]
    fn forge_service_url_non_default_port() {
        assert_eq!(
            forge_service_url("aivcsd-lite", "aivcs-repo", 8080),
            "http://aivcsd-lite.aivcs-repo.svc.cluster.local:8080"
        );
    }

    #[test]
    fn in_cluster_constant_uses_service_port_not_container_port() {
        assert_eq!(
            IN_CLUSTER_FORGE_URL,
            "http://aivcsd-lite.aivcs-repo.svc.cluster.local"
        );
    }

    #[test]
    fn login_defaults_to_edge_service() {
        let (url, method) = login_target(&LoginOptions::default()).unwrap();
        assert_eq!(url, EDGE_FORGE_URL);
        assert_eq!(method, "edge_service");
    }

    #[test]
    fn in_cluster_mode_is_explicit() {
        let opts = LoginOptions {
            in_cluster: true,
            ..LoginOptions::default()
        };
        let (url, method) = login_target(&opts).unwrap();
        assert_eq!(url, "http://aivcsd-lite.aivcs-repo.svc.cluster.local");
        assert_eq!(method, "cluster_dns");
    }

    #[test]
    fn remote_plain_http_is_rejected() {
        let opts = LoginOptions {
            url: Some("http://aivcsd.example.test".into()),
            ..LoginOptions::default()
        };
        assert!(login_target(&opts).is_err());
    }

    #[test]
    fn config_roundtrip() {
        let cfg = ForgeSessionConfig {
            forge_url: forge_service_url(DEFAULT_SERVICE, DEFAULT_NAMESPACE, 80),
            kube_context: DEFAULT_CONTEXT.to_string(),
            kube_namespace: DEFAULT_NAMESPACE.to_string(),
            kube_service: DEFAULT_SERVICE.to_string(),
            login_method: "cluster_dns".to_string(),
            logged_in_at: "2026-08-15T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: ForgeSessionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }
}
