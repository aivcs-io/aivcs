//! `aivcs login` — authenticate to the AIVCS forge access service.
//!
//! Off-cluster callers use the stable authenticated edge (`https://…`) or
//! `--tailscale` for subnet-routed cluster Service IPs. Kubernetes workloads
//! may opt into Service DNS. None of these require `kubectl port-forward`.

use crate::forge_url_policy::{forge_service_url, validate_forge_url};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const DEFAULT_CONTEXT: &str = "aivcs-core";
const DEFAULT_NAMESPACE: &str = "forge-v2";
const DEFAULT_SERVICE: &str = "forge-v2";
const DEFAULT_SECRET: &str = "forge-v2-token";
const DEFAULT_SECRET_KEY: &str = "token";
const DEFAULT_SERVICE_PORT: u16 = 80;
/// Public Forge v2 endpoint used by laptop clients.
pub const EDGE_FORGE_URL: &str = "https://forge-v2.aivcs.io";

pub const DEFAULT_ISSUER_URL: &str = "https://issuer.aivcs.io";

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
    pub issuer: Option<String>,
    pub device: bool,
    pub in_cluster: bool,
    pub tailscale: bool,
    pub tls: bool,
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
            issuer: None,
            device: false,
            in_cluster: false,
            tailscale: false,
            tls: false,
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

pub fn forge_service_url_for_login(service: &str, namespace: &str, port: u16, tls: bool) -> String {
    forge_service_url(service, namespace, port, tls)
}

