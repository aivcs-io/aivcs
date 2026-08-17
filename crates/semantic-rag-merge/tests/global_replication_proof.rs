use anyhow::Result;
use oxidized_state::{CommitId, CommitRecord, MemoryRecord, SurrealHandle};
use semantic_rag_merge::semantic_merge;

/// Helper to replicate specific commits and their memories from a source DB to a target DB idempotently
async fn replicate_commits(
    source: &SurrealHandle,
    target: &SurrealHandle,
    hashes: &[&str],
) -> Result<()> {
    for hash in hashes {
        // Only save the commit if it does not already exist in the target database
        if target.get_commit(hash).await?.is_none() {
            if let Some(commit) = source.get_commit(hash).await? {
                target.save_commit(&commit).await?;
            }
        }

        let memories = source.get_memories(hash).await?;
        let target_memories = target.get_memories(hash).await?;
        let target_keys: std::collections::HashSet<_> =
            target_memories.iter().map(|m| &m.key).collect();

        for memory in memories {
            if !target_keys.contains(&memory.key) {
                target.save_memory(&memory).await?;
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn test_multi_region_asynchronous_replication_and_convergence_proof() -> Result<()> {
    // Phase 1: Setup two independent regional database instances (Region A and Region B)
    // Representing two regional, isolated, unmeshed SurrealKV write backends.
    let region_a = SurrealHandle::setup_db().await?;
    let region_b = SurrealHandle::setup_db().await?;

    let root_id = CommitId::from_state(b"root-genesis");
    let genesis = CommitRecord::new(root_id.clone(), vec![], "Genesis state", "genesis-agent");
    region_a.save_commit(&genesis).await?;
    region_b.save_commit(&genesis).await?;

    let genesis_mem = MemoryRecord::new(&root_id.hash, "system-config", "version: 1.0.0");
    region_a.save_memory(&genesis_mem).await?;
    region_b.save_memory(&genesis_mem).await?;

    // Phase 2: Divergent local writes occur concurrently in each region (without WAN dependency)
    // Region A: local agent writes state A
    let commit_id_a = CommitId::from_state(b"write-a");
    let commit_a = CommitRecord::new(
        commit_id_a.clone(),
        vec![root_id.hash.clone()],
        "Local change in Region A",
        "agent-region-a",
    );
    region_a.save_commit(&commit_a).await?;

    let mem_a = MemoryRecord::new(&commit_id_a.hash, "region-metrics-a", "latency: 10ms");
    let conflict_a = MemoryRecord::new(&commit_id_a.hash, "db-mode", "mode: standalone-read");
    region_a.save_memory(&mem_a).await?;
    region_a.save_memory(&conflict_a).await?;

    // Region B: local agent writes state B (concurrently)
    let commit_id_b = CommitId::from_state(b"write-b");
    let commit_b = CommitRecord::new(
        commit_id_b.clone(),
        vec![root_id.hash.clone()],
        "Local change in Region B",
        "agent-region-b",
    );
    region_b.save_commit(&commit_b).await?;

    let mem_b = MemoryRecord::new(&commit_id_b.hash, "region-metrics-b", "latency: 42ms");
    let conflict_b = MemoryRecord::new(
        &commit_id_b.hash,
        "db-mode",
        "mode: cluster-distributed-heavy",
    );
    region_b.save_memory(&mem_b).await?;
    region_b.save_memory(&conflict_b).await?;

    // Phase 3: Asynchronous replication over WAN (cross-region push/pull sync of both branches)
    let sync_hashes = vec![
        root_id.hash.as_str(),
        commit_id_a.hash.as_str(),
        commit_id_b.hash.as_str(),
    ];
    replicate_commits(&region_a, &region_b, &sync_hashes).await?;
    replicate_commits(&region_b, &region_a, &sync_hashes).await?;

    // Phase 4: Convergence resolution (Semantic Merge resolution of the divergent heads)
    // Both regions now have both heads and apply the semantic merge to arrive at the same state.
    let merge_result_a = semantic_merge(
        &region_a,
        &commit_id_a.hash,
        &commit_id_b.hash,
        "Resolve Region A and Region B state",
        "sync-reconciler-agent",
    )
    .await?;

    let merge_result_b = semantic_merge(
        &region_b,
        &commit_id_a.hash,
        &commit_id_b.hash,
        "Resolve Region A and Region B state",
        "sync-reconciler-agent",
    )
    .await?;

    // Phase 5: Verification of Eventual Consistency / State Convergence
    let merged_memories_a = region_a
        .get_memories(&merge_result_a.merge_commit_id.hash)
        .await?;
    let merged_memories_b = region_b
        .get_memories(&merge_result_b.merge_commit_id.hash)
        .await?;

    // Verify both regions resolved to the identical number of memories
    // (2 unique metrics memories + 1 resolved database conflict) = 3 memories in total
    assert_eq!(merged_memories_a.len(), 3);
    assert_eq!(merged_memories_b.len(), 3);

    // Verify key metrics were merged correctly and the values are identical in both regions
    let keys_a: Vec<&str> = merged_memories_a.iter().map(|m| m.key.as_str()).collect();
    let keys_b: Vec<&str> = merged_memories_b.iter().map(|m| m.key.as_str()).collect();
    assert_eq!(keys_a, keys_b);

    // Verify the conflict on "db-mode" converged to the same winner
    let resolved_a = merged_memories_a
        .iter()
        .find(|m| m.key == "db-mode")
        .unwrap();
    let resolved_b = merged_memories_b
        .iter()
        .find(|m| m.key == "db-mode")
        .unwrap();
    assert_eq!(resolved_a.content, resolved_b.content);

    // The conflict score heuristic should select the richer/longer content:
    assert!(resolved_a.content.contains("cluster-distributed-heavy"));

    println!(
        "Proof complete: Asynchronous replication resolved divergent regions to converged state."
    );
    Ok(())
}
