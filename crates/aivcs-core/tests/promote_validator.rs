use aivcs_core::domain::agent_spec::AgentSpec;
use aivcs_core::domain::error::AivcsError;
use aivcs_core::ReleaseRegistryApi;
use oxidized_state::fakes::MemoryReleaseRegistry;

fn make_spec(seed: &str) -> AgentSpec {
    AgentSpec::new(
        "abc123def456abc123def456abc123def456abc1".to_string(),
        format!("graph-{}", seed),
        format!("prompts-{}", seed),
        format!("tools-{}", seed),
        format!("config-{}", seed),
    )
    .expect("make_spec")
}

fn api() -> ReleaseRegistryApi<MemoryReleaseRegistry> {
    ReleaseRegistryApi::new(MemoryReleaseRegistry::new())
}

#[tokio::test]
async fn valid_spec_promotes_to_registry() {
    let api = api();
    let spec = make_spec("v1");

    let record = api
        .promote("my-agent", &spec, "ci", None, None)
        .await
        .expect("valid spec should promote");

    assert_eq!(record.name, "my-agent");
    assert_eq!(record.spec_digest.as_str(), spec.spec_digest);
}

#[tokio::test]
async fn promote_rejected_when_tools_digest_empty() {
    let api = api();
    let mut spec = make_spec("v1");
    spec.tools_digest = String::new();

    let err = api
        .promote("agent", &spec, "ci", None, None)
        .await
        .unwrap_err();

    assert!(
        matches!(err, AivcsError::InvalidAgentSpec(ref msg) if msg.contains("tools_digest")),
        "unexpected error: {:?}",
        err
    );
}

#[tokio::test]
async fn promote_rejected_when_graph_digest_empty() {
    let api = api();
    let mut spec = make_spec("v1");
    spec.graph_digest = String::new();

    let err = api
        .promote("agent", &spec, "ci", None, None)
        .await
        .unwrap_err();

    assert!(
        matches!(err, AivcsError::InvalidAgentSpec(ref msg) if msg.contains("graph_digest")),
        "unexpected error: {:?}",
        err
    );
}

#[tokio::test]
async fn promote_rejected_when_prompts_digest_empty() {
    let api = api();
    let mut spec = make_spec("v1");
    spec.prompts_digest = String::new();

    let err = api
        .promote("agent", &spec, "ci", None, None)
        .await
        .unwrap_err();

    assert!(
        matches!(err, AivcsError::InvalidAgentSpec(ref msg) if msg.contains("prompts_digest")),
        "unexpected error: {:?}",
        err
    );
}

#[tokio::test]
async fn promote_rejected_when_config_digest_empty() {
    let api = api();
    let mut spec = make_spec("v1");
    spec.config_digest = String::new();

    let err = api
        .promote("agent", &spec, "ci", None, None)
        .await
        .unwrap_err();

    assert!(
        matches!(err, AivcsError::InvalidAgentSpec(ref msg) if msg.contains("config_digest")),
        "unexpected error: {:?}",
        err
    );
}

#[tokio::test]
async fn promote_rejected_when_git_sha_empty() {
    let api = api();
    let mut spec = make_spec("v1");
    spec.git_sha = String::new();

    let err = api
        .promote("agent", &spec, "ci", None, None)
        .await
        .unwrap_err();

    assert!(
        matches!(err, AivcsError::InvalidAgentSpec(ref msg) if msg.contains("git_sha")),
        "unexpected error: {:?}",
        err
    );
}

#[tokio::test]
async fn promote_rejected_when_spec_digest_mismatches_components() {
    let api = api();
    let mut spec = make_spec("v1");
    // Tamper with a component after spec_digest was computed
    spec.graph_digest = "graph-TAMPERED".to_string();

    let err = api
        .promote("agent", &spec, "ci", None, None)
        .await
        .unwrap_err();

    assert!(
        matches!(err, AivcsError::DigestMismatch { .. }),
        "expected DigestMismatch, got {:?}",
        err
    );
}

#[tokio::test]
async fn component_check_precedes_digest_check() {
    let api = api();
    let mut spec = make_spec("v1");
    // Empty a component AND tamper another — component check should fire first
    spec.tools_digest = String::new();
    spec.graph_digest = "tampered".to_string();

    let err = api
        .promote("agent", &spec, "ci", None, None)
        .await
        .unwrap_err();

    // graph_digest is checked before tools_digest but "tampered" is non-empty,
    // so prompts_digest (non-empty) passes, tools_digest (empty) should be the error
    assert!(
        matches!(err, AivcsError::InvalidAgentSpec(ref msg) if msg.contains("tools_digest")),
        "expected tools_digest error first, got: {:?}",
        err
    );
}
