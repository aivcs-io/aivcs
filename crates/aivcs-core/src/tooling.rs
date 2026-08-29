//! Tooling and sandbox policy core.
//!
//! Provides a deterministic execution layer for tool calls with:
//! - capability-scoped policy checks
//! - input/output JSON field validation
//! - timeout, retry, and circuit-breaker controls

use std::collections::HashMap;
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::Mutex;

/// Capability class required by a tool.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCapability {
    ShellExec,
    FileRead,
    FileWrite,
    GitRead,
    GitWrite,
    NetworkFetch,
    Custom(String),
}

impl ToolCapability {
    fn as_policy_key(&self) -> String {
        match self {
            Self::ShellExec => "shell_exec".to_string(),
            Self::FileRead => "file_read".to_string(),
            Self::FileWrite => "file_write".to_string(),
            Self::GitRead => "git_read".to_string(),
            Self::GitWrite => "git_write".to_string(),
            Self::NetworkFetch => "network_fetch".to_string(),
            Self::Custom(name) => format!("custom:{name}"),
        }
    }
}

impl std::fmt::Display for ToolCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_policy_key())
    }
}

/// Minimal JSON schema: required top-level fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct JsonFieldSchema {
    pub required_fields: Vec<String>,
}

impl JsonFieldSchema {
    pub fn required<const N: usize>(fields: [&str; N]) -> Self {
        Self {
            required_fields: fields.iter().map(|f| (*f).to_string()).collect(),
        }
    }
}

/// Tool spec in the capability registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: String,
    pub capability: ToolCapability,
    pub input_schema: JsonFieldSchema,
    pub output_schema: JsonFieldSchema,
}

/// In-memory capability registry.
#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<String, ToolSpec>,
}

impl ToolRegistry {
    pub fn register(&mut self, spec: ToolSpec) -> Result<(), ToolExecutionError> {
        if self.tools.contains_key(&spec.name) {
            return Err(ToolExecutionError::DuplicateTool {
                tool_name: spec.name,
            });
        }
        self.tools.insert(spec.name.clone(), spec);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.get(name)
    }
}

/// Policy action for a capability or tool.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Allow,
    Deny,
    RequireApproval,
}

/// Policy matrix for capability and tool-level controls.
#[derive(Debug, Clone, Default)]
pub struct PolicyMatrix {
    by_capability: HashMap<ToolCapability, PolicyAction>,
    by_tool: HashMap<String, PolicyAction>,
}

impl PolicyMatrix {
    /// Secure baseline for high-risk operations.
    pub fn safe_defaults() -> Self {
        Self::default()
            .with_capability(ToolCapability::ShellExec, PolicyAction::RequireApproval)
            .with_capability(ToolCapability::FileWrite, PolicyAction::RequireApproval)
            .with_capability(ToolCapability::GitWrite, PolicyAction::RequireApproval)
            .with_capability(ToolCapability::NetworkFetch, PolicyAction::RequireApproval)
    }

    pub fn with_capability(mut self, capability: ToolCapability, action: PolicyAction) -> Self {
        self.by_capability.insert(capability, action);
        self
    }

    pub fn with_tool_action(mut self, tool_name: impl Into<String>, action: PolicyAction) -> Self {
        self.by_tool.insert(tool_name.into(), action);
        self
    }

    fn action_for(&self, tool: &ToolSpec) -> PolicyAction {
        if let Some(action) = self.by_tool.get(&tool.name) {
            *action
        } else if let Some(action) = self.by_capability.get(&tool.capability) {
            *action
        } else {
            PolicyAction::Allow
        }
    }
}

/// Tool call request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolInvocation {
    pub name: String,
    pub input: Value,
}

impl ToolInvocation {
    pub fn new(name: impl Into<String>, input: Value) -> Self {
        Self {
            name: name.into(),
            input,
        }
    }
}

/// Timeout/retry/circuit-breaker execution controls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolExecutionConfig {
    pub timeout_ms: u64,
    pub max_retries: u32,
    pub circuit_breaker_threshold: u32,
}

impl Default for ToolExecutionConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 5_000,
            max_retries: 0,
            circuit_breaker_threshold: 3,
        }
    }
}

/// Input or output validation stage.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SchemaStage {
    Input,
    Output,
}

/// Tool execution status for observability.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Succeeded,
}

/// Telemetry emitted for a successful tool execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolTelemetry {
    pub run_id: Option<String>,
    pub tool_name: String,
    pub retries: u32,
    pub duration_ms: u128,
    pub status: ToolCallStatus,
}

/// Successful tool execution output + telemetry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolExecutionReport {
    pub output: Value,
    pub telemetry: ToolTelemetry,
}

/// Execution failure taxonomy.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ToolExecutionError {
    #[error("unknown tool: {tool_name}")]
    UnknownTool { tool_name: String },

    #[error("duplicate tool registration: {tool_name}")]
    DuplicateTool { tool_name: String },

    #[error("policy denied tool '{tool_name}': {reason}")]
    PolicyDenied { tool_name: String, reason: String },

    #[error("approval required for tool '{tool_name}': {reason}")]
    ApprovalRequired { tool_name: String, reason: String },

    #[error("schema violation for tool '{tool_name}' ({stage:?}): missing field '{field}'")]
    SchemaViolation {
        tool_name: String,
        stage: SchemaStage,
        field: String,
    },

    #[error("tool '{tool_name}' timed out after {timeout_ms}ms")]
    Timeout { tool_name: String, timeout_ms: u64 },

    #[error("tool '{tool_name}' adapter error: {message}")]
    Adapter { tool_name: String, message: String },

    #[error("circuit breaker open for tool '{tool_name}' (failures={failures})")]
    CircuitOpen { tool_name: String, failures: u32 },
}

