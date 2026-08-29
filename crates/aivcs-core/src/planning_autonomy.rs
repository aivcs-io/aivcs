//! Planning and long-horizon autonomy primitives.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A high-level goal containing epics and tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalPlan {
    pub id: String,
    pub objective: String,
    pub epics: Vec<EpicPlan>,
}

/// Epic decomposition unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpicPlan {
    pub id: String,
    pub title: String,
    pub tasks: Vec<TaskPlan>,
}

/// Task decomposition unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPlan {
    pub id: String,
    pub title: String,
    pub depends_on: Vec<String>,
    pub estimate_hours: u32,
}

/// Runtime task status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PlanTaskStatus {
    Pending,
    InProgress,
    Done,
    Blocked { reason: String },
    Failed { reason: String },
}

/// Executable task node in the DAG.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanTask {
    pub id: String,
    pub title: String,
    pub depends_on: Vec<String>,
    pub estimate_hours: u32,
    pub status: PlanTaskStatus,
    pub confidence: f32,
    pub updated_at: DateTime<Utc>,
}

impl PlanTask {
    pub fn pending(id: &str, depends_on: Vec<String>, updated_at: DateTime<Utc>) -> Self {
        Self {
            id: id.to_string(),
            title: id.to_string(),
            depends_on,
            estimate_hours: 1,
            status: PlanTaskStatus::Pending,
            confidence: 1.0,
            updated_at,
        }
    }
}

/// Executable DAG form of a goal plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionDag {
    pub goal_id: String,
    pub objective: String,
    pub tasks: BTreeMap<String, PlanTask>,
}

impl ExecutionDag {
    pub fn validate(&self) -> Result<(), PlanningError> {
        for (task_id, task) in &self.tasks {
            for dep in &task.depends_on {
                if !self.tasks.contains_key(dep) {
                    return Err(PlanningError::MissingDependency {
                        task_id: task_id.clone(),
                        missing_dependency: dep.clone(),
                    });
                }
            }
        }

        let mut indegree: BTreeMap<String, usize> =
            self.tasks.keys().map(|k| (k.clone(), 0usize)).collect();
        let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (task_id, task) in &self.tasks {
            for dep in &task.depends_on {
                edges.entry(dep.clone()).or_default().push(task_id.clone());
                *indegree.get_mut(task_id).expect("task in indegree") += 1;
            }
        }

        let mut queue: VecDeque<String> = indegree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(k, _)| k.clone())
            .collect();

        let mut visited = 0usize;
        while let Some(node) = queue.pop_front() {
            visited += 1;
            if let Some(neighbors) = edges.get(&node) {
                for n in neighbors {
                    let entry = indegree.get_mut(n).expect("neighbor in indegree");
                    *entry -= 1;
                    if *entry == 0 {
                        queue.push_back(n.clone());
                    }
                }
            }
        }

        if visited != self.tasks.len() {
            return Err(PlanningError::CycleDetected);
        }
        Ok(())
    }
}

/// Scheduling controls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerConstraints {
    pub max_parallel: usize,
    pub blocked_tasks: BTreeSet<String>,
}

/// Progress report over task execution state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressReport {
    pub total_tasks: usize,
    pub done_tasks: usize,
    pub in_progress_tasks: usize,
    pub blocked_tasks: usize,
    pub failed_tasks: usize,
    pub pending_tasks: usize,
    pub completion_ratio: f32,
    pub confidence: f32,
    pub blockers: Vec<String>,
}

/// Replan trigger policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplanPolicy {
    pub min_confidence: f32,
    pub max_blocked_ratio: f32,
    pub trigger_on_failure: bool,
    pub max_stale_hours: i64,
}

/// Replan reason taxonomy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ReplanReason {
    LowConfidence {
        observed: f32,
        threshold: f32,
    },
    BlockedRatio {
        observed: f32,
        threshold: f32,
    },
    FailedTasks {
        count: usize,
    },
    StaleProgress {
        stale_hours: i64,
        threshold_hours: i64,
    },
}

/// Replan decision output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplanDecision {
    pub should_replan: bool,
    pub reasons: Vec<ReplanReason>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PlanningError {
    #[error("task '{task_id}' has missing dependency '{missing_dependency}'")]
    MissingDependency {
        task_id: String,
        missing_dependency: String,
    },
    #[error("dependency cycle detected in execution DAG")]
    CycleDetected,
}

