use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tracing::{error, info, warn, Level};
use uuid::Uuid;

// Public key PEM loaded at runtime

/// Maximum age of a `HumanApproval` grant before it stops counting as a valid
/// authorisation. Per the AIVCS Zero-Trust MCP Identity Model
/// (stevedores-org/aivcs#228, Feature 3.1): *"The grant must be single-use and
/// expire (e.g., within 2 hours)."* Expressed in hours rather than as a fixed
/// `Duration` so the policy value is human-greppable in audit logs.
const APPROVAL_TTL_HOURS: i64 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpClaims {
    sub: String,
    aud: String,
    tenant_id: String,
    workspace_id: String,
    agent_id: String,
    run_id: String,
    task_id: String,
    scopes: Vec<String>,
    max_risk: String,
    delegated_by: String,
    jti: String,
    exp: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuthorityRecord {
    authority_id: String,
    actor: String,
    agent_instance: String,
    on_behalf_of: String,
    tenant_id: String,
    workspace_id: String,
    repo: String,
    run_id: String,
    task_id: String,
    tool_id: String,
    tool_manifest_hash: String,
    tool_schema_version: String,
    action: String,
    payload_digest: String,
    policy_decision_id: String,
    policy_version: String,
    approval_id: Option<String>,
    risk_level: String,
    expiry: i64,
    outcome: String, // "pending" | "executed" | "denied" | "expired" | "superseded"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HumanApproval {
    approval_id: String,
    run_id: String,
    task_id: String,
    action: String,
    payload_digest: String,
    approved_by: String,
    created_at: DateTime<Utc>,
    used: bool,
}

#[derive(Debug, Deserialize)]
struct ToolCallRequest {
    tool: String,
    arguments: Value,
    repo: String,
    /// Reserved for the client-side approval-id passing pattern (#228 Feature
    /// 3.1). The gateway currently looks up approvals by the
    /// (run_id, task_id, action, payload_digest) tuple on `McpClaims`, so this
    /// field is accepted on the wire but not yet consulted. Kept on the
    /// request shape so callers can start sending it before the lookup path
    /// switches over.
    #[allow(dead_code)]
    approval_id: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────
// Memory Tool Types (Phase 2.2)
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
struct MemoryWriteRequest {
    kind: String, // "event" | "summary" | "fact" | "session_vector"
    content: Value,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    importance: Option<f64>,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    commit_id: Option<String>,
    #[serde(default)]
    key: Option<String>, // Required for session_vector
    #[serde(default)]
    ttl_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
struct MemoryQueryRequest {
    query_text: String,
    #[serde(default)]
    kinds: Vec<String>,
    #[serde(default)]
    tags_any: Vec<String>,
    #[serde(default)]
    since_ms: Option<i64>,
    #[serde(default)]
    until_ms: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default = "default_query_mode")]
    mode: String, // "filter" | "lexical" | "semantic" | "hybrid"
    #[serde(default)]
    commit_id: Option<String>,
}

#[allow(dead_code)]
fn default_query_mode() -> String {
    "hybrid".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
struct MemoryContextPackRequest {
    query_text: String,
    #[serde(default = "default_budget_tokens")]
    budget_tokens: usize,
    #[serde(default)]
    kinds: Vec<String>,
    #[serde(default)]
    commit_id: Option<String>,
    #[serde(default = "default_include_session")]
    include_session: bool,
}

#[allow(dead_code)]
fn default_budget_tokens() -> usize {
    3000
}

#[allow(dead_code)]
fn default_include_session() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
struct MemoryDeleteRequest {
    memory_id: String,
}

#[derive(Debug, Serialize)]
struct ToolCallResponse {
    status: String, // "success" | "approval_required" | "denied"
    authority_id: Option<String>,
    result: Option<Value>,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateApprovalRequest {
    run_id: String,
    task_id: String,
    action: String,
    payload_digest: String,
    approved_by: String,
}

// In-memory hot list for JTI revocation (Phase 5)
struct RevocationList {
    revoked_jtis: HashSet<String>,
    revoked_sessions: HashSet<String>,
}

struct GatewayState {
    db: aivcs_core::SurrealHandle,
    authority_records: Mutex<HashMap<String, AuthorityRecord>>,
    approvals: Mutex<HashMap<String, HumanApproval>>,
    revocations: Mutex<RevocationList>,
    public_key_pem: String,
    // Phase 2.2 Phase 3: Hybrid backend support
    backend_selector: BackendSelector,
    mom_config: Option<MomBackendConfig>,
}

#[tokio::main]
async fn main() -> std::result::Result<(), anyhow::Error> {
    aivcs_core::init_tracing(false, Level::INFO);
    info!("🚀 aivcs-mcp-gateway starting");

    // Connect to in-memory SurrealDB for development and tests
    let db = aivcs_core::SurrealHandle::setup_db().await?;

    let public_key_pem = if let Ok(pem) = std::env::var("AIVCS_MCP_VERIFICATION_KEY") {
        pem
    } else if let Ok(path) = std::env::var("AIVCS_MCP_VERIFICATION_KEY_FILE") {
        std::fs::read_to_string(path).unwrap_or_default()
    } else {
        String::new()
    };
    // Initialize hybrid backend support (Phase 2.2 Phase 3)
    let backend_selector = BackendSelector::from_env();
    let mom_config = MomBackendConfig::from_env();
    if mom_config.is_some() {
        info!(
            "MomBackend configured; backend_selector: {:?}",
            std::env::var("MEMORY_BACKEND_MODE").unwrap_or_default()
        );
    }

    let state = Arc::new(GatewayState {
        db,
        authority_records: Mutex::new(HashMap::new()),
        approvals: Mutex::new(HashMap::new()),
        revocations: Mutex::new(RevocationList {
            revoked_jtis: HashSet::new(),
            revoked_sessions: HashSet::new(),
        }),
        public_key_pem,
        backend_selector,
        mom_config,
    });

    let app = Router::new()
        .route("/health", get(health_check))
        .route("/v1/mcp/tools/list", get(list_tools))
        .route("/v1/mcp/tools/call", post(call_tool))
        .route("/v1/mcp/approvals", post(create_approval))
        .route("/v1/mcp/revocation", post(revoke_token))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8082));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("📡 listening on {}", addr);

    axum::serve(listener, app).await?;
    Ok(())
}

/// Returns true when a tool's risk level is above the token's `max_risk` ceiling.
/// Mirrors the visibility rules in `list_tools`.
fn exceeds_max_risk(risk_level: &str, max_risk: &str) -> bool {
    match risk_level {
        "read" => false,
        "write" => max_risk == "read",
        "destructive" => max_risk != "write",
        _ => true,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Memory Tool Backend Handlers (Phase 2.2 Phase 2)
// ─────────────────────────────────────────────────────────────────────────

/// Generate a deterministic embedding vector from text content.
///
/// MVP implementation uses hash-based deterministic vectors (768 dims, normalized to [-1, 1]).
/// Production should integrate with embedding models (OpenAI, Hugging Face, etc.).
fn generate_embedding(text: &str) -> Vec<f32> {
    use sha2::{Digest, Sha256};

    // Generate deterministic hash-based embedding for MVP
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let hash = hasher.finalize();

    // Create 768-dim vector from hash bytes (repeated and normalized)
    const DIM: usize = 768;
    let mut embedding = vec![0.0f32; DIM];

    for (i, &byte) in hash.iter().cycle().take(DIM).enumerate() {
        // Normalize byte (0-255) to (-1, 1) range
        embedding[i] = ((byte as f32) / 127.5) - 1.0;
    }

    embedding
}

/// Deserialize and validate memory tool request arguments from JSON Value.
#[allow(dead_code)]
fn deserialize_memory_write_args(args: &Value) -> std::result::Result<MemoryWriteRequest, String> {
    serde_json::from_value::<MemoryWriteRequest>(args.clone())
        .map_err(|e| format!("Invalid memory::write arguments: {}", e))
}

#[allow(dead_code)]
fn deserialize_memory_query_args(args: &Value) -> std::result::Result<MemoryQueryRequest, String> {
    serde_json::from_value::<MemoryQueryRequest>(args.clone())
        .map_err(|e| format!("Invalid memory::query arguments: {}", e))
}

#[allow(dead_code)]
fn deserialize_memory_context_pack_args(
    args: &Value,
) -> std::result::Result<MemoryContextPackRequest, String> {
    serde_json::from_value::<MemoryContextPackRequest>(args.clone())
        .map_err(|e| format!("Invalid memory::context_pack arguments: {}", e))
}

#[allow(dead_code)]
fn deserialize_memory_delete_args(
    args: &Value,
) -> std::result::Result<MemoryDeleteRequest, String> {
    serde_json::from_value::<MemoryDeleteRequest>(args.clone())
        .map_err(|e| format!("Invalid memory::delete arguments: {}", e))
}

/// Return a UTF-8-safe prefix no longer than `max_bytes`.
fn utf8_prefix(value: &str, max_bytes: usize) -> &str {
    let end = value.len().min(max_bytes);
    let boundary = (0..=end)
        .rev()
        .find(|&index| value.is_char_boundary(index))
        .unwrap_or(0);
    &value[..boundary]
}

/// Write a memory record to SurrealDB.
#[allow(dead_code)]
async fn memory_write_handler(
    req: MemoryWriteRequest,
    db: &aivcs_core::SurrealHandle,
) -> std::result::Result<Value, String> {
    let content_str = req.content.to_string();
    let embedding = generate_embedding(&content_str);

    let memory_record = aivcs_core::MemoryRecord {
        id: None,
        commit_id: req.commit_id.unwrap_or_else(|| "unknown".to_string()),
        key: req
            .key
            .unwrap_or_else(|| format!("{}:{}", req.kind, Uuid::new_v4())),
        content: content_str,
        embedding: Some(embedding),
        metadata: json!({
            "kind": req.kind,
            "tags": req.tags,
            "importance": req.importance,
            "confidence": req.confidence,
            "source": req.source,
            "ttl_ms": req.ttl_ms,
        }),
        created_at: Utc::now(),
    };

    let saved = db
        .save_memory(&memory_record)
        .await
        .map_err(|e| format!("Failed to save memory: {}", e))?;

    Ok(json!({
        "memory_id": format!("aivcs:mem-{}", saved.id.as_ref().map(|id| id.to_string()).unwrap_or_else(|| Uuid::new_v4().to_string())),
        "created_at_ms": Utc::now().timestamp_millis(),
        "status": "persisted",
        "commit_id": saved.commit_id,
    }))
}

/// Query memories from SurrealDB.
#[allow(dead_code)]
async fn memory_query_handler(
    req: MemoryQueryRequest,
    db: &aivcs_core::SurrealHandle,
) -> std::result::Result<Value, String> {
    let commit_id = req.commit_id.unwrap_or_else(|| "unknown".to_string());
    let memories = db
        .get_memories(&commit_id)
        .await
        .map_err(|e| format!("Failed to query memories: {}", e))?;

    // Filter by kinds if provided
    let filtered: Vec<_> = if req.kinds.is_empty() {
        memories
    } else {
        memories
            .into_iter()
            .filter(|m| {
                if let Ok(metadata) = serde_json::from_str::<Value>(&m.metadata.to_string()) {
                    metadata
                        .get("kind")
                        .and_then(|k| k.as_str())
                        .map(|kind| req.kinds.contains(&kind.to_string()))
                        .unwrap_or(false)
                } else {
                    false
                }
            })
            .collect()
    };

    // Apply limit
    let limit = req.limit.unwrap_or(10);
    let items: Vec<Value> = filtered
        .iter()
        .take(limit)
        .map(|m| {
            json!({
                "memory_id": format!("aivcs:mem-{}", m.id.as_ref().map(|id| id.to_string()).unwrap_or_else(|| "unknown".to_string())),
                "kind": m.metadata.get("kind").unwrap_or(&Value::Null),
                "content_preview": utf8_prefix(&m.content, 200),
                "created_at_ms": m.created_at.timestamp_millis(),
            })
        })
        .collect();

    Ok(json!({
        "items": items,
        "total_hits": filtered.len(),
    }))
}

/// Pack memory context within a token budget.
#[allow(dead_code)]
async fn memory_context_pack_handler(
    req: MemoryContextPackRequest,
    db: &aivcs_core::SurrealHandle,
) -> std::result::Result<Value, String> {
    let commit_id = req.commit_id.unwrap_or_else(|| "unknown".to_string());
    let memories = db
        .get_memories(&commit_id)
        .await
        .map_err(|e| format!("Failed to pack context: {}", e))?;

    // Build context segments from memories
    let segments: Vec<aivcs_core::ContextSegment> = memories
        .iter()
        .map(|m| {
            aivcs_core::ContextSegment::new(
                m.key.clone(),
                m.content.clone(),
                50, // default priority
            )
        })
        .collect();

    // Assemble context within budget
    let assembler = aivcs_core::ContextAssembler::new(req.budget_tokens);
    let assembled = assembler.assemble(segments);

    Ok(json!({
        "highlights": assembled.segments.iter().map(|s| s.label.clone()).collect::<Vec<_>>(),
        "summaries": ["Context snapshot"],
        "facts": assembled
            .segments
            .iter()
            .map(|s| utf8_prefix(&s.content, 100).to_string())
            .collect::<Vec<_>>(),
        "citations": assembled.segments.iter().map(|s| format!("aivcs:mem-{}", s.label)).collect::<Vec<_>>(),
        "token_usage": assembled.total_tokens,
    }))
}

/// Delete a memory record from SurrealDB.
#[allow(dead_code)]
async fn memory_delete_handler(
    req: MemoryDeleteRequest,
    db: &aivcs_core::SurrealHandle,
) -> std::result::Result<Value, String> {
    tracing::info!("memory::delete for {}", req.memory_id);

    let memory_key = req
        .memory_id
        .strip_prefix("aivcs:mem-")
        .unwrap_or(&req.memory_id);
    let deleted = db
        .delete_memory_by_id(memory_key)
        .await
        .map_err(|e| format!("Failed to delete memory: {}", e))?;
    if !deleted {
        return Err(format!("Memory not found: {}", req.memory_id));
    }

    Ok(json!({
        "status": "deleted",
        "memory_id": req.memory_id,
        "backend": "surreal"
    }))
}

// ─────────────────────────────────────────────────────────────────────────
// MomBackend HTTP Client (Phase 2.2 Phase 3)
// ─────────────────────────────────────────────────────────────────────────

/// Configuration for MomBackend HTTP client.
#[derive(Debug, Clone)]
struct MomBackendConfig {
    /// Base URL for Mom service (e.g., "https://mom.internal")
    base_url: String,
    /// API key for authentication (TODO Phase 2.2 Phase 4: use in HTTP client)
    #[allow(dead_code)]
    api_key: String,
    /// Timeout in seconds (TODO Phase 2.2 Phase 4: use in HTTP client)
    #[allow(dead_code)]
    timeout_secs: u64,
}

impl MomBackendConfig {
    /// Load from environment or return None if not configured.
    #[allow(dead_code)]
    fn from_env() -> Option<Self> {
        let base_url = std::env::var("MOM_BACKEND_URL").ok()?;
        let api_key = std::env::var("MOM_BACKEND_API_KEY").ok()?;
        Some(Self {
            base_url,
            api_key,
            timeout_secs: std::env::var("MOM_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10),
        })
    }
}

/// Write memory via external MomBackend HTTP service.
#[allow(dead_code)]
async fn mom_backend_write(
    config: &MomBackendConfig,
    req: &MemoryWriteRequest,
) -> std::result::Result<Value, String> {
    let url = format!("{}/api/v1/memories", config.base_url);
    let payload = json!({
        "kind": req.kind,
        "content": req.content,
        "tags": req.tags,
        "importance": req.importance,
        "confidence": req.confidence,
        "source": req.source,
        "commit_id": req.commit_id,
    });

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(config.timeout_secs))
        .send()
        .await
        .map_err(|e| format!("MomBackend HTTP error: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("MomBackend write failed with status {}", status));
    }

    let body = response
        .json::<Value>()
        .await
        .map_err(|e| format!("Failed to parse MomBackend response: {}", e))?;

    tracing::info!("MomBackend write successful: {:?}", body.get("memory_id"));
    Ok(body)
}

/// Query memories via external MomBackend HTTP service.
#[allow(dead_code)]
async fn mom_backend_query(
    config: &MomBackendConfig,
    req: &MemoryQueryRequest,
) -> std::result::Result<Value, String> {
    let url = format!("{}/api/v1/memories/search", config.base_url);
    let payload = json!({
        "query": req.query_text,
        "kinds": req.kinds,
        "limit": req.limit.unwrap_or(10),
        "mode": req.mode,
    });

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(config.timeout_secs))
        .send()
        .await
        .map_err(|e| format!("MomBackend HTTP error: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("MomBackend query failed with status {}", status));
    }

    let body = response
        .json::<Value>()
        .await
        .map_err(|e| format!("Failed to parse MomBackend response: {}", e))?;

    tracing::info!(
        "MomBackend query successful: {} hits",
        body.get("total_hits").and_then(|v| v.as_u64()).unwrap_or(0)
    );
    Ok(body)
}

/// Pack memory context via external MomBackend HTTP service.
async fn mom_backend_context_pack(
    config: &MomBackendConfig,
    req: &MemoryContextPackRequest,
) -> std::result::Result<Value, String> {
    let url = format!("{}/api/v1/memories/context_pack", config.base_url);
    let payload = json!({
        "commit_id": req.commit_id,
        "budget_tokens": req.budget_tokens,
    });

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(config.timeout_secs))
        .send()
        .await
        .map_err(|e| format!("MomBackend HTTP error: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "MomBackend context_pack failed with status {}",
            status
        ));
    }

    let body = response
        .json::<Value>()
        .await
        .map_err(|e| format!("Failed to parse MomBackend response: {}", e))?;

    tracing::info!("MomBackend context_pack successful");
    Ok(body)
}

/// Delete memory via external MomBackend HTTP service.
async fn mom_backend_delete(
    config: &MomBackendConfig,
    req: &MemoryDeleteRequest,
) -> std::result::Result<Value, String> {
    let url = format!("{}/api/v1/memories/{}", config.base_url, req.memory_id);

    let client = reqwest::Client::new();
    let response = client
        .delete(&url)
        .bearer_auth(&config.api_key)
        .timeout(std::time::Duration::from_secs(config.timeout_secs))
        .send()
        .await
        .map_err(|e| format!("MomBackend HTTP error: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("MomBackend delete failed with status {}", status));
    }

    let body = response
        .json::<Value>()
        .await
        .map_err(|e| format!("Failed to parse MomBackend response: {}", e))?;

    tracing::info!("MomBackend delete successful for {}", req.memory_id);
    Ok(body)
}

// ─────────────────────────────────────────────────────────────────────────
// Hybrid Backend Selection (Phase 2.2 Phase 3)
// ─────────────────────────────────────────────────────────────────────────

/// Selects between SurrealDB and MomBackend based on environment and configuration.
#[allow(dead_code)]
enum BackendSelector {
    /// Use local SurrealDB only.
    LocalOnly,
    /// Use MomBackend as primary, fallback to SurrealDB.
    MomPrimary,
    /// Use SurrealDB as primary, fallback to Mom.
    SurrealPrimary,
}

impl BackendSelector {
    /// Initialize selector from environment.
    #[allow(dead_code)]
    fn from_env() -> Self {
        match std::env::var("MEMORY_BACKEND_MODE").as_deref() {
            Ok("mom") => Self::MomPrimary,
            Ok("hybrid") => Self::SurrealPrimary,
            _ => Self::LocalOnly,
        }
    }
}

/// Hybrid memory write: tries primary backend, falls back to secondary.
#[allow(dead_code)]
async fn hybrid_memory_write(
    req: MemoryWriteRequest,
    db: &aivcs_core::SurrealHandle,
    selector: &BackendSelector,
    mom_config: &Option<MomBackendConfig>,
) -> std::result::Result<Value, String> {
    match selector {
        BackendSelector::LocalOnly => memory_write_handler(req, db).await,
        BackendSelector::MomPrimary => match mom_config {
            Some(config) => match mom_backend_write(config, &req).await {
                Ok(result) => Ok(result),
                Err(e) => {
                    tracing::warn!("MomBackend write failed, falling back to SurrealDB: {}", e);
                    memory_write_handler(req, db).await
                }
            },
            None => {
                tracing::warn!("MomBackend not configured, using SurrealDB");
                memory_write_handler(req, db).await
            }
        },
        BackendSelector::SurrealPrimary => match memory_write_handler(req.clone(), db).await {
            Ok(result) => Ok(result),
            Err(e) if mom_config.is_some() => {
                tracing::warn!("SurrealDB write failed, attempting MomBackend: {}", e);
                mom_backend_write(mom_config.as_ref().unwrap(), &req).await
            }
            Err(e) => Err(e),
        },
    }
}

/// Hybrid memory query: tries primary backend, falls back to secondary.
#[allow(dead_code)]
async fn hybrid_memory_query(
    req: MemoryQueryRequest,
    db: &aivcs_core::SurrealHandle,
    selector: &BackendSelector,
    mom_config: &Option<MomBackendConfig>,
) -> std::result::Result<Value, String> {
    match selector {
        BackendSelector::LocalOnly => memory_query_handler(req, db).await,
        BackendSelector::MomPrimary => match mom_config {
            Some(config) => match mom_backend_query(config, &req).await {
                Ok(result) => Ok(result),
                Err(e) => {
                    tracing::warn!("MomBackend query failed, falling back to SurrealDB: {}", e);
                    memory_query_handler(req, db).await
                }
            },
            None => {
                tracing::warn!("MomBackend not configured, using SurrealDB");
                memory_query_handler(req, db).await
            }
        },
        BackendSelector::SurrealPrimary => match memory_query_handler(req.clone(), db).await {
            Ok(result) => Ok(result),
            Err(e) if mom_config.is_some() => {
                tracing::warn!("SurrealDB query failed, attempting MomBackend: {}", e);
                mom_backend_query(mom_config.as_ref().unwrap(), &req).await
            }
            Err(e) => Err(e),
        },
    }
}

/// Hybrid memory context pack: tries primary backend, falls back to secondary.
async fn hybrid_memory_context_pack(
    req: MemoryContextPackRequest,
    db: &aivcs_core::SurrealHandle,
    selector: &BackendSelector,
    mom_config: &Option<MomBackendConfig>,
) -> std::result::Result<Value, String> {
    match selector {
        BackendSelector::LocalOnly => memory_context_pack_handler(req, db).await,
        BackendSelector::MomPrimary => match mom_config {
            Some(config) => match mom_backend_context_pack(config, &req).await {
                Ok(result) => Ok(result),
                Err(e) => {
                    tracing::warn!(
                        "MomBackend context_pack failed, falling back to SurrealDB: {}",
                        e
                    );
                    memory_context_pack_handler(req, db).await
                }
            },
            None => {
                tracing::warn!("MomBackend not configured, using SurrealDB");
                memory_context_pack_handler(req, db).await
            }
        },
        BackendSelector::SurrealPrimary => match memory_context_pack_handler(req.clone(), db).await
        {
            Ok(result) => Ok(result),
            Err(e) if mom_config.is_some() => {
                tracing::warn!(
                    "SurrealDB context_pack failed, attempting MomBackend: {}",
                    e
                );
                mom_backend_context_pack(mom_config.as_ref().unwrap(), &req).await
            }
            Err(e) => Err(e),
        },
    }
}

/// Hybrid memory delete: tries primary backend, falls back to secondary.
async fn hybrid_memory_delete(
    req: MemoryDeleteRequest,
    db: &aivcs_core::SurrealHandle,
    selector: &BackendSelector,
    mom_config: &Option<MomBackendConfig>,
) -> std::result::Result<Value, String> {
    match selector {
        BackendSelector::LocalOnly => memory_delete_handler(req, db).await,
        BackendSelector::MomPrimary => match mom_config {
            Some(config) => match mom_backend_delete(config, &req).await {
                Ok(result) => Ok(result),
                Err(e) => {
                    tracing::warn!("MomBackend delete failed, falling back to SurrealDB: {}", e);
                    memory_delete_handler(req, db).await
                }
            },
            None => {
                tracing::warn!("MomBackend not configured, using SurrealDB");
                memory_delete_handler(req, db).await
            }
        },
        BackendSelector::SurrealPrimary => match memory_delete_handler(req.clone(), db).await {
            Ok(result) => Ok(result),
            Err(e) if mom_config.is_some() => {
                tracing::warn!("SurrealDB delete failed, attempting MomBackend: {}", e);
                mom_backend_delete(mom_config.as_ref().unwrap(), &req).await
            }
            Err(e) => Err(e),
        },
    }
}
async fn health_check() -> Json<Value> {
    Json(json!({
        "status": "healthy",
        "service": "aivcs-mcp-gateway",
        "timestamp": Utc::now()
    }))
}

// Helper to validate headers and token
fn validate_auth(
    headers: &HeaderMap,
    revocations: &RevocationList,
    public_key_pem: &str,
) -> std::result::Result<McpClaims, (StatusCode, Json<Value>)> {
    // Check MCP headers
    let version = headers
        .get("MCP-Protocol-Version")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let session_id = headers
        .get("Mcp-Session-Id")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if version.is_empty() || session_id.is_empty() {
        warn!("Missing required MCP headers");
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing_required_headers" })),
        ));
    }

    if revocations.revoked_sessions.contains(session_id) {
        warn!("Session {} is revoked", session_id);
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "session_revoked" })),
        ));
    }

    // Check authorization token
    let auth_header = headers
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if !auth_header.starts_with("Bearer ") {
        warn!("Missing or malformed Authorization header");
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "missing_bearer_token" })),
        ));
    }

    let token = auth_header.trim_start_matches("Bearer ").trim();

    // Verify JWT
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&["https://mcp.aivcs.lornu.ai"]);

    if public_key_pem.trim().is_empty() {
        error!("Verification key not configured: set AIVCS_MCP_VERIFICATION_KEY or AIVCS_MCP_VERIFICATION_KEY_FILE");
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "verification_key_not_configured" })),
        ));
    }

    let dec_key = DecodingKey::from_rsa_pem(public_key_pem.as_bytes()).map_err(|e| {
        error!("Decoding key load failed: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "internal_error" })),
        )
    })?;

    let token_data = decode::<McpClaims>(token, &dec_key, &validation).map_err(|e| {
        warn!("JWT validation failed: {}", e);
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid_token", "detail": e.to_string() })),
        )
    })?;

    let claims = token_data.claims;

    // Check if token JTI is revoked
    if revocations.revoked_jtis.contains(&claims.jti) {
        warn!("Token JTI {} is revoked", claims.jti);
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "token_revoked" })),
        ));
    }

    Ok(claims)
}