fn login_target(opts: &LoginOptions) -> Result<(String, &'static str)> {
    if let Some(url) = opts
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        let normalized = url.trim_end_matches('/').to_string();
        validate_forge_url(&normalized, opts.tailscale)?;
        let method = if opts.tailscale {
            "tailscale_explicit"
        } else {
            "explicit"
        };
        return Ok((normalized, method));
    }

    if opts.tailscale {
        let cluster_ip = resolve_service_cluster_ip(&opts.context, &opts.namespace, &opts.service)?;
        let normalized = format!("http://{cluster_ip}");
        validate_forge_url(&normalized, true)?;
        return Ok((normalized, "tailscale_subnet"));
    }

    if opts.in_cluster {
        return Ok((
            forge_service_url(&opts.service, &opts.namespace, opts.port, opts.tls),
            if opts.tls {
                "cluster_dns_tls"
            } else {
                "cluster_dns"
            },
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

#[derive(Debug, Deserialize)]
struct OidcDiscovery {
    #[serde(default)]
    device_authorization_endpoint: Option<String>,
    #[serde(default)]
    token_endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeviceAuthResponse {
    device_code: String,
    user_code: String,
    verification_uri: Option<String>,
    verification_uri_complete: Option<String>,
    #[serde(default = "default_device_expires_in")]
    expires_in: u64,
    #[serde(default = "default_device_interval")]
    interval: u64,
}

fn default_device_expires_in() -> u64 {
    900
}

fn default_device_interval() -> u64 {
    5
}

#[derive(Debug, Deserialize)]
struct TokenSuccessResponse {
    access_token: Option<String>,
    id_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: Option<String>,
    error_description: Option<String>,
}

pub async fn run_device_flow(issuer_url: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("build HTTP client for device authorization flow")?;

    let normalized_issuer = issuer_url.trim_end_matches('/');

    // 1. Discover endpoints from .well-known/openid-configuration
    let disco_url = format!("{normalized_issuer}/.well-known/openid-configuration");
    let (device_endpoint, token_endpoint) = if let Ok(resp) = client.get(&disco_url).send().await {
        if resp.status().is_success() {
            if let Ok(disco) = resp.json::<OidcDiscovery>().await {
                (
                    disco
                        .device_authorization_endpoint
                        .unwrap_or_else(|| format!("{normalized_issuer}/oauth/device_authorization")),
                    disco
                        .token_endpoint
                        .unwrap_or_else(|| format!("{normalized_issuer}/oauth/token")),
                )
            } else {
                (
                    format!("{normalized_issuer}/oauth/device_authorization"),
                    format!("{normalized_issuer}/oauth/token"),
                )
            }
        } else {
            (
                format!("{normalized_issuer}/oauth/device_authorization"),
                format!("{normalized_issuer}/oauth/token"),
            )
        }
    } else {
        (
            format!("{normalized_issuer}/oauth/device_authorization"),
            format!("{normalized_issuer}/oauth/token"),
        )
    };

    // 2. Request device authorization (RFC 8628 §3.1)
    let auth_resp = client
        .post(&device_endpoint)
        .form(&[("client_id", "aivcs-cli"), ("scope", "openid profile")])
        .send()
        .await
        .with_context(|| format!("send request to device authorization endpoint {device_endpoint}"))?;

    if !auth_resp.status().is_success() {
        let status = auth_resp.status();
        let body = auth_resp.text().await.unwrap_or_default();
        return Err(anyhow!("Device authorization failed ({status}): {body}"));
    }

    let auth_data: DeviceAuthResponse = auth_resp
        .json()
        .await
        .context("parse device authorization response JSON")?;

    println!();
    println!("=== AIVCS Device Authorization Flow ===");
    println!("  User Code:        {}", auth_data.user_code);
    if let Some(ref complete_url) = auth_data.verification_uri_complete {
        println!("  Verification URL: {complete_url}");
    } else if let Some(ref verify_url) = auth_data.verification_uri {
        println!("  Verification URL: {verify_url}");
    }
    println!();
    println!(
        "Please open the verification URL in your browser and approve code {}.",
        auth_data.user_code
    );
    println!("Waiting for authorization...");

    let mut poll_interval = if auth_data.interval == 0 {
        5
    } else {
        auth_data.interval
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(auth_data.expires_in);

    // 3. Poll token endpoint (RFC 8628 §3.4)
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "Device authorization request timed out after {} seconds",
                auth_data.expires_in
            ));
        }

        tokio::time::sleep(Duration::from_secs(poll_interval)).await;

        let token_resp = client
            .post(&token_endpoint)
            .form(&[
                (
                    "grant_type",
                    "urn:ietf:params:oauth:grant-type:device_code",
                ),
                ("client_id", "aivcs-cli"),
                ("device_code", &auth_data.device_code),
            ])
            .send()
            .await;

        let resp = match token_resp {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("Token poll network error: {e}");
                continue;
            }
        };

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if status.is_success() {
            if let Ok(success) = serde_json::from_str::<TokenSuccessResponse>(&body) {
                if let Some(token) = success.access_token.or(success.id_token) {
                    if !token.trim().is_empty() {
                        return Ok(token.trim().to_string());
                    }
                }
            }
            return Err(anyhow!(
                "Received successful response from token endpoint but could not extract token: {body}"
            ));
        }

        if let Ok(err_payload) = serde_json::from_str::<TokenErrorResponse>(&body) {
            let err_code = err_payload.error.as_deref().unwrap_or("");
            let err_desc = err_payload.error_description.as_deref().unwrap_or("");

            if err_code == "authorization_pending" || err_desc.contains("authorization_pending") {
                continue;
            } else if err_code == "slow_down" || err_desc.contains("slow_down") {
                poll_interval += 5;
                continue;
            } else if err_code == "access_denied" || err_desc.contains("access_denied") {
                return Err(anyhow!("Authorization request was denied by the user."));
            } else if err_code == "expired_token" || err_desc.contains("expired_token") {
                return Err(anyhow!(
                    "Device authorization code expired. Please run `aivcs login --device` again."
                ));
            } else {
                return Err(anyhow!("Device token error ({err_code}): {err_desc}"));
            }
        }
    }
}

