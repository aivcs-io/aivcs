//! In-memory fakes for storage traits (testing only)
//!
//! Provides `MemoryCasStore`, `MemoryRunLedger`, and `MemoryReleaseRegistry`
//! that satisfy the trait contracts without any external dependencies.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::Utc;

use crate::error::StorageError;
use crate::storage_traits::*;

// ---------------------------------------------------------------------------
// MemoryCasStore
// ---------------------------------------------------------------------------

/// In-memory content-addressed store backed by a `HashMap<digest, bytes>`.
#[derive(Debug, Default)]
pub struct MemoryCasStore {
    store: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryCasStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl CasStore for MemoryCasStore {
    async fn put(&self, data: &[u8]) -> StorageResult<ContentDigest> {
        let digest = ContentDigest::from_bytes(data);
        let mut store = self.store.lock().unwrap();
        store.insert(digest.as_str().to_string(), data.to_vec());
        Ok(digest)
    }

    async fn get(&self, digest: &ContentDigest) -> StorageResult<Vec<u8>> {
        let store = self.store.lock().unwrap();
        store
            .get(digest.as_str())
            .cloned()
            .ok_or_else(|| StorageError::NotFound {
                digest: digest.as_str().to_string(),
            })
    }

    async fn contains(&self, digest: &ContentDigest) -> StorageResult<bool> {
        let store = self.store.lock().unwrap();
        Ok(store.contains_key(digest.as_str()))
    }

    async fn delete(&self, digest: &ContentDigest) -> StorageResult<()> {
        let mut store = self.store.lock().unwrap();
        store.remove(digest.as_str());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// MemoryRunLedger
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct RunState {
    record: RunRecord,
    events: Vec<RunEvent>,
}

/// In-memory run ledger backed by a `HashMap<RunId, RunState>`.
#[derive(Debug, Default)]
pub struct MemoryRunLedger {
    runs: Mutex<HashMap<String, RunState>>,
}

impl MemoryRunLedger {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl RunLedger for MemoryRunLedger {
    async fn create_run(
        &self,
        spec_digest: &ContentDigest,
        metadata: RunMetadata,
    ) -> StorageResult<RunId> {
        let run_id = RunId::new();
        let record = RunRecord {
            run_id: run_id.clone(),
            spec_digest: spec_digest.clone(),
            metadata,
            status: RunStatus::Running,
            summary: None,
            created_at: Utc::now(),
            completed_at: None,
        };
        let mut runs = self.runs.lock().unwrap();
        runs.insert(
            run_id.0.clone(),
            RunState {
                record,
                events: Vec::new(),
            },
        );
        Ok(run_id)
    }

    async fn append_event(&self, run_id: &RunId, event: RunEvent) -> StorageResult<()> {
        let mut runs = self.runs.lock().unwrap();
        let state = runs
            .get_mut(&run_id.0)
            .ok_or_else(|| StorageError::RunNotFound {
                run_id: run_id.0.clone(),
            })?;
        if state.record.status != RunStatus::Running {
            return Err(StorageError::InvalidRunState {
                run_id: run_id.0.clone(),
                status: format!("{:?}", state.record.status),
                expected: "Running".to_string(),
            });
        }
        state.events.push(event);
        Ok(())
    }

    async fn complete_run(&self, run_id: &RunId, summary: RunSummary) -> StorageResult<()> {
        let mut runs = self.runs.lock().unwrap();
        let state = runs
            .get_mut(&run_id.0)
            .ok_or_else(|| StorageError::RunNotFound {
                run_id: run_id.0.clone(),
            })?;
        if state.record.status != RunStatus::Running {
            return Err(StorageError::InvalidRunState {
                run_id: run_id.0.clone(),
                status: format!("{:?}", state.record.status),
                expected: "Running".to_string(),
            });
        }
        state.record.status = RunStatus::Completed;
        state.record.summary = Some(summary);
        state.record.completed_at = Some(Utc::now());
        Ok(())
    }

    async fn fail_run(&self, run_id: &RunId, summary: RunSummary) -> StorageResult<()> {
        let mut runs = self.runs.lock().unwrap();
        let state = runs
            .get_mut(&run_id.0)
            .ok_or_else(|| StorageError::RunNotFound {
                run_id: run_id.0.clone(),
            })?;
        if state.record.status != RunStatus::Running {
            return Err(StorageError::InvalidRunState {
                run_id: run_id.0.clone(),
                status: format!("{:?}", state.record.status),
                expected: "Running".to_string(),
            });
        }
        state.record.status = RunStatus::Failed;
        state.record.summary = Some(summary);
        state.record.completed_at = Some(Utc::now());
        Ok(())
    }

    async fn cancel_run(&self, run_id: &RunId, summary: RunSummary) -> StorageResult<()> {
        let mut runs = self.runs.lock().unwrap();
        let state = runs
            .get_mut(&run_id.0)
            .ok_or_else(|| StorageError::RunNotFound {
                run_id: run_id.0.clone(),
            })?;
        if state.record.status != RunStatus::Running {
            return Err(StorageError::InvalidRunState {
                run_id: run_id.0.clone(),
                status: format!("{:?}", state.record.status),
                expected: "Running".to_string(),
            });
        }
        state.record.status = RunStatus::Cancelled;
        state.record.summary = Some(summary);
        state.record.completed_at = Some(Utc::now());
        Ok(())
    }

    async fn get_run(&self, run_id: &RunId) -> StorageResult<RunRecord> {
        let runs = self.runs.lock().unwrap();
        runs.get(&run_id.0)
            .map(|s| s.record.clone())
            .ok_or_else(|| StorageError::RunNotFound {
                run_id: run_id.0.clone(),
            })
    }

    async fn get_events(&self, run_id: &RunId) -> StorageResult<Vec<RunEvent>> {
        let runs = self.runs.lock().unwrap();
        let state = runs
            .get(&run_id.0)
            .ok_or_else(|| StorageError::RunNotFound {
                run_id: run_id.0.clone(),
            })?;
        let mut events = state.events.clone();
        events.sort_by_key(|e| e.seq);
        Ok(events)
    }

    async fn list_runs(
        &self,
        spec_digest: Option<&ContentDigest>,
    ) -> StorageResult<Vec<RunRecord>> {
        let runs = self.runs.lock().unwrap();
        let records: Vec<RunRecord> = runs
            .values()
            .filter(|s| {
                spec_digest
                    .map(|d| s.record.spec_digest == *d)
                    .unwrap_or(true)
            })
            .map(|s| s.record.clone())
            .collect();
        Ok(records)
    }
}

// ---------------------------------------------------------------------------
// MemoryReleaseRegistry
// ---------------------------------------------------------------------------

/// In-memory release registry backed by a `HashMap<name, Vec<ReleaseRecord>>`.
///
/// Each agent name maps to its full release history (newest last internally).
#[derive(Debug, Default)]
pub struct MemoryReleaseRegistry {
    releases: Mutex<HashMap<String, Vec<ReleaseRecord>>>,
}

impl MemoryReleaseRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ReleaseRegistry for MemoryReleaseRegistry {
    async fn promote(
        &self,
        name: &str,
        spec_digest: &ContentDigest,
        metadata: ReleaseMetadata,
    ) -> StorageResult<ReleaseRecord> {
        let record = ReleaseRecord {
            name: name.to_string(),
            spec_digest: spec_digest.clone(),
            metadata,
            created_at: Utc::now(),
        };
        let mut releases = self.releases.lock().unwrap();
        releases
            .entry(name.to_string())
            .or_default()
            .push(record.clone());
        Ok(record)
    }

    async fn rollback(&self, name: &str) -> StorageResult<ReleaseRecord> {
        let mut releases = self.releases.lock().unwrap();
        let history = releases
            .get_mut(name)
            .ok_or_else(|| StorageError::ReleaseNotFound {
                name: name.to_string(),
            })?;
        if history.len() < 2 {
            return Err(StorageError::NoPreviousRelease {
                name: name.to_string(),
            });
        }
        // Append-only: clone the previous release as a new entry
        // instead of destroying the current one.
        let previous = history[history.len() - 2].clone();
        history.push(previous.clone());
        Ok(previous)
    }

    async fn current(&self, name: &str) -> StorageResult<Option<ReleaseRecord>> {
        let releases = self.releases.lock().unwrap();
        Ok(releases.get(name).and_then(|h| h.last().cloned()))
    }

    async fn history(&self, name: &str) -> StorageResult<Vec<ReleaseRecord>> {
        let releases = self.releases.lock().unwrap();
        let mut history = releases.get(name).cloned().unwrap_or_default();
        history.reverse(); // newest first
        Ok(history)
    }
}
