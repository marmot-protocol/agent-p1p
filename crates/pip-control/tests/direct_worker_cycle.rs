use std::cell::RefCell;
use std::rc::Rc;

use pip_contracts::{WorkerResult, WorkerRole};
use pip_control::{
    DirectQueue, DirectQueueCycle, DirectWorkerCycle, DirectWorkerCycleContext, DirectWorkerError,
    DirectWorkerRuntime, DirectWorkerRuntimeError, execute_direct_queue_once,
    recommended_direct_lease_seconds, reconcile_direct_queue_once, run_direct_worker_once_with,
};

#[test]
fn direct_job_lease_covers_the_longest_policy_runtime_plus_recovery_margin() {
    assert_eq!(
        recommended_direct_lease_seconds(&active_policy()).unwrap(),
        2_820
    );
}
use pip_controller::DirectTaskSpec;
use pip_store::{EffectInput, EventInput, NewCase, PolicyInput, Store, TransitionInput};
use serde_json::{Value, json};

#[derive(Clone)]
struct FakeRuntime {
    result: Result<WorkerResult, DirectWorkerRuntimeError>,
    tasks: Rc<RefCell<Vec<(DirectTaskSpec, u64)>>>,
}

impl DirectWorkerRuntime for FakeRuntime {
    fn execute(
        &self,
        task: &DirectTaskSpec,
        attempt_id: u64,
    ) -> Result<WorkerResult, DirectWorkerRuntimeError> {
        self.tasks.borrow_mut().push((task.clone(), attempt_id));
        self.result.clone()
    }
}

#[test]
fn leased_direct_builder_executes_and_enters_the_shared_ingestion_path() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder(directory.path());
    let runtime = runtime(Ok(builder_result()));

    assert_eq!(
        run_direct_worker_once_with(
            &mut store,
            &active_policy(),
            &runtime,
            context("direct-worker-1", 100),
        )
        .unwrap(),
        DirectWorkerCycle::Ingested {
            task_id: task_id().into(),
            transition_count: 2,
        }
    );
    assert_eq!(runtime.tasks.borrow().len(), 1);
    assert_eq!(
        runtime.tasks.borrow()[0].0.model,
        "cursor-grok-4.6-high-fast"
    );
    assert_eq!(runtime.tasks.borrow()[0].1, 1);
    assert_eq!(store.run_count().unwrap(), 1);
    assert!(store.run_by_task_id(task_id()).unwrap().is_some());
    let status = store.status(101).unwrap();
    assert_eq!(status.outbox_leased, 0);
    assert_eq!(status.outbox_superseded, 1);
    assert_eq!(status.direct_attempts_complete, 1);
}

