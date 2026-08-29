//! `DataMeshRunStore` against a simulated data-fabric (wiremock) — validates the
//! collection/query mapping and the event-sourced newest-per-run fold without a
//! live data-mesh.

use aivcs_runs::{DataMeshRunStore, RunStatus, RunStore};
use data_mesh::{Client, ClientConfig};
use serde_json::json;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn store_for(server: &MockServer) -> DataMeshRunStore {
    let client = Client::new(ClientConfig {
        base_url: server.uri(),
        tenant_id: "t".into(),
        tenant_role: "builder".into(),
        cf_client_id: None,
        cf_client_secret: None,
        bearer_token: None,
    });
    DataMeshRunStore::new(client)
}

#[tokio::test]
async fn enqueue_posts_to_ci_executions_and_returns_queued_run() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"/v1/collections/ci(_|%5F)executions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":"srv-1","status":"created"})),
        )
        .mount(&server)
        .await;

    let store = store_for(&server);
    let run = store.enqueue("lornu-ai", "aivcs.io").await.unwrap();
    assert_eq!(run.status, RunStatus::Queued);
    assert_eq!(run.owner, "lornu-ai");
    assert_eq!(run.repo, "aivcs.io");
    assert!(run.run_id.starts_with("run_"));
}

#[tokio::test]
async fn list_by_repo_folds_to_newest_event_per_run() {
    let server = MockServer::start().await;
    // Newest-first: run_A has a later `running` event over its `queued` event.
    let docs = json!({"documents":[
        {"run_id":"run_A","owner":"o","repo":"r","status":"running","created_at":"2026-07-29T00:00:00Z","repo_key":"o/r"},
        {"run_id":"run_A","owner":"o","repo":"r","status":"queued","created_at":"2026-07-29T00:00:00Z","repo_key":"o/r"},
        {"run_id":"run_B","owner":"o","repo":"r","status":"queued","created_at":"2026-07-29T00:00:01Z","repo_key":"o/r"}
    ]});
    Mock::given(method("GET"))
        .and(path_regex(r"/v1/collections/ci(_|%5F)executions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(docs))
        .mount(&server)
        .await;

    let store = store_for(&server);
    let runs = store.list_by_repo("o", "r").await.unwrap();
    assert_eq!(runs.len(), 2, "one entry per run_id");
    let a = runs
        .iter()
        .find(|r| r.run_id == "run_A")
        .expect("run_A present");
    assert_eq!(a.status, RunStatus::Running, "newest event wins the fold");
}

#[tokio::test]
async fn get_returns_the_latest_event() {
    let server = MockServer::start().await;
    let docs = json!({"documents":[
        {"run_id":"run_A","owner":"o","repo":"r","status":"succeeded","created_at":"2026-07-29T00:00:00Z","repo_key":"o/r"}
    ]});
    Mock::given(method("GET"))
        .and(path_regex(r"/v1/collections/ci(_|%5F)executions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(docs))
        .mount(&server)
        .await;

    let store = store_for(&server);
    let got = store.get("run_A").await.unwrap().expect("run exists");
    assert_eq!(got.status, RunStatus::Succeeded);
}

/// One queued run; the idempotent claim write succeeds → this engine wins.
#[tokio::test]
async fn claim_wins_when_write_is_created() {
    let server = MockServer::start().await;
    let queued = json!({"documents":[
        {"run_id":"run_A","owner":"o","repo":"r","status":"queued","created_at":"2026-07-29T00:00:00Z","repo_key":"o/r"}
    ]});
    // Both the status scan and the run_id confirm return the queued run.
    Mock::given(method("GET"))
        .and(path_regex(r"/v1/collections/ci(_|%5F)executions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(queued))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/v1/collections/ci(_|%5F)executions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":"x","status":"created"})),
        )
        .mount(&server)
        .await;

    let store = store_for(&server);
    let claimed = store
        .claim_next_queued()
        .await
        .unwrap()
        .expect("won the claim");
    assert_eq!(claimed.run_id, "run_A");
    assert_eq!(claimed.status, RunStatus::Running);
}

/// The idempotent claim write dedups (`status = "exists"`) → another engine
/// already claimed it, so this engine must not double-claim.
#[tokio::test]
async fn claim_loses_race_when_write_already_exists() {
    let server = MockServer::start().await;
    let queued = json!({"documents":[
        {"run_id":"run_A","owner":"o","repo":"r","status":"queued","created_at":"2026-07-29T00:00:00Z","repo_key":"o/r"}
    ]});
    Mock::given(method("GET"))
        .and(path_regex(r"/v1/collections/ci(_|%5F)executions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(queued))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/v1/collections/ci(_|%5F)executions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"x","status":"exists"})))
        .mount(&server)
        .await;

    let store = store_for(&server);
    assert!(
        store.claim_next_queued().await.unwrap().is_none(),
        "lost the race → no claim"
    );
}
