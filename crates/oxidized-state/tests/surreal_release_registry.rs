use std::sync::Arc;
use std::time::Duration;

use oxidized_state::{
    ContentDigest, ReleaseMetadata, ReleaseRegistry, StorageError, SurrealDbReleaseRegistry,
    SurrealHandle,
};

fn sample_release_meta(label: &str) -> ReleaseMetadata {
    ReleaseMetadata {
        version_label: Some(label.to_string()),
        promoted_by: "ci".to_string(),
        notes: None,
    }
}

#[tokio::test]
async fn surreal_registry_promote_current_and_history() {
    let handle = Arc::new(SurrealHandle::setup_db().await.unwrap());
    let registry = SurrealDbReleaseRegistry::new(handle);
    let d1 = ContentDigest::from_bytes(b"v1");
    let d2 = ContentDigest::from_bytes(b"v2");

    registry
        .promote("agent", &d1, sample_release_meta("v1"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1)).await;
    registry
        .promote("agent", &d2, sample_release_meta("v2"))
        .await
        .unwrap();

    let current = registry.current("agent").await.unwrap().unwrap();
    assert_eq!(current.spec_digest, d2);

    let history = registry.history("agent").await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].spec_digest, d2);
    assert_eq!(history[1].spec_digest, d1);
}

#[tokio::test]
async fn surreal_registry_rollback_re_appends_previous_release() {
    let handle = Arc::new(SurrealHandle::setup_db().await.unwrap());
    let registry = SurrealDbReleaseRegistry::new(handle);
    let d1 = ContentDigest::from_bytes(b"v1");
    let d2 = ContentDigest::from_bytes(b"v2");

    registry
        .promote("agent", &d1, sample_release_meta("v1"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1)).await;
    registry
        .promote("agent", &d2, sample_release_meta("v2"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1)).await;

    let rolled_back = registry.rollback("agent").await.unwrap();
    assert_eq!(rolled_back.spec_digest, d1);

    let history = registry.history("agent").await.unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].spec_digest, d1);
    assert_eq!(history[1].spec_digest, d2);
    assert_eq!(history[2].spec_digest, d1);
}

#[tokio::test]
async fn surreal_registry_rollback_fails_for_unknown_agent() {
    let handle = Arc::new(SurrealHandle::setup_db().await.unwrap());
    let registry = SurrealDbReleaseRegistry::new(handle);

    let err = registry.rollback("unknown").await.unwrap_err();
    assert!(matches!(err, StorageError::ReleaseNotFound { .. }));
}

#[tokio::test]
async fn surreal_registry_rollback_fails_with_single_release() {
    let handle = Arc::new(SurrealHandle::setup_db().await.unwrap());
    let registry = SurrealDbReleaseRegistry::new(handle);
    let d1 = ContentDigest::from_bytes(b"v1");

    registry
        .promote("agent", &d1, sample_release_meta("v1"))
        .await
        .unwrap();

    let err = registry.rollback("agent").await.unwrap_err();
    assert!(matches!(err, StorageError::NoPreviousRelease { .. }));
}
