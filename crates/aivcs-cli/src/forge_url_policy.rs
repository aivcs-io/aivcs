//! Forge access URL policy for `aivcs login` and remote clients.
//!
//! HTTPS is always permitted. Plain HTTP is limited to loopback, in-cluster
//! Service DNS, and — with `--tailscale` — RFC1918 / tailnet-routed addresses.

use anyhow::{anyhow, Result};
use std::net::IpAddr;

/// Returns true when `url` may be used as a forge base URL.
pub fn is_allowed_forge_url(url: &str, tailscale: bool) -> bool {
    validate_forge_url(url, tailscale).is_ok()
}

pub fn validate_forge_url(url: &str, tailscale: bool) -> Result<()> {
    let normalized = url.trim().trim_end_matches('/');
    if normalized.is_empty() {
        return Err(anyhow!("forge URL must not be empty"));
    }

    if normalized.starts_with("https://") {
        return Ok(());
    }

    if normalized.starts_with("http://127.0.0.1") || normalized.starts_with("http://localhost") {
        return Ok(());
    }

    let Some(host) = host_from_url(normalized) else {
        return Err(anyhow!(
            "forge URL must start with http:// or https:// (got {normalized:?})"
        ));
    };

    if host.ends_with(".svc.cluster.local") {
        return Ok(());
    }

    if tailscale && is_tailscale_routable_host(&host) {
        return Ok(());
    }

    Err(anyhow!(
        "forge URL must use HTTPS; HTTP is allowed only for loopback, Kubernetes Service DNS, \
         or private/tailnet addresses with `aivcs login --tailscale`"
    ))
}

pub fn forge_service_url(service: &str, namespace: &str, port: u16, tls: bool) -> String {
    let scheme = if tls || port == 443 { "https" } else { "http" };
    if port == 80 || port == 443 {
        format!("{scheme}://{service}.{namespace}.svc.cluster.local")
    } else {
        format!("{scheme}://{service}.{namespace}.svc.cluster.local:{port}")
    }
}

fn host_from_url(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let authority = rest.split('/').next()?;
    Some(
        authority
            .split('@')
            .next_back()?
            .rsplit_once(':')
            .map(|(host, _)| host.to_string())
            .unwrap_or_else(|| authority.to_string()),
    )
}

fn is_tailscale_routable_host(host: &str) -> bool {
    if host.ends_with(".ts.net") {
        return true;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return is_private_or_tailscale_ip(&ip);
    }
    false
}

fn is_private_or_tailscale_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() {
                return true;
            }
            let o = v4.octets();
            // RFC1918
            o[0] == 10
                || (o[0] == 172 && (16..=31).contains(&o[1]))
                || (o[0] == 192 && o[1] == 168)
                // Tailscale CGNAT 100.64.0.0/10
                || (o[0] == 100 && (64..=127).contains(&o[1]))
        }
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_any_host_is_allowed() {
        assert!(is_allowed_forge_url("https://forge.example.com", false));
        assert!(is_allowed_forge_url(
            "https://forge.node.ts.net",
            false
        ));
        assert!(is_allowed_forge_url("https://172.20.176.231", false));
    }

    #[test]
    fn plain_http_private_ip_requires_tailscale_flag() {
        assert!(!is_allowed_forge_url("http://172.20.176.231", false));
        assert!(is_allowed_forge_url("http://172.20.176.231", true));
        assert!(is_allowed_forge_url("http://100.108.157.16", true));
    }

    #[test]
    fn plain_http_public_host_is_rejected() {
        assert!(!is_allowed_forge_url("http://forge.example.test", false));
        assert!(!is_allowed_forge_url("http://forge.example.test", true));
    }

    #[test]
    fn cluster_dns_http_allowed_without_tailscale() {
        assert!(is_allowed_forge_url(
            "http://forge.default.svc.cluster.local",
            false
        ));
    }

    #[test]
    fn tailscale_magicdns_https_allowed_without_flag() {
        assert!(is_allowed_forge_url(
            "https://core.node.ts.net",
            false
        ));
    }

    #[test]
    fn forge_service_url_tls_uses_https_scheme() {
        assert_eq!(
            forge_service_url("forge", "default", 443, true),
            "https://forge.default.svc.cluster.local"
        );
    }
}