pub async fn run_login(opts: LoginOptions) -> Result<()> {
    fs::create_dir_all(aivcs_home()).context("create ~/.aivcs")?;

    let (forge_url, login_method) = login_target(&opts)?;
    let (token, effective_method) = if opts.device {
        let env_issuer = std::env::var("AIVCS_ISSUER_URL").ok();
        let issuer_url = opts
            .issuer
            .as_deref()
            .or(env_issuer.as_deref())
            .unwrap_or(DEFAULT_ISSUER_URL);
        (run_device_flow(issuer_url).await?, "device_flow")
    } else {
        (resolve_token(&opts)?, login_method)
    };

    probe_forge(&forge_url, &token).await.with_context(|| {
        format!(
            "forge access service unreachable at {forge_url}. \
             Off-cluster: {EDGE_FORGE_URL} or `aivcs login --tailscale`; \
             in-cluster: `aivcs login --in-cluster`; workloads: `AIVCS_FORGE_URL`."
        )
    })?;

    fs::write(&opts.token_file, format!("{token}\n"))
        .with_context(|| format!("write token to {}", opts.token_file.display()))?;

    let config = ForgeSessionConfig {
        forge_url: forge_url.clone(),
        kube_context: opts.context.clone(),
        kube_namespace: opts.namespace.clone(),
        kube_service: opts.service.clone(),
        login_method: effective_method.to_string(),
        logged_in_at: chrono::Utc::now().to_rfc3339(),
    };
    let config_json = serde_json::to_string_pretty(&config)?;
    fs::write(&opts.config_file, config_json)
        .with_context(|| format!("write {}", opts.config_file.display()))?;

    println!("Logged in to forge at {forge_url}");
    println!("  method:    {effective_method}");
    if opts.in_cluster || opts.tailscale {
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

fn resolve_service_cluster_ip(context: &str, namespace: &str, service: &str) -> Result<String> {
    let output = Command::new("kubectl")
        .args([
            "--context",
            context,
            "-n",
            namespace,
            "get",
            "svc",
            service,
            "-o",
            "jsonpath={.spec.clusterIP}",
        ])
        .output()
        .context("kubectl not found or failed to run")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "kubectl get svc {service} in {namespace} failed: {stderr}. \
             Ensure Tailscale subnet routes are accepted and the service exists."
        ));
    }

    let cluster_ip = String::from_utf8(output.stdout)
        .context("clusterIP jsonpath output")?
        .trim()
        .to_string();
    if cluster_ip.is_empty() || cluster_ip == "None" {
        return Err(anyhow!("service {namespace}/{service} has no ClusterIP"));
    }
    Ok(cluster_ip)
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
            forge_service_url("aivcs-forge-pg", "aivcs-forge-pg", 80, false),
            "http://aivcs-forge-pg.aivcs-forge-pg.svc.cluster.local"
        );
    }

    #[test]
    fn forge_service_url_non_default_port() {
        assert_eq!(
            forge_service_url("aivcs-forge-pg", "aivcs-forge-pg", 8080, false),
            "http://aivcs-forge-pg.aivcs-forge-pg.svc.cluster.local:8080"
        );
    }

    #[test]
    fn in_cluster_constant_uses_service_port_not_container_port() {
        assert_eq!(
            IN_CLUSTER_FORGE_URL,
            "http://aivcs-forge-pg.aivcs-forge-pg.svc.cluster.local"
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
        assert_eq!(
            url,
            "http://aivcs-forge-pg.aivcs-forge-pg.svc.cluster.local"
        );
        assert_eq!(method, "cluster_dns");
    }

    #[test]
    fn in_cluster_tls_uses_https() {
        let opts = LoginOptions {
            in_cluster: true,
            tls: true,
            port: 443,
            ..LoginOptions::default()
        };
        let (url, method) = login_target(&opts).unwrap();
        assert_eq!(
            url,
            "https://aivcs-forge-pg.aivcs-forge-pg.svc.cluster.local"
        );
        assert_eq!(method, "cluster_dns_tls");
    }

    #[test]
    fn explicit_https_tailnet_url_is_allowed() {
        let opts = LoginOptions {
            url: Some("https://aivcs-forge-pg.tailbab6b0.ts.net".into()),
            ..LoginOptions::default()
        };
        let (url, method) = login_target(&opts).unwrap();
        assert_eq!(url, "https://aivcs-forge-pg.tailbab6b0.ts.net");
        assert_eq!(method, "explicit");
    }

    #[test]
    fn tailscale_explicit_http_private_ip() {
        let opts = LoginOptions {
            url: Some("http://172.20.176.231".into()),
            tailscale: true,
            ..LoginOptions::default()
        };
        let (url, method) = login_target(&opts).unwrap();
        assert_eq!(url, "http://172.20.176.231");
        assert_eq!(method, "tailscale_explicit");
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
            forge_url: forge_service_url(DEFAULT_SERVICE, DEFAULT_NAMESPACE, 80, false),
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
