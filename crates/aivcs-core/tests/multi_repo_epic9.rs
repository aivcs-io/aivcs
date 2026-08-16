//! Integration tests for EPIC9: Multi-Repo and CI/CD Orchestration (Issue #131).
//!
//! Acceptance criteria covered:
//! - Cross-repo changes execute in dependency order
//! - Downstream breakage blocks rollout automatically
//! - CI signals consolidate per objective
//! - Release artifacts map to plan/run provenance

use aivcs_core::domain::ci::CIStatus;
use aivcs_core::multi_repo::{
    CIHealthView, CrossRepoGraph, MultiRepoOrchestrator, ReleaseProvenance, RepoDependency, RepoId,
};
use chrono::Utc;
use uuid::Uuid;

fn repo_id(name: &str) -> RepoId {
    RepoId::new(name)
}

// ---- Cross-repo execution order ----

#[test]
fn epic9_execution_order_dependency_first() {
    let a = repo_id("org/lib-a");
    let b = repo_id("org/lib-b");
    let c = repo_id("org/app");
    let graph = CrossRepoGraph::new(
        vec![a.clone(), b.clone(), c.clone()],
        vec![
            RepoDependency {
                dependent: b.clone(),
                dependency: a.clone(),
            },
            RepoDependency {
                dependent: c.clone(),
                dependency: b.clone(),
            },
        ],
    );
    let plan = MultiRepoOrchestrator::execution_plan(&graph).expect("acyclic graph");
    assert_eq!(plan.order.len(), 3);
    assert_eq!(plan.order[0].name, "org/lib-a");
    assert_eq!(plan.order[1].name, "org/lib-b");
    assert_eq!(plan.order[2].name, "org/app");
}

#[test]
fn epic9_execution_order_cycle_returns_error() {
    let a = repo_id("a");
    let b = repo_id("b");
    let graph = CrossRepoGraph::new(
        vec![a.clone(), b.clone()],
        vec![
            RepoDependency {
                dependent: b.clone(),
                dependency: a.clone(),
            },
            RepoDependency {
                dependent: a.clone(),
                dependency: b.clone(),
            },
        ],
    );
    let res = MultiRepoOrchestrator::execution_plan(&graph);
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(err.to_string().to_lowercase().contains("cycle"));
}

// ---- Downstream breakage blocks rollout ----

#[test]
fn epic9_rollout_blocked_when_any_repo_fails() {
    let health = CIHealthView::new(
        "release-v1",
        vec![
            aivcs_core::RepoCIStatus {
                repo: repo_id("org/a"),
                status: CIStatus::Passed,
                run_id: Some(Uuid::new_v4()),
                updated_at: Utc::now(),
            },
            aivcs_core::RepoCIStatus {
                repo: repo_id("org/b"),
                status: CIStatus::Failed,
                run_id: Some(Uuid::new_v4()),
                updated_at: Utc::now(),
            },
        ],
    );
    assert!(health.rollout_blocked());
    assert!(!health.can_rollout());
    assert!(MultiRepoOrchestrator::check_rollout_gate(&health).is_err());
}

#[test]
fn epic9_rollout_allowed_when_all_pass() {
    let health = CIHealthView::new(
        "release-v1",
        vec![
            aivcs_core::RepoCIStatus {
                repo: repo_id("org/a"),
                status: CIStatus::Passed,
                run_id: Some(Uuid::new_v4()),
                updated_at: Utc::now(),
            },
            aivcs_core::RepoCIStatus {
                repo: repo_id("org/b"),
                status: CIStatus::Passed,
                run_id: Some(Uuid::new_v4()),
                updated_at: Utc::now(),
            },
        ],
    );
    assert!(!health.rollout_blocked());
    assert!(health.can_rollout());
    assert!(MultiRepoOrchestrator::check_rollout_gate(&health).is_ok());
}

// ---- CI signals consolidate per objective ----

#[test]
fn epic9_consolidate_health_per_objective() {
    let repos = vec![
        aivcs_core::RepoCIStatus {
            repo: repo_id("stevedores-org/aivcs"),
            status: CIStatus::Passed,
            run_id: Some(Uuid::new_v4()),
            updated_at: Utc::now(),
        },
        aivcs_core::RepoCIStatus {
            repo: repo_id("stevedores-org/oxidizedRAG"),
            status: CIStatus::Passed,
            run_id: Some(Uuid::new_v4()),
            updated_at: Utc::now(),
        },
    ];
    let view = MultiRepoOrchestrator::consolidate_health("epic9-plan-1", repos);
    assert_eq!(view.objective_id, "epic9-plan-1");
    assert_eq!(view.repos.len(), 2);
    assert_eq!(view.overall_status(), CIStatus::Passed);
}

// ---- Release provenance linkage ----

#[test]
fn epic9_release_provenance_links_artifact_to_run() {
    let run_id = Uuid::new_v4();
    let p = ReleaseProvenance::new(
        repo_id("stevedores-org/aivcs"),
        run_id,
        "abc123def".to_string(),
        "spec-digest-64chars".to_string(),
        Some("plan-131".to_string()),
    );
    assert_eq!(p.repo.name, "stevedores-org/aivcs");
    assert_eq!(p.run_id, run_id);
    assert_eq!(p.git_sha, "abc123def");
    assert_eq!(p.spec_digest, "spec-digest-64chars");
    assert_eq!(p.plan_id.as_deref(), Some("plan-131"));
}

#[test]
fn epic9_release_provenance_serde_roundtrip() {
    let p = ReleaseProvenance::new(
        repo_id("org/repo"),
        Uuid::new_v4(),
        "sha".to_string(),
        "digest".to_string(),
        None,
    );
    let json = serde_json::to_string(&p).expect("serialize");
    let back: ReleaseProvenance = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(p.repo, back.repo);
    assert_eq!(p.run_id, back.run_id);
    assert_eq!(p.git_sha, back.git_sha);
}
