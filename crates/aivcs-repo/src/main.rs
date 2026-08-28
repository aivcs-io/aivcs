//! aivcs-repo — a single small binary that creates AIVCS code repositories.
//!
//! One service, one image, one deployment (infra-code #1990). SoT is **aivcsd**
//! (Option A): this service authenticates the caller against the fleet IdP,
//! validates the name, and calls aivcsd `POST /v1/repos`, which registers the
//! repository in the forge (persisting to the data-mesh underneath) and returns
//! the `aivcs://` URI. This binary holds no state and never talks to the
//! data-mesh directly — aivcsd is the source of truth (aivcs-is-source-of-truth;
//! code-governance runtime-integration-via-mesh, #1215; design #1237 / #83).
//!
//! Std-only so the image stays tiny; in-mesh calls are plaintext HTTP to the
//! sidecar, which supplies mTLS. Mesh/IdP are consumed, never redeployed.

use std::env;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const MAX_BODY_BYTES: usize = 16 * 1024;

struct Config {
    listen_addr: String,
    /// Base URL of aivcsd, reached over the mesh
    /// (e.g. http://aivcsd-lite.aivcs-repo.svc.cluster.local). Required.
    aivcsd_url: String,
    /// Health path on the forge backend for `/readyz` upstream checks.
    /// aivcsd-lite (service-kit) exposes `/healthz` only — not `/readyz`.
    aivcsd_health_path: String,
    /// agent-idp UserInfo endpoint (over the mesh). Resolved at startup from
    /// service-discovery (`DISCOVERY_URL` + `capability=identity`) or the
    /// explicit override `IDP_INTROSPECT_URL`. Unset only when auth is disabled.
    idp_userinfo_url: Option<String>,
    /// Explicit opt-out for running without an IdP (local dev only). Anything
    /// other than `true` keeps the service fail-closed.
    auth_disabled: bool,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let aivcsd_url = env::var("AIVCSD_URL")
            .map_err(|_| "AIVCSD_URL is required (aivcsd base URL, over the mesh)".to_string())
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .and_then(|value| {
                if value.is_empty() {
                    Err("AIVCSD_URL is required (aivcsd base URL, over the mesh)".to_string())
                } else {
                    Ok(value)
                }
            })?;
        Ok(Config {
            listen_addr: env::var("AIVCS_LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            aivcsd_url,
            aivcsd_health_path: env::var("AIVCSD_HEALTH_PATH")
                .unwrap_or_else(|_| "/healthz".into())
                .trim()
                .trim_start_matches('/')
                .to_string(),
            idp_userinfo_url: resolve_idp_userinfo_url(
                env::var("AIVCS_REPO_AUTH_DISABLED").as_deref() == Ok("true"),
            )?,
            auth_disabled: env::var("AIVCS_REPO_AUTH_DISABLED").as_deref() == Ok("true"),
        })
    }
}

/// Minimal view of a `ServicePublication` from `GET /v1/discover`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DiscoveryPublication {
    service_id: String,
    capability: String,
    exposure_class: String,
    identity: String,
    port: u16,
    cluster: String,
    namespace: String,
}

