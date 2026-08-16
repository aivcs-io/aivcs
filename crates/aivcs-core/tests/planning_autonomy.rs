use std::collections::{BTreeMap, BTreeSet};

use aivcs_core::{
    compute_progress, decompose_goal_to_dag, evaluate_replan, schedule_next_ready_tasks, EpicPlan,
    GoalPlan, PlanTask, PlanTaskStatus, ReplanPolicy, SchedulerConstraints, TaskPlan,
};
use chrono::{Duration, Utc};

fn mk_task(id: &str, deps: &[&str]) -> TaskPlan {
    TaskPlan {
        id: id.to_string(),
        title: format!("task-{id}"),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        estimate_hours: 4,
    }
}

#[test]
fn complex_objective_decomposes_into_executable_dag() {
    let goal = GoalPlan {
        id: "goal-1".to_string(),
        objective: "deliver private coding assistant".to_string(),
        epics: vec![
            EpicPlan {
                id: "epic-a".to_string(),
                title: "platform".to_string(),
                tasks: vec![mk_task("t1", &[]), mk_task("t2", &["t1"])],
            },
            EpicPlan {
                id: "epic-b".to_string(),
                title: "runtime".to_string(),
                tasks: vec![mk_task("t3", &["t2"]), mk_task("t4", &["t2"])],
            },
        ],
    };

    let dag = decompose_goal_to_dag(&goal).expect("decompose");
    dag.validate().expect("valid dag");

    assert_eq!(dag.tasks.len(), 4);
    assert!(dag
        .tasks
        .get("t2")
        .unwrap()
        .depends_on
        .contains(&"t1".to_string()));
}

#[test]
fn scheduler_respects_dependencies_and_constraints() {
    let now = Utc::now();
    let mut tasks = BTreeMap::new();
    tasks.insert("t1".to_string(), PlanTask::pending("t1", vec![], now));
    tasks.insert(
        "t2".to_string(),
        PlanTask::pending("t2", vec!["t1".to_string()], now),
    );
    tasks.insert(
        "t3".to_string(),
        PlanTask::pending("t3", vec!["t1".to_string()], now),
    );

    let dag = aivcs_core::ExecutionDag {
        goal_id: "g".to_string(),
        objective: "obj".to_string(),
        tasks,
    };

    let c = SchedulerConstraints {
        max_parallel: 1,
        blocked_tasks: BTreeSet::new(),
    };
    let ready = schedule_next_ready_tasks(&dag, &c).expect("schedule");
    assert_eq!(ready, vec!["t1".to_string()]);

    let mut dag2 = dag.clone();
    dag2.tasks.get_mut("t1").unwrap().status = PlanTaskStatus::Done;
    let c2 = SchedulerConstraints {
        max_parallel: 2,
        blocked_tasks: BTreeSet::from(["t3".to_string()]),
    };
    let ready2 = schedule_next_ready_tasks(&dag2, &c2).expect("schedule2");
    assert_eq!(ready2, vec!["t2".to_string()]);
}

#[test]
fn progress_reporting_matches_execution_reality() {
    let now = Utc::now();
    let mut tasks = BTreeMap::new();
    let mut t1 = PlanTask::pending("t1", vec![], now);
    t1.status = PlanTaskStatus::Done;
    t1.confidence = 0.9;
    let mut t2 = PlanTask::pending("t2", vec![], now);
    t2.status = PlanTaskStatus::InProgress;
    t2.confidence = 0.7;
    let mut t3 = PlanTask::pending("t3", vec![], now);
    t3.status = PlanTaskStatus::Blocked {
        reason: "waiting on API key".to_string(),
    };
    t3.confidence = 0.5;

    tasks.insert("t1".to_string(), t1);
    tasks.insert("t2".to_string(), t2);
    tasks.insert("t3".to_string(), t3);

    let dag = aivcs_core::ExecutionDag {
        goal_id: "g".to_string(),
        objective: "obj".to_string(),
        tasks,
    };

    let report = compute_progress(&dag);
    assert_eq!(report.total_tasks, 3);
    assert_eq!(report.done_tasks, 1);
    assert_eq!(report.in_progress_tasks, 1);
    assert_eq!(report.blocked_tasks, 1);
    assert_eq!(report.completion_ratio, 1.0 / 3.0);
    assert_eq!(report.blockers, vec!["waiting on API key".to_string()]);
}

#[test]
fn replans_trigger_automatically_on_drift_failure_and_blockers() {
    let now = Utc::now();
    let mut tasks = BTreeMap::new();

    let mut t1 = PlanTask::pending("t1", vec![], now - Duration::hours(30));
    t1.status = PlanTaskStatus::Failed {
        reason: "compile failed".to_string(),
    };
    t1.confidence = 0.3;

    let mut t2 = PlanTask::pending("t2", vec![], now - Duration::hours(30));
    t2.status = PlanTaskStatus::Blocked {
        reason: "dependency outage".to_string(),
    };
    t2.confidence = 0.4;

    let t3 = PlanTask::pending("t3", vec![], now - Duration::hours(30));

    tasks.insert("t1".to_string(), t1);
    tasks.insert("t2".to_string(), t2);
    tasks.insert("t3".to_string(), t3);

    let dag = aivcs_core::ExecutionDag {
        goal_id: "g".to_string(),
        objective: "obj".to_string(),
        tasks,
    };

    let policy = ReplanPolicy {
        min_confidence: 0.6,
        max_blocked_ratio: 0.2,
        trigger_on_failure: true,
        max_stale_hours: 12,
    };

    let decision = evaluate_replan(&dag, &policy, now);
    assert!(decision.should_replan);
    assert!(!decision.reasons.is_empty());
}
