//! AIVCS Core Library
//!
//! Re-exports core components for programmatic access to AIVCS functionality.

pub mod a2a;
pub mod cas;
pub mod ci_snapshot;
pub mod compat;
pub mod deploy;
pub mod deploy_runner;
pub mod diff;
pub mod domain;
pub mod event_adapter;
pub mod forge;
pub mod gate;
pub mod git;
pub mod git_host;
pub mod github;
pub mod gitlab;
pub mod hitl_controls;
pub mod infra;
pub mod memory;
pub mod memory_context;
pub mod metrics;
pub mod multi_repo;
pub mod obs;
pub mod orchestration;
pub mod parallel;
pub mod planning_autonomy;
pub mod publish_gate;
pub mod quality_guardrails;
pub mod recording;
pub mod release_registry;
pub mod replay;
pub mod reporting;
pub mod role_orchestration;
pub mod sandbox;
pub mod self_healing;
pub mod telemetry;
pub mod tooling;
pub mod trace_artifact;

pub use domain::{
    validate_run_event, AgentSpec, AgentSpecFields, AivcsError, DeterministicEvalRunner,
    EvalCaseResult, EvalRunReport, EvalSuite, EvalTestCase, EvalThresholds, Event, EventKind,
    Release, ReleaseEnvironment, ReleasePointer, Result, Run, RunStatus, ScorerConfig, ScorerType,
    SnapshotMeta, ValidationError,
};

pub use event_adapter::{subscribe_ledger_to_bus, LedgerHandler};

pub use a2a::{
    emit_code_committed_best_effort, maybe_emit_code_committed_from_env, A2aRetryPolicy,
    CodeCommittedEvent, HttpJsonRpcTransport, DEFAULT_A2A_METHOD,
};

pub use forge::ForgeClient;
pub use git::{
    capture_head_sha, detect_current_branch, detect_github_repository, is_git_repo, is_owner_repo,
    is_valid_github_name, parse_github_remote,
};
pub use git_host::{detect_forge_repository, parse_git_remote, GitHost};
pub use infra::cloudflare_lb::{
    build_audit_report, fetch_load_balancers, fetch_pools, parse_allowlist, prune_orphans,
    render_audit_markdown, resolve_cf_credentials, AuditReport,
};
pub use infra::flux::{build_reconcile_command, run_reconcile};

pub use memory::{DecisionRecorder, DecisionRecorderConfig};

pub use ci_snapshot::{build_ci_snapshot, compute_workspace_hash, find_repo_root, run_local_ci};

pub use oxidized_state::{
    CommitId, CommitRecord, DecisionRecord, MemoryProvenanceRecord, MemoryRecord,
    ProvenanceSourceType, SnapshotRecord, SurrealHandle,
};

pub use nix_env_manager::{
    generate_environment_hash, generate_logic_hash, is_attic_available, is_nix_available,
    AtticClient, AtticConfig, FlakeMetadata, HashSource, NixHash,
};

pub use semantic_rag_merge::{
    diff_memory_vectors, resolve_conflict_state, semantic_merge, synthesize_memory,
    AutoResolvedValue, MemoryConflict, MergeResult, VectorStoreDelta,
};

pub use cas::fs::FsCasStore;
pub use cas::{CasError, CasStore, Digest};
pub use compat::{
    evaluate_compat, CompatRule, CompatRuleSet, CompatVerdict, CompatViolation, PromoteContext,
};
pub use deploy::{deploy_by_digest, DeployResult};
pub use deploy_runner::{DeployByDigestRunner, DeployRunOutput};
pub use diff::lcs_diff::{
    diff_tool_calls as diff_tool_calls_lcs, DiffSummary, ParamChange,
    ToolCallChange as LcsToolCallChange, ToolCallEntry,
};
pub use diff::semantic_graph::{
    diff_graph_snapshots, extract_graph_snapshot, format_semantic_diff_markdown, GraphSnapshot,
    SemanticGraphDiff,
};
pub use diff::state_diff::{
    diff_run_states, diff_scoped_state, extract_last_checkpoint, ScopedStateDiff, StateDelta,
    CHECKPOINT_SAVED_KIND,
};
pub use orchestration::{
    default_role_templates, deterministic_role_order, merge_role_outputs, validate_handoff,
    validate_parallel_roles, AgentRole, HandoffValidationError, MergeConflict,
    MergeConflictStrategy, MergeOutcome, ParallelPlanError, RoleHandoff, RoleOutput, RoleTemplate,
};
pub use parallel::{
    fork_agent_parallel, BranchStatus, ForkResult, ParallelConfig, ParallelManager,
};
pub use planning_autonomy::{
    compute_progress, decompose_goal_to_dag, evaluate_replan, schedule_next_ready_tasks, EpicPlan,
    ExecutionDag, GoalPlan, PlanTask, PlanTaskStatus, PlanningError, ProgressReport,
    ReplanDecision, ReplanPolicy, ReplanReason, SchedulerConstraints, TaskPlan,
};