fn env_optional(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn discovery_required() -> bool {
    matches!(
        env::var("DISCOVERY_REQUIRED").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

/// Resolve agent-idp UserInfo for mesh auth (code-governance#1238 M2 / SD5).
fn resolve_idp_userinfo_url(auth_disabled: bool) -> Result<Option<String>, String> {
    if let Some(url) = env_optional("IDP_INTROSPECT_URL") {
        return Ok(Some(url));
    }

    let Some(discovery_url) = env_optional("DISCOVERY_URL") else {
        if auth_disabled {
            return Ok(None);
        }
        return Err(
            "IdP required: set DISCOVERY_URL (service-discovery) or IDP_INTROSPECT_URL (override)"
                .into(),
        );
    };

    let brand = env::var("DISCOVERY_BRAND").unwrap_or_else(|_| "aivcs".into());
    let capability = env::var("IDP_CAPABILITY").unwrap_or_else(|_| "identity".into());
    let service_id = env::var("IDP_SERVICE_ID").unwrap_or_else(|_| "agent-idp/agent-idp".into());
    let userinfo_path = env::var("IDP_USERINFO_PATH").unwrap_or_else(|_| "/oauth/userinfo".into());
    let local_cluster =
        env_optional("DISCOVERY_LOCAL_CLUSTER").or_else(|| env_optional("AIVCS_CLUSTER"));

    discover_idp_userinfo_url(
        &discovery_url,
        &brand,
        &capability,
        &service_id,
        &userinfo_path,
        local_cluster.as_deref(),
    )
    .map(Some)
    .map_err(|error| {
        if discovery_required() {
            format!("DISCOVERY_REQUIRED=1: {error}")
        } else {
            error
        }
    })
}

fn discover_idp_userinfo_url(
    discovery_url: &str,
    brand: &str,
    capability: &str,
    service_id: &str,
    userinfo_path: &str,
    local_cluster: Option<&str>,
) -> Result<String, String> {
    let query = format!(
        "/v1/discover?brand={brand}&capability={capability}",
        brand = url_query_component(brand),
        capability = url_query_component(capability),
    );
    let url = format!("{}{query}", discovery_url.trim_end_matches('/'),);
    let (status, body) = request("GET", &url, None, None, &[])
        .map_err(|error| format!("discovery request to {url} failed: {error}"))?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "discovery returned HTTP {status} for capability={capability}"
        ));
    }

    let publications = parse_discovery_publications(&body)?;
    let publication = select_idp_publication(publications, service_id)?;
    let host = mesh_host_for_publication(&publication, local_cluster);
    let port = if publication.port == 0 {
        8080
    } else {
        publication.port
    };
    Ok(userinfo_url(&host, port, userinfo_path))
}

