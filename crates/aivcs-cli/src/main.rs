//! AIVCS Core - Agent Version Control System CLI
//!
//! Version agent state (logic + state + environment), runs, snapshots, and releases.
//! Forge source transport uses `publish` / `fetch` only — not git verbs.

mod cognition;
mod forge_import;
mod forge_login;
mod forge_remote;
mod forge_url_policy;
mod infra;
mod oci;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use nix_env_manager::{
    generate_environment_hash, generate_logic_hash, is_attic_available, is_nix_available,
    AtticClient, NixHash,
};
use oxidized_state::{
    CommitId, CommitRecord, ReleaseRegistry, RunEvent, RunLedger, SurrealDbReleaseRegistry,
    SurrealHandle, SurrealRunLedger,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn, Level};

use aivcs_ci::{BuiltinStage, CiGate, CiPipeline, CiSpec, StageConfig};
use aivcs_core::{diff_tool_calls, fork_agent_parallel, ToolCallChange};

// clap `value_parser` for `--owner` / `--repo` flags. Rejects names that
// downstream sites would splice into REST URL paths, A2A event payloads, or
// `Command::args` calls. Keeps the validation invariant at the CLI boundary
// rather than discovering bad input deep in the GitHub client.
fn github_name_arg(value: &str) -> std::result::Result<String, String> {
    if aivcs_core::is_valid_github_name(value) {
        Ok(value.to_string())
    } else {
        Err(format!(
            "{value:?} is not a valid GitHub user/org or repo name \
             (allowed: ASCII alphanumeric, '.', '_', '-'; \
             first and last character must be alphanumeric; max 100 bytes)"
        ))
    }
}

