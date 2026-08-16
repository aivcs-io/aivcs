use std::sync::Arc;

use async_trait::async_trait;

use crate::storage_traits::{
    ContentDigest, ReleaseMetadata, ReleaseRecord, ReleaseRegistry, StorageResult,
};
use crate::SurrealHandle;

/// SurrealDB-backed implementation of the ReleaseRegistry trait.
#[derive(Clone)]
pub struct SurrealDbReleaseRegistry {
    handle: Arc<SurrealHandle>,
}

impl SurrealDbReleaseRegistry {
    pub fn new(handle: Arc<SurrealHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl ReleaseRegistry for SurrealDbReleaseRegistry {
    async fn promote(
        &self,
        name: &str,
        spec_digest: &ContentDigest,
        metadata: ReleaseMetadata,
    ) -> StorageResult<ReleaseRecord> {
        self.handle
            .release_promote(name, spec_digest, metadata)
            .await
    }

    async fn rollback(&self, name: &str) -> StorageResult<ReleaseRecord> {
        self.handle.release_rollback(name).await
    }

    async fn current(&self, name: &str) -> StorageResult<Option<ReleaseRecord>> {
        self.handle.release_current(name).await
    }

    async fn history(&self, name: &str) -> StorageResult<Vec<ReleaseRecord>> {
        self.handle.release_history(name).await
    }
}