fn url_query_component(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn parse_discovery_publications(body: &str) -> Result<Vec<DiscoveryPublication>, String> {
    let trimmed = body.trim();
    if !trimmed.starts_with('[') {
        return Err("discovery response is not a JSON array".into());
    }
    let mut publications = Vec::new();
    for object in split_json_objects(trimmed) {
        if let Some(publication) = parse_discovery_publication(object) {
            publications.push(publication);
        }
    }
    Ok(publications)
}

fn split_json_objects(array_body: &str) -> Vec<&str> {
    let mut objects = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (index, byte) in array_body.bytes().enumerate() {
        match byte {
            b'{' if depth == 0 => {
                depth = 1;
                start = Some(index);
            }
            b'{' => depth += 1,
            b'}' if depth == 1 => {
                if let Some(start_index) = start {
                    objects.push(&array_body[start_index..=index]);
                }
                depth = 0;
                start = None;
            }
            b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    objects
}

fn parse_discovery_publication(object: &str) -> Option<DiscoveryPublication> {
    Some(DiscoveryPublication {
        service_id: json_string_field(object, "serviceId")?,
        capability: json_string_field(object, "capability").unwrap_or_default(),
        exposure_class: json_string_field(object, "exposureClass").unwrap_or_default(),
        identity: json_string_field(object, "identity").unwrap_or_default(),
        port: json_u16_field(object, "port").unwrap_or(8080),
        cluster: json_string_field(object, "cluster").unwrap_or_default(),
        namespace: json_string_field(object, "namespace").unwrap_or_default(),
    })
}

fn json_string_field(object: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let pos = object.find(&needle)?;
    let after_key = object[pos + needle.len()..].trim_start();
    let after_colon = after_key.strip_prefix(':')?.trim_start();
    let rest = after_colon.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn json_u16_field(object: &str, key: &str) -> Option<u16> {
    let needle = format!("\"{key}\"");
    let pos = object.find(&needle)?;
    let after_key = object[pos + needle.len()..].trim_start();
    let after_colon = after_key.strip_prefix(':')?.trim_start();
    let digits: String = after_colon
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn select_idp_publication(
    publications: Vec<DiscoveryPublication>,
    service_id: &str,
) -> Result<DiscoveryPublication, String> {
    publications
        .into_iter()
        .find(|publication| publication.service_id == service_id)
        .ok_or_else(|| {
            format!("no discovery publication for serviceId={service_id} (capability=identity)")
        })
}

fn service_name_from_id(service_id: &str) -> &str {
    service_id.rsplit('/').next().unwrap_or(service_id)
}

/// Derive the in-mesh hostname for a publication (SD5 — not a hard-coded mirror URL).
fn mesh_host_for_publication(
    publication: &DiscoveryPublication,
    local_cluster: Option<&str>,
) -> String {
    let service = service_name_from_id(&publication.service_id);
    if local_cluster == Some(publication.cluster.as_str()) {
        if publication.identity.contains(".svc.") {
            publication
                .identity
                .split(':')
                .next()
                .unwrap_or(&publication.identity)
                .to_string()
        } else {
            format!("{}.{}.svc.cluster.local", service, publication.namespace)
        }
    } else {
        format!(
            "{}-{}.{}.svc.cluster.local",
            service, publication.cluster, publication.namespace
        )
    }
}

fn userinfo_url(host: &str, port: u16, path: &str) -> String {
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!("http://{host}:{port}{path}")
}

fn main() -> io::Result<()> {
    let config = match Config::from_env() {
        Ok(config) => Arc::new(config),
        Err(error) => {
            eprintln!("configuration error: {error}");
            std::process::exit(2);
        }
    };
    if config.idp_userinfo_url.is_none() {
        if !config.auth_disabled {
            eprintln!(
                "IdP UserInfo is required (resolve via DISCOVERY_URL + capability=identity, \
                 or set IDP_INTROSPECT_URL). Set AIVCS_REPO_AUTH_DISABLED=true for local dev."
            );
            std::process::exit(2);
        }
        eprintln!(
            "WARNING: AIVCS_REPO_AUTH_DISABLED=true — requests are UNAUTHENTICATED (dev only)"
        );
    } else if let Some(url) = config.idp_userinfo_url.as_deref() {
        eprintln!("aivcs-repo IdP UserInfo: {url}");
    }
    let listener = TcpListener::bind(&config.listen_addr)?;
    eprintln!(
        "aivcs-repo listening on {} → aivcsd {}",
        config.listen_addr, config.aivcsd_url
    );
    // Thread-per-connection: a slow aivcsd/IdP call must not block the health
    // probes or other creates behind it.
    for connection in listener.incoming() {
        match connection {
            Ok(mut stream) => {
                let config = Arc::clone(&config);
                thread::spawn(move || {
                    if let Err(error) = handle_connection(&mut stream, &config) {
                        eprintln!("request failed: {error}");
                    }
                });
            }
            Err(error) => eprintln!("connection failed: {error}"),
        }
    }
    Ok(())
}

fn handle_connection(stream: &mut TcpStream, config: &Config) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();

    let mut content_length = 0usize;
    let mut authorization: Option<String> = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header == "\r\n" || header == "\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(MAX_BODY_BYTES + 1);
            } else if name.eq_ignore_ascii_case("authorization") {
                authorization = Some(value.trim().to_string());
            }
        }
    }

    match (method.as_str(), target.as_str()) {
        ("GET", "/healthz") => respond(stream, 200, r#"{"status":"ok"}"#),
        // readyz fails closed on aivcsd unreachability.
        ("GET", "/readyz") => match aivcsd_ready(config) {
            true => respond(stream, 200, r#"{"status":"ok"}"#),
            false => respond_error(stream, 503, "aivcsd unreachable"),
        },
        ("POST", "/v1/repositories") if content_length <= MAX_BODY_BYTES => {
            let token = authorization.as_deref().and_then(bearer_token);
            if !authenticate(config, token) {
                return respond_error(stream, 401, "missing or invalid bearer token");
            }
            let mut body = vec![0; content_length];
            reader.read_exact(&mut body)?;
            match parse_create_request(&body) {
                Some(name) if valid_name(&name) => create_repository(stream, config, &name, token),
                Some(_) => respond_error(
                    stream,
                    422,
                    "name must be 1-63 lowercase letters, digits, or hyphens and cannot start or end with a hyphen",
                ),
                None => respond_error(stream, 400, "request body must be JSON with a name field"),
            }
        }
        ("POST", "/v1/repositories") => respond_error(stream, 413, "request body is too large"),
        _ => respond_error(stream, 404, "not found"),
    }
}

/// Authenticate the caller against agent-idp UserInfo. Fail-closed: if an IdP is
/// configured, only a Bearer token UserInfo accepts (HTTP 200) passes.
/// **Fail closed.** An absent IdP URL used to return `true`, so every request
/// authenticated and a log line was the only mitigation. A warning is not a
/// control, and `FR_SHARED_FAIL_CLOSED_HTTP_AUTH` is explicit that absence must
/// never mean permission — especially here, since infra-code#1990 ships no
/// `AuthorizationPolicy`, so app auth is currently the only gate on a meshed
/// Service. Running open is now an explicit opt-in.
fn authenticate(config: &Config, token: Option<&str>) -> bool {
    let Some(userinfo_url) = config.idp_userinfo_url.as_deref() else {
        return config.auth_disabled;
    };
    let Some(token) = token else {
        return false;
    };
    // UserInfo is a GET with the bearer; 200 means the token is valid. (No body
    // substring guessing — the status is the contract.)
    match request(
        "GET",
        userinfo_url,
        None,
        None,
        &[("authorization", format!("Bearer {token}"))],
    ) {
        Ok((status, _)) => status == 200,
        Err(_) => false,
    }
}

/// Extract the token from an `Authorization: Bearer <token>` header value.
/// Rejects tokens with whitespace/control chars so they cannot inject a header
/// when relayed to UserInfo or aivcsd.
fn bearer_token(authorization: &str) -> Option<&str> {
    let (scheme, token) = authorization.split_once(' ')?;
    let token = token.trim();
    if scheme.eq_ignore_ascii_case("bearer")
        && !token.is_empty()
        && token.bytes().all(|b| b > 0x20 && b != 0x7f)
    {
        Some(token)
    } else {
        None
    }
}

/// Register the repository with aivcsd — the single integration seam. Calls
/// aivcsd `POST /v1/repos` (forwarding the caller's bearer so aivcsd authorizes),
/// and relays aivcsd's status and body (which carries the `aivcs://` URI).
fn create_repository(
    stream: &mut TcpStream,
    config: &Config,
    name: &str,
    token: Option<&str>,
) -> io::Result<()> {
    let url = format!("{}/v1/repos", config.aivcsd_url);
    let payload = format!("{{\"name\":{}}}", json_string(name));
    let mut headers: Vec<(&str, String)> = Vec::new();
    if let Some(token) = token {
        headers.push(("authorization", format!("Bearer {token}")));
    }

    match request(
        "POST",
        &url,
        Some(&payload),
        Some("application/json"),
        &headers,
    ) {
        // Relay aivcsd's own status + body verbatim: it owns the aivcs:// URI,
        // the 201 vs 409 (already exists) distinction, and validation.
        Ok((status, body)) => respond(stream, status, body.trim()),
        Err(error) => {
            eprintln!("aivcsd call failed: {error}");
            respond_error(stream, 502, "could not reach aivcsd")
        }
    }
}

fn aivcsd_ready(config: &Config) -> bool {
    let url = format!("{}/{}", config.aivcsd_url, config.aivcsd_health_path);
    matches!(
        request("GET", &url, None, None, &[]),
        Ok((status, _)) if (200..300).contains(&status)
    )
}

// ---- request/response shaping --------------------------------------------------

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Extract the `name` string from the request JSON. Tolerant of extra fields,
/// key ordering, and whitespace (a real client sends `{"name":"x","org":"y"}`);
/// `valid_name` is the actual gatekeeper on the value. Rejects escaped values
/// (repository names never contain escapes).
fn parse_create_request(body: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    let key = text.find("\"name\"")?;
    let after_colon = text[key + 6..].trim_start().strip_prefix(':')?.trim_start();
    let value = after_colon.strip_prefix('"')?;
    let end = value.find('"')?;
    let name = &value[..end];
    if name.contains('\\') {
        return None;
    }
    Some(name.to_owned())
}

fn json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\u{:04x}", character as u32);
            }
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

// ---- minimal std-only HTTP client (plaintext over the linkerd mesh) ------------

struct Endpoint {
    host: String,
    port: u16,
    path: String,
}

fn parse_http_url(url: &str) -> Result<Endpoint, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("only http:// URLs are supported (mesh plaintext): {url}"))?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (
            host.to_string(),
            port.parse().map_err(|_| format!("bad port in {url}"))?,
        ),
        None => (authority.to_string(), 80u16),
    };
    if host.is_empty() {
        return Err(format!("empty host in {url}"));
    }
    Ok(Endpoint {
        host,
        port,
        path: path.to_string(),
    })
}