fn default_author() -> String {
    if let Ok(author) = std::env::var("AIVCS_AUTHOR") {
        if !author.trim().is_empty() {
            return author.trim().to_string();
        }
    }
    let token_path = forge_login::aivcs_home().join("token");
    if let Ok(token) = std::fs::read_to_string(&token_path) {
        if let Some(claims) = forge_login::parse_token_claims(&token) {
            if let Some(email) = claims.email {
                let name = claims.sub.unwrap_or_else(|| "aivcs-user".to_string());
                return format!("{name} <{email}>");
            }
            if let Some(sub) = claims.sub {
                return format!("{sub} <{sub}@localhost>");
            }
        }
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
    format!("{user} <{user}@localhost>")
}

#[derive(Parser)]
#[command(name = "aivcs")]
#[command(author = "Stevedores Org")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "AI Agent Version Control System (AIVCS)", long_about = None)]
struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Emit JSON-formatted log lines
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum LoginAction {
    /// Show current forge login and connectivity
    Status,
}

#[derive(Subcommand)]
enum OrgAction {
    /// List all organizations available to the authenticated user
    List,
    /// Switch the active organization context for subsequent commands
    Switch {
        /// Organization slug (e.g. 'aivcs', 'acme', 'lornu')
        slug: String,
    },
}

#[derive(Subcommand)]
enum Commands {
    /// Display current authenticated identity, provider, and active organization context
    Whoami,

    /// Manage multi-tenant organization memberships and switch active context
    Org {
        #[command(subcommand)]
        action: OrgAction,
    },

    /// Initialize a new AIVCS repository
    Init {
        /// Path to initialize (default: current directory)
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Log in through the AIVCS access service (no port-forward)
    Login {
        #[command(subcommand)]
        action: Option<LoginAction>,

        /// Authenticate using RFC 8628 OAuth 2.0 Device Authorization Flow (for headless / CI environments)
        #[arg(long, default_value_t = false)]
        device: bool,

        /// OIDC / Token Issuer URL (default: https://issuer.aivcs.io or AIVCS_ISSUER_URL)
        #[arg(long)]
        issuer: Option<String>,

        /// Forge access URL override (HTTPS always allowed; HTTP for loopback, cluster DNS, or with --tailscale)
        #[arg(long, conflicts_with = "in_cluster")]
        url: Option<String>,

        /// Use Kubernetes Service DNS instead of the default edge service
        #[arg(long, default_value_t = false, conflicts_with = "tailscale")]
        in_cluster: bool,

        /// Reach forge over Tailscale subnet routes (RFC1918 / 100.64.0.0/10) or explicit --url https://*.ts.net
        #[arg(long, default_value_t = false, conflicts_with = "in_cluster")]
        tailscale: bool,

        /// Use HTTPS for in-cluster Service DNS (port 443 when --port is omitted)
        #[arg(long, default_value_t = false)]
        tls: bool,

        /// kubectl context (default: aivcs-core)
        #[arg(long, default_value = "aivcs-core")]
        context: String,

        /// Forge service namespace (default: forge-v2)
        #[arg(long, default_value = "forge-v2")]
        namespace: String,

        /// Forge service name (default: forge-v2)
        #[arg(long, default_value = "forge-v2")]
        service: String,

        /// Secret holding the forge bearer token (default: forge-v2-token)
        #[arg(long, default_value = "forge-v2-token")]
        secret: String,

        /// Secret data key for the bearer token (default: token)
        #[arg(long, default_value = "token")]
        secret_key: String,

        /// Kubernetes Service port (default: 80)
        #[arg(long, default_value_t = 80)]
        port: u16,
    },

    /// Import a read-only GitHub repo to the AIVCS forge (og-agent-forge onboard)
    Import {
        /// GitHub URL or org/repo slug (e.g. lornu-ai/agent-envelope-ai)
        source: String,

        /// Forge repo slug (default: `aivcs/<name>` from GitHub `*/<name>`)
        #[arg(short, long)]
        repo: Option<String>,

        /// Git branch to clone (default: main)
        #[arg(long, default_value = "main")]
        git_branch: String,

        /// Forge branch to publish (default: main)
        #[arg(short, long, default_value = "main")]
        branch: String,

        /// Commit message
        #[arg(
            short,
            long,
            default_value = "Import via aivcs (read-only GitHub bootstrap)"
        )]
        message: String,

        /// Author string
        #[arg(short, long)]
        author: Option<String>,

        /// Remote forge URL override
        #[arg(long)]
        remote: Option<String>,

        /// Auth token override
        #[arg(long)]
        token: Option<String>,

        /// Keep a local working copy without .git at this path
        #[arg(long)]
        keep: Option<PathBuf>,

        /// Rewrite github.com/… provenance to aivcs:// in Cargo.toml, propel.toml, flake.nix
        #[arg(long, default_value_t = true, alias = "AIVCS-provenance")]
        forge_provenance: bool,
    },

    /// Create a snapshot of agent state, linked to the current git HEAD
    ///
    /// # TDD: test_agent_git_snapshot_cli_returns_valid_id
    Snapshot {
        /// Path to agent state file (JSON)
        #[arg(short, long)]
        state: PathBuf,

        /// Commit message
        #[arg(short, long, default_value = "Auto-snapshot")]
        message: String,

        /// Author/agent name
        #[arg(short, long, default_value = "agent")]
        author: String,

        /// Parent commit ID to chain from (optional)
        #[arg(long)]
        parent: Option<String>,

        /// Git SHA to associate (auto-detected from cwd if omitted)
        #[arg(long)]
        git_sha: Option<String>,

        /// CAS storage directory (default: .aivcs/cas in current directory)
        #[arg(long)]
        cas_dir: Option<PathBuf>,
    },

    /// Restore agent to a previous state
    Restore {
        /// Commit ID to restore
        commit: String,

        /// Output path for restored state
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Replay a recorded run artifact from disk by run ID
    ReplayArtifact {
        /// Run ID to replay
        #[arg(long)]
        run: String,

        /// Root directory containing run artifacts (default: .aivcs/runs)
        #[arg(long)]
        artifacts_dir: Option<PathBuf>,

        /// Optional output file path for replayed artifact
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Show commit history
    Log {
        /// Commit ID to start history from
        #[arg(default_value = "")]
        reference: String,

        /// Maximum number of commits to show
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Merge two commits
    Merge {
        /// Source commit to merge from
        source: String,

        /// Target commit to merge into
        target: String,

        /// Merge commit message
        #[arg(short, long)]
        message: Option<String>,
    },

    /// Show differences for specs or runs
    Diff {
        #[command(subcommand)]
        action: DiffAction,
    },

    /// Environment management (Nix/Attic)
    Env {
        #[command(subcommand)]
        action: EnvAction,
    },

    /// Release registry operations (promote/rollback/current/history)
    Release {
        #[command(subcommand)]
        action: ReleaseAction,
    },

    /// Fork multiple parallel exploration commits (Phase 4)
    Fork {
        /// Parent commit ID to fork from
        parent: String,

        /// Number of branches to create
        #[arg(short, long, default_value = "5")]
        count: u8,

        /// Branch name prefix
        #[arg(short, long, default_value = "experiment")]
        prefix: String,
    },

    /// Show reasoning trace for time-travel debugging (Phase 4)
    Trace {
        /// Commit ID to trace
        commit: String,

        /// Maximum depth of trace
        #[arg(short, long, default_value = "20")]
        depth: usize,
    },

    /// Diff the tool-call sequences of two runs
    DiffRuns {
        /// First run ID
        #[arg(long)]
        run_a: String,

        /// Second run ID
        #[arg(long)]
        run_b: String,
    },

    /// CI pipeline operations
    Ci {
        #[command(subcommand)]
        action: CiAction,
    },

    /// Git forge change-request operations (GitHub PR or GitLab MR)
    Pr {
        #[command(subcommand)]
        action: PrAction,
    },

    /// Infra reconcilers (Cloudflare LB, Flux) — no GitHub Actions
    Infra {
        #[command(subcommand)]
        action: infra::InfraAction,
    },

    /// OCI build & publish (Nix + skopeo → GAR; GitLab CI path)
    Oci {
        #[command(subcommand)]
        action: oci::OciAction,
    },

    /// Generate reports for integration, health, or audits
    Report {
        #[command(subcommand)]
        action: ReportAction,
    },

    /// Generate a summary note linking GitHub PR to aivcs CommitId.
    PrNote {
        /// AIVCS commit ID to summarize (required)
        #[arg(long)]
        commit: String,
    },

    /// Emit a semantic graph/prompt diff for PR bodies (SDF dual-ledger Phase 2.1).
    PrSemanticSummary {
        /// Head commit ID
        #[arg(long)]
        head: String,

        /// Base commit ID to compare against
        #[arg(long)]
        base: String,
    },

    /// Publish a tree to the AIVCS CAS forge (closing GAPS.md US-2.5)
    Publish {
        /// Path to directory/tree to publish (default: current directory)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Repository slug (e.g. aivcs/aivcs)
        #[arg(short, long)]
        repo: String,

        /// Commit message
        #[arg(short, long, default_value = "Publish via aivcs CLI")]
        message: String,

        /// Author string (defaults to AIVCS_AUTHOR or the AIVCS agent identity)
        #[arg(short, long)]
        author: Option<String>,

        /// Branch to update (default: main)
        #[arg(short, long, default_value = "main")]
        branch: String,

        /// Remote forge URL override
        #[arg(long)]
        remote: Option<String>,

        /// Auth token override
        #[arg(long)]
        token: Option<String>,

        /// Make the repository public (default is private)
        #[arg(long)]
        public: bool,
    },

    /// Fetch and materialize a repository tree from the AIVCS forge
    Fetch {
        /// Repository slug (e.g. aivcs/aivcs)
        #[arg(short, long)]
        repo: String,

        /// Commit ID or branch name to fetch (default: main)
        #[arg(short = 'R', long = "ref", default_value = "main")]
        reference: String,

        /// Output directory to extract files into (default: .)
        #[arg(short, long, default_value = ".")]
        output: PathBuf,

        /// Remote forge URL override
        #[arg(long)]
        remote: Option<String>,

        /// Auth token override
        #[arg(long)]
        token: Option<String>,
    },

    /// Clone a repository from an aivcs:// URI or repo slug into a new directory
    Clone {
        /// Repository URI (aivcs://org/repo) or slug (org/repo)
        url: String,

        /// Target destination directory
        #[arg(default_value = "")]
        destination: String,

        /// Branch to clone (default: main)
        #[arg(short, long, default_value = "main")]
        branch: String,

        /// Remote forge URL override
        #[arg(long)]
        remote: Option<String>,

        /// Auth token override
        #[arg(long)]
        token: Option<String>,
    },

    /// Push local changes to the remote AIVCS forge
    Push {
        /// Local tree directory (default: .)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Repository slug (e.g. aivcs/aivcs)
        #[arg(short, long)]
        repo: Option<String>,

        /// Target branch (default: main)
        #[arg(short, long, default_value = "main")]
        branch: String,

        /// Commit message
        #[arg(short, long, default_value = "aivcs push")]
        message: String,

        /// Remote forge URL override
        #[arg(long)]
        remote: Option<String>,

        /// Make the repository public (default is private)
        #[arg(long)]
        public: bool,
    },

    /// Pull remote changes into the local working directory
    Pull {
        /// Local tree directory (default: .)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Repository slug (e.g. aivcs/aivcs)
        #[arg(short, long)]
        repo: Option<String>,

        /// Remote branch (default: main)
        #[arg(short, long, default_value = "main")]
        branch: String,

        /// Remote forge URL override
        #[arg(long)]
        remote: Option<String>,
    },

    /// Single-turn cognition query to brains for decisive action planning
    Reason {
        /// The objective or goal for the current turn
        #[arg(short, long)]
        goal: String,

        /// Task tracking ID (optional)
        #[arg(long)]
        task_id: Option<String>,

        /// Calling agent identifier (e.g. og-ci, og-builder, antigravity)
        #[arg(short, long, default_value = "antigravity")]
        agent: String,

        /// Path to JSON context file (optional)
        #[arg(short, long)]
        context_file: Option<PathBuf>,

        /// Brains cognition service endpoint override
        #[arg(long)]
        endpoint: Option<String>,
    },
}

#[derive(Subcommand)]
enum CiAction {
    /// Run CI stages and record execution
    Run {
        /// Workspace path (default: current directory)
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Stages to run (comma-separated: fmt,check,clippy,test)
        #[arg(short, long, default_value = "fmt,check")]
        stages: String,

        /// Skip caching
        #[arg(long)]
        no_cache: bool,

        /// Auto-repair (use fix commands)
        #[arg(long)]
        fix: bool,
    },
}

#[derive(Subcommand)]
enum ReportAction {
    /// Cross-organizational integration health & performance audit (Issue #184)
    CrossOrg {
        /// Optional path to cross-repo dependency graph (JSON)
        #[arg(short, long)]
        graph: Option<PathBuf>,

        /// Objective identifier to aggregate health for
        #[arg(short, long, default_value = "main")]
        objective: String,

        /// Output path for the Markdown report
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum PrAction {
    /// Open a new Pull Request for a branch
    Open {
        /// PR title
        #[arg(short, long)]
        title: String,

        /// PR body (markdown)
        #[arg(short, long)]
        body: String,

        /// Head branch (source of changes)
        #[arg(short, long)]
        head: String,

        /// Base branch (target for changes, default: main)
        #[arg(short, long, default_value = "main")]
        base: String,

        /// GitHub organization/owner
        #[arg(long, value_parser = github_name_arg)]
        owner: String,

        /// GitHub repository name
        #[arg(long, value_parser = github_name_arg)]
        repo: String,

        /// Request review from the Librarian Agent
        #[arg(long, default_value_t = true)]
        librarian: bool,
    },

    /// Create a GitHub branch from a base ref
    Branch {
        /// Branch name to create
        #[arg(short, long)]
        name: String,

        /// Base branch or ref
        #[arg(short, long, default_value = "main")]
        base: String,

        /// GitHub organization/owner
        #[arg(long, value_parser = github_name_arg)]
        owner: String,

        /// GitHub repository name
        #[arg(long, value_parser = github_name_arg)]
        repo: String,
    },

    /// Commit a file to a GitHub branch via the Contents API
    Commit {
        /// Target branch
        #[arg(short, long)]
        branch: String,

        /// Repository-relative file path
        #[arg(short, long)]
        path: String,

        /// Commit message
        #[arg(short, long)]
        message: String,

        /// Local file to upload
        #[arg(short, long)]
        file: PathBuf,

        /// GitHub organization/owner
        #[arg(long, value_parser = github_name_arg)]
        owner: String,

        /// GitHub repository name
        #[arg(long, value_parser = github_name_arg)]
        repo: String,
    },

    /// Zero-touch pipeline: branch → commit → CODE_COMMITTED → open PR
    ///
    /// Runs the full autonomous builder flow in one invocation. Intended for
    /// ephemeral ADK Agent Jobs that invoke `aivcs` via `uv`.
    Pipeline {
        /// Feature branch to create and commit to
        #[arg(long)]
        branch: String,

        /// Base branch or ref
        #[arg(long, default_value = "develop")]
        base: String,

        /// Repository-relative file path
        #[arg(long)]
        path: String,

        /// Local file to upload
        #[arg(long)]
        file: PathBuf,

        /// Commit message
        #[arg(long)]
        message: String,

        /// Pull request title
        #[arg(long)]
        title: String,

        /// Pull request body (markdown)
        #[arg(long)]
        body: String,

        /// GitHub organization/owner
        #[arg(long, value_parser = github_name_arg)]
        owner: String,

        /// GitHub repository name
        #[arg(long, value_parser = github_name_arg)]
        repo: String,

        /// Request review from the Librarian Agent
        #[arg(long, default_value_t = true)]
        librarian: bool,

        /// Skip branch creation (for retries when the branch already exists)
        #[arg(long, default_value_t = false)]
        skip_branch: bool,
    },

    /// Verify aivcs-ci-snapshot against HEAD
    VerifySnapshot {
        /// Optional PR body text. If omitted, it will try to find it via GITHUB_EVENT_PATH.
        #[arg(long)]
        pr_body: Option<String>,
    },

    /// Verify aivcs-commit reproducibility (env hash match and CAS existence)
    VerifyReproducibility {
        /// Optional PR body text. If omitted, it will try to find it via GITHUB_EVENT_PATH.
        #[arg(long)]
        pr_body: Option<String>,
    },
}

#[derive(Subcommand)]
enum EnvAction {
    /// Show environment hash for a flake
    Hash {
        /// Path to flake directory
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Show hash of Rust source code (logic hash)
    LogicHash {
        /// Path to source directory
        #[arg(default_value = "src")]
        path: PathBuf,
    },

    /// Check Attic cache status
    CacheInfo,

    /// Check if environment is cached
    IsCached {
        /// Environment hash to check
        hash: String,
    },

    /// Show system info (Nix/Attic availability)
    Info,
}

#[derive(Subcommand)]
enum DiffAction {
    /// Diff two spec JSON files
    Spec {
        /// First spec JSON file
        a: PathBuf,
        /// Second spec JSON file
        b: PathBuf,
        /// Emit JSON output instead of terminal text
        #[arg(long)]
        json: bool,
    },
    /// Diff two run event-log JSON files
    Run {
        /// First run events JSON file (array of RunEvent)
        a: PathBuf,
        /// Second run events JSON file (array of RunEvent)
        b: PathBuf,
        /// Emit JSON output instead of terminal text
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ReleaseAction {
    /// Promote a validated agent spec as the latest release
    Promote {
        /// Agent name
        name: String,
        /// Git commit SHA linked to this spec
        #[arg(long)]
        git_sha: String,
        /// SHA256 hex of graph definition
        #[arg(long)]
        graph_digest: String,
        /// SHA256 hex of prompts definition
        #[arg(long)]
        prompts_digest: String,
        /// SHA256 hex of tools definition
        #[arg(long)]
        tools_digest: String,
        /// SHA256 hex of configuration
        #[arg(long)]
        config_digest: String,
        /// Who promoted this release
        #[arg(long, default_value = "aivcs-cli")]
        promoted_by: String,
        /// Optional version label (e.g. v1.2.3)
        #[arg(long)]
        version: Option<String>,
        /// Optional release notes
        #[arg(long)]
        notes: Option<String>,
    },
    /// Roll back the agent to the previous release (append-only history)
    Rollback {
        /// Agent name
        name: String,
    },
    /// Show the current release pointer for an agent
    Current {
        /// Agent name
        name: String,
    },
    /// Show release history for an agent (newest first)
    History {
        /// Agent name
        name: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Setup logging
    let level = if cli.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };
    aivcs_core::init_tracing(cli.json, level);

    // Initialize database connection
    let handle = SurrealHandle::setup_from_env()
        .await
        .context("Failed to connect to AIVCS database")?;

    match cli.command {
        Commands::Whoami => forge_login::show_whoami(),
        Commands::Org { action } => match action {
            OrgAction::List => forge_login::list_orgs(),
            OrgAction::Switch { slug } => forge_login::switch_org(&slug),
        },
        Commands::Init { path } => cmd_init(&handle, &path).await,
        Commands::Login {
            action,
            device,
            issuer,
            url,
            in_cluster,
            tailscale,
            tls,
            context,
            namespace,
            service,
            secret,
            secret_key,
            port,
        } => match action {
            Some(LoginAction::Status) => forge_login::run_status().await,
            None => {
                forge_login::run_login(forge_login::LoginOptions {
                    url,
                    issuer,
                    device,
                    in_cluster,
                    tailscale,
                    tls,
                    context,
                    namespace,
                    service,
                    secret,
                    secret_key,
                    port,
                    ..forge_login::LoginOptions::default()
                })
                .await
            }
        },
        Commands::Import {
            source,
            repo,
            git_branch,
            branch,
            message,
            author,
            remote,
            token,
            keep,
            forge_provenance,
        } => {
            forge_import::run_import(forge_import::ImportOptions {
                source,
                repo,
                git_branch,
                forge_branch: branch,
                message,
                author,
                remote,
                token,
                keep_dir: keep,
                forge_provenance,
            })
            .await
        }
        Commands::Snapshot {
            state,
            message,
            author,
            parent,
            git_sha,
            cas_dir,
        } => {
            cmd_snapshot(
                &handle,
                &state,
                &message,
                &author,
                parent.as_deref(),
                git_sha.as_deref(),
                cas_dir.as_deref(),
            )
            .await?;
            Ok(())
        }
        Commands::Restore { commit, output } => {
            cmd_restore(&handle, &commit, output.as_deref()).await
        }
        Commands::ReplayArtifact {
            run,
            artifacts_dir,
            output,
        } => cmd_replay_artifact(&run, artifacts_dir.as_deref(), output.as_deref()),
        Commands::Log { reference, limit } => cmd_log(&handle, &reference, limit).await,
        Commands::Merge {
            source,
            target,
            message,
        } => cmd_merge(&handle, &source, &target, message.as_deref()).await,
        Commands::Diff { action } => cmd_diff(action).await,
        Commands::Env { action } => match action {
            EnvAction::Hash { path } => cmd_env_hash(&path).await,
            EnvAction::LogicHash { path } => cmd_logic_hash(&path).await,
            EnvAction::CacheInfo => cmd_cache_info().await,
            EnvAction::IsCached { hash } => cmd_is_cached(&hash).await,
            EnvAction::Info => cmd_env_info().await,
        },
        Commands::Release { action } => match action {
            ReleaseAction::Promote {
                name,
                git_sha,
                graph_digest,
                prompts_digest,
                tools_digest,
                config_digest,
                promoted_by,
                version,
                notes,
            } => {
                cmd_release_promote(
                    &handle,
                    &name,
                    &git_sha,
                    &graph_digest,
                    &prompts_digest,
                    &tools_digest,
                    &config_digest,
                    &promoted_by,
                    version.as_deref(),
                    notes.as_deref(),
                )
                .await
            }
            ReleaseAction::Rollback { name } => cmd_release_rollback(&handle, &name).await,
            ReleaseAction::Current { name } => cmd_release_current(&handle, &name).await,
            ReleaseAction::History { name } => cmd_release_history(&handle, &name).await,
        },
        Commands::Fork {
            parent,
            count,
            prefix,
        } => cmd_fork(&handle, &parent, count, &prefix).await,
        Commands::Trace { commit, depth } => cmd_trace(&handle, &commit, depth).await,
        Commands::DiffRuns { run_a, run_b } => {
            let ledger = SurrealRunLedger::from_env()
                .await
                .context("Failed to connect to run ledger")?;
            cmd_diff_runs(&ledger, &run_a, &run_b).await
        }
        Commands::Ci { action } => match action {
            CiAction::Run {
                workspace,
                stages,
                no_cache,
                fix,
            } => cmd_ci_run(&workspace, &stages, no_cache, fix).await,
        },
        Commands::Pr { action } => match action {
            PrAction::Open {
                title,
                body,
                head,
                base,
                owner,
                repo,
                librarian,
            } => {
                cmd_pr_open(PrOpenArgs {
                    title,
                    body,
                    head,
                    base,
                    owner,
                    repo,
                    librarian,
                })
                .await
            }
            PrAction::Branch {
                name,
                base,
                owner,
                repo,
            } => cmd_pr_branch(name, base, owner, repo).await,
            PrAction::Commit {
                branch,
                path,
                message,
                file,
                owner,
                repo,
            } => cmd_pr_commit(&handle, branch, path, message, file, owner, repo).await,
            PrAction::Pipeline {
                branch,
                base,
                path,
                file,
                message,
                title,
                body,
                owner,
                repo,
                librarian,
                skip_branch,
            } => {
                cmd_pr_pipeline(
                    &handle,
                    PrPipelineArgs {
                        branch,
                        base,
                        path,
                        file,
                        message,
                        title,
                        body,
                        owner,
                        repo,
                        librarian,
                        skip_branch,
                    },
                )
                .await
            }
            PrAction::VerifySnapshot { pr_body } => cmd_pr_verify_snapshot(pr_body).await,
            PrAction::VerifyReproducibility { pr_body } => {
                cmd_pr_verify_reproducibility(pr_body).await
            }
        },
        Commands::Report { action } => match action {
            ReportAction::CrossOrg {
                graph,
                objective,
                output,
            } => cmd_report_cross_org(graph, &objective, output).await,
        },
        Commands::PrNote { commit } => cmd_pr_note(&handle, &commit).await,
        Commands::PrSemanticSummary { head, base } => {
            cmd_pr_semantic_summary(&handle, &head, &base).await
        }
        Commands::Publish {
            path,
            repo,
            message,
            author,
            branch,
            remote,
            token,
            public,
        } => {
            let resolved_author = author
                .or_else(|| std::env::var("AIVCS_AUTHOR").ok())
                .unwrap_or_else(default_author);
            let client = forge_remote::ForgeRemoteClient::new(remote.as_deref(), token.as_deref());
            let commit_id = client
                .publish(
                    &path,
                    &repo,
                    &message,
                    &resolved_author,
                    &branch,
                    Some(!public),
                )
                .await?;
            println!("Published to aivcs://{}@{}", repo, branch);
            println!("Commit ID: {}", commit_id);
            Ok(())
        }
        Commands::Fetch {
            repo,
            reference,
            output,
            remote,
            token,
        } => {
            let client = forge_remote::ForgeRemoteClient::new(remote.as_deref(), token.as_deref());
            client.fetch(&repo, &reference, &output).await?;
            println!("Fetched {}/{} to {}", repo, reference, output.display());
            Ok(())
        }
        Commands::Clone {
            url,
            destination,
            branch,
            remote,
            token,
        } => {
            let client = forge_remote::ForgeRemoteClient::new(remote.as_deref(), token.as_deref());
            let dest_path = if destination.is_empty() {
                None
            } else {
                Some(std::path::Path::new(&destination))
            };
            let out = client.clone(&url, dest_path, &branch).await?;
            println!("Cloned {} ({}) to {}", url, branch, out.display());
            Ok(())
        }
        Commands::Push {
            path,
            repo,
            branch,
            message,
            remote,
            public,
        } => {
            let repo_name = repo.ok_or_else(|| anyhow!("repository name required (e.g. --repo <org>/<repo>)"))?;
            let client = forge_remote::ForgeRemoteClient::new(remote.as_deref(), None);
            let author = default_author();
            let commit_id = client
                .publish(&path, &repo_name, &message, &author, &branch, Some(!public))
                .await?;
            println!(
                "Pushed {} -> aivcs://{}@{}",
                path.display(),
                repo_name,
                branch
            );
            println!("Head Commit: {}", commit_id);
            Ok(())
        }
        Commands::Pull {
            path,
            repo,
            branch,
            remote,
        } => {
            let repo_name = repo.unwrap_or_else(|| "aivcs/aivcs".to_string());
            let client = forge_remote::ForgeRemoteClient::new(remote.as_deref(), None);
            client.fetch(&repo_name, &branch, &path).await?;
            println!(
                "Pulled aivcs://{}@{} into {}",
                repo_name,
                branch,
                path.display()
            );
            Ok(())
        }
        Commands::Reason {
            goal,
            task_id,
            agent,
            context_file,
            endpoint,
        } => {
            let client = cognition::BrainsClient::new(endpoint.as_deref());
            let ctx = if let Some(cf) = context_file {
                let data = std::fs::read_to_string(cf)?;
                serde_json::from_str(&data).ok()
            } else {
                None
            };
            let res = client
                .reason(&goal, task_id.as_deref(), Some(&agent), ctx)
                .await?;
            println!("--- Brains Cognition Decision ---");
            println!("Decision:    {}", res.decision);
            println!("Next Action: {}", res.next_action);
            if let Some(exp) = res.explanation {
                println!("Explanation: {}", exp);
            }
            if let Some(conf) = res.confidence {
                println!("Confidence:  {:.2}", conf);
            }
            Ok(())
        }
        Commands::Infra { action } => infra::run(action).await,
        Commands::Oci { action } => oci::run(action),
    }
}

async fn cmd_report_cross_org(
    graph_path: Option<PathBuf>,
    _objective: &str,
    output: Option<PathBuf>,
) -> Result<()> {
    use aivcs_core::{
        multi_repo::{audit::AuditEngine, model::CrossRepoGraph},
        reporting::{render_cross_org_audit_md, CrossOrgAuditArtifact},
    };

    let graph = if let Some(path) = graph_path {
        let content = std::fs::read_to_string(&path)?;
        serde_json::from_str(&content)?
    } else {
        // Default minimal graph for demonstration if no path provided
        CrossRepoGraph::default()
    };

    let engine = AuditEngine::new(&graph);
    let audit = engine.audit();

    let artifact = CrossOrgAuditArtifact {
        generated_at: chrono::Utc::now(),
        coupling: audit.coupling,
        critical_spofs: audit.critical_spofs,
        health: None,
    };

    let report = render_cross_org_audit_md(&artifact);

    if let Some(path) = output {
        std::fs::write(&path, report)?;
        println!("Report written to {:?}", path);
    } else {
        println!("{}", report);
    }

    Ok(())
}

struct PrOpenArgs {
    title: String,
    body: String,
    head: String,
    base: String,
    owner: String,
    repo: String,
    librarian: bool,
}

async fn cmd_pr_open(args: PrOpenArgs) -> Result<()> {
    let PrOpenArgs {
        title,
        mut body,
        head,
        base,
        owner,
        repo,
        librarian,
    } = args;

    let repo_root = aivcs_core::find_repo_root();

    // Always generate snapshot and embed in body
    let snapshot = aivcs_core::build_ci_snapshot(&repo_root)?;
    let digest = snapshot.digest();
    body = format!(
        "{}\n\n<!-- aivcs-ci-snapshot: sha256:{} config-hash:{} -->",
        body, digest, snapshot.local_ci_config_hash
    );

    let client = forge_client_from_env(owner, repo)?;

    let pr_number = client
        .open_change_request(&title, &body, &head, &base, librarian)
        .await?;

    let label = match client.host() {
        aivcs_core::GitHost::GitHub => "PR",
        aivcs_core::GitHost::GitLab => "MR",
    };
    println!("Successfully opened {label} #{pr_number}");

    Ok(())
}

async fn cmd_pr_branch(name: String, base: String, owner: String, repo: String) -> Result<()> {
    let client = forge_client_from_env(owner, repo)?;
    let sha = client.create_branch(&name, &base).await?;
    println!("Created branch '{}' from '{}' ({})", name, base, sha);
    Ok(())
}

async fn read_file_for_github_commit(file: &PathBuf) -> Result<Vec<u8>> {
    tokio::fs::read(file)
        .await
        .with_context(|| format!("Failed to read file for commit: {:?}", file))
}

async fn cmd_pr_commit(
    _handle: &SurrealHandle,
    branch: String,
    path: String,
    message: String,
    file: PathBuf,
    owner: String,
    repo: String,
) -> Result<()> {
    let content = read_file_for_github_commit(&file).await?;
    let forge_repo = format!("{owner}/{repo}");

    let client = forge_client_from_env(owner, repo)?;
    let sha = client
        .commit_file(&branch, &path, &content, &message)
        .await?;
    println!("Committed '{path}' to branch '{branch}' ({sha})");

    aivcs_core::maybe_emit_code_committed_from_env(
        &branch,
        &sha,
        vec![path.clone()],
        "forge-pr-commit",
        Some(&forge_repo),
        None,
    )
    .await;

    Ok(())
}

struct PrPipelineArgs {
    branch: String,
    base: String,
    path: String,
    file: PathBuf,
    message: String,
    title: String,
    body: String,
    owner: String,
    repo: String,
    librarian: bool,
    skip_branch: bool,
}

async fn cmd_pr_pipeline(_handle: &SurrealHandle, args: PrPipelineArgs) -> Result<()> {
    let PrPipelineArgs {
        branch,
        base,
        path,
        file,
        message,
        title,
        mut body,
        owner,
        repo,
        librarian,
        skip_branch,
    } = args;

    let repo_root = aivcs_core::find_repo_root();

    // Always generate snapshot and embed in body
    let snapshot = aivcs_core::build_ci_snapshot(&repo_root)?;
    let digest = snapshot.digest();
    body = format!(
        "{}\n\n<!-- aivcs-ci-snapshot: sha256:{} config-hash:{} -->",
        body, digest, snapshot.local_ci_config_hash
    );

    let forge_repo = format!("{owner}/{repo}");
    let client = forge_client_from_env(owner.clone(), repo.clone())?;

    if !skip_branch {
        let sha = client.create_branch(&branch, &base).await?;
        println!("Created branch '{branch}' from '{base}' ({sha})");
    }

    let content = read_file_for_github_commit(&file).await?;

    let sha = client
        .commit_file(&branch, &path, &content, &message)
        .await?;
    println!("Committed '{path}' to branch '{branch}' ({sha})");

    aivcs_core::maybe_emit_code_committed_from_env(
        &branch,
        &sha,
        vec![path.clone()],
        "forge-pr-pipeline",
        Some(&forge_repo),
        None,
    )
    .await;

    let pr_number = client
        .open_change_request(&title, &body, &branch, &base, librarian)
        .await?;
    let label = match client.host() {
        aivcs_core::GitHost::GitHub => "PR",
        aivcs_core::GitHost::GitLab => "MR",
    };
    println!("Successfully opened {label} #{pr_number}");

    Ok(())
}

async fn cmd_pr_verify_snapshot(pr_body: Option<String>) -> Result<()> {
    let repo_root = aivcs_core::find_repo_root();

    // 1. Get PR body
    let body = if let Some(b) = pr_body {
        b
    } else if let Ok(event_path) = std::env::var("GITHUB_EVENT_PATH") {
        let content =
            std::fs::read_to_string(event_path).context("Failed to read GITHUB_EVENT_PATH")?;
        let event: serde_json::Value =
            serde_json::from_str(&content).context("Failed to parse GITHUB_EVENT_PATH JSON")?;

        if let Some(body_val) = event.pointer("/pull_request/body") {
            body_val.as_str().unwrap_or_default().to_string()
        } else {
            anyhow::bail!("Could not find pull_request.body in GITHUB_EVENT_PATH");
        }
    } else {
        anyhow::bail!("No PR body provided and GITHUB_EVENT_PATH is not set");
    };

    // 2. Extract snapshot digest from PR body
    // Look for: <!-- aivcs-ci-snapshot: sha256:<digest> config-hash:<hash> -->
    let pattern = "<!-- aivcs-ci-snapshot: sha256:";
    let idx = body.find(pattern).context("No aivcs-ci-snapshot digest found in PR body. Did you open this PR using 'aivcs pr pipeline'?")?;
    let start = idx + pattern.len();
    let rest = &body[start..];
    let end_idx = rest
        .find(' ')
        .or_else(|| rest.find("-->"))
        .context("Malformed aivcs-ci-snapshot in PR body")?;
    let expected_digest = rest[..end_idx].trim().to_string();

    println!(
        "Extracted snapshot digest from PR body: {}",
        expected_digest
    );

    // 3. Recompute snapshot
    let snapshot = aivcs_core::build_ci_snapshot(&repo_root)?;
    let computed_digest = snapshot.digest();
    println!("Computed snapshot digest from HEAD: {}", computed_digest);

    if expected_digest == computed_digest {
        println!("✓ Snapshot verification successful!");
        Ok(())
    } else {
        println!("Expected snapshot details:");
        println!("  repo_sha: {}", snapshot.repo_sha);
        println!("  workspace_hash: {}", snapshot.workspace_hash);
        println!("  local_ci_config_hash: {}", snapshot.local_ci_config_hash);
        println!("  env_hash: {}", snapshot.env_hash);
        anyhow::bail!("✗ Snapshot verification failed! The current HEAD state does not match the local-ci snapshot in the PR body. Did you push unstaged/untested changes without using aivcs pr pipeline?")
    }
}

/// PRs that only change code/docs/tests without capturing an agent cognitive
/// snapshot may declare an exempt sentinel instead of a real CommitId hash.
/// Format: `code-only-<scope>-no-cognitive-snapshot` (see `.github/pull_request_template.md`).
fn is_no_cognitive_snapshot_exempt(commit_id: &str) -> bool {
    commit_id.starts_with("code-only-") && commit_id.ends_with("-no-cognitive-snapshot")
}

async fn cmd_pr_verify_reproducibility(pr_body: Option<String>) -> Result<()> {
    let repo_root = aivcs_core::find_repo_root();
    println!("cmd_pr_verify_reproducibility: repo_root = {:?}", repo_root);

    // 1. Get PR body
    let body = if let Some(b) = pr_body {
        b
    } else if let Ok(event_path) = std::env::var("GITHUB_EVENT_PATH") {
        let content =
            std::fs::read_to_string(event_path).context("Failed to read GITHUB_EVENT_PATH")?;
        let event: serde_json::Value =
            serde_json::from_str(&content).context("Failed to parse GITHUB_EVENT_PATH JSON")?;

        if let Some(body_val) = event.pointer("/pull_request/body") {
            body_val.as_str().unwrap_or_default().to_string()
        } else {
            anyhow::bail!("Could not find pull_request.body in GITHUB_EVENT_PATH");
        }
    } else {
        anyhow::bail!("No PR body provided and GITHUB_EVENT_PATH is not set");
    };

    // 2. Extract aivcs-commit: <CommitId>
    let commit_pattern = "aivcs-commit:";
    let idx = body.find(commit_pattern).with_context(|| {
        "No 'aivcs-commit' field found in PR body. Run `aivcs pr-note` and paste the output, \
         or for code/docs-only changes without a cognitive snapshot add e.g. \
         `aivcs-commit: code-only-docs-change-no-cognitive-snapshot` (see .github/pull_request_template.md)."
    })?;
    let start = idx + commit_pattern.len();
    let rest = &body[start..];
    let end_idx = rest.find('\n').unwrap_or(rest.len());
    let expected_commit_id = rest[..end_idx].trim().to_string();

    println!(
        "Extracted aivcs-commit from PR body: {}",
        expected_commit_id
    );

    if is_no_cognitive_snapshot_exempt(&expected_commit_id) {
        println!(
            "✓ Exempt sentinel '{expected_commit_id}' — skipping cognitive snapshot / CAS verification."
        );
        println!("✓ aivcs reproducibility check successful!");
        return Ok(());
    }

    // 3. Extract Env Hash (optional / if present)
    let env_pattern = "- **Env Hash**:";
    let env_hash_opt = if let Some(env_idx) = body.find(env_pattern) {
        let start = env_idx + env_pattern.len();
        let rest = &body[start..];
        let end_idx = rest.find('\n').unwrap_or(rest.len());
        Some(rest[..end_idx].trim().to_string())
    } else {
        None
    };

    // 4. Extract State Hash (optional / if present)
    let state_pattern = "- **State Hash**:";
    let state_hash_opt = if let Some(state_idx) = body.find(state_pattern) {
        let start = state_idx + state_pattern.len();
        let rest = &body[start..];
        let end_idx = rest.find('\n').unwrap_or(rest.len());
        Some(rest[..end_idx].trim().to_string())
    } else {
        None
    };

    // 5. Extract Logic Hash (optional / if present)
    let logic_pattern = "- **Logic Hash**:";
    let logic_hash_opt = if let Some(logic_idx) = body.find(logic_pattern) {
        let start = logic_idx + logic_pattern.len();
        let rest = &body[start..];
        let end_idx = rest.find('\n').unwrap_or(rest.len());
        Some(rest[..end_idx].trim().to_string())
    } else {
        None
    };

    // 6. Validate composite CommitId match
    if let (Some(state_hash), env_hash, logic_hash) = (
        state_hash_opt.clone(),
        env_hash_opt.clone(),
        logic_hash_opt.clone(),
    ) {
        let computed_commit =
            CommitId::new(logic_hash.as_deref(), &state_hash, env_hash.as_deref());
        if computed_commit.hash != expected_commit_id {
            anyhow::bail!(
                "✗ Commit ID validation failed! The composite CommitId in the PR body ({}) does not match the computed hash ({}) from the listed components.",
                expected_commit_id,
                computed_commit.hash
            );
        }
        println!("✓ Composite CommitId integrity validated.");
    }

    // 7. Verify CAS file exists locally (validates the commit data exists in repository)
    if let Some(state_hash) = state_hash_opt {
        let cas_path = repo_root
            .join(".aivcs")
            .join("cas")
            .join("objects")
            .join(&state_hash[..2])
            .join(&state_hash[2..]);
        if !cas_path.exists() {
            anyhow::bail!(
                "✗ CAS file not found at {:?}! Please ensure you have committed and pushed the CAS objects in '.aivcs/cas/objects/' to your PR.",
                cas_path
            );
        }
        println!("✓ CAS object verified: {:?}", cas_path);
    }

    // 8. Generate workspace Nix environment hash and verify against the Env Hash
    let workspace_env = nix_env_manager::generate_environment_hash(&repo_root)
        .context("Failed to generate Nix environment hash for the workspace")?;
    println!("Computed workspace environment: {:?}", workspace_env);
    println!(
        "Computed workspace environment hash: {}",
        workspace_env.hash
    );

    if let Some(ref env_hash) = env_hash_opt {
        if env_hash != &workspace_env.hash {
            anyhow::bail!(
                "✗ Nix environment hash mismatch! PR body has '{}', but workspace has '{}'. Please make sure your Nix flake and lock file match the snapshot environment.",
                env_hash,
                workspace_env.hash
            );
        }
        println!("✓ Nix environment hash matches!");
    } else {
        println!("⚠ No Env Hash found in PR body summary. Skipping env hash comparison.");
    }

    println!("✓ aivcs reproducibility check successful!");
    Ok(())
}

fn forge_client_from_env(owner: String, repo: String) -> Result<aivcs_core::ForgeClient> {
    aivcs_core::ForgeClient::from_env(owner, repo)
}

/// Initialize a new AIVCS repository
async fn cmd_init(handle: &SurrealHandle, path: &PathBuf) -> Result<()> {
    info!("Initializing AIVCS repository at {:?}", path);

    // Create initial commit
    let initial_state = serde_json::json!({
        "initialized": true,
        "version": env!("CARGO_PKG_VERSION"),
    });

    let commit_id = CommitId::from_state(serde_json::to_vec(&initial_state)?.as_slice());

    // Save initial snapshot
    handle.save_snapshot(&commit_id, initial_state).await?;

    // Create initial commit record
    let commit = CommitRecord::new(commit_id.clone(), vec![], "Initial commit", "system");
    handle.save_commit(&commit).await?;

    println!("Initialized AIVCS repository at {:?}", path);
    println!("Initial commit: {}", commit_id.short());

    Ok(())
}

/// Create a snapshot of agent state, linked to the current git HEAD
async fn cmd_snapshot(
    handle: &SurrealHandle,
    state_path: &PathBuf,
    message: &str,
    author: &str,
    parent: Option<&str>,
    git_sha_override: Option<&str>,
    cas_dir: Option<&std::path::Path>,
) -> Result<CommitId> {
    // Read state file
    let state_content = std::fs::read_to_string(state_path)
        .context(format!("Failed to read state file: {:?}", state_path))?;

    let state: serde_json::Value =
        serde_json::from_str(&state_content).context("Failed to parse state as JSON")?;

    // Resolve git SHA: use override, or auto-detect from cwd. Sentinel
    // `None` means "no git context" — used by the local-print path and the
    // A2A emit gate below. See the matching block on the emit site.
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let git_sha: Option<String> = match git_sha_override {
        Some(sha) => Some(sha.to_string()),
        None => aivcs_core::capture_head_sha(&cwd).ok(),
    };

    // Generate logic and environment hashes for composite CommitId
    let logic_hash = generate_logic_hash(&cwd.join("src")).ok();
    let env_hash = generate_environment_hash(&cwd).ok();

    // Store state in CAS
    let cas_root = cas_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".aivcs/cas"));
    let cas = aivcs_core::FsCasStore::new(&cas_root)
        .map_err(|e| anyhow::anyhow!("failed to open CAS store: {e}"))?;
    let cas_digest = aivcs_core::CasStore::put(&cas, state_content.as_bytes())
        .map_err(|e| anyhow::anyhow!("CAS put failed: {e}"))?;

    // Parent chain via explicit commit ID only
    let parent_ids = parent.map(|id| vec![id.to_string()]).unwrap_or_default();

    // Create composite commit ID
    let commit_id = CommitId::new(
        logic_hash.as_deref(),
        &cas_digest.to_hex(),
        env_hash.as_ref().map(|h| h.hash.as_str()),
    );

    // Save snapshot to SurrealDB
    handle.save_snapshot(&commit_id, state).await?;

    // Create commit record
    let commit = CommitRecord::new(commit_id.clone(), parent_ids.clone(), message, author);
    handle.save_commit(&commit).await?;

    // Create graph edges for all parents
    for pid in &parent_ids {
        handle.save_commit_graph_edge(&commit_id.hash, pid).await?;
    }

    // Same A2A gate as `cmd_merge`: only emit CODE_COMMITTED when we have
    // a real git SHA. Emitting with a placeholder hash would be a lie to
    // any consumer that joins on `commit_sha`.
    if let Some(sha) = &git_sha {
        aivcs_core::maybe_emit_code_committed_from_env(
            "snapshot",
            sha,
            vec![state_path.display().to_string()],
            author,
            None,
            Some(&commit_id.hash),
        )
        .await;
    } else {
        warn!(
            aivcs_commit_id = %commit_id.hash,
            "skipping CODE_COMMITTED emission for snapshot: git SHA unavailable"
        );
    }

    info!(
        cas_digest = %cas_digest,
        git_sha = %git_sha.as_deref().unwrap_or("(unavailable)"),
        "snapshot stored in CAS"
    );

    println!(
        "[{}] {} ({})",
        commit_id.short(),
        message,
        commit_id.short()
    );
    println!("Commit:    {}", commit_id);
    println!(
        "Git SHA:   {}",
        git_sha.as_deref().unwrap_or("(unavailable)")
    );
    println!("CAS digest: {}", cas_digest);

    Ok(commit_id)
}

/// Restore agent to a previous state
async fn cmd_restore(
    handle: &SurrealHandle,
    reference: &str,
    output: Option<&std::path::Path>,
) -> Result<()> {
    let commit_hash = reference.to_string();
    let snapshot = handle
        .load_snapshot(&commit_hash)
        .await
        .context(format!("Commit not found: {}", reference))?;

    let state_json = serde_json::to_string_pretty(&snapshot.state)?;

    if let Some(path) = output {
        std::fs::write(path, &state_json).context(format!("Failed to write to {:?}", path))?;
        println!("Restored state to {:?}", path);
    } else {
        println!("{}", state_json);
    }

    Ok(())
}

/// Replay a recorded run artifact from disk.
///
/// Expected layout:
/// - `<artifacts_dir>/<run_id>/output.json`
/// - `<artifacts_dir>/<run_id>/output.digest`
fn cmd_replay_artifact(
    run_id: &str,
    artifacts_dir: Option<&std::path::Path>,
    output: Option<&std::path::Path>,
) -> Result<()> {
    let root = artifacts_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".aivcs/runs"));
    let run_dir = root.join(run_id);
    let output_path = run_dir.join("output.json");
    let digest_path = run_dir.join("output.digest");

    if !output_path.exists() {
        anyhow::bail!("Recorded artifact not found: {:?}", output_path);
    }
    if !digest_path.exists() {
        anyhow::bail!("Recorded digest not found: {:?}", digest_path);
    }

    let artifact_bytes = std::fs::read(&output_path)
        .with_context(|| format!("Failed to read recorded artifact: {:?}", output_path))?;

    // Validate artifact is JSON for replayability.
    let _: serde_json::Value = serde_json::from_slice(&artifact_bytes)
        .with_context(|| format!("Recorded artifact is not valid JSON: {:?}", output_path))?;

    let expected_digest = std::fs::read_to_string(&digest_path)
        .with_context(|| format!("Failed to read recorded digest: {:?}", digest_path))?
        .trim()
        .to_string();

    let actual_digest = aivcs_core::Digest::compute(&artifact_bytes).to_hex();
    if actual_digest != expected_digest {
        anyhow::bail!(
            "Replay digest mismatch for run {}: expected {}, got {}",
            run_id,
            expected_digest,
            actual_digest
        );
    }

    if let Some(path) = output {
        std::fs::write(path, &artifact_bytes)
            .with_context(|| format!("Failed to write replay output to {:?}", path))?;
        println!("Replayed run {} to {:?}", run_id, path);
    } else {
        println!("{}", String::from_utf8_lossy(&artifact_bytes));
    }

    println!("Replay digest verified: {}", actual_digest);
    Ok(())
}

/// Show commit history
async fn cmd_log(handle: &SurrealHandle, reference: &str, limit: usize) -> Result<()> {
    if reference.is_empty() {
        anyhow::bail!("commit ID required: aivcs log <commit_id>");
    }
    let start_commit = reference.to_string();

    let history = handle.get_commit_history(&start_commit, limit).await?;

    if history.is_empty() {
        println!("No commits found for '{}'", reference);
        return Ok(());
    }

    for commit in history {
        println!("commit {}", commit.commit_id);
        println!("Author: {}", commit.author);
        println!(
            "Date:   {}",
            commit.created_at.format("%Y-%m-%d %H:%M:%S UTC")
        );
        println!();
        println!("    {}", commit.message);
        println!();
    }

    Ok(())
}

/// Merge two branches
async fn cmd_merge(
    handle: &SurrealHandle,
    source: &str,
    target: &str,
    message: Option<&str>,
) -> Result<()> {
    let source_commit = source.to_string();
    let target_commit = target.to_string();

    let merge_message = message
        .map(String::from)
        .unwrap_or_else(|| format!("Merge commit '{}' into '{}'", source, target));

    // Perform semantic merge
    let result = semantic_rag_merge::semantic_merge(
        handle,
        &source_commit,
        &target_commit,
        &merge_message,
        "agent-git",
    )
    .await?;

    // CODE_COMMITTED carries a `commit_sha` (git) AND an optional
    // `aivcs_commit_id` (this merge's aivcs hash). The git side has no
    // meaningful value if git isn't available, and emitting a 40-char
    // string of zeros is indistinguishable from a real SHA to downstream
    // consumers — strictly worse than not emitting at all. Skip the A2A
    // side-effect and log; the aivcs merge itself is already durable.
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    match aivcs_core::capture_head_sha(&cwd) {
        Ok(git_sha) => {
            aivcs_core::maybe_emit_code_committed_from_env(
                target,
                &git_sha,
                Vec::new(),
                "agent-git",
                None,
                Some(&result.merge_commit_id.hash),
            )
            .await;
        }
        Err(e) => {
            warn!(
                aivcs_commit_id = %result.merge_commit_id.hash,
                error = %e,
                "skipping CODE_COMMITTED emission for merge: git SHA unavailable"
            );
        }
    }

    println!("Merge complete: {}", result.merge_commit_id.short());
    println!("{}", result.summary);

    if !result.manual_conflicts.is_empty() {
        println!("\nUnresolved conflicts:");
        for conflict in &result.manual_conflicts {
            println!("  - {}", conflict.key);
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct SpecDiffOutput {
    changed_paths: Vec<String>,
    only_in_a: Vec<String>,
    only_in_b: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct RunDiffOutput {
    events_a: usize,
    events_b: usize,
    tool_call_changes: usize,
    added: usize,
    removed: usize,
    reordered: usize,
    param_changed: usize,
}

async fn cmd_diff(action: DiffAction) -> Result<()> {
    match action {
        DiffAction::Spec { a, b, json } => cmd_diff_spec(&a, &b, json),
        DiffAction::Run { a, b, json } => cmd_diff_run(&a, &b, json),
    }
}

fn cmd_diff_spec(a: &PathBuf, b: &PathBuf, json: bool) -> Result<()> {
    let left: Value = read_json_file(a)?;
    let right: Value = read_json_file(b)?;
    let diff = build_spec_diff(&left, &right);

    if json {
        println!("{}", serde_json::to_string_pretty(&diff)?);
    } else {
        println!("{}", render_spec_diff_text(&diff));
    }
    Ok(())
}

fn cmd_diff_run(a: &PathBuf, b: &PathBuf, json: bool) -> Result<()> {
    let left: Vec<RunEvent> = read_json_file(a)?;
    let right: Vec<RunEvent> = read_json_file(b)?;
    let diff = build_run_diff(&left, &right);

    if json {
        println!("{}", serde_json::to_string_pretty(&diff)?);
    } else {
        println!("{}", render_run_diff_text(&diff));
    }
    Ok(())
}

fn read_json_file<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Result<T> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read JSON file: {:?}", path))?;
    serde_json::from_str(&content).with_context(|| format!("Invalid JSON in {:?}", path))
}

fn collect_leaf_paths(prefix: &str, value: &Value, out: &mut BTreeMap<String, Value>) {
    if let Some(obj) = value.as_object() {
        for (k, v) in obj {
            let next = if prefix.is_empty() {
                format!("/{}", k.replace('~', "~0").replace('/', "~1"))
            } else {
                format!("{}/{}", prefix, k.replace('~', "~0").replace('/', "~1"))
            };
            collect_leaf_paths(&next, v, out);
        }
        return;
    }

    if let Some(arr) = value.as_array() {
        for (idx, v) in arr.iter().enumerate() {
            let next = if prefix.is_empty() {
                format!("/{}", idx)
            } else {
                format!("{}/{}", prefix, idx)
            };
            collect_leaf_paths(&next, v, out);
        }
        return;
    }

    let path = if prefix.is_empty() {
        "/".to_string()
    } else {
        prefix.to_string()
    };
    out.insert(path, value.clone());
}

fn build_spec_diff(a: &Value, b: &Value) -> SpecDiffOutput {
    let mut left = BTreeMap::new();
    let mut right = BTreeMap::new();
    collect_leaf_paths("", a, &mut left);
    collect_leaf_paths("", b, &mut right);

    let mut changed_paths = Vec::new();
    let mut only_in_a = Vec::new();
    let mut only_in_b = Vec::new();

    for (path, val_a) in &left {
        match right.get(path) {
            Some(val_b) if val_a != val_b => changed_paths.push(path.clone()),
            None => only_in_a.push(path.clone()),
            _ => {}
        }
    }

    for path in right.keys() {
        if !left.contains_key(path) {
            only_in_b.push(path.clone());
        }
    }

    SpecDiffOutput {
        changed_paths,
        only_in_a,
        only_in_b,
    }
}

fn build_run_diff(a: &[RunEvent], b: &[RunEvent]) -> RunDiffOutput {
    let tool_diff = diff_tool_calls(a, b);
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut reordered = 0usize;
    let mut param_changed = 0usize;

    for change in &tool_diff.changes {
        match change {
            ToolCallChange::Added(_) => added += 1,
            ToolCallChange::Removed(_) => removed += 1,
            ToolCallChange::Reordered { .. } => reordered += 1,
            ToolCallChange::ParamChanged { .. } => param_changed += 1,
        }
    }

    RunDiffOutput {
        events_a: a.len(),
        events_b: b.len(),
        tool_call_changes: tool_diff.changes.len(),
        added,
        removed,
        reordered,
        param_changed,
    }
}

fn render_spec_diff_text(diff: &SpecDiffOutput) -> String {
    let mut out = String::new();
    out.push_str("Spec Diff\n");
    out.push_str("=========\n");
    out.push_str(&format!("changed_paths: {}\n", diff.changed_paths.len()));
    out.push_str(&format!("only_in_a: {}\n", diff.only_in_a.len()));
    out.push_str(&format!("only_in_b: {}\n", diff.only_in_b.len()));

    if !diff.changed_paths.is_empty() {
        out.push_str("\nChanged:\n");
        for p in &diff.changed_paths {
            out.push_str(&format!("  ~ {}\n", p));
        }
    }
    if !diff.only_in_a.is_empty() {
        out.push_str("\nOnly in A:\n");
        for p in &diff.only_in_a {
            out.push_str(&format!("  - {}\n", p));
        }
    }
    if !diff.only_in_b.is_empty() {
        out.push_str("\nOnly in B:\n");
        for p in &diff.only_in_b {
            out.push_str(&format!("  + {}\n", p));
        }
    }

    out.trim_end().to_string()
}

fn render_run_diff_text(diff: &RunDiffOutput) -> String {
    format!(
        "Run Diff\n========\nevents_a: {}\nevents_b: {}\ntool_call_changes: {}\n  added: {}\n  removed: {}\n  reordered: {}\n  param_changed: {}",
        diff.events_a,
        diff.events_b,
        diff.tool_call_changes,
        diff.added,
        diff.removed,
        diff.reordered,
        diff.param_changed
    )
}

/// Truncate a string for display (with ellipsis)
fn truncate(s: &str, max_len: usize) -> String {
    let truncated: String = s.chars().take(max_len).collect();
    if s.chars().count() > max_len {
        format!("{}...", truncated)
    } else {
        truncated
    }
}

/// Truncate an ID/hash for display (no ellipsis)
fn truncate_id(s: &str, max_len: usize) -> String {
    s.chars().take(max_len).collect()
}

// ========== Environment Commands (Phase 2) ==========

/// Generate and display environment hash
async fn cmd_env_hash(path: &PathBuf) -> Result<()> {
    let hash = generate_environment_hash(path).context(format!(
        "Failed to generate environment hash for {:?}",
        path
    ))?;

    println!("Environment Hash: {}", hash.hash);
    println!("Source: {:?}", hash.source);
    println!("Short: {}", hash.short());

    Ok(())
}

/// Generate and display logic hash (Rust source)
async fn cmd_logic_hash(path: &PathBuf) -> Result<()> {
    let hash = generate_logic_hash(path)
        .context(format!("Failed to generate logic hash for {:?}", path))?;

    println!("Logic Hash: {}", hash);
    println!("Short: {}", truncate_id(&hash, 12));

    Ok(())
}

/// Show Attic cache information
async fn cmd_cache_info() -> Result<()> {
    let client = AtticClient::from_env();
    let info = client
        .get_cache_info()
        .await
        .context("Failed to get cache info")?;

    println!("Cache Name: {}", info.name);
    println!("Server: {}", info.server);
    println!("Available: {}", if info.available { "yes" } else { "no" });

    if let Some(details) = info.info {
        println!("\nDetails:");
        println!("{}", details);
    }

    Ok(())
}

/// Check if environment is cached
async fn cmd_is_cached(hash: &str) -> Result<()> {
    let client = AtticClient::from_env();
    let nix_hash = NixHash::new(hash.to_string(), nix_env_manager::HashSource::FlakeLock);

    let cached = client.is_environment_cached(&nix_hash).await;

    if cached {
        println!("Environment {} is CACHED", truncate_id(hash, 12));
    } else {
        println!("Environment {} is NOT cached", truncate_id(hash, 12));
    }

    Ok(())
}

/// Show environment system info
async fn cmd_env_info() -> Result<()> {
    use aivcs_core::domain::EnvValidation;

    let validation = EnvValidation::check();

    println!("AIVCS Environment Info");
    println!("======================");
    println!();
    println!("Platform: {}", validation.platform);
    println!(
        "Nix shell: {}",
        if validation.is_nix_shell { "yes" } else { "no" }
    );
    println!();

    // Nix availability
    let nix = is_nix_available();
    println!("Nix installed: {}", if nix { "yes" } else { "no" });

    if nix {
        // Get Nix version
        if let Ok(output) = std::process::Command::new("nix").arg("--version").output() {
            if output.status.success() {
                let version = String::from_utf8_lossy(&output.stdout);
                println!("Nix version: {}", version.trim());
            }
        }
    }

    // Attic availability
    let attic = is_attic_available();
    println!("Attic installed: {}", if attic { "yes" } else { "no" });

    if attic {
        if let Ok(output) = std::process::Command::new("attic")
            .arg("--version")
            .output()
        {
            if output.status.success() {
                let version = String::from_utf8_lossy(&output.stdout);
                println!("Attic version: {}", version.trim());
            }
        }
    }

    println!();

    // Environment variables
    println!("Environment Variables:");
    if let Ok(server) = std::env::var("ATTIC_SERVER") {
        println!("  ATTIC_SERVER: {}", server);
    } else {
        println!("  ATTIC_SERVER: (not set)");
    }
    if let Ok(cache) = std::env::var("ATTIC_CACHE") {
        println!("  ATTIC_CACHE: {}", cache);
    } else {
        println!("  ATTIC_CACHE: (not set)");
    }
    if std::env::var("ATTIC_TOKEN").is_ok() {
        println!("  ATTIC_TOKEN: (set)");
    } else {
        println!("  ATTIC_TOKEN: (not set)");
    }
    if let Some(tmp) = &validation.tmpdir {
        println!("  TMPDIR: {}", tmp);
    }

    let tips = validation.recommendations();
    if !tips.is_empty() {
        println!();
        println!("Recommendations:");
        for tip in tips {
            println!("  - {}", tip);
        }
    }

    Ok(())
}

// ========== Release Registry Commands (Phase 4) ==========

#[allow(clippy::too_many_arguments)]
async fn cmd_release_promote(
    handle: &SurrealHandle,
    name: &str,
    git_sha: &str,
    graph_digest: &str,
    prompts_digest: &str,
    tools_digest: &str,
    config_digest: &str,
    promoted_by: &str,
    version: Option<&str>,
    notes: Option<&str>,
) -> Result<()> {
    let spec = aivcs_core::AgentSpec::new(
        git_sha.to_string(),
        graph_digest.to_string(),
        prompts_digest.to_string(),
        tools_digest.to_string(),
        config_digest.to_string(),
    )
    .context("failed to build AgentSpec")?;

    let registry = SurrealDbReleaseRegistry::new(Arc::new(handle.clone()));
    let api = aivcs_core::ReleaseRegistryApi::new(registry);

    let release = api
        .promote(
            name,
            &spec,
            promoted_by,
            version.map(ToString::to_string),
            notes.map(ToString::to_string),
        )
        .await
        .context("promote failed")?;

    println!(
        "Promoted {} -> {}",
        release.name,
        release.spec_digest.as_str()
    );
    Ok(())
}

async fn cmd_release_rollback(handle: &SurrealHandle, name: &str) -> Result<()> {
    let registry = SurrealDbReleaseRegistry::new(Arc::new(handle.clone()));
    let release = registry.rollback(name).await?;
    println!(
        "Rolled back {} -> {}",
        release.name,
        release.spec_digest.as_str()
    );
    Ok(())
}

async fn cmd_release_current(handle: &SurrealHandle, name: &str) -> Result<()> {
    let registry = SurrealDbReleaseRegistry::new(Arc::new(handle.clone()));
    let current = registry.current(name).await?;
    match current {
        Some(release) => {
            println!(
                "Current {} -> {}",
                release.name,
                release.spec_digest.as_str()
            );
        }
        None => println!("No release found for {}", name),
    }
    Ok(())
}

async fn cmd_release_history(handle: &SurrealHandle, name: &str) -> Result<()> {
    let registry = SurrealDbReleaseRegistry::new(Arc::new(handle.clone()));
    let history = registry.history(name).await?;

    if history.is_empty() {
        println!("No release history for {}", name);
        return Ok(());
    }

    for release in history {
        println!(
            "{} {} {}",
            release.created_at.to_rfc3339(),
            release.name,
            release.spec_digest.as_str()
        );
    }
    Ok(())
}

// ========== Parallel Simulation Commands (Phase 4) ==========

/// Fork multiple parallel branches for exploration
async fn cmd_fork(handle: &SurrealHandle, parent: &str, count: u8, prefix: &str) -> Result<()> {
    let parent_commit = parent.to_string();

    println!(
        "Forking {} parallel commits from {} with prefix '{}'",
        count,
        truncate_id(&parent_commit, 8),
        prefix
    );

    let handle_arc = Arc::new(handle.clone());

    let result = fork_agent_parallel(handle_arc, &parent_commit, count, prefix).await?;

    println!("\nCreated {} parallel forks:", result.fork_labels.len());
    for (i, label) in result.fork_labels.iter().enumerate() {
        println!("  {} -> {}", label, result.commit_ids[i].short());
    }

    println!("\nUse 'aivcs trace <commit>' to inspect a fork's reasoning");

    Ok(())
}

/// Show reasoning trace for time-travel debugging
async fn cmd_trace(handle: &SurrealHandle, reference: &str, depth: usize) -> Result<()> {
    let commit_hash = reference.to_string();

    println!("Reasoning Trace for {}", truncate_id(&commit_hash, 12));
    println!("=========================================\n");

    // Get commit history (limited by depth)
    let history = handle.get_commit_history(&commit_hash, depth).await?;

    if history.is_empty() {
        println!("No commits found for '{}'", reference);
        return Ok(());
    }

    // Load snapshots and display trace
    for (i, commit) in history.iter().enumerate() {
        let step_marker = if i == 0 { "HEAD" } else { &format!("~{}", i) };

        println!(
            "[{}] {} - {}",
            step_marker,
            commit.commit_id.short(),
            commit.message
        );
        println!(
            "    Author: {} | {}",
            commit.author,
            commit.created_at.format("%Y-%m-%d %H:%M:%S")
        );

        // Try to load and display state summary
        if let Ok(snapshot) = handle.load_snapshot(&commit.commit_id.hash).await {
            if let Some(obj) = snapshot.state.as_object() {
                // Show key state fields
                let keys: Vec<_> = obj.keys().take(5).collect();
                for key in keys {
                    if let Some(value) = obj.get(key) {
                        let value_str = match value {
                            serde_json::Value::String(s) => truncate(s, 40),
                            serde_json::Value::Number(n) => n.to_string(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            _ => format!("{}", value).chars().take(40).collect(),
                        };
                        println!("    {}: {}", key, value_str);
                    }
                }
            }
        }
        println!();
    }

    println!(
        "Showing {} of {} commits (use --depth to see more)",
        history.len(),
        depth
    );

    Ok(())
}

/// Diff the tool-call sequences of two runs
async fn cmd_diff_runs(ledger: &dyn RunLedger, id_a: &str, id_b: &str) -> Result<()> {
    let (events_a, summary_a) = aivcs_core::replay_run(ledger, id_a)
        .await
        .with_context(|| format!("replay failed for run: {}", id_a))?;
    let (events_b, summary_b) = aivcs_core::replay_run(ledger, id_b)
        .await
        .with_context(|| format!("replay failed for run: {}", id_b))?;

    let diff = diff_tool_calls(&events_a, &events_b);

    println!("A: {} ({})", summary_a.run_id, summary_a.agent_name);
    println!("B: {} ({})", summary_b.run_id, summary_b.agent_name);
    println!();

    if diff.is_empty() {
        println!("Tool-call sequences are identical.");
        return Ok(());
    }

    for change in &diff.changes {
        match change {
            ToolCallChange::Added(call) => {
                println!("  + [{}] {}", call.seq, call.tool_name);
            }
            ToolCallChange::Removed(call) => {
                println!("  - [{}] {}", call.seq, call.tool_name);
            }
            ToolCallChange::Reordered {
                call,
                from_index,
                to_index,
            } => {
                println!(
                    "  ~ {} (pos {} -> {})",
                    call.tool_name, from_index, to_index
                );
            }
            ToolCallChange::ParamChanged {
                tool_name,
                seq_a,
                seq_b,
                deltas,
            } => {
                println!("  Δ {} (A:[{}] / B:[{}])", tool_name, seq_a, seq_b);
                for d in deltas {
                    println!("      {} : {} -> {}", d.key, d.before, d.after);
                }
            }
        }
    }

    println!("\nChanges: {}", diff.changes.len());
    Ok(())
}

/// Run CI stages and record execution
async fn cmd_ci_run(
    workspace: &PathBuf,
    stages_str: &str,
    no_cache: bool,
    fix: bool,
) -> Result<()> {
    if no_cache {
        eprintln!("warning: --no-cache is not yet implemented; proceeding without caching changes");
    }
    if fix {
        eprintln!("warning: --fix is not yet implemented; stages will run in check-only mode");
    }

    // Get git SHA
    let git_sha = if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
    {
        String::from_utf8(output.stdout)
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        "unknown".to_string()
    };

    // Get toolchain hash
    let toolchain_hash = if let Ok(output) = std::process::Command::new("rustup")
        .args(["show", "active-toolchain"])
        .output()
    {
        String::from_utf8(output.stdout)
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        "unknown".to_string()
    };

    // Parse stages
    let stage_names: Vec<String> = stages_str
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .collect();

    let mut stage_configs = Vec::new();
    for stage_name in &stage_names {
        let config = match stage_name.as_str() {
            "fmt" => StageConfig::from_builtin(BuiltinStage::CargoFmt, 300),
            "check" => StageConfig::from_builtin(BuiltinStage::CargoCheck, 300),
            "clippy" => StageConfig::from_builtin(BuiltinStage::CargoClippy, 600),
            "test" => StageConfig::from_builtin(BuiltinStage::CargoTest, 1200),
            _ => anyhow::bail!("Unknown stage: {}", stage_name),
        };
        stage_configs.push(config);
    }

    // Create CI spec
    let ci_spec = CiSpec::new(
        workspace.clone(),
        &stage_names,
        git_sha.clone(),
        toolchain_hash.clone(),
    );

    println!("Running CI pipeline for workspace: {:?}", workspace);
    println!("Stages: {}", stages_str);
    println!("Git SHA: {}", git_sha);
    println!();

    // Run pipeline
    let ledger_arc = std::sync::Arc::new(oxidized_state::SurrealRunLedger::from_env().await?);
    let result = CiPipeline::run(ledger_arc.clone(), &ci_spec, stage_configs)
        .await
        .context("CI pipeline failed to run")?;

    // Print results
    println!("Run ID: {}", result.run_id);
    println!(
        "Status: {}",
        if result.success {
            "✓ PASSED"
        } else {
            "✗ FAILED"
        }
    );
    println!("Duration: {}ms", result.duration_ms);
    println!();

    for stage_result in &result.stages {
        let status = if stage_result.passed() { "✓" } else { "✗" };
        println!(
            "  {} {} ({}ms, exit code: {})",
            status, stage_result.stage_name, stage_result.duration_ms, stage_result.exit_code
        );
    }

    println!();
    println!(
        "Summary: {}/{} stages passed",
        result.passed_count(),
        result.stages.len()
    );

    // Evaluate gate
    let events = ledger_arc
        .get_events(&oxidized_state::RunId(result.run_id.clone()))
        .await?;

    let verdict = CiGate::evaluate(&events);
    println!(
        "Gate: {}",
        if verdict.passed {
            "✓ PASSED"
        } else {
            "✗ FAILED"
        }
    );

    if !verdict.violations.is_empty() {
        println!("Violations:");
        for violation in &verdict.violations {
            println!("  - {}", violation);
        }
    }

    if result.success && verdict.passed {
        println!("\n✓ All checks passed!");
        Ok(())
    } else {
        anyhow::bail!("CI checks failed")
    }
}

async fn cmd_pr_note(handle: &SurrealHandle, commit_id: &str) -> Result<()> {
    let commit = resolve_commit(handle, commit_id).await?;

    // Format output matching acceptance criteria.
    //
    // The `<!-- aivcs-linkage -->` HTML comment is a *visible section
    // header* for the metadata that follows. The lines underneath
    // (`aivcs-commit:`, `intent-id:`) are human-readable plain text and
    // are the durable index into the aivcs ledger — the comment marker
    // is just an anchor that reviewers (and the Phase 1 verifier) can
    // locate without parsing markdown headings. This is a different
    // pattern from `<!-- aivcs-ci-snapshot: sha256:... -->` over in
    // `ci_snapshot`, where the comment itself buries an opaque
    // workspace digest as the payload. Here the comment is metadata
    // *about* the section, not the metadata payload.
    // See stevedores-org/aivcs#231 (M2).
    println!("<!-- aivcs-linkage -->");
    println!("aivcs-commit: {}", commit.commit_id.hash);
    if let Some(intent_id) = read_objective_id() {
        println!("intent-id: {}", intent_id);
    }
    println!();
    println!("### AIVCS Commit Summary");
    println!("- **Message**: {}", commit.message);
    println!("- **Author**: {}", commit.author);
    println!("- **Created**: {}", commit.created_at);
    println!("- **State Hash**: {}", commit.commit_id.state_hash);
    if let Some(logic) = &commit.commit_id.logic_hash {
        println!("- **Logic Hash**: {}", logic);
    }
    if let Some(env) = &commit.commit_id.env_hash {
        println!("- **Env Hash**: {}", env);
    }

    Ok(())
}

async fn resolve_commit(handle: &SurrealHandle, commit_id: &str) -> Result<CommitRecord> {
    handle
        .get_commit(commit_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Commit '{}' not found in AIVCS database", commit_id))
}

async fn cmd_pr_semantic_summary(
    handle: &SurrealHandle,
    head_id: &str,
    base_id: &str,
) -> Result<()> {
    use aivcs_core::{diff_graph_snapshots, extract_graph_snapshot, format_semantic_diff_markdown};

    let head_commit = resolve_commit(handle, head_id).await?;
    let base_commit = resolve_commit(handle, base_id).await?;

    cmd_pr_note(handle, head_id).await?;

    let head_snapshot = handle
        .load_snapshot(&head_commit.commit_id.hash)
        .await
        .context("Failed to load head commit snapshot")?;
    let base_snapshot = handle
        .load_snapshot(&base_commit.commit_id.hash)
        .await
        .context("Failed to load base commit snapshot")?;

    let head_graph = extract_graph_snapshot(&head_snapshot.state);
    let base_graph = extract_graph_snapshot(&base_snapshot.state);

    println!();
    println!("### Semantic diff (`{base_id}` → `{head_id}`)");
    println!();

    if !head_graph.has_semantic_content() && !base_graph.has_semantic_content() {
        println!("_No semantic delta — only code/infra changes._");
        return Ok(());
    }

    let diff = diff_graph_snapshots(&base_graph, &head_graph);
    println!("{}", format_semantic_diff_markdown(&diff));

    Ok(())
}

fn read_objective_id() -> Option<String> {
    let content = std::fs::read_to_string("intent/current.yaml").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.trim().strip_prefix("objective_id:") {
            return Some(rest.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_agent_git_snapshot_cli_returns_valid_id() {
        let handle = SurrealHandle::setup_db().await.unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        cmd_init(&handle, &temp_dir.path().to_path_buf())
            .await
            .unwrap();

        let initial_state = serde_json::json!({
            "initialized": true,
            "version": env!("CARGO_PKG_VERSION"),
        });
        let init_id = CommitId::from_state(serde_json::to_vec(&initial_state).unwrap().as_slice());

        let state_path = temp_dir.path().join("state.json");
        std::fs::write(&state_path, r#"{"step": 1, "value": "test"}"#).unwrap();

        let result = cmd_snapshot(
            &handle,
            &state_path,
            "Test snapshot",
            "test-agent",
            Some(&init_id.hash),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            Some(temp_dir.path().join("cas").as_path()),
        )
        .await;

        assert!(result.is_ok(), "Snapshot failed: {:?}", result.err());
        assert!(handle.get_commit(&init_id.hash).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_pr_note_command() {
        let handle = SurrealHandle::setup_db().await.unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        cmd_init(&handle, &temp_dir.path().to_path_buf())
            .await
            .unwrap();

        let state_path = temp_dir.path().join("state.json");
        std::fs::write(&state_path, r#"{"step": 1, "value": "test"}"#).unwrap();
        let cas_dir = temp_dir.path().join("cas");

        let commit_id = cmd_snapshot(
            &handle,
            &state_path,
            "Initial commit",
            "test-author",
            None,
            Some("1234567890abcdef1234567890abcdef12345678"),
            Some(&cas_dir),
        )
        .await
        .unwrap();

        let res = cmd_pr_note(&handle, &commit_id.hash).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_pr_semantic_summary_command() {
        let handle = SurrealHandle::setup_db().await.unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        cmd_init(&handle, &temp_dir.path().to_path_buf())
            .await
            .unwrap();

        let base_state = temp_dir.path().join("base.json");
        std::fs::write(
            &base_state,
            r#"{
                "graph": {
                    "entry": "start",
                    "exits": ["end"],
                    "nodes": ["start", "plan"],
                    "edges": [{"from": "start", "to": "plan"}]
                },
                "prompts": {"plan": "Plan v1"}
            }"#,
        )
        .unwrap();
        let base_id = cmd_snapshot(
            &handle,
            &base_state,
            "base graph",
            "agent",
            None,
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            Some(temp_dir.path().join("cas").as_path()),
        )
        .await
        .unwrap();

        let head_state = temp_dir.path().join("head.json");
        std::fs::write(
            &head_state,
            r#"{
                "graph": {
                    "entry": "start",
                    "exits": ["end"],
                    "nodes": ["start", "plan", "review"],
                    "edges": [
                        {"from": "start", "to": "plan"},
                        {"from": "plan", "to": "review"}
                    ]
                },
                "prompts": {"plan": "Plan v2", "review": "Review output"}
            }"#,
        )
        .unwrap();
        let head_id = cmd_snapshot(
            &handle,
            &head_state,
            "feature graph",
            "agent",
            Some(&base_id.hash),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            Some(temp_dir.path().join("cas").as_path()),
        )
        .await
        .unwrap();

        let res = cmd_pr_semantic_summary(&handle, &head_id.hash, &base_id.hash).await;
        assert!(res.is_ok(), "pr-semantic-summary failed: {:?}", res.err());
    }

    #[tokio::test]
    async fn test_pr_verify_reproducibility() {
        let repo_root = aivcs_core::find_repo_root();
        println!(
            "test_pr_verify_reproducibility: repo_root = {:?}",
            repo_root
        );
        let state_hash = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";

        // Mock the CAS file location locally just for this test
        let cas_dir = repo_root
            .join(".aivcs")
            .join("cas")
            .join("objects")
            .join(&state_hash[..2]);
        std::fs::create_dir_all(&cas_dir).unwrap();
        let cas_file = cas_dir.join(&state_hash[2..]);
        std::fs::write(&cas_file, b"test").unwrap();

        let workspace_env = nix_env_manager::generate_environment_hash(&repo_root).unwrap();
        println!(
            "test_pr_verify_reproducibility: workspace_env = {:?}",
            workspace_env
        );
        let computed = CommitId::new(None, state_hash, Some(&workspace_env.hash));

        let pr_body = format!(
            "aivcs-commit: {}\n- **State Hash**: {}\n- **Env Hash**: {}\n",
            computed.hash, state_hash, workspace_env.hash
        );

        let res = cmd_pr_verify_reproducibility(Some(pr_body)).await;

        // Cleanup mock CAS file
        let _ = std::fs::remove_file(&cas_file);

        assert!(res.is_ok(), "reproducibility check failed: {:?}", res.err());
    }

    #[tokio::test]
    async fn test_pr_verify_reproducibility_docs_only_exempt() {
        let pr_body = "aivcs-commit: code-only-docs-change-no-cognitive-snapshot\n".to_string();
        let res = cmd_pr_verify_reproducibility(Some(pr_body)).await;
        assert!(
            res.is_ok(),
            "docs-only exempt sentinel should pass: {:?}",
            res.err()
        );
    }

    #[tokio::test]
    async fn test_cmd_fork_creates_forks_in_same_db() {
        let handle = SurrealHandle::setup_db().await.unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        cmd_init(&handle, &temp_dir.path().to_path_buf())
            .await
            .unwrap();

        let initial_state = serde_json::json!({
            "initialized": true,
            "version": env!("CARGO_PKG_VERSION"),
        });
        let init_id = CommitId::from_state(serde_json::to_vec(&initial_state).unwrap().as_slice());

        let state_path = temp_dir.path().join("state.json");
        std::fs::write(&state_path, r#"{"step": 1, "value": "test"}"#).unwrap();
        let base_id = cmd_snapshot(
            &handle,
            &state_path,
            "Base",
            "agent",
            Some(&init_id.hash),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            Some(temp_dir.path().join("cas").as_path()),
        )
        .await
        .unwrap();

        let result = cmd_fork(&handle, &base_id.hash, 2, "test-fork").await;

        assert!(result.is_ok(), "Fork failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_snapshot_linked_to_git_sha() {
        let handle = SurrealHandle::setup_db().await.unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        cmd_init(&handle, &temp_dir.path().to_path_buf())
            .await
            .unwrap();

        let state_path = temp_dir.path().join("state.json");
        std::fs::write(&state_path, r#"{"model": "gpt-4", "step": 42}"#).unwrap();

        let git_sha = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let cas_dir = temp_dir.path().join("cas");

        let commit_id = cmd_snapshot(
            &handle,
            &state_path,
            "Linked to git commit",
            "agent-v1",
            None,
            Some(git_sha),
            Some(cas_dir.as_path()),
        )
        .await
        .unwrap();

        assert!(handle.get_commit(&commit_id.hash).await.unwrap().is_some());

        let store = aivcs_core::FsCasStore::new(&cas_dir).unwrap();
        let state_content = std::fs::read_to_string(&state_path).unwrap();
        let digest = aivcs_core::Digest::compute(state_content.as_bytes());
        assert!(
            aivcs_core::CasStore::exists(&store, &digest).unwrap(),
            "State should exist in CAS after snapshot"
        );

        // Verify CAS roundtrip
        let retrieved = aivcs_core::CasStore::get(&store, &digest).unwrap();
        assert_eq!(retrieved, state_content.as_bytes());
    }

    #[tokio::test]
    async fn test_snapshot_auto_detects_git_sha_in_repo() {
        // Create a temporary git repo
        let temp_dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "test-user"])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "initial"])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();

        // Capture the actual HEAD SHA
        let expected_sha = aivcs_core::capture_head_sha(temp_dir.path()).unwrap();

        assert_eq!(expected_sha.len(), 40);
        assert!(expected_sha.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_replay_golden_digest_equality() {
        let temp_dir = tempfile::tempdir().unwrap();
        let run_id = "run-golden-1";
        let run_dir = temp_dir.path().join(run_id);
        std::fs::create_dir_all(&run_dir).unwrap();

        let output_bytes = br#"{"result":"ok","tokens":42}"#;
        std::fs::write(run_dir.join("output.json"), output_bytes).unwrap();
        let digest = aivcs_core::Digest::compute(output_bytes).to_hex();
        std::fs::write(run_dir.join("output.digest"), format!("{}\n", digest)).unwrap();

        let replayed = temp_dir.path().join("replayed.json");
        let result = cmd_replay_artifact(run_id, Some(temp_dir.path()), Some(replayed.as_path()));
        assert!(result.is_ok(), "replay failed: {:?}", result.err());

        let written = std::fs::read(replayed).unwrap();
        assert_eq!(written, output_bytes);
    }

    #[test]
    fn test_replay_missing_artifact_rejected() {
        let temp_dir = tempfile::tempdir().unwrap();
        let run_id = "run-missing-1";

        let err = cmd_replay_artifact(run_id, Some(temp_dir.path()), None).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Recorded artifact not found"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_spec_diff_json_output_stability() {
        let a = json!({
            "model": "gpt-4",
            "routing": {"strategy": "math"},
            "threshold": 0.9
        });
        let b = json!({
            "model": "gpt-4o",
            "routing": {"strategy": "search"},
            "new_flag": true
        });

        let diff = build_spec_diff(&a, &b);
        let actual = serde_json::to_string_pretty(&diff).unwrap();
        let expected = r#"{
  "changed_paths": [
    "/model",
    "/routing/strategy"
  ],
  "only_in_a": [
    "/threshold"
  ],
  "only_in_b": [
    "/new_flag"
  ]
}"#;

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_run_diff_json_output_stability() {
        let a: Vec<RunEvent> = serde_json::from_value(json!([{
            "seq": 1,
            "kind": "tool_called",
            "payload": {"tool_name":"search","query":"rust"},
            "timestamp": "2026-01-01T00:00:00Z"
        }]))
        .unwrap();
        let b: Vec<RunEvent> = serde_json::from_value(json!([{
            "seq": 1,
            "kind": "tool_called",
            "payload": {"tool_name":"search","query":"python"},
            "timestamp": "2026-01-01T00:00:00Z"
        }]))
        .unwrap();

        let diff = build_run_diff(&a, &b);
        let actual = serde_json::to_string_pretty(&diff).unwrap();
        let expected = r#"{
  "events_a": 1,
  "events_b": 1,
  "tool_call_changes": 1,
  "added": 0,
  "removed": 0,
  "reordered": 0,
  "param_changed": 1
}"#;

        assert_eq!(actual, expected);
    }
}