async fn list_tools(
    State(state): State<Arc<GatewayState>>,
    headers: HeaderMap,
) -> axum::response::Response {
    let revocations = state.revocations.lock().unwrap();
    let claims = match validate_auth(&headers, &revocations, &state.public_key_pem) {
        Ok(c) => c,
        Err(err) => return err.into_response(),
    };

    info!("Listing tools for agent: {}", claims.agent_id);

    // Dynamic filtering based on max risk and scopes in the token
    let mut tools = vec![json!({
        "name": "repo.diff.read",
        "description": "Read file diffs in the repository",
        "risk_level": "read",
        "required_scopes": ["repo.diff.read"]
    })];

    if claims.scopes.contains(&"repo.diff.write".to_string()) && claims.max_risk != "read" {
        tools.push(json!({
            "name": "repo.diff.write",
            "description": "Write code diff changes",
            "risk_level": "write",
            "required_scopes": ["repo.diff.write"]
        }));
    }

    if claims.scopes.contains(&"repo.merge.execute".to_string()) && claims.max_risk == "write" {
        tools.push(json!({
            "name": "repo.merge.execute",
            "description": "Merge feature branches with human guardrails",
            "risk_level": "destructive",
            "required_scopes": ["repo.merge.execute"]
        }));
    }

    Json(json!({ "tools": tools })).into_response()
}