/// One plaintext HTTP/1.1 request over the mesh. `content_type` and
/// `extra_headers` are appended; the response status and body are returned.
fn request(
    method: &str,
    url: &str,
    body: Option<&str>,
    content_type: Option<&str>,
    extra_headers: &[(&str, String)],
) -> io::Result<(u16, String)> {
    let endpoint =
        parse_http_url(url).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let addr = (endpoint.host.as_str(), endpoint.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address for host"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let payload = body.unwrap_or("");
    let mut head = format!("{method} {} HTTP/1.1\r\n", endpoint.path);
    head.push_str(&format!("Host: {}\r\n", endpoint.host));
    if let Some(content_type) = content_type {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    for (name, value) in extra_headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n", payload.len()));
    head.push_str("Connection: close\r\n\r\n");
    head.push_str(payload);
    stream.write_all(head.as_bytes())?;
    stream.flush()?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> io::Result<(u16, String)> {
    let text = String::from_utf8_lossy(raw);
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no status code"))?;
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Ok((status, body))
}

// ---- HTTP responses ------------------------------------------------------------

fn respond_error(stream: &mut TcpStream, status: u16, message: &str) -> io::Result<()> {
    let body = format!("{{\"error\":{}}}", json_string(message));
    respond(stream, status, &body)
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) -> io::Result<()> {
    let reason = reason_phrase(status);
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Content",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        authenticate, bearer_token, json_string, mesh_host_for_publication, parse_create_request,
        parse_discovery_publication, parse_discovery_publications, parse_http_url, parse_response,
        select_idp_publication, userinfo_url, valid_name, Config, DiscoveryPublication,
    };

    #[test]
    fn accepts_dns_label_repository_names() {
        assert!(valid_name("agent-code-42"));
        assert!(valid_name("a"));
    }

    #[test]
    fn rejects_unsafe_repository_names() {
        for name in ["", "../escape", "Upper", "-start", "end-", "with.dot"] {
            assert!(!valid_name(name), "accepted {name:?}");
        }
        assert!(!valid_name(&"a".repeat(64)));
    }

    #[test]
    fn extracts_bearer_token_and_rejects_injectable_ones() {
        assert_eq!(bearer_token("Bearer abc.def"), Some("abc.def"));
        assert_eq!(bearer_token("bearer  abc.def "), Some("abc.def"));
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("Bearer "), None);
        // header-injection / whitespace tokens are refused
        assert_eq!(bearer_token("Bearer a\r\nX-Evil: 1"), None);
        assert_eq!(bearer_token("Bearer a b"), None);
    }

    #[test]
    fn parses_name_tolerant_of_extra_fields_and_whitespace() {
        assert_eq!(
            parse_create_request(br#"{"name":"agent-code"}"#).as_deref(),
            Some("agent-code")
        );
        assert_eq!(
            parse_create_request(
                br#" { "org":"lornu-ai" , "name" : "repo-x" , "visibility":"private" } "#
            )
            .as_deref(),
            Some("repo-x")
        );
        assert!(parse_create_request(br#"{"other":"x"}"#).is_none());
    }

    #[test]
    fn body_names_the_repo_and_escapes_it() {
        // The payload sent to aivcsd is exactly {"name":"<escaped>"}.
        assert_eq!(
            format!("{{\"name\":{}}}", json_string("repo-x")),
            r#"{"name":"repo-x"}"#
        );
    }

    #[test]
    fn parses_http_url_into_host_port_path() {
        let endpoint =
            parse_http_url("http://aivcsd.aivcs.svc.cluster.local:8080/v1/repos").unwrap();
        assert_eq!(endpoint.host, "aivcsd.aivcs.svc.cluster.local");
        assert_eq!(endpoint.port, 8080);
        assert_eq!(endpoint.path, "/v1/repos");
        assert!(parse_http_url("https://x/y").is_err());
    }

    #[test]
    fn reads_status_and_body_from_response() {
        let raw =
            b"HTTP/1.1 201 Created\r\nContent-Type: application/json\r\n\r\n{\"uri\":\"aivcs://lornu-ai/x\"}";
        let (status, body) = parse_response(raw).unwrap();
        assert_eq!(status, 201);
        assert_eq!(body, "{\"uri\":\"aivcs://lornu-ai/x\"}");
    }

    #[test]
    fn escapes_json_response_strings() {
        assert_eq!(json_string("a\"b\\c\n"), r#""a\"b\\c\n""#);
    }

    fn auth_config(idp: Option<&str>, auth_disabled: bool) -> Config {
        Config {
            listen_addr: "0.0.0.0:8080".into(),
            aivcsd_url: "http://aivcsd".into(),
            aivcsd_health_path: "healthz".into(),
            idp_userinfo_url: idp.map(str::to_string),
            auth_disabled,
        }
    }

    /// Regression guard: an unset IdP URL returned `true`, authenticating every
    /// request. #1990 ships no AuthorizationPolicy, so this was the only gate.
    #[test]
    fn missing_idp_denies_by_default() {
        assert!(!authenticate(&auth_config(None, false), Some("anything")));
        assert!(!authenticate(&auth_config(None, false), None));
    }

    /// Open operation must be an explicit, visible opt-in.
    #[test]
    fn auth_disabled_is_the_only_way_to_run_open() {
        assert!(authenticate(&auth_config(None, true), None));
    }

    /// A configured IdP with no bearer denies before any network call.
    #[test]
    fn configured_idp_without_a_bearer_denies() {
        let config = auth_config(Some("http://agent-idp/oauth/userinfo"), false);
        assert!(!authenticate(&config, None));
    }

    #[test]
    fn parses_discovery_publication_from_catalog_json() {
        let body = r#"[{
            "serviceId":"agent-idp/agent-idp",
            "capability":"identity",
            "exposureClass":"Internal",
            "identity":"agent-idp.agent-idp.svc.cluster.local",
            "port":8080,
            "cluster":"aivcs-platform",
            "namespace":"agent-idp"
        }]"#;
        let pubs = parse_discovery_publications(body).unwrap();
        assert_eq!(pubs.len(), 1);
        assert_eq!(pubs[0].service_id, "agent-idp/agent-idp");
        assert_eq!(pubs[0].port, 8080);
    }

    #[test]
    fn selects_publication_by_service_id() {
        let pubs = vec![
            DiscoveryPublication {
                service_id: "identity-broker/identity-broker".into(),
                capability: "identity".into(),
                exposure_class: "Public".into(),
                identity: "identity-broker".into(),
                port: 443,
                cluster: "aivcs-platform".into(),
                namespace: "identity-broker".into(),
            },
            DiscoveryPublication {
                service_id: "agent-idp/agent-idp".into(),
                capability: "identity".into(),
                exposure_class: "Internal".into(),
                identity: "agent-idp.agent-idp.svc.cluster.local".into(),
                port: 8080,
                cluster: "aivcs-platform".into(),
                namespace: "agent-idp".into(),
            },
        ];
        let picked = select_idp_publication(pubs, "agent-idp/agent-idp").unwrap();
        assert_eq!(picked.service_id, "agent-idp/agent-idp");
    }

    #[test]
    fn cross_cluster_mesh_host_uses_linkerd_mirror_pattern() {
        let publication = DiscoveryPublication {
            service_id: "agent-idp/agent-idp".into(),
            capability: "identity".into(),
            exposure_class: "Internal".into(),
            identity: "agent-idp.agent-idp.svc.cluster.local".into(),
            port: 8080,
            cluster: "aivcs-platform".into(),
            namespace: "agent-idp".into(),
        };
        assert_eq!(
            mesh_host_for_publication(&publication, Some("aivcs-core")),
            "agent-idp-aivcs-platform.agent-idp.svc.cluster.local"
        );
    }

    #[test]
    fn same_cluster_mesh_host_uses_identity_annotation() {
        let publication = DiscoveryPublication {
            service_id: "agent-idp/agent-idp".into(),
            capability: "identity".into(),
            exposure_class: "Internal".into(),
            identity: "agent-idp.agent-idp.svc.cluster.local".into(),
            port: 8080,
            cluster: "aivcs-platform".into(),
            namespace: "agent-idp".into(),
        };
        assert_eq!(
            mesh_host_for_publication(&publication, Some("aivcs-platform")),
            "agent-idp.agent-idp.svc.cluster.local"
        );
    }

    #[test]
    fn userinfo_url_builds_mesh_plaintext_endpoint() {
        assert_eq!(
            userinfo_url(
                "agent-idp-aivcs-platform.agent-idp.svc.cluster.local",
                8080,
                "/oauth/userinfo"
            ),
            "http://agent-idp-aivcs-platform.agent-idp.svc.cluster.local:8080/oauth/userinfo"
        );
    }

    #[test]
    fn parse_discovery_publication_object_fields() {
        let object =
            r#"{"serviceId":"agent-idp/agent-idp","port":8080,"cluster":"aivcs-platform"}"#;
        let publication = parse_discovery_publication(object).unwrap();
        assert_eq!(publication.service_id, "agent-idp/agent-idp");
        assert_eq!(publication.port, 8080);
        assert_eq!(publication.cluster, "aivcs-platform");
    }
}
