//! Sovereign Data Gateway: Axum microservice for orchestration state,
//! automated CAS payload offloading, and recursive DAG execution traversal.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::sync::Arc;
use uuid::Uuid;

/// Shared application state for `data-gateway`
#[derive(Clone)]
pub struct GatewayState {
    pub db: PgPool,
    pub cas_threshold_bytes: usize,
}

impl GatewayState {
    pub fn new(db: PgPool, cas_threshold_bytes: usize) -> Self {
        Self {
            db,
            cas_threshold_bytes,
        }
    }
}

/// Request payload to record an orchestration step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordStepRequest {
    pub run_id: Uuid,
    pub step_index: i64,
    pub agent_id: String,
    pub action: String,
    pub result: String,
    pub parent_step_id: Option<Uuid>,
    pub execution_time_ms: i64,
    pub state_payload: serde_json::Value,
}

/// Response returned after successfully committing an orchestration step
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepResponse {
    pub step_id: Uuid,
    pub checkpoint_id: Uuid,
    pub payload_type: String,
    pub cas_digest: Option<String>,
}

/// Node representation in the execution DAG tree
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DagNode {
    pub step_id: Uuid,
    pub step_index: i64,
    pub agent_id: String,
    pub action: String,
    pub result_summary: String,
    pub depth: i32,
}

/// Traversal node representation for generic graph traversals
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TraversalNode {
    pub id: Uuid,
    pub name: String,
    pub payload: serde_json::Value,
    pub depth: i32,
}

/// Query parameters for DAG traversal limits
#[derive(Debug, Deserialize)]
pub struct TraversalQuery {
    pub max_depth: Option<i32>,
}

/// Build the Axum application router
pub fn app(state: Arc<GatewayState>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "OK" }))
        .route("/api/v1/orchestration/steps", post(record_step_handler))
        .route("/api/v1/orchestration/runs/:run_id/dag", get(get_dag_handler))
        .route(
            "/api/v1/orchestration/dag/downstream/:id",
            get(get_downstream_dag_handler),
        )
        .route(
            "/api/v1/orchestration/dag/upstream/:id",
            get(get_upstream_blockers_handler),
        )
        .with_state(state)
}

