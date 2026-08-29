//! Attic binary cache client
//!
//! Provides integration with Attic for caching Nix build artifacts.
//! Attic is a self-hosted Nix binary cache server.

use crate::error::NixError;
use crate::flake::NixHash;
use crate::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};

/// Attic configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtticConfig {
    /// Attic server URL
    pub server_url: String,
    /// Cache name to use
    pub cache_name: String,
    /// Authentication token (optional for public caches)
    pub token: Option<String>,
    /// Whether to use CLI or HTTP API
    pub use_cli: bool,
}

impl Default for AtticConfig {
    fn default() -> Self {
        AtticConfig {
            server_url: std::env::var("ATTIC_SERVER")
                .unwrap_or_else(|_| "https://cache.nixos.org".to_string()),
            cache_name: std::env::var("ATTIC_CACHE").unwrap_or_else(|_| "aivcs".to_string()),
            token: std::env::var("ATTIC_TOKEN").ok(),
            use_cli: true,
        }
    }
}

impl AtticConfig {
    /// Create a new config from environment variables
    pub fn from_env() -> Self {
        Self::default()
    }

    /// Create config for a specific server
    pub fn new(server_url: &str, cache_name: &str) -> Self {
        AtticConfig {
            server_url: server_url.to_string(),
            cache_name: cache_name.to_string(),
            token: None,
            use_cli: true,
        }
    }

    /// Set authentication token
    pub fn with_token(mut self, token: &str) -> Self {
        self.token = Some(token.to_string());
        self
    }
}

/// Attic client for binary cache operations
pub struct AtticClient {
    config: AtticConfig,
    http_client: reqwest::Client,
}

impl AtticClient {
    /// Create a new Attic client
    pub fn new(config: AtticConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .user_agent("aivcs-nix-env-manager/0.1.0")
            .build()
            .expect("Failed to create HTTP client");

        AtticClient {
            config,
            http_client,
        }
    }

    /// Create client from environment variables
    pub fn from_env() -> Self {
        Self::new(AtticConfig::from_env())
    }

    /// Check if an environment is cached
    ///
    /// # TDD: test_pull_nonexistent_hash_fails_gracefully
    pub async fn is_environment_cached(&self, hash: &NixHash) -> bool {
        if self.config.use_cli {
            self.is_cached_cli(hash).await
        } else {
            self.is_cached_http(hash).await
        }
    }

    /// Check cache using CLI
    async fn is_cached_cli(&self, hash: &NixHash) -> bool {
        // Use nix path-info to check if path exists in cache
        let store_path = format!("/nix/store/{}-aivcs-env", hash.short());

        let output = Command::new("nix")
            .args(["path-info", "--store", &self.config.server_url, &store_path])
            .output();

        match output {
            Ok(o) => o.status.success(),
            Err(_) => false,
        }
    }

