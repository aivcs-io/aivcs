use oxidized_state::{CommitId, CommitRecord, SurrealHandle};

#[tokio::test]
async fn test_duplicate_snapshot_fails() {
    let handle = SurrealHandle::setup_db().await.unwrap();

    let state = serde_json::json!({"step": 1});
    let commit_id = CommitId::from_state(serde_json::to_vec(&state).unwrap().as_slice());

    // First commit
    let commit1 = CommitRecord::new(commit_id.clone(), vec![], "First", "test");
    handle.save_commit(&commit1).await.unwrap();

    // Second commit with same state but different message
    let commit2 = CommitRecord::new(commit_id.clone(), vec![], "Second", "test");
    let result = handle.save_commit(&commit2).await;

    assert!(
        result.is_err(),
        "Second commit with same CommitId should fail due to UNIQUE constraint. Got: {:?}",
        result.ok()
    );
}