/// Adapter contract for actual tool invocation.
#[async_trait]
pub trait ToolAdapter: Send + Sync + 'static {
    async fn call(&self, tool_name: &str, input: &Value) -> std::result::Result<Value, String>;
}

/// Policy-aware tool executor.
pub struct ToolExecutor<A: ToolAdapter> {
    registry: ToolRegistry,
    policy: PolicyMatrix,
    adapter: A,
    config: ToolExecutionConfig,
    failure_counts: Mutex<HashMap<String, u32>>,
}

impl<A: ToolAdapter> ToolExecutor<A> {
    pub fn new(
        registry: ToolRegistry,
        policy: PolicyMatrix,
        adapter: A,
        config: ToolExecutionConfig,
    ) -> Self {
        Self {
            registry,
            policy,
            adapter,
            config,
            failure_counts: Mutex::new(HashMap::new()),
        }
    }

    /// Convenience constructor that applies secure policy defaults.
    pub fn new_with_safe_defaults(
        registry: ToolRegistry,
        adapter: A,
        config: ToolExecutionConfig,
    ) -> Self {
        Self::new(registry, PolicyMatrix::safe_defaults(), adapter, config)
    }

    pub async fn execute(
        &self,
        call: ToolInvocation,
        run_id: Option<String>,
    ) -> Result<ToolExecutionReport, ToolExecutionError> {
        let started = Instant::now();

        let spec =
            self.registry
                .get(&call.name)
                .ok_or_else(|| ToolExecutionError::UnknownTool {
                    tool_name: call.name.clone(),
                })?;

        match self.policy.action_for(spec) {
            PolicyAction::Allow => {}
            PolicyAction::Deny => {
                return Err(ToolExecutionError::PolicyDenied {
                    tool_name: call.name.clone(),
                    reason: format!("capability '{}' is denied", spec.capability.as_policy_key()),
                });
            }
            PolicyAction::RequireApproval => {
                return Err(ToolExecutionError::ApprovalRequired {
                    tool_name: call.name.clone(),
                    reason: format!(
                        "capability '{}' requires explicit approval",
                        spec.capability.as_policy_key()
                    ),
                });
            }
        }

        validate_schema(
            &call.name,
            SchemaStage::Input,
            &spec.input_schema,
            &call.input,
        )?;

        let current_failures = self.current_failure_count(&call.name).await;
        if self.config.circuit_breaker_threshold > 0
            && current_failures >= self.config.circuit_breaker_threshold
        {
            return Err(ToolExecutionError::CircuitOpen {
                tool_name: call.name.clone(),
                failures: current_failures,
            });
        }

        let mut retries = 0u32;
        let max_attempts = self.config.max_retries + 1;
        for attempt in 0..max_attempts {
            let timeout = tokio::time::Duration::from_millis(self.config.timeout_ms);
            let call_result =
                tokio::time::timeout(timeout, self.adapter.call(&call.name, &call.input)).await;

            match call_result {
                Err(_) => {
                    if attempt < self.config.max_retries {
                        retries += 1;
                        continue;
                    }
                    self.increment_failure(&call.name).await;
                    return Err(ToolExecutionError::Timeout {
                        tool_name: call.name.clone(),
                        timeout_ms: self.config.timeout_ms,
                    });
                }
                Ok(Err(message)) => {
                    if attempt < self.config.max_retries {
                        retries += 1;
                        continue;
                    }
                    self.increment_failure(&call.name).await;
                    return Err(ToolExecutionError::Adapter {
                        tool_name: call.name.clone(),
                        message,
                    });
                }
                Ok(Ok(output)) => {
                    validate_schema(
                        &call.name,
                        SchemaStage::Output,
                        &spec.output_schema,
                        &output,
                    )?;
                    self.reset_failure(&call.name).await;
                    return Ok(ToolExecutionReport {
                        output,
                        telemetry: ToolTelemetry {
                            run_id,
                            tool_name: call.name,
                            retries,
                            duration_ms: started.elapsed().as_millis(),
                            status: ToolCallStatus::Succeeded,
                        },
                    });
                }
            }
        }

        Err(ToolExecutionError::Adapter {
            tool_name: call.name,
            message: "unreachable execution state".to_string(),
        })
    }

    async fn current_failure_count(&self, tool_name: &str) -> u32 {
        let guard = self.failure_counts.lock().await;
        *guard.get(tool_name).unwrap_or(&0)
    }

    async fn increment_failure(&self, tool_name: &str) {
        let mut guard = self.failure_counts.lock().await;
        let count = guard.entry(tool_name.to_string()).or_insert(0);
        *count += 1;
    }

    async fn reset_failure(&self, tool_name: &str) {
        let mut guard = self.failure_counts.lock().await;
        guard.insert(tool_name.to_string(), 0);
    }
}

fn validate_schema(
    tool_name: &str,
    stage: SchemaStage,
    schema: &JsonFieldSchema,
    payload: &Value,
) -> Result<(), ToolExecutionError> {
    for field in &schema.required_fields {
        if payload.get(field).is_none() {
            return Err(ToolExecutionError::SchemaViolation {
                tool_name: tool_name.to_string(),
                stage,
                field: field.clone(),
            });
        }
    }
    Ok(())
}