    /// Check cache using HTTP API
    async fn is_cached_http(&self, hash: &NixHash) -> bool {
        let url = format!(
            "{}/{}/{}.narinfo",
            self.config.server_url,
            self.config.cache_name,
            hash.short()
        );

        match self.http_client.head(&url).send().await {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }

    /// Pull environment from cache
    ///
    /// Returns the path to the cached environment
    pub async fn pull_environment(&self, hash: &NixHash) -> Result<PathBuf> {
        info!("Pulling environment {} from Attic", hash.short());

        if self.config.use_cli {
            self.pull_cli(hash).await
        } else {
            self.pull_http(hash).await
        }
    }

    /// Pull using Attic CLI
    async fn pull_cli(&self, hash: &NixHash) -> Result<PathBuf> {
        let store_path = format!("/nix/store/{}-aivcs-env", hash.short());

        // Try to fetch from cache
        let output = Command::new("nix")
            .args(["copy", "--from", &self.config.server_url, &store_path])
            .output()?;

        if output.status.success() {
            debug!("Successfully pulled environment from cache");
            Ok(PathBuf::from(&store_path))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to pull from cache: {}", stderr);
            Err(NixError::EnvironmentNotCached(hash.hash.clone()))
        }
    }

    /// Pull using HTTP API (placeholder)
    async fn pull_http(&self, hash: &NixHash) -> Result<PathBuf> {
        // HTTP-based pulling would require implementing NAR fetching
        // For now, fall back to CLI
        warn!("HTTP pull not implemented, falling back to CLI");
        self.pull_cli(hash).await
    }

    /// Push environment to cache
    ///
    /// Takes the store path of a built environment and pushes it to the cache
    pub async fn push_environment(&self, hash: &NixHash, store_path: &Path) -> Result<()> {
        info!("Pushing environment {} to Attic", hash.short());

        if self.config.use_cli {
            self.push_cli(hash, store_path).await
        } else {
            self.push_http(hash, store_path).await
        }
    }

    /// Push using Attic CLI
    async fn push_cli(&self, _hash: &NixHash, store_path: &Path) -> Result<()> {
        // Check if attic CLI is available
        let attic_available = Command::new("attic")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if attic_available {
            // Use attic CLI
            let output = Command::new("attic")
                .args([
                    "push",
                    &self.config.cache_name,
                    &store_path.to_string_lossy(),
                ])
                .output()?;

            if output.status.success() {
                debug!("Successfully pushed to Attic cache");
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(NixError::AtticCommandFailed(stderr.to_string()))
            }
        } else {
            // Fall back to nix copy
            let output = Command::new("nix")
                .args([
                    "copy",
                    "--to",
                    &self.config.server_url,
                    &store_path.to_string_lossy(),
                ])
                .output()?;

            if output.status.success() {
                debug!("Successfully pushed using nix copy");
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(NixError::NixCommandFailed(stderr.to_string()))
            }
        }
    }

    /// Push using HTTP API (placeholder)
    async fn push_http(&self, _hash: &NixHash, _store_path: &Path) -> Result<()> {
        // HTTP-based pushing would require implementing NAR uploading
        warn!("HTTP push not implemented");
        Err(NixError::AtticNotConfigured)
    }

    /// Build and cache an environment from a flake
    pub async fn build_and_cache(&self, flake_path: &Path) -> Result<(NixHash, PathBuf)> {
        info!("Building environment from {:?}", flake_path);

        // Build the flake
        let output = Command::new("nix")
            .args(["build", "--json", "--no-link"])
            .current_dir(flake_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(NixError::NixCommandFailed(stderr.to_string()));
        }

        // Parse the output to get the store path
        #[derive(Deserialize)]
        struct BuildOutput {
            outputs: std::collections::HashMap<String, String>,
        }

        let outputs: Vec<BuildOutput> = serde_json::from_slice(&output.stdout)?;
        let store_path = outputs
            .first()
            .and_then(|o| o.outputs.get("out"))
            .ok_or_else(|| NixError::NixCommandFailed("No output path".to_string()))?;

        let store_path = PathBuf::from(store_path);

        // Generate the environment hash
        let hash = crate::generate_environment_hash(flake_path)?;

        // Push to cache
        self.push_environment(&hash, &store_path).await?;

        Ok((hash, store_path))
    }

    /// Get cache statistics
    pub async fn get_cache_info(&self) -> Result<CacheInfo> {
        if self.config.use_cli {
            // Try attic cache info
            let output = Command::new("attic")
                .args(["cache", "info", &self.config.cache_name])
                .output();

            match output {
                Ok(o) if o.status.success() => {
                    let stdout = String::from_utf8_lossy(&o.stdout);
                    Ok(CacheInfo {
                        name: self.config.cache_name.clone(),
                        server: self.config.server_url.clone(),
                        available: true,
                        info: Some(stdout.to_string()),
                    })
                }
                _ => Ok(CacheInfo {
                    name: self.config.cache_name.clone(),
                    server: self.config.server_url.clone(),
                    available: false,
                    info: None,
                }),
            }
        } else {
            // HTTP health check
            let url = format!("{}/nix-cache-info", self.config.server_url);
            match self.http_client.get(&url).send().await {
                Ok(response) if response.status().is_success() => {
                    let body = response.text().await.unwrap_or_default();
                    Ok(CacheInfo {
                        name: self.config.cache_name.clone(),
                        server: self.config.server_url.clone(),
                        available: true,
                        info: Some(body),
                    })
                }
                _ => Ok(CacheInfo {
                    name: self.config.cache_name.clone(),
                    server: self.config.server_url.clone(),
                    available: false,
                    info: None,
                }),
            }
        }
    }
}

/// Cache information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheInfo {
    /// Cache name
    pub name: String,
    /// Server URL
    pub server: String,
    /// Whether the cache is available
    pub available: bool,
    /// Additional info from the server
    pub info: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attic_config_default() {
        let config = AtticConfig::default();
        assert!(!config.server_url.is_empty());
        assert!(!config.cache_name.is_empty());
        assert!(config.use_cli);
    }

    #[test]
    fn test_attic_config_new() {
        let config = AtticConfig::new("https://my-cache.example.com", "my-cache");
        assert_eq!(config.server_url, "https://my-cache.example.com");
        assert_eq!(config.cache_name, "my-cache");
    }

    #[test]
    fn test_attic_config_with_token() {
        let config = AtticConfig::default().with_token("secret-token");
        assert_eq!(config.token, Some("secret-token".to_string()));
    }

    #[tokio::test]
    async fn test_pull_nonexistent_hash_fails_gracefully() {
        let client = AtticClient::from_env();
        let fake_hash = NixHash::new(
            "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            crate::flake::HashSource::FlakeLock,
        );

        // Should return false, not panic
        let cached = client.is_environment_cached(&fake_hash).await;
        assert!(!cached);
    }

    #[tokio::test]
    async fn test_get_cache_info() {
        let client = AtticClient::from_env();
        let info = client.get_cache_info().await;

        // Should succeed even if cache is not available
        assert!(info.is_ok());
    }
}