#[axum::debug_handler]
async fn call_tool(
    State(state): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Json(req): Json<Value>,
) -> axum::response::Response {
    let claims = {
        let revocations = state.revocations.lock().unwrap();
        match validate_auth(&headers, &revocations, &state.public_key_pem) {
            Ok(c) => c,
            Err(err) => return err.into_response(),
        }
    };

    let req: ToolCallRequest = match serde_json::from_value(req) {
        Ok(r) => r,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": err.to_string()})),
            )
                .into_response()
        }
    };

    info!(
        "Agent {} requesting tool {} on repo {} (run={} task={})",
        claims.agent_id, req.tool, req.repo, claims.run_id, claims.task_id
    );

    // Step 1: Resolve risk level and scope requirements for tool
    let (risk_level, required_scope) = match req.tool.as_str() {
        "repo.diff.read" => ("read", "repo.diff.read"),
        "repo.diff.write" => ("write", "repo.diff.write"),
        "repo.merge.execute" => ("destructive", "repo.merge.execute"),
        // Memory tools (Phase 2.2)
        "memory::write" => ("write", "memory.write"),
        "memory::query" => ("read", "memory.read"),
        "memory::context_pack" => ("read", "memory.read"),
        "memory::delete" => ("destructive", "memory.delete"),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "unknown_tool" })),
            )
                .into_response()
        }
    };

    // Step 2: Validate token scope and max risk
    if !claims.scopes.contains(&required_scope.to_string()) {
        return Json(ToolCallResponse {
            status: "denied".to_string(),
            authority_id: None,
            result: None,
            reason: Some(format!("Missing required scope: {}", required_scope)),
        })
        .into_response();
    }

    if exceeds_max_risk(risk_level, &claims.max_risk) {
        return Json(ToolCallResponse {
            status: "denied".to_string(),
            authority_id: None,
            result: None,
            reason: Some("Tool risk level exceeds maximum allowed risk level".to_string()),
        })
        .into_response();
    }

    // Step 3: Compute canonical payload digest to lock down tool arguments (rpelevin requirement)
    let payload_string = serde_json::to_string(&req.arguments).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(req.tool.as_bytes());
    hasher.update(payload_string.as_bytes());
    let payload_digest = hex::encode(hasher.finalize());

    // Step 4: Policy & Human Approval Check
    //
    // For destructive tools we need a `HumanApproval` matching the
    // (run_id, task_id, action, payload_digest) tuple that is:
    //   - not already consumed (`used == false`),
    //   - not older than `APPROVAL_TTL_HOURS` (#228 Feature 3.1).
    //
    // We deliberately classify "stale or consumed" separately from
    // "no approval ever recorded" so operators can tell why their grant
    // didn't take effect from the response `reason` alone, without
    // grepping the audit log.
    let mut active_approval_id = None;
    let mut stale_status: Option<&'static str> = None;
    if risk_level == "destructive" {
        let now = Utc::now();
        let ttl = chrono::Duration::hours(APPROVAL_TTL_HOURS);
        let mut approvals = state.approvals.lock().unwrap();

        // First pass: pick a fresh, unused matching approval and consume it.
        let fresh_match_id = approvals
            .values()
            .find(|a| {
                a.run_id == claims.run_id
                    && a.task_id == claims.task_id
                    && a.action == req.tool
                    && a.payload_digest == payload_digest
                    && !a.used
                    && (now - a.created_at) <= ttl
            })
            .map(|a| a.approval_id.clone());

        if let Some(id) = fresh_match_id {
            let appr = approvals.get_mut(&id).expect("just found this id");
            info!("Valid human approval found: {}", id);
            appr.used = true; // Mark as single-use consumed
            active_approval_id = Some(id);
        } else {
            // Second pass: classify *any* matching approval so the reason
            // tells the operator what actually went wrong.
            stale_status = approvals
                .values()
                .filter(|a| {
                    a.run_id == claims.run_id
                        && a.task_id == claims.task_id
                        && a.action == req.tool
                        && a.payload_digest == payload_digest
                })
                .map(|a| {
                    if a.used {
                        "consumed"
                    } else if (now - a.created_at) > ttl {
                        "expired"
                    } else {
                        // Should be unreachable given the first pass, but
                        // tag it as "stale" rather than panic.
                        "stale"
                    }
                })
                .next();
        }
    }

    if risk_level == "destructive" && active_approval_id.is_none() {
        let outcome_tag = match stale_status {
            Some("expired") => "expired",
            Some("consumed") => "consumed",
            _ => "escalated",
        };
        warn!(outcome = outcome_tag, "Tool requires fresh human approval");

        // Record the policy decision to SurrealDB with the precise outcome
        // (escalated / expired / consumed) so the audit trail distinguishes
        // a never-approved request from one whose grant aged out or was
        // already used.
        let mut decision = aivcs_core::DecisionRecord::new(
            Uuid::new_v4().to_string(),
            "".to_string(),
            format!("run:{}/task:{}", claims.run_id, claims.task_id),
            req.tool.clone(),
            match outcome_tag {
                "expired" => format!(
                    "Existing approval expired (TTL = {}h); a fresh approval is required",
                    APPROVAL_TTL_HOURS
                ),
                "consumed" => {
                    "Existing approval already consumed; a fresh approval is required".to_string()
                }
                _ => "Requires human approval before execution".to_string(),
            },
            1.0,
        );
        decision.alternatives = vec!["Deny".to_string(), "Escalate".to_string()];
        decision.outcome = Some(outcome_tag.to_string());
        let _ = state.db.save_decision(&decision).await;

        let reason = match outcome_tag {
            "expired" => format!(
                "Existing human approval expired (TTL = {}h). Request a fresh approval. Payload digest: {}",
                APPROVAL_TTL_HOURS, payload_digest
            ),
            "consumed" => format!(
                "Existing human approval already consumed. Request a fresh approval. Payload digest: {}",
                payload_digest
            ),
            _ => format!(
                "Human approval required for action. Payload digest: {}",
                payload_digest
            ),
        };

        return Json(ToolCallResponse {
            status: "approval_required".to_string(),
            authority_id: None,
            result: None,
            reason: Some(reason),
        })
        .into_response();
    }

    // Step 5: Mint One-Use Authority Record (rpelevin invariant)
    let authority_id = format!("auth-{}", Uuid::new_v4());
    let policy_decision_id = Uuid::new_v4().to_string();

    let authority = AuthorityRecord {
        authority_id: authority_id.clone(),
        actor: claims.sub.clone(),
        agent_instance: claims.agent_id.clone(),
        on_behalf_of: claims.delegated_by.clone(),
        tenant_id: claims.tenant_id.clone(),
        workspace_id: claims.workspace_id.clone(),
        repo: req.repo.clone(),
        run_id: claims.run_id.clone(),
        task_id: claims.task_id.clone(),
        tool_id: req.tool.clone(),
        tool_manifest_hash: "sha256-manifest-placeholder".to_string(),
        tool_schema_version: "2025-06-18".to_string(),
        action: req.tool.clone(),
        payload_digest: payload_digest.clone(),
        policy_decision_id: policy_decision_id.clone(),
        policy_version: "1.0.0".to_string(),
        approval_id: active_approval_id,
        risk_level: risk_level.to_string(),
        expiry: (Utc::now() + chrono::Duration::minutes(5)).timestamp(),
        outcome: "executed".to_string(), // single-use execution consumes it immediately
    };

    // Save authority record
    state
        .authority_records
        .lock()
        .unwrap()
        .insert(authority_id.clone(), authority);

    // Save policy decision to SurrealDB
    let mut decision = aivcs_core::DecisionRecord::new(
        policy_decision_id,
        "".to_string(),
        format!("run:{}/task:{}", claims.run_id, claims.task_id),
        req.tool.clone(),
        format!(
            "Authorized tool execution with authority_id={}",
            authority_id
        ),
        1.0,
    );
    decision.alternatives = vec!["Allow".to_string()];
    decision.outcome = Some("allowed".to_string());
    let _ = state.db.save_decision(&decision).await;

    // Execute tool (memory tools use real backend, repo tools still mocked)
    let result = match req.tool.as_str() {
        "repo.diff.read" => json!({ "diff": "--- a/src/main.rs\n+++ b/src/main.rs\n" }),
        "repo.diff.write" => json!({ "status": "changes_written" }),
        "repo.merge.execute" => {
            json!({ "status": "merged", "commit_id": "sha256-merge-placeholder" })
        }
        // Memory tools (Phase 2.2) — hybrid backend integration (Phase 2.2 Phase 3)
        "memory::write" => match deserialize_memory_write_args(&req.arguments) {
            Ok(args) => match hybrid_memory_write(
                args,
                &state.db,
                &state.backend_selector,
                &state.mom_config,
            )
            .await
            {
                Ok(response) => response,
                Err(e) => {
                    warn!("memory::write handler failed: {}", e);
                    return Json(ToolCallResponse {
                        status: "denied".to_string(),
                        authority_id: Some(authority_id),
                        result: None,
                        reason: Some(e),
                    })
                    .into_response();
                }
            },
            Err(e) => {
                warn!("memory::write argument validation failed: {}", e);
                return Json(ToolCallResponse {
                    status: "denied".to_string(),
                    authority_id: Some(authority_id),
                    result: None,
                    reason: Some(e),
                })
                .into_response();
            }
        },
        "memory::query" => match deserialize_memory_query_args(&req.arguments) {
            Ok(args) => match hybrid_memory_query(
                args,
                &state.db,
                &state.backend_selector,
                &state.mom_config,
            )
            .await
            {
                Ok(response) => response,
                Err(e) => {
                    warn!("memory::query handler failed: {}", e);
                    return Json(ToolCallResponse {
                        status: "denied".to_string(),
                        authority_id: Some(authority_id),
                        result: None,
                        reason: Some(e),
                    })
                    .into_response();
                }
            },
            Err(e) => {
                warn!("memory::query argument validation failed: {}", e);
                return Json(ToolCallResponse {
                    status: "denied".to_string(),
                    authority_id: Some(authority_id),
                    result: None,
                    reason: Some(e),
                })
                .into_response();
            }
        },
        "memory::context_pack" => match deserialize_memory_context_pack_args(&req.arguments) {
            Ok(args) => match hybrid_memory_context_pack(
                args,
                &state.db,
                &state.backend_selector,
                &state.mom_config,
            )
            .await
            {
                Ok(response) => response,
                Err(e) => {
                    warn!("memory::context_pack handler failed: {}", e);
                    return Json(ToolCallResponse {
                        status: "denied".to_string(),
                        authority_id: Some(authority_id),
                        result: None,
                        reason: Some(e),
                    })
                    .into_response();
                }
            },
            Err(e) => {
                warn!("memory::context_pack argument validation failed: {}", e);
                return Json(ToolCallResponse {
                    status: "denied".to_string(),
                    authority_id: Some(authority_id),
                    result: None,
                    reason: Some(e),
                })
                .into_response();
            }
        },
        "memory::delete" => match deserialize_memory_delete_args(&req.arguments) {
            Ok(args) => match hybrid_memory_delete(
                args,
                &state.db,
                &state.backend_selector,
                &state.mom_config,
            )
            .await
            {
                Ok(response) => response,
                Err(e) => {
                    warn!("memory::delete handler failed: {}", e);
                    return Json(ToolCallResponse {
                        status: "denied".to_string(),
                        authority_id: Some(authority_id),
                        result: None,
                        reason: Some(e),
                    })
                    .into_response();
                }
            },
            Err(e) => {
                warn!("memory::delete argument validation failed: {}", e);
                return Json(ToolCallResponse {
                    status: "denied".to_string(),
                    authority_id: Some(authority_id),
                    result: None,
                    reason: Some(e),
                })
                .into_response();
            }
        },
        _ => json!({}),
    };

    info!("Tool {} executed successfully", req.tool);

    Json(ToolCallResponse {
        status: "success".to_string(),
        authority_id: Some(authority_id),
        result: Some(result),
        reason: None,
    })
    .into_response()
}

