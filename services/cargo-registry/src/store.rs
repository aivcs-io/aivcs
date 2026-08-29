//! Object storage for the registry.
//!
//! AWS S3 in production (us-east-2), behind a trait so routes are testable
//! without a bucket. This replaces the Cloudflare R2 buckets the retired Worker
//! used — see code-governance `TDD_RETIRE_CLOUDFLARE_WORKERS` §3.2.

use async_trait::async_trait;
#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("object not found")]
    NotFound,
    #[error("storage unavailable: {0}")]
    Unavailable(String),
}

#[async_trait]
pub trait Store: Send + Sync {
    async fn get(&self, bucket: Bucket, key: &str) -> Result<Vec<u8>, StoreError>;
    async fn put(&self, bucket: Bucket, key: &str, body: Vec<u8>) -> Result<(), StoreError>;
    async fn exists(&self, bucket: Bucket, key: &str) -> Result<bool, StoreError>;
    /// Every key under `prefix`, paginated to completion. Used by index
    /// repair, which must see the whole index rather than a first page.
    async fn list(&self, bucket: Bucket, prefix: &str) -> Result<Vec<String>, StoreError>;
    /// Cheap liveness probe against the backing store; drives `/readyz` (N1).
    async fn healthy(&self) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bucket {
    Index,
    Crates,
}

// ── S3 ───────────────────────────────────────────────────────────────────────

pub struct S3Store {
    client: aws_sdk_s3::Client,
    index_bucket: String,
    crates_bucket: String,
}

impl S3Store {
    pub fn new(client: aws_sdk_s3::Client, index_bucket: String, crates_bucket: String) -> Self {
        Self {
            client,
            index_bucket,
            crates_bucket,
        }
    }

    fn bucket_name(&self, b: Bucket) -> &str {
        match b {
            Bucket::Index => &self.index_bucket,
            Bucket::Crates => &self.crates_bucket,
        }
    }
}

#[async_trait]
impl Store for S3Store {
    async fn get(&self, bucket: Bucket, key: &str) -> Result<Vec<u8>, StoreError> {
        let out = self
            .client
            .get_object()
            .bucket(self.bucket_name(bucket))
            .key(key)
            .send()
            .await;
        match out {
            Ok(o) => {
                let bytes = o
                    .body
                    .collect()
                    .await
                    .map_err(|e| StoreError::Unavailable(e.to_string()))?;
                Ok(bytes.into_bytes().to_vec())
            }
            Err(e) => {
                let svc = e.into_service_error();
                if svc.is_no_such_key() {
                    Err(StoreError::NotFound)
                } else {
                    Err(StoreError::Unavailable(svc.to_string()))
                }
            }
        }
    }

    async fn put(&self, bucket: Bucket, key: &str, body: Vec<u8>) -> Result<(), StoreError> {
        self.client
            .put_object()
            .bucket(self.bucket_name(bucket))
            .key(key)
            .body(body.into())
            .send()
            .await
            .map_err(|e| StoreError::Unavailable(e.to_string()))?;
        Ok(())
    }

    async fn exists(&self, bucket: Bucket, key: &str) -> Result<bool, StoreError> {
        match self
            .client
            .head_object()
            .bucket(self.bucket_name(bucket))
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(e) => {
                let svc = e.into_service_error();
                if svc.is_not_found() {
                    Ok(false)
                } else {
                    Err(StoreError::Unavailable(svc.to_string()))
                }
            }
        }
    }

    async fn list(&self, bucket: Bucket, prefix: &str) -> Result<Vec<String>, StoreError> {
        let mut keys = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(self.bucket_name(bucket))
                .prefix(prefix);
            if let Some(t) = &token {
                req = req.continuation_token(t);
            }
            let out = req
                .send()
                .await
                .map_err(|e| StoreError::Unavailable(e.to_string()))?;
            keys.extend(
                out.contents()
                    .iter()
                    .filter_map(|o| o.key().map(str::to_string)),
            );
            // Truncated results are the norm past 1000 objects; stopping here
            // would silently repair only part of the index.
            match out.next_continuation_token() {
                Some(t) => token = Some(t.to_string()),
                None => break,
            }
        }
        Ok(keys)
    }

    async fn healthy(&self) -> bool {
        // HEAD on the index bucket: proves credentials and reachability without
        // depending on any particular object existing.
        self.client
            .head_bucket()
            .bucket(&self.index_bucket)
            .send()
            .await
            .is_ok()
    }
}

// ── In-memory (tests) ────────────────────────────────────────────────────────

#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore {
    objects: Mutex<HashMap<(Bucket, String), Vec<u8>>>,
    /// Flip to simulate a storage outage and assert `/readyz` goes 503.
    pub unavailable: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
#[async_trait]
impl Store for MemoryStore {
    async fn get(&self, bucket: Bucket, key: &str) -> Result<Vec<u8>, StoreError> {
        if self.unavailable.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(StoreError::Unavailable("simulated".into()));
        }
        self.objects
            .lock()
            .unwrap()
            .get(&(bucket, key.to_string()))
            .cloned()
            .ok_or(StoreError::NotFound)
    }

    async fn put(&self, bucket: Bucket, key: &str, body: Vec<u8>) -> Result<(), StoreError> {
        if self.unavailable.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(StoreError::Unavailable("simulated".into()));
        }
        self.objects
            .lock()
            .unwrap()
            .insert((bucket, key.to_string()), body);
        Ok(())
    }

    async fn exists(&self, bucket: Bucket, key: &str) -> Result<bool, StoreError> {
        if self.unavailable.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(StoreError::Unavailable("simulated".into()));
        }
        Ok(self
            .objects
            .lock()
            .unwrap()
            .contains_key(&(bucket, key.to_string())))
    }

    async fn list(&self, bucket: Bucket, prefix: &str) -> Result<Vec<String>, StoreError> {
        if self.unavailable.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(StoreError::Unavailable("simulated".into()));
        }
        let mut keys: Vec<String> = self
            .objects
            .lock()
            .unwrap()
            .keys()
            .filter(|(b, k)| *b == bucket && k.starts_with(prefix))
            .map(|(_, k)| k.clone())
            .collect();
        keys.sort();
        Ok(keys)
    }

    async fn healthy(&self) -> bool {
        !self.unavailable.load(std::sync::atomic::Ordering::Relaxed)
    }
}