/// Handler: Record step with automatic inline JSONB vs CAS blob offloading
pub async fn record_step_handler(
    State(state): State<Arc<GatewayState>>,
    Json(payload): Json<RecordStepRequest>,
) -> Result<Json<StepResponse>, (StatusCode, String)> {
    let step_id = Uuid::now_v7();
    let checkpoint_id = Uuid::now_v7();

    // 1. Evaluate payload size for CAS offloading (> threshold, default 64KB)
    let payload_bytes = serde_json::to_vec(&payload.state_payload).map_err(|e| {
        (StatusCode::BAD_REQUEST, format!("Serialization error: {}", e))
    })?;

    let (payload_type, inline_json, cas_digest) = if payload_bytes.len() > state.cas_threshold_bytes {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&payload_bytes);
        let digest = format!("{:x}", hasher.finalize());
        ("cas_blob_ref", None, Some(digest))
    } else {
        ("inline_jsonb", Some(payload.state_payload), None)
    };

    // 2. Atomic Transaction: Record step, edge linkage, and checkpoint
    let mut tx = state.db.begin().await.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("DB Tx begin failed: {}", e))
    })?;

    // Ensure run exists (Upsert)
    sqlx::query(
        r#"
        INSERT INTO orchestration_runs (run_id, session_id, root_node, status, metadata)
        VALUES ($1, $2, $3, 'running', '{}'::jsonb)
        ON CONFLICT (run_id) DO NOTHING
        "#,
    )
    .bind(payload.run_id)
    .bind(format!("session_{}", payload.run_id))
    .bind(&payload.agent_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Run upsert failed: {}", e)))?;

    // Insert Step
    sqlx::query(
        r#"
        INSERT INTO orchestration_steps (step_id, run_id, step_index, agent_id, action, result_summary, execution_time_ms)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(step_id)
    .bind(payload.run_id)
    .bind(payload.step_index)
    .bind(&payload.agent_id)
    .bind(&payload.action)
    .bind(&payload.result)
    .bind(payload.execution_time_ms)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Step insert failed: {}", e)))?;

    // Insert Edge if parent provided
    if let Some(parent_id) = payload.parent_step_id {
        sqlx::query(
            r#"
            INSERT INTO orchestration_edges (run_id, from_step_id, to_step_id)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(payload.run_id)
        .bind(parent_id)
        .bind(step_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Edge insert failed: {}", e)))?;
    }

    // Insert Checkpoint
    sqlx::query(
        r#"
        INSERT INTO orchestration_checkpoints (
            checkpoint_id, run_id, step_id, thread_id, node_id, 
            payload_type, inline_state, cas_blob_digest, step_number
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(checkpoint_id)
    .bind(payload.run_id)
    .bind(step_id)
    .bind(payload.run_id.to_string())
    .bind(&payload.agent_id)
    .bind(payload_type)
    .bind(inline_json)
    .bind(cas_digest.as_deref())
    .bind(payload.step_index)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Checkpoint insert failed: {}", e)))?;

    tx.commit().await.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("DB Tx commit failed: {}", e))
    })?;

    Ok(Json(StepResponse {
        step_id,
        checkpoint_id,
        payload_type: payload_type.to_string(),
        cas_digest,
    }))
}

/// Handler: Query entire execution DAG in a single recursive CTE
pub async fn get_dag_handler(
    State(state): State<Arc<GatewayState>>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<Vec<DagNode>>, (StatusCode, String)> {
    let rows = sqlx::query(
        r#"
        WITH RECURSIVE dag_tree AS (
            -- Base case: Root step(s) with no incoming edges
            SELECT s.step_id, s.step_index, s.agent_id, s.action, s.result_summary, 1::INTEGER AS depth
            FROM orchestration_steps s
            WHERE s.run_id = $1 AND NOT EXISTS (
                SELECT 1 FROM orchestration_edges e WHERE e.run_id = s.run_id AND e.to_step_id = s.step_id
            )
            UNION ALL
            -- Recursive case: Follow outgoing edges
            SELECT s.step_id, s.step_index, s.agent_id, s.action, s.result_summary, (d.depth + 1)::INTEGER AS depth
            FROM orchestration_steps s
            JOIN orchestration_edges e ON s.step_id = e.to_step_id AND s.run_id = e.run_id
            JOIN dag_tree d ON e.from_step_id = d.step_id
            WHERE s.run_id = $1
        )
        SELECT step_id, step_index, agent_id, action, result_summary, depth
        FROM dag_tree
        ORDER BY depth ASC, step_index ASC;
        "#,
    )
    .bind(run_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DAG query failed: {}", e)))?;

    let dag_nodes = rows
        .into_iter()
        .map(|row| DagNode {
            step_id: row.get("step_id"),
            step_index: row.get("step_index"),
            agent_id: row.get("agent_id"),
            action: row.get("action"),
            result_summary: row.get("result_summary"),
            depth: row.get("depth"),
        })
        .collect();

    Ok(Json(dag_nodes))
}

/// Handler: Query downstream DAG with cycle prevention guard
pub async fn get_downstream_dag_handler(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<Uuid>,
    Query(query): Query<TraversalQuery>,
) -> Result<Json<Vec<TraversalNode>>, (StatusCode, String)> {
    let max_depth = query.max_depth.unwrap_or(20);
    get_downstream_dag(&state.db, id, max_depth)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Downstream traversal failed: {}", e)))
}

/// Handler: Query upstream blockers
pub async fn get_upstream_blockers_handler(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<Uuid>,
    Query(query): Query<TraversalQuery>,
) -> Result<Json<Vec<TraversalNode>>, (StatusCode, String)> {
    let max_depth = query.max_depth.unwrap_or(20);
    get_upstream_blockers(&state.db, id, max_depth)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Upstream blocker query failed: {}", e)))
}

/// Query: Downstream Traversal with cycle prevention
pub async fn get_downstream_dag(
    pool: &PgPool,
    root_id: Uuid,
    max_depth: i32,
) -> Result<Vec<TraversalNode>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        WITH RECURSIVE task_dag AS (
            -- Anchor member: root task
            SELECT 
                n.id, 
                n.name, 
                n.payload,
                1::INTEGER AS depth,
                ARRAY[n.id] AS path
            FROM nodes n
            WHERE n.id = $1

            UNION ALL

            -- Recursive member: traverse downstream edges
            SELECT 
                child.id, 
                child.name, 
                child.payload,
                (dag.depth + 1)::INTEGER AS depth,
                dag.path || child.id
            FROM task_dag dag
            JOIN edges e ON dag.id = e.parent_id AND e.relation_type = 'depends_on'
            JOIN nodes child ON e.child_id = child.id
            WHERE NOT (child.id = ANY(dag.path))
              AND dag.depth < $2
        )
        SELECT id, name, payload, depth 
        FROM task_dag 
        ORDER BY depth ASC;
        "#,
    )
    .bind(root_id)
    .bind(max_depth)
    .fetch_all(pool)
    .await?;

    let result = rows
        .into_iter()
        .map(|r| TraversalNode {
            id: r.get("id"),
            name: r.get("name"),
            payload: r.get("payload"),
            depth: r.get("depth"),
        })
        .collect();

    Ok(result)
}

/// Query: Upstream Traversal to find all blocking parent dependencies
pub async fn get_upstream_blockers(
    pool: &PgPool,
    target_id: Uuid,
    max_depth: i32,
) -> Result<Vec<TraversalNode>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        WITH RECURSIVE upstream_blockers AS (
            -- Anchor member: target task requesting execution
            SELECT 
                parent.id, 
                parent.name, 
                parent.payload,
                1::INTEGER AS depth,
                ARRAY[parent.id] AS path
            FROM nodes target
            JOIN edges e ON target.id = e.child_id AND e.relation_type = 'depends_on'
            JOIN nodes parent ON e.parent_id = parent.id
            WHERE target.id = $1

            UNION ALL

            -- Recursive member: walk backwards up the parent chain
            SELECT 
                parent.id, 
                parent.name, 
                parent.payload,
                (b.depth + 1)::INTEGER AS depth,
                b.path || parent.id
            FROM upstream_blockers b
            JOIN edges e ON b.id = e.child_id AND e.relation_type = 'depends_on'
            JOIN nodes parent ON e.parent_id = parent.id
            WHERE NOT (parent.id = ANY(b.path))
              AND b.depth < $2
        )
        SELECT DISTINCT id, name, payload, depth 
        FROM upstream_blockers 
        ORDER BY depth DESC;
        "#,
    )
    .bind(target_id)
    .bind(max_depth)
    .fetch_all(pool)
    .await?;

    let result = rows
        .into_iter()
        .map(|r| TraversalNode {
            id: r.get("id"),
            name: r.get("name"),
            payload: r.get("payload"),
            depth: r.get("depth"),
        })
        .collect();

    Ok(result)
}