async fn create_approval(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<CreateApprovalRequest>,
) -> Json<Value> {
    let approval_id = format!("appr-{}", Uuid::new_v4());
    let approval = HumanApproval {
        approval_id: approval_id.clone(),
        run_id: req.run_id,
        task_id: req.task_id,
        action: req.action,
        payload_digest: req.payload_digest,
        approved_by: req.approved_by,
        created_at: Utc::now(),
        used: false,
    };

    state
        .approvals
        .lock()
        .unwrap()
        .insert(approval_id.clone(), approval);

    info!("Human approval registered: {}", approval_id);

    Json(json!({
        "status": "approval_created",
        "approval_id": approval_id
    }))
}

async fn revoke_token(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<Value>,
) -> Json<Value> {
    let mut revocations = state.revocations.lock().unwrap();

    if let Some(jti) = req.get("jti").and_then(|v| v.as_str()) {
        revocations.revoked_jtis.insert(jti.to_string());
        info!("Revoked JTI: {}", jti);
    }

    if let Some(session_id) = req.get("session_id").and_then(|v| v.as_str()) {
        revocations.revoked_sessions.insert(session_id.to_string());
        info!("Revoked session ID: {}", session_id);
    }

    Json(json!({ "status": "revocation_updated" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::EncodingKey;
    use tower::ServiceExt;

    fn mint_test_token(max_risk: &str, scopes: Vec<&str>) -> String {
        // TEST-ONLY FIXTURE KEY. Used solely to sign tokens within this test
        // module (#[cfg(test)], compiled out of release builds); never loaded
        // from, or written to, any runtime config, secret store, or
        // production path. Do not reuse in production. Kept as a static blob
        // rather than generated at test time to avoid adding an RSA-keygen
        // dependency to an already fragile build.
        let priv_b64 = "LS0tLS1CRUdJTiBQUklWQVRFIEtFWS0tLS0tCk1JSUV2Z0lCQURBTkJna3Foa2lHOXcwQkFRRUZBQVNDQktnd2dnU2tBZ0VBQW9JQkFRRGZYMU5uMDZ3dEVreXkKamQycDdIeVZlSUZ0MzRJNGhhcVNXSFp1MW1NQ0ttWjdsTUJJcVMxWWI2bFpBNzZhZGk3S2JkVG4zSVpBdUtyaAprbElSWThDUTNPWDl5aGphcmY1SzQwMkxVRkdDMUFjU3h3cHMwSW9QTFVOUWNBekNQWllwZDhtWTJ1SmJVQks1ClBGZXhmVzhRVkQwTnl1UGlKQmJwS0JqYTAwVzJ0T2Z5MnF0RHpTeVRHSmE4ekkwdHZoZXBabEhrRytITHFZL0gKQWVyRlFpS3FqZ1dTcUZERjhHemJERUl5TUJYOWU5RHJUZkttZHJ1b2RKOWVzeFlFeWRpRUtMSmFTT3psWU5xUQpxN1hvNVlIYkZoQ0UwdGlSRkNSdjQwMGtTaHVCYzM3RGt4SDZ0NER1SFV1SDNTdmVENEFFM0gvM3dmWmxsUWxQCm9qNG56bTlwQWdNQkFBRUNnZ0VBYVNJMjRZRnhZbTFnaUJIWnFPYlQ1STRwYlF0c0FTcDRsQlRxK1ZRU21heFgKUEFkUlVXRy9KQWE2VUZsQTF2YVZJMVg2aFg3MytYSnhpMllSRm5vNjRuUDJGRE9RNnl4RnFmMitPN244QTNYRQpOb1JVVmM3NWpCY2p2YkpmYnZVSnZrN1JKZzZ2eDRheXFWakxkWkN5TzU5S2RUbHZkTHJEeGMzSGxRY25vc3cxCnUreXcwbHIxL2ZiVklzR1hxVzREMy8zaGpIZlhlK09oMzEwalNkMnIzblBCV0tIMjBEUDZrSlZVcW1wU2YvUnoKRGFhYURkaXFYOVl0NGNrMWJxQ2NtaDQrSEpIcHYyaFM4TU05ZWFjVE1kdGZyYUx2MDR0V1FDOHZJVm5rNTF0bgpyOG13NW5mbWhkczYzMytpb0FjdHVrdEZMRGVHNlVJdHRYK1FqMHZGQVFLQmdRRDNZb1c4RmUrKzREWjVUMDZpCnZlRUUwVXpZWkljTWo0WHFEOTVhclFITU5jWmxaRXY1Mk14b3RGeStIcytWNzUyVUgwdkdORW8yVlA3YXNNb08KWEM5OHRhOFZhSzJ0bzFzUnZyL0NNYm9xV3E1N01YMkRveU53ZGVqY21jbEZxa1VyY0k0UXRtdmRxcmoxOTRRMwpOekJOOXVhZ3J0WXQ3aUR3ODdvYkFLZGNFUUtCZ1FEbkpycGc3K3U3emRVZ0w3MHo1TkhrVmJnNk1EN0ZDWFJhCkJIOGRnYUMyR0tDblpmU0EzcHhBQUQ2QW05TmpWaGpudGpOb1VEbE14VDBlb0pzU3BmSmpKMVk5VkVTcWpvcWUKdTh0VmRRazFSSDdmL29KckFHZWlOQSs1aFVRbUZaYk9ENW5yK0M4dDlvcmkvbzdSS3JkUHAzajBwc2U4K1UvUgp0OVgySFNRVjJRS0JnUURBdzdHSHhPUWlyTjFsbTRtZndDdGxzSjJiaElIREpOYnBjdUlGY0FnVmt0VjhUakh4CmhxQ0krZm5HWDRYTHhJSGFXS1NYMWtqNW16TlhQeWpERmN3ZTlnZHV2RG1STXRnVXRMa0JYZlE5YXBuSS91QloKd2JZc3ZJUHQyWnQvUUZWVHF3bllOZjFKSmUyb0krMlBoTjZMOGRiMTRDYWVkWTZQa3FzeXZVaXJzUUtCZ1FDQgp3S2VXaXBiVkVURzFvNWFkYnJDemI3cStUeDZ0RkNXUDhqNDRuZTlNeUg1RitXRktoYXRIOGRzajdsUzJ5am1vCnVBb2JZQTBLSHgyejk0dVU2RG9ybG9VK1gvTTdtbEFOMG5UTlA2a3ZrWWQyelRNQVJYWG5BenBnZFlKUHJvYTgKbk4xV0xEYXZvbGxNR29Db3dVV3RIT0UwMC9vREJoL2NKVW1ob2JJRDRRS0JnRjVHbFlhMU96MWd2Q2cxQy9YbQp6RUJySHBrQnlRMTMvQWZzNi91VDJPb1BTM0hNUjZBQUlOeThieXZKL0trYWN1elliSjhPRGlXVTIvZFR0SldKCksxR2NPZGdUQUFEQmtQNTc5NXNOVmZKRjk1OUMyRnhnQnA1U0w4K0Mwc2lNNy9wUlFZMnF4OFZydEh5VUF4S2cKcFhQT2RCQ1g3TXZxdXB0VEN5dEt1a0ltCi0tLS0tRU5EIFBSSVZBVEUgS0VZLS0tLS0=";
        let private_key_pem = String::from_utf8(
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, priv_b64).unwrap(),
        )
        .unwrap();
        let private_key = EncodingKey::from_rsa_pem(private_key_pem.as_bytes()).unwrap();
        let exp = (Utc::now() + chrono::Duration::minutes(5)).timestamp() as usize;

        let claims = McpClaims {
            sub: "agent_instance:test-agent-123".to_string(),
            aud: "https://mcp.aivcs.lornu.ai".to_string(),
            tenant_id: "tenant-default".to_string(),
            workspace_id: "ws-default".to_string(),
            agent_id: "agent-opt".to_string(),
            run_id: "run-test-run".to_string(),
            task_id: "task-test-task".to_string(),
            scopes: scopes.into_iter().map(|s| s.to_string()).collect(),
            max_risk: max_risk.to_string(),
            delegated_by: "policy:builder-feature-branch-write".to_string(),
            jti: "test-jti-1".to_string(),
            exp,
        };

        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some("key-id-aivcs".to_string());

        jsonwebtoken::encode(&header, &claims, &private_key).unwrap()
    }

    /// Build a fresh gateway router + return the shared state so tests can
    /// inspect or mutate the in-memory approvals/revocations table directly
    /// (needed e.g. to back-date a `HumanApproval` past the TTL).
    async fn setup_router_with_state() -> (Router, Arc<GatewayState>) {
        let db = aivcs_core::SurrealHandle::setup_db().await.unwrap();
        let pub_b64 = "LS0tLS1CRUdJTiBQVUJMSUMgS0VZLS0tLS0KTUlJQklqQU5CZ2txaGtpRzl3MEJBUUVGQUFPQ0FROEFNSUlCQ2dLQ0FRRUEzMTlUWjlPc0xSSk1zbzNkcWV4OApsWGlCYmQrQ09JV3FrbGgyYnRaakFpcG1lNVRBU0trdFdHK3BXUU8rbW5ZdXltM1U1OXlHUUxpcTRaSlNFV1BBCmtOemwvY29ZMnEzK1N1Tk5pMUJSZ3RRSEVzY0tiTkNLRHkxRFVIQU13ajJXS1hmSm1OcmlXMUFTdVR4WHNYMXYKRUZROURjcmo0aVFXNlNnWTJ0TkZ0clRuOHRxclE4MHNreGlXdk15TkxiNFhxV1pSNUJ2aHk2bVB4d0hxeFVJaQpxbzRGa3FoUXhmQnMyd3hDTWpBVi9YdlE2MDN5cG5hN3FIU2ZYck1XQk1uWWhDaXlXa2pzNVdEYWtLdTE2T1dCCjJ4WVFoTkxZa1JRa2IrTk5KRW9iZ1hOK3c1TVIrcmVBN2gxTGg5MHIzZytBQk54Lzk4SDJaWlVKVDZJK0o4NXYKYVFJREFRQUIKLS0tLS1FTkQgUFVCTElDIEtFWS0tLS0t";
        let public_key_pem = String::from_utf8(
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, pub_b64).unwrap(),
        )
        .unwrap();
        let state = Arc::new(GatewayState {
            db,
            authority_records: Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
            revocations: Mutex::new(RevocationList {
                revoked_jtis: HashSet::new(),
                revoked_sessions: HashSet::new(),
            }),
            public_key_pem,
            backend_selector: BackendSelector::LocalOnly,
            mom_config: None,
        });

        let router = Router::new()
            .route("/v1/mcp/tools/list", get(list_tools))
            .route("/v1/mcp/tools/call", post(call_tool))
            .route("/v1/mcp/approvals", post(create_approval))
            .route("/v1/mcp/revocation", post(revoke_token))
            .with_state(state.clone());

        (router, state)
    }

    /// Convenience wrapper for the existing tests that don't need to poke at
    /// state directly.
    async fn setup_router() -> Router {
        setup_router_with_state().await.0
    }

    #[tokio::test]
    async fn test_gateway_zero_trust_mcp_validation() {
        let app = setup_router().await;

        // 1. Check list tools with valid credentials
        let token = mint_test_token("write", vec!["repo.diff.read", "repo.diff.write"]);

        let req = axum::http::Request::builder()
            .uri("/v1/mcp/tools/list")
            .header("Authorization", format!("Bearer {}", token))
            .header("MCP-Protocol-Version", "2025-06-18")
            .header("Mcp-Session-Id", "session-1")
            .body(axum::body::Body::empty())
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(json["tools"].as_array().unwrap().len() >= 2);

        // 2. Call read tool - should succeed
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/mcp/tools/call")
            .header("Authorization", format!("Bearer {}", token))
            .header("MCP-Protocol-Version", "2025-06-18")
            .header("Mcp-Session-Id", "session-1")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&json!({
                    "tool": "repo.diff.read",
                    "arguments": {},
                    "repo": "stevedores-org/aivcs"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let res_json: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res_json["status"], "success");
        assert!(res_json["authority_id"].as_str().is_some());

        // 3. Call destructive tool without human approval - should escalate
        let token_merge = mint_test_token("write", vec!["repo.merge.execute"]);
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/mcp/tools/call")
            .header("Authorization", format!("Bearer {}", token_merge))
            .header("MCP-Protocol-Version", "2025-06-18")
            .header("Mcp-Session-Id", "session-1")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&json!({
                    "tool": "repo.merge.execute",
                    "arguments": {
                        "branch": "develop"
                    },
                    "repo": "stevedores-org/aivcs"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let res_json: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res_json["status"], "approval_required");

        // 4. Register human approval and call again - should succeed
        let payload_string = serde_json::to_string(&json!({ "branch": "develop" })).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(b"repo.merge.execute");
        hasher.update(payload_string.as_bytes());
        let payload_digest = hex::encode(hasher.finalize());

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/mcp/approvals")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&json!({
                    "run_id": "run-test-run",
                    "task_id": "task-test-task",
                    "action": "repo.merge.execute",
                    "payload_digest": payload_digest,
                    "approved_by": "human:supervisor@lornu.ai"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Call again with approval
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/mcp/tools/call")
            .header("Authorization", format!("Bearer {}", token_merge))
            .header("MCP-Protocol-Version", "2025-06-18")
            .header("Mcp-Session-Id", "session-1")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&json!({
                    "tool": "repo.merge.execute",
                    "arguments": {
                        "branch": "develop"
                    },
                    "repo": "stevedores-org/aivcs"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let res_json: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res_json["status"], "success");
        assert!(res_json["authority_id"].as_str().is_some());

        // 5. Replaying the exact same request should fail because human approval is single-use
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/mcp/tools/call")
            .header("Authorization", format!("Bearer {}", token_merge))
            .header("MCP-Protocol-Version", "2025-06-18")
            .header("Mcp-Session-Id", "session-1")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&json!({
                    "tool": "repo.merge.execute",
                    "arguments": {
                        "branch": "develop"
                    },
                    "repo": "stevedores-org/aivcs"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let res_json: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res_json["status"], "approval_required");
    }

    #[tokio::test]
    async fn test_gateway_max_risk_escalation() {
        let app = setup_router().await;

        // Token with max_risk="read" but having merge scope
        let token = mint_test_token("read", vec!["repo.merge.execute"]);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/mcp/tools/call")
            .header("Authorization", format!("Bearer {}", token))
            .header("MCP-Protocol-Version", "2025-06-18")
            .header("Mcp-Session-Id", "session-1")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&json!({
                    "tool": "repo.merge.execute",
                    "arguments": {
                        "branch": "develop"
                    },
                    "repo": "stevedores-org/aivcs"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let res_json: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res_json["status"], "denied");
        assert!(res_json["reason"]
            .as_str()
            .unwrap()
            .contains("exceeds maximum allowed risk"));
    }

    #[tokio::test]
    async fn test_gateway_max_risk_blocks_write_tool() {
        let app = setup_router().await;

        // Token with max_risk="read" but write scope — must not bypass list_tools filtering.
        let token = mint_test_token("read", vec!["repo.diff.write"]);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/mcp/tools/call")
            .header("Authorization", format!("Bearer {}", token))
            .header("MCP-Protocol-Version", "2025-06-18")
            .header("Mcp-Session-Id", "session-1")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&json!({
                    "tool": "repo.diff.write",
                    "arguments": {},
                    "repo": "stevedores-org/aivcs"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let res_json: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res_json["status"], "denied");
        assert!(res_json["reason"]
            .as_str()
            .unwrap()
            .contains("exceeds maximum allowed risk"));
    }

    #[tokio::test]
    async fn test_gateway_revocation() {
        let app = setup_router().await;
        let token = mint_test_token("write", vec!["repo.diff.read"]);

        // Revoke the JTI
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/mcp/revocation")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&json!({
                    "jti": "test-jti-1"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Try to call tool - should fail with 401 token_revoked
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/mcp/tools/call")
            .header("Authorization", format!("Bearer {}", token))
            .header("MCP-Protocol-Version", "2025-06-18")
            .header("Mcp-Session-Id", "session-1")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&json!({
                    "tool": "repo.diff.read",
                    "arguments": {},
                    "repo": "stevedores-org/aivcs"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let res_json: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res_json["error"], "token_revoked");
    }

    /// Issue [#228](aivcs://stevedores-org/aivcs/issues/228)
    /// Feature 3.1 requires human approvals to expire — the AC says
    /// *"The grant must be single-use and expire (e.g., within 2 hours)."*
    ///
    /// This test:
    /// 1. Posts a human approval via the public `/v1/mcp/approvals` endpoint.
    /// 2. Reaches into the in-memory store and back-dates the approval's
    ///    `created_at` past `APPROVAL_TTL_HOURS`.
    /// 3. Calls the destructive tool with the same `(run_id, task_id, action,
    ///    payload_digest)` tuple and asserts the gateway responds with
    ///    `approval_required` whose `reason` names the *expired* path
    ///    (not the never-approved one) so operators can tell the two cases
    ///    apart without grepping the audit log.
    #[tokio::test]
    async fn test_gateway_human_approval_ttl_expiry() {
        let (app, state) = setup_router_with_state().await;
        let token_merge = mint_test_token("write", vec!["repo.merge.execute"]);

        // 1. Register a fresh human approval for the merge action.
        let payload_string = serde_json::to_string(&json!({ "branch": "develop" })).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(b"repo.merge.execute");
        hasher.update(payload_string.as_bytes());
        let payload_digest = hex::encode(hasher.finalize());

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/mcp/approvals")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&json!({
                    "run_id": "run-test-run",
                    "task_id": "task-test-task",
                    "action": "repo.merge.execute",
                    "payload_digest": payload_digest,
                    "approved_by": "human:supervisor@lornu.ai"
                }))
                .unwrap(),
            ))
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // 2. Back-date the approval's `created_at` past the TTL. Touching
        // the in-memory state directly is the simplest test seam — the
        // alternative (sleeping > 2h or wiring a time source) is impractical.
        {
            let mut approvals = state.approvals.lock().unwrap();
            assert_eq!(approvals.len(), 1, "exactly one approval should be staged");
            for appr in approvals.values_mut() {
                appr.created_at = Utc::now() - chrono::Duration::hours(APPROVAL_TTL_HOURS + 1);
            }
        }

        // 3. Call the destructive tool — must be rejected with the
        //    `expired`-specific reason.
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/mcp/tools/call")
            .header("Authorization", format!("Bearer {}", token_merge))
            .header("MCP-Protocol-Version", "2025-06-18")
            .header("Mcp-Session-Id", "session-1")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&json!({
                    "tool": "repo.merge.execute",
                    "arguments": { "branch": "develop" },
                    "repo": "stevedores-org/aivcs"
                }))
                .unwrap(),
            ))
            .unwrap();
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let res_json: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res_json["status"], "approval_required");

        // The reason must explicitly call out "expired" (not the
        // never-approved phrasing) so the caller can act differently.
        let reason = res_json["reason"].as_str().unwrap_or("");
        assert!(
            reason.to_lowercase().contains("expired"),
            "expired-approval reason should contain 'expired'; got: {reason}"
        );
        assert!(
            reason.contains(&format!("{}h", APPROVAL_TTL_HOURS)),
            "expired-approval reason should surface the TTL; got: {reason}"
        );

        // And the in-memory approval must NOT have been marked `used` —
        // we never consumed it, we rejected it.
        let approvals = state.approvals.lock().unwrap();
        for appr in approvals.values() {
            assert!(
                !appr.used,
                "expired approval must not be marked as consumed"
            );
        }
    }

    #[tokio::test]
    async fn test_memory_tools_dispatch() {
        let app = setup_router().await;
        let token = mint_test_token(
            "write",
            vec!["memory.write", "memory.read", "memory.delete"],
        );

        // Test memory::write dispatch
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/mcp/tools/call")
            .header("Authorization", format!("Bearer {}", token))
            .header("MCP-Protocol-Version", "2025-06-18")
            .header("Mcp-Session-Id", "session-1")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&json!({
                    "tool": "memory::write",
                    "arguments": {
                        "kind": "session_vector",
                        "content": { "vector": [0.1, 0.2, 0.3] },
                        "commit_id": "abc123"
                    },
                    "repo": "stevedores-org/aivcs"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let res_json: Value = serde_json::from_slice(&body_bytes).unwrap();
        eprintln!("Response: {}", res_json);
        assert_eq!(res_json["status"], "success");
        assert!(res_json["result"]["memory_id"].as_str().is_some());
    }

    #[test]
    fn utf8_prefix_never_splits_a_code_point() {
        let content = "a".repeat(199) + "é";
        assert_eq!(utf8_prefix(&content, 200), "a".repeat(199));
        assert_eq!(utf8_prefix("😀agent", 1), "");
        assert_eq!(utf8_prefix("😀agent", 4), "😀");
    }

    #[tokio::test]
    async fn memory_delete_removes_the_written_record() {
        let db = aivcs_core::SurrealHandle::setup_db().await.unwrap();
        let written = memory_write_handler(
            MemoryWriteRequest {
                kind: "fact".to_string(),
                content: json!("memory to delete"),
                tags: vec![],
                importance: None,
                confidence: None,
                source: None,
                commit_id: Some("commit-delete".to_string()),
                key: Some("delete-me".to_string()),
                ttl_ms: None,
            },
            &db,
        )
        .await
        .unwrap();

        let memory_id = written["memory_id"].as_str().unwrap().to_string();
        let deleted = memory_delete_handler(
            MemoryDeleteRequest {
                memory_id: memory_id.clone(),
            },
            &db,
        )
        .await
        .unwrap();

        assert_eq!(deleted["status"], "deleted");
        assert!(db.get_memories("commit-delete").await.unwrap().is_empty());
        assert!(
            memory_delete_handler(MemoryDeleteRequest { memory_id }, &db,)
                .await
                .is_err()
        );
    }
}