#[test]
fn controller_queue_and_credential_free_executor_converge_without_worker_ledger_access() {
    let directory = tempfile::tempdir().unwrap();
    let queue_root = directory.path().join("direct-queue");
    for child in ["inbox", "results", "archive"] {
        std::fs::create_dir_all(queue_root.join(child)).unwrap();
    }
    let queue = DirectQueue::new(&queue_root).unwrap();
    let mut store = queued_builder(directory.path());
    let runtime = runtime(Ok(builder_result()));

    assert_eq!(
        reconcile_direct_queue_once(
            &mut store,
            &active_policy(),
            &queue,
            "controller",
            100,
            30,
            true,
        )
        .unwrap(),
        DirectQueueCycle::Prepared {
            attempt_id: 1,
            task_id: task_id().into(),
        }
    );
    assert_eq!(store.status(100).unwrap().direct_attempts_running, 1);

    assert_eq!(
        execute_direct_queue_once(&runtime, &queue, 101).unwrap(),
        DirectQueueCycle::Executed {
            attempt_id: 1,
            succeeded: true,
        }
    );
    assert_eq!(runtime.tasks.borrow().len(), 1);
    assert_eq!(store.run_count().unwrap(), 0);

    assert_eq!(
        reconcile_direct_queue_once(
            &mut store,
            &active_policy(),
            &queue,
            "controller",
            102,
            30,
            false,
        )
        .unwrap(),
        DirectQueueCycle::AuthorizationBlocked
    );
    assert_eq!(store.run_count().unwrap(), 0);

    assert_eq!(
        reconcile_direct_queue_once(
            &mut store,
            &active_policy(),
            &queue,
            "controller",
            103,
            30,
            true,
        )
        .unwrap(),
        DirectQueueCycle::Ingested {
            task_id: task_id().into(),
            transition_count: 2,
        }
    );
    assert_eq!(store.run_count().unwrap(), 1);
    assert!(
        std::fs::read_dir(queue_root.join("inbox"))
            .unwrap()
            .next()
            .is_none()
    );
    assert!(
        std::fs::read_dir(queue_root.join("results"))
            .unwrap()
            .next()
            .is_none()
    );
    assert_eq!(
        std::fs::read_dir(queue_root.join("archive"))
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn shadow_reviewer_finishes_after_required_path_advances_without_changing_case_state() {
    let directory = tempfile::tempdir().unwrap();
    let queue_root = directory.path().join("direct-queue");
    for child in ["inbox", "results", "archive"] {
        std::fs::create_dir_all(queue_root.join(child)).unwrap();
    }
    let queue = DirectQueue::new(&queue_root).unwrap();
    let mut store = queued_shadow_review(directory.path());
    store
        .apply_transition(
            &TransitionInput {
                case_key: case_key().into(),
                expected_revision: 2,
                next_state: "FINAL_REVIEW".into(),
                remediation_round: 0,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some("b".repeat(40)),
                observed_at: 10,
                event: EventInput {
                    event_id: "event-required-reviews-approved".into(),
                    event_type: "REVIEWS_APPROVED".into(),
                    payload: json!({"required_reviewers": ["general-sol", "secperf-kimi"]}),
                },
                run: None,
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: Vec::new(),
            },
            None,
        )
        .unwrap();

    assert_eq!(
        reconcile_direct_queue_once(
            &mut store,
            &active_policy(),
            &queue,
            "controller",
            11,
            30,
            true,
        )
        .unwrap(),
        DirectQueueCycle::Prepared {
            attempt_id: 1,
            task_id: shadow_task_id().into(),
        }
    );
    let runtime = runtime(Ok(shadow_review_result()));
    execute_direct_queue_once(&runtime, &queue, 12).unwrap();
    assert_eq!(
        reconcile_direct_queue_once(
            &mut store,
            &active_policy(),
            &queue,
            "controller",
            13,
            30,
            true,
        )
        .unwrap(),
        DirectQueueCycle::Observed {
            task_id: shadow_task_id().into(),
        }
    );
    assert_eq!(
        store.case(case_key()).unwrap().unwrap().state,
        "FINAL_REVIEW"
    );
    assert_eq!(store.run_count().unwrap(), 0);
    assert_eq!(store.review_observation_count().unwrap(), 1);
    let observations = store.review_observations_for_case(case_key()).unwrap();
    assert_eq!(observations[0].reviewer_id, "secperf-opus");
    assert_eq!(observations[0].reviewed_head_sha, "b".repeat(40));
}

#[test]
fn queued_provider_failure_is_recorded_by_controller_and_releases_the_effect() {
    let directory = tempfile::tempdir().unwrap();
    let queue_root = directory.path().join("direct-queue");
    for child in ["inbox", "results", "archive"] {
        std::fs::create_dir_all(queue_root.join(child)).unwrap();
    }
    let queue = DirectQueue::new(&queue_root).unwrap();
    let mut store = queued_builder(directory.path());
    reconcile_direct_queue_once(
        &mut store,
        &active_policy(),
        &queue,
        "controller",
        100,
        30,
        true,
    )
    .unwrap();
    let runtime = runtime(Err(DirectWorkerRuntimeError::Unavailable("outage".into())));
    assert_eq!(
        execute_direct_queue_once(&runtime, &queue, 101).unwrap(),
        DirectQueueCycle::Executed {
            attempt_id: 1,
            succeeded: false
        }
    );
    assert_eq!(
        reconcile_direct_queue_once(
            &mut store,
            &active_policy(),
            &queue,
            "controller",
            102,
            30,
            true,
        )
        .unwrap(),
        DirectQueueCycle::Failed { attempt_id: 1 }
    );
    assert_eq!(store.status(102).unwrap().direct_attempts_failed, 1);
    assert!(
        store
            .claim_effect_matching("retry", 102, 30, &["RUN_DIRECT_WORKER"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn shadow_failure_converges_after_crash_between_attempt_and_effect_completion() {
    let directory = tempfile::tempdir().unwrap();
    let queue_root = directory.path().join("direct-queue");
    for child in ["inbox", "results", "archive"] {
        std::fs::create_dir_all(queue_root.join(child)).unwrap();
    }
    let queue = DirectQueue::new(&queue_root).unwrap();
    let mut store = queued_shadow_review(directory.path());

    assert!(matches!(
        reconcile_direct_queue_once(
            &mut store,
            &active_policy(),
            &queue,
            "controller",
            100,
            30,
            true,
        )
        .unwrap(),
        DirectQueueCycle::Prepared { attempt_id: 1, .. }
    ));
    let runtime = runtime(Err(DirectWorkerRuntimeError::Unavailable("outage".into())));
    execute_direct_queue_once(&runtime, &queue, 101).unwrap();

    // Simulate a crash after the attempt failure committed but before the
    // detached observer effect and its evidence were completed.
    store
        .fail_direct_attempt(1, "controller", 102, "outage")
        .unwrap();

    assert_eq!(
        reconcile_direct_queue_once(
            &mut store,
            &active_policy(),
            &queue,
            "controller",
            103,
            30,
            true,
        )
        .unwrap(),
        DirectQueueCycle::Failed { attempt_id: 1 }
    );
    let status = store.status(103).unwrap();
    assert_eq!(status.direct_attempts_failed, 1);
    assert_eq!(status.outbox_pending, 0);
    assert_eq!(status.outbox_leased, 0);
    assert_eq!(status.outbox_delivered, 1);
    assert!(
        store
            .immutable_history_for_case(case_key())
            .unwrap()
            .evidence
            .iter()
            .any(|evidence| evidence.kind == "DETACHED_REVIEW_FAILURE")
    );
}

#[test]
fn provider_failure_releases_the_job_for_immediate_retry_without_state_change() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder(directory.path());
    let runtime = runtime(Err(DirectWorkerRuntimeError::Unavailable(
        "fixture outage".into(),
    )));

    assert!(matches!(
        run_direct_worker_once_with(
            &mut store,
            &active_policy(),
            &runtime,
            context("direct-worker-1", 100),
        ),
        Err(DirectWorkerError::Runtime(_))
    ));
    assert_eq!(store.run_count().unwrap(), 0);
    assert_eq!(
        store.case(case_key()).unwrap().unwrap().state,
        "READY_TO_BUILD"
    );
    let status = store.status(100).unwrap();
    assert_eq!(status.outbox_pending, 1);
    assert_eq!(status.outbox_leased, 0);
    assert_eq!(status.direct_attempts_failed, 1);
    assert!(
        store
            .claim_effect_matching("direct-worker-2", 100, 30, &["RUN_DIRECT_WORKER"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn completed_result_is_ingested_after_a_crash_without_rerunning_the_provider() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder(directory.path());
    let claimed = store
        .claim_effect_matching("crashed-worker", 90, 30, &["RUN_DIRECT_WORKER"])
        .unwrap()
        .unwrap();
    let attempt_id = store.begin_direct_attempt(&claimed, task_id(), 90).unwrap();
    store
        .complete_direct_attempt(
            attempt_id,
            "crashed-worker",
            91,
            &serde_json::to_value(builder_result()).unwrap(),
        )
        .unwrap();
    store
        .release_effect(&claimed.effect_id, "crashed-worker")
        .unwrap();
    let runtime = runtime(Err(DirectWorkerRuntimeError::Unavailable(
        "must not execute".into(),
    )));

    assert_eq!(
        run_direct_worker_once_with(
            &mut store,
            &active_policy(),
            &runtime,
            context("recovery-worker", 100),
        )
        .unwrap(),
        DirectWorkerCycle::Ingested {
            task_id: task_id().into(),
            transition_count: 2,
        }
    );
    assert!(runtime.tasks.borrow().is_empty());
    assert_eq!(store.status(101).unwrap().direct_attempts_complete, 1);
}

#[test]
fn stale_authorization_never_claims_or_executes_a_direct_job() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder(directory.path());
    let runtime = runtime(Ok(builder_result()));
    let mut cycle = context("direct-worker-1", 100);
    cycle.authorization_valid = false;

    assert_eq!(
        run_direct_worker_once_with(&mut store, &active_policy(), &runtime, cycle).unwrap(),
        DirectWorkerCycle::AuthorizationBlocked
    );
    assert!(runtime.tasks.borrow().is_empty());
    let status = store.status(100).unwrap();
    assert_eq!(status.outbox_pending, 1);
    assert_eq!(status.outbox_leased, 0);
}

#[test]
fn corrupted_direct_job_is_released_and_never_reaches_the_runtime() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder_with(directory.path(), |task| {
        task["model"] = json!("auto");
    });
    let runtime = runtime(Ok(builder_result()));

    assert!(matches!(
        run_direct_worker_once_with(
            &mut store,
            &active_policy(),
            &runtime,
            context("direct-worker-1", 100),
        ),
        Err(DirectWorkerError::InvalidJob)
    ));
    assert!(runtime.tasks.borrow().is_empty());
    let status = store.status(100).unwrap();
    assert_eq!(status.outbox_pending, 1);
    assert_eq!(status.outbox_leased, 0);
}

fn context(owner: &str, now: u64) -> DirectWorkerCycleContext<'_> {
    DirectWorkerCycleContext {
        owner,
        now,
        lease_seconds: 30,
        authorization_valid: true,
    }
}

fn runtime(result: Result<WorkerResult, DirectWorkerRuntimeError>) -> FakeRuntime {
    FakeRuntime {
        result,
        tasks: Rc::new(RefCell::new(Vec::new())),
    }
}

fn active_policy() -> pip_control::RepositoryPolicy {
    let mut value: Value = serde_json::from_slice(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    value["intake"]["enabled"] = json!(true);
    value["intake"]["paused"] = json!(false);
    value["dispatch_enabled"] = json!(true);
    value["github"]["automation_actor_id"] = json!(202880);
    value["github"]["reviewer_general_actor_id"] = json!(202881);
    value["github"]["reviewer_secperf_actor_id"] = json!(202882);
    pip_control::load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn queued_builder(root: &std::path::Path) -> Store {
    queued_builder_with(root, |_| {})
}

fn queued_builder_with(root: &std::path::Path, mutate: impl FnOnce(&mut Value)) -> Store {
    let mut store = Store::open(root.join("ledger.db")).unwrap();
    let policy = active_policy();
    store
        .record_policy(&PolicyInput {
            repository_id: 1_055_628_515,
            revision: policy.revision,
            accepted_at: 1,
            payload: json!({"fixture": true}),
        })
        .unwrap();
    store
        .create_case(&NewCase {
            case_key: case_key().into(),
            repository_id: 1_055_628_515,
            issue_number: 1240,
            workflow_version: 3,
            policy_revision: policy.revision,
            initial_state: "PLANNING".into(),
            observed_at: 1,
            event: EventInput {
                event_id: "event-intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({"label":"pip-ok"}),
            },
            effects: Vec::new(),
        })
        .unwrap();
    let mut task = serde_json::to_value(direct_task()).unwrap();
    mutate(&mut task);
    store
        .apply_transition(
            &TransitionInput {
                case_key: case_key().into(),
                expected_revision: 1,
                next_state: "READY_TO_BUILD".into(),
                remediation_round: 1,
                plan_version: 1,
                pr_number: None,
                head_sha: None,
                observed_at: 2,
                event: EventInput {
                    event_id: "event-plan-published".into(),
                    event_type: "PLAN_PUBLISHED".into(),
                    payload: json!({"plan_version": 1}),
                },
                run: None,
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: "effect-dispatch-builder:direct:builder".into(),
                    effect_type: "RUN_DIRECT_WORKER".into(),
                    payload: task,
                }],
            },
            None,
        )
        .unwrap();
    store
}

fn direct_task() -> DirectTaskSpec {
    DirectTaskSpec {
        schema_version: 1,
        source_effect_id: "effect-dispatch-builder".into(),
        task_id: task_id().into(),
        title: format!("Run builder for {}", case_key()),
        body: json!({
            "case_key": case_key(),
            "repository_id": 1_055_628_515_u64,
            "issue_number": 1240,
            "workflow_version": 3,
            "state_revision": 2,
            "role": "builder",
            "remediation_round": 1,
            "plan_version": 1,
            "assigned_branch": "pip/repo-1055628515/issue-1240/workflow-3",
            "assigned_worktree": "/var/lib/pip/worktrees/mdk/repo-1055628515-issue-1240-workflow-3",
            "execution": "direct",
            "provider": "cursor",
            "model": "cursor-grok-4.6-high-fast",
            "requested_model": "cursor/cursor-grok-4.6-high-fast",
            "skills_repository_commit": "a".repeat(40),
            "immutable_evidence_bundle": {"schema_version": 1, "sha256": "b".repeat(64)},
        }),
        role: WorkerRole::Builder,
        profile: "builder-grok".into(),
        workspace: "/var/lib/pip/worktrees/mdk/repo-1055628515-issue-1240-workflow-3".into(),
        skills: vec!["workflow-contract".into(), "builder-grok".into()],
        provider: "cursor".into(),
        model: "cursor-grok-4.6-high-fast".into(),
        max_runtime: "PT45M".into(),
        priority: 50,
    }
}

fn queued_shadow_review(root: &std::path::Path) -> Store {
    let mut store = Store::open(root.join("ledger.db")).unwrap();
    let policy = active_policy();
    store
        .record_policy(&PolicyInput {
            repository_id: 1_055_628_515,
            revision: policy.revision,
            accepted_at: 1,
            payload: json!({"fixture": true}),
        })
        .unwrap();
    store
        .create_case(&NewCase {
            case_key: case_key().into(),
            repository_id: 1_055_628_515,
            issue_number: 1240,
            workflow_version: 3,
            policy_revision: policy.revision,
            initial_state: "WAITING_CI".into(),
            observed_at: 1,
            event: EventInput {
                event_id: "event-waiting-ci".into(),
                event_type: "DRAFT_PR_PUBLISHED".into(),
                payload: json!({"head_sha": "b".repeat(40)}),
            },
            effects: Vec::new(),
        })
        .unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: case_key().into(),
                expected_revision: 1,
                next_state: "REVIEWING".into(),
                remediation_round: 0,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some("b".repeat(40)),
                observed_at: 2,
                event: EventInput {
                    event_id: "event-reviewing".into(),
                    event_type: "CI_ACCEPTED".into(),
                    payload: json!({"head_sha": "b".repeat(40)}),
                },
                run: None,
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: "effect-dispatch-reviewers:direct:secperf-opus".into(),
                    effect_type: "RUN_DIRECT_OBSERVER".into(),
                    payload: serde_json::to_value(shadow_task()).unwrap(),
                }],
            },
            None,
        )
        .unwrap();
    store
}

fn shadow_task() -> DirectTaskSpec {
    DirectTaskSpec {
        schema_version: 1,
        source_effect_id: "effect-dispatch-reviewers".into(),
        task_id: shadow_task_id().into(),
        title: format!("Run secperf-opus for {}", case_key()),
        body: json!({
            "case_key": case_key(),
            "repository_id": 1_055_628_515_u64,
            "issue_number": 1240,
            "workflow_version": 3,
            "state_revision": 2,
            "role": "reviewer-secperf",
            "reviewer_id": "secperf-opus",
            "review_mode": "shadow",
            "remediation_round": 0,
            "plan_version": 1,
            "review_round": 1,
            "pr_number": 77,
            "expected_head_sha": "b".repeat(40),
            "execution": "direct",
            "provider": "cursor",
            "model": "claude-opus-5-thinking-high",
            "requested_model": "cursor/claude-opus-5-thinking-high",
            "skills_repository_commit": "a".repeat(40),
            "immutable_evidence_bundle": {"schema_version": 1, "sha256": "b".repeat(64)},
        }),
        role: WorkerRole::ReviewerSecperf,
        profile: "reviewer-secperf-opus".into(),
        workspace: "/var/lib/pip/worktrees/mdk/repo-1055628515-issue-1240-workflow-3".into(),
        skills: vec!["workflow-contract".into(), "reviewer-secperf".into()],
        provider: "cursor".into(),
        model: "claude-opus-5-thinking-high".into(),
        max_runtime: "PT30M".into(),
        priority: 40,
    }
}

fn shadow_review_result() -> WorkerResult {
    let mut value = serde_json::from_str::<Value>(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap()["results"][3]
        .clone();
    value["workflow_version"] = json!(3);
    value["case"]["repository_id"] = json!(1_055_628_515_u64);
    value["case"]["workflow_version"] = json!(3);
    value["task_id"] = json!(shadow_task_id());
    value["reviewer_id"] = json!("secperf-opus");
    value["requested_model"] = json!("cursor/claude-opus-5-thinking-high");
    value["actual_model"] = json!("cursor/claude-opus-5-thinking-high");
    serde_json::from_value(value).unwrap()
}

fn builder_result() -> WorkerResult {
    let mut value = serde_json::from_str::<Value>(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap()["results"][1]
        .clone();
    value["workflow_version"] = json!(3);
    value["case"]["repository_id"] = json!(1_055_628_515_u64);
    value["case"]["workflow_version"] = json!(3);
    value["task_id"] = json!(task_id());
    value["requested_model"] = json!("cursor/cursor-grok-4.6-high-fast");
    value["actual_model"] = json!("cursor/cursor-grok-4.6-high-fast");
    serde_json::from_value(value).unwrap()
}

fn case_key() -> &'static str {
    "repo:1055628515#1240@3"
}

fn task_id() -> &'static str {
    "repo:1055628515#1240@3:builder:round:1:revision:2:worker"
}

fn shadow_task_id() -> &'static str {
    "repo:1055628515#1240@3:secperf-opus:round:1:revision:2:worker"
}