/// Decompose a goal into executable DAG tasks.
pub fn decompose_goal_to_dag(goal: &GoalPlan) -> Result<ExecutionDag, PlanningError> {
    let mut tasks = BTreeMap::new();
    let now = Utc::now();
    for epic in &goal.epics {
        for t in &epic.tasks {
            tasks.insert(
                t.id.clone(),
                PlanTask {
                    id: t.id.clone(),
                    title: t.title.clone(),
                    depends_on: t.depends_on.clone(),
                    estimate_hours: t.estimate_hours,
                    status: PlanTaskStatus::Pending,
                    confidence: 1.0,
                    updated_at: now,
                },
            );
        }
    }

    let dag = ExecutionDag {
        goal_id: goal.id.clone(),
        objective: goal.objective.clone(),
        tasks,
    };
    dag.validate()?;
    Ok(dag)
}

/// Return dependency-ready tasks respecting constraints.
pub fn schedule_next_ready_tasks(
    dag: &ExecutionDag,
    constraints: &SchedulerConstraints,
) -> Result<Vec<String>, PlanningError> {
    dag.validate()?;
    if constraints.max_parallel == 0 {
        return Ok(Vec::new());
    }

    let mut ready: Vec<String> = dag
        .tasks
        .iter()
        .filter_map(|(id, task)| match task.status {
            PlanTaskStatus::Pending => Some((id, task)),
            _ => None,
        })
        .filter(|(id, _)| !constraints.blocked_tasks.contains(*id))
        .filter(|(_, task)| {
            task.depends_on.iter().all(|dep| {
                matches!(
                    dag.tasks.get(dep).map(|t| &t.status),
                    Some(PlanTaskStatus::Done)
                )
            })
        })
        .map(|(id, _)| id.clone())
        .collect();

    ready.sort();
    ready.truncate(constraints.max_parallel);
    Ok(ready)
}

/// Compute progress and confidence.
pub fn compute_progress(dag: &ExecutionDag) -> ProgressReport {
    let total = dag.tasks.len();
    let mut done = 0usize;
    let mut in_progress = 0usize;
    let mut blocked = 0usize;
    let mut failed = 0usize;
    let mut pending = 0usize;
    let mut blockers = Vec::new();
    let mut confidence_sum = 0.0f32;

    for task in dag.tasks.values() {
        confidence_sum += task.confidence;
        match &task.status {
            PlanTaskStatus::Done => done += 1,
            PlanTaskStatus::InProgress => in_progress += 1,
            PlanTaskStatus::Blocked { reason } => {
                blocked += 1;
                blockers.push(reason.clone());
            }
            PlanTaskStatus::Failed { .. } => failed += 1,
            PlanTaskStatus::Pending => pending += 1,
        }
    }

    let completion_ratio = if total == 0 {
        0.0
    } else {
        done as f32 / total as f32
    };
    let confidence = if total == 0 {
        0.0
    } else {
        confidence_sum / total as f32
    };

    ProgressReport {
        total_tasks: total,
        done_tasks: done,
        in_progress_tasks: in_progress,
        blocked_tasks: blocked,
        failed_tasks: failed,
        pending_tasks: pending,
        completion_ratio,
        confidence,
        blockers,
    }
}

/// Evaluate replan triggers from execution drift/failure state.
pub fn evaluate_replan(
    dag: &ExecutionDag,
    policy: &ReplanPolicy,
    now: DateTime<Utc>,
) -> ReplanDecision {
    let report = compute_progress(dag);
    let mut reasons = Vec::new();

    if report.confidence < policy.min_confidence {
        reasons.push(ReplanReason::LowConfidence {
            observed: report.confidence,
            threshold: policy.min_confidence,
        });
    }

    let blocked_ratio = if report.total_tasks == 0 {
        0.0
    } else {
        report.blocked_tasks as f32 / report.total_tasks as f32
    };
    if blocked_ratio > policy.max_blocked_ratio {
        reasons.push(ReplanReason::BlockedRatio {
            observed: blocked_ratio,
            threshold: policy.max_blocked_ratio,
        });
    }

    if policy.trigger_on_failure && report.failed_tasks > 0 {
        reasons.push(ReplanReason::FailedTasks {
            count: report.failed_tasks,
        });
    }

    if let Some(oldest) = dag.tasks.values().map(|t| t.updated_at).min() {
        let stale_hours = (now - oldest).num_hours();
        if stale_hours > policy.max_stale_hours {
            reasons.push(ReplanReason::StaleProgress {
                stale_hours,
                threshold_hours: policy.max_stale_hours,
            });
        }
    }

    ReplanDecision {
        should_replan: !reasons.is_empty(),
        reasons,
    }
}
