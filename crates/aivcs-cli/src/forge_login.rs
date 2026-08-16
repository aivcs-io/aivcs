//! `aivcs login` — authenticate to the in-cluster forge via Kubernetes service DNS.
//!
//! Resolves `http://<service>.<namespace>.svc.cluster.local` (ClusterIP port 80)
//! so operators on OrbStack / VPN / mesh-attached laptops reach `aivcsd-lite`
//! without `kubectl port-forward`.

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

    let forge_url = forge_service_url(&opts.service, &opts.namespace, opts.port);
    let token = resolve_token(&opts)?;

    probe_forge(&forge_url, &token).await.with_context(|| {
        format!(
            "forge unreachable at {forge_url}. Ensure kubectl context `{}` is active \
             and cluster DNS resolves (OrbStack k8s, Tailscale, or in-cluster). \
             Do not use kubectl port-forward — run `aivcs login` instead.",
            opts.context
        )
    })?;

    fs::write(&opts.token_file, format!("{token}\n"))
        .with_context(|| format!("write token to {}", opts.token_file.display()))?;

    let config = ForgeSessionConfig {
        forge_url: forge_url.clone(),
        kube_context: opts.context.clone(),
        kube_namespace: opts.namespace.clone(),
        kube_service: opts.service.clone(),
        login_method: "cluster_dns".to_string(),
        logged_in_at: chrono::Utc::now().to_rfc3339(),
    };
    let config_json = serde_json::to_string_pretty(&config)?;
    fs::write(&opts.config_file, config_json)
        .with_context(|| format!("write {}", opts.config_file.display()))?;

    println!("Logged in to forge at {forge_url}");
    println!("  context:   {}", opts.context);
    println!("  namespace: {}", opts.namespace);
    println!("  service:   {}", opts.service);
    println!("  token:     {}", opts.token_file.display());
    println!("  config:    {}", opts.config_file.display());
    println!();
    println!("Publish/fetch/clone now use cluster DNS — no port-forward required.");
    Ok(())
}

pub async fn run_status() -> Result<()> {
    let config_path = aivcs_home().join("config.json");
    let token_path = aivcs_home().join("token");

    if !config_path.exists() {
        println!("Not logged in. Run: aivcs login --context aivcs-core");
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
    Ok(String::from_utf8(bytes).context("secret token utf-8")?)
}

async fn probe_forge(url: &str, token: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    let health_url = format!("{}/healthz", url.trim_end_matches('/'));
    let resp = client
        .get(&health_url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .with_context(|| format!("GET {health_url}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("forge health check HTTP {status}: {body}"));
    }
    Ok(())
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