pub use diff::node_paths::{
    diff_node_paths, extract_node_path, NodeDivergence, NodePathDiff, NodeStep,
};
pub use diff::tool_calls::{diff_tool_calls, ParamDelta, ToolCall, ToolCallChange, ToolCallDiff};
pub use gate::{
    evaluate_gate, CaseResult, EvalReport, GateRule, GateRuleSet, GateVerdict, Violation,
};
pub use recording::GraphRunRecorder;
pub use release_registry::ReleaseRegistryApi;
pub use replay::{find_resume_point, replay_run, verify_spec_digest, ReplaySummary, ResumePoint};
pub use reporting::{
    render_diff_summary_md, write_diff_summary_md, write_eval_results_json, DiffSummaryArtifact,
    EvalCaseResultArtifact, EvalResultsArtifact, EvalSummaryArtifact,
};

pub use trace_artifact::{
    read_trace_artifact, write_trace_artifact, RetentionPolicy, RunTraceArtifact,
};

pub use quality_guardrails::{
    evaluate_quality_guardrails, read_guardrail_artifact, release_block_reason,
    write_guardrail_artifact, CheckFinding, CheckResult, GuardrailArtifact, GuardrailCoverage,
    GuardrailPolicyProfile, GuardrailVerdict, QualityCheck, QualitySeverity, ReleaseAction,
};
pub use role_orchestration::error::{RoleError, RoleResult};
pub use role_orchestration::executor::{
    execute_roles_parallel, token_from_result, ParallelRoleConfig, RoleRunResult,
};
pub use role_orchestration::merge::{merge_parallel_outputs, MergedRoleOutput, RoleConflict};
pub use role_orchestration::roles::HandoffToken;
pub use role_orchestration::router::{
    build_execution_plan, validate_handoff_sequence, ExecutionPlan, RoleStep,
};
pub use self_healing::{
    classify_failure, execute_recovery_loop, read_recovery_artifact, write_recovery_artifact,
    FailureClass, FailureSignal, RecoveryAction, RecoveryAttemptResult, RecoveryDecision,
    RecoveryLog, RecoveryOutcome, RecoveryPolicy,
};

pub use sandbox::{
    evaluate_tool_request, execute_with_controls, CircuitBreaker, PolicyVerdict, SandboxConfig,
    SandboxError, SandboxResult, ToolExecutionResult, ToolPolicyRule, ToolPolicySet, ToolRequest,
};

pub use memory::{
    assemble_context, compact_index, CompactionPolicy, CompactionResult, ContextBudget,
    ContextItem, ContextWindow, DecisionRationale, IndexQuery, IndexResult, MemoryEntry,
    MemoryEntryKind, MemoryError, MemoryIndex, MemoryResult, RationaleEntry, RationaleOutcome,
};

pub use memory_context::{
    estimate_tokens, read_memory_context_artifact, write_memory_context_artifact, AssembledContext,
    CompactionPolicy as MemoryContextCompactionPolicy,
    CompactionResult as MemoryContextCompactionResult, CompactionStrategy, ContextAssembler,
    ContextSegment, DecisionImportance, DecisionRationale as MemoryContextDecisionRationale,
    MatchStrategy, MemoryContextArtifact, MemoryEntry as MemoryContextEntry, MemoryHit,
    MemoryIndex as MemoryContextIndex, MemoryQuery, RationaleLedger,
};
pub use metrics::METRICS;
pub use multi_repo::{
    BackportExecutor, BackportOutcome, BackportPolicy, BackportTask, CIHealthView, CiAggregator,
    CiHealthReport, CiRunFetcher, CrossRepoGraph, MultiRepoError, MultiRepoExecutionPlan,
    MultiRepoOrchestrator, MultiRepoResult, ReleaseProvenance, ReleaseSequencer, RepoCIStatus,
    RepoDependency, RepoDependencyGraph, RepoExecutionPlan, RepoHealth, RepoHealthStatus, RepoId,
    RepoNode, RepoReleaseStatus, RepoReleaser, RepoStep, SequenceItem, SequenceOutcome,
    SequencePlan,
};
pub use obs::{
    emit_event_appended, emit_gate_evaluated, emit_run_finalize_error, emit_run_finished,
    emit_run_started, RunSpan,
};
pub use telemetry::init_tracing;
pub use tooling::{
    JsonFieldSchema, PolicyAction, PolicyMatrix, SchemaStage, ToolAdapter, ToolCallStatus,
    ToolCapability, ToolExecutionConfig, ToolExecutionError, ToolExecutionReport, ToolExecutor,
    ToolInvocation, ToolRegistry, ToolSpec, ToolTelemetry,
};

pub use hitl_controls::{
    apply_intervention, evaluate_checkpoint, read_hitl_artifact, submit_vote, write_hitl_artifact,
    ApprovalCheckpoint, ApprovalPolicy, ApprovalRule, ApprovalVote, CheckpointStatus,
    DecisionSummary, ExplainabilitySummary, HitlArtifact, HitlError, HitlResult, Intervention,
    InterventionAction, RiskTier, VoteDecision,
};

/// AIVCS version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
