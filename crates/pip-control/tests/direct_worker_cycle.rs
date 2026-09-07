use std::cell::RefCell;
use std::rc::Rc;

use pip_contracts::{WorkerResult, WorkerRole};
use pip_control::{
    DirectQueue, DirectQueueCycle, DirectQueueError, DirectWorkerRuntime, DirectWorkerRuntimeError,
    execute_direct_queue_once, recommended_direct_lease_seconds, reconcile_direct_queue_once,
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
fn serial_worker_leases_only_one_job_until_its_handoff_is_reconciled() {
    let directory = tempfile::tempdir().unwrap();
    let mut required = required_review_task();
    // The observer sorts first by effect ID; queue age/identity must not
    // override the required-vs-comparison scheduling class.
    required.source_effect_id = "zzz-dispatch-reviewers".into();
    let expected_task = required.task_id.clone();
    let mut store = queued_shadow_review_with(
        directory.path(),
        vec![EffectInput {
            effect_id: "zzz-dispatch-reviewers:direct:secperf-kimi".into(),
            effect_type: "RUN_DIRECT_WORKER".into(),
            payload: serde_json::to_value(required).unwrap(),
        }],
    );
    let queue = queue(directory.path());
    let policy = active_policy();
    assert_eq!(
        reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 100, 30, true,)
            .unwrap(),
        DirectQueueCycle::Prepared {
            attempt_id: 1,
            task_id: expected_task
        }
    );
    drop(store);
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    for now in [101, 132] {
        assert_eq!(
            reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", now, 30, true,)
                .unwrap(),
            DirectQueueCycle::Idle
        );
        assert_eq!(store.status(now).unwrap().direct_attempts_running, 1);
    }
    let runtime = runtime(Ok(builder_result()));
    execute_direct_queue_once(&runtime, &queue, 132).unwrap();
    assert!(
        runtime.tasks.borrow().is_empty(),
        "expired handoff cannot run a model"
    );
    assert!(matches!(
        reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 133, 30, true,)
            .unwrap(),
        DirectQueueCycle::Failed { attempt_id: 1 }
    ));
    assert!(matches!(
        reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 134, 30, true,)
            .unwrap(),
        DirectQueueCycle::Prepared { attempt_id: 2, .. }
    ));
}

fn required_review_task() -> DirectTaskSpec {
    let mut required = shadow_task();
    required.task_id = required.task_id.replace("secperf-opus", "secperf-kimi");
    required.title = format!("Run secperf-kimi for {}", case_key());
    required.priority = 50;
    required.profile = "reviewer-secperf-kimi".into();
    required.model = "kimi-k3-max".into();
    required.body["reviewer_id"] = json!("secperf-kimi");
    required.body["review_mode"] = json!("required");
    required.body["model"] = json!("kimi-k3-max");
    required.body["requested_model"] = json!("cursor/kimi-k3-max");
    required
}

#[test]
fn required_review_is_retained_under_changed_policy_and_accepted_after_peer_only_progress() {
    for intervening_event in ["REVIEW_RECORDED", "CI_ACCEPTED"] {
        let directory = tempfile::tempdir().unwrap();
        let task = required_review_task();
        let mut result = serde_json::to_value(shadow_review_result()).unwrap();
        result["task_id"] = json!(task.task_id);
        result["reviewer_id"] = json!("secperf-kimi");
        result["requested_model"] = json!("cursor/kimi-k3-max");
        result["actual_model"] = json!("cursor/kimi-k3-max");
        let mut store = queued_shadow_review_with(
            directory.path(),
            vec![EffectInput {
                effect_id: "effect-dispatch-reviewers:direct:secperf-kimi".into(),
                effect_type: "RUN_DIRECT_WORKER".into(),
                payload: serde_json::to_value(task).unwrap(),
            }],
        );
        let queue = queue(directory.path());
        let policy = active_policy();
        assert!(matches!(
            reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 100, 30, true,)
                .unwrap(),
            DirectQueueCycle::Prepared { attempt_id: 1, .. }
        ));
        let runtime = runtime(Ok(serde_json::from_value(result).unwrap()));
        execute_direct_queue_once(&runtime, &queue, 101).unwrap();
        store
            .apply_transition(
                &TransitionInput {
                    case_key: case_key().into(),
                    expected_revision: 2,
                    next_state: "REVIEWING".into(),
                    remediation_round: 0,
                    plan_version: 1,
                    pr_number: Some(77),
                    head_sha: Some("b".repeat(40)),
                    observed_at: 102,
                    event: EventInput {
                        event_id: "peer-or-retry".into(),
                        event_type: intervening_event.into(),
                        payload: json!({}),
                    },
                    run: None,
                    evidence: vec![],
                    findings: vec![],
                    effects: vec![],
                },
                None,
            )
            .unwrap();
        let mut paused = policy.clone();
        paused.revision += 1;
        for role in &mut paused.roles {
            if role.reviewer_id.as_deref() == Some("secperf-kimi") {
                role.model = "different-model".into();
            }
        }
        paused.intake.paused = true;
        paused.dispatch_enabled = false;
        for now in [103, 104] {
            assert_eq!(
                reconcile_direct_queue_once(
                    &mut store,
                    &paused,
                    &queue,
                    "controller",
                    now,
                    30,
                    false
                )
                .unwrap(),
                DirectQueueCycle::Retained { attempt_id: 1 }
            );
            assert_eq!(store.case(case_key()).unwrap().unwrap().state_revision, 3);
            assert_eq!(store.run_count().unwrap(), 0);
        }
        let collected =
            reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 105, 30, true)
                .unwrap();
        if intervening_event == "REVIEW_RECORDED" {
            assert!(matches!(collected, DirectQueueCycle::Ingested { .. }));
            assert_eq!(store.run_count().unwrap(), 1);
        } else {
            assert_eq!(collected, DirectQueueCycle::Cleaned { attempt_id: 1 });
            assert_eq!(store.run_count().unwrap(), 0);
            assert_eq!(store.case(case_key()).unwrap().unwrap().state_revision, 3);
        }
        assert_eq!(store.status(105).unwrap().direct_attempts_complete, 1);
        assert_eq!(runtime.tasks.borrow().len(), 1);
    }
}

#[test]
fn paused_queue_retains_valid_completed_work_without_advancing_or_rerunning_it() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder(directory.path());
    let queue = queue(directory.path());
    let runtime = runtime(Ok(builder_result()));
    let policy = active_policy();
    reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 100, 30, true).unwrap();
    execute_direct_queue_once(&runtime, &queue, 101).unwrap();
    let case_before = store.case(case_key()).unwrap();
    for now in [102, 103] {
        let mut paused = policy.clone();
        paused.intake.paused = true;
        paused.dispatch_enabled = false;
        let policy_path = directory.path().join("policy.json");
        std::fs::write(&policy_path, serde_json::to_vec(&paused).unwrap()).unwrap();
        let mut args = vec!["controller-cycle".to_owned()];
        for (name, value) in [
            ("--policy", policy_path.to_str().unwrap().to_owned()),
            ("--database", store.path().to_str().unwrap().to_owned()),
            (
                "--direct-queue",
                directory
                    .path()
                    .join("direct-queue")
                    .to_str()
                    .unwrap()
                    .to_owned(),
            ),
            ("--now", now.to_string()),
            ("--owner", "controller".into()),
            ("--github-token", "/missing/token".into()),
            ("--github-reviewer-general-app", "/missing/app".into()),
            ("--github-reviewer-general-key", "/missing/key".into()),
            ("--github-reviewer-secperf-app", "/missing/app".into()),
            ("--github-reviewer-secperf-key", "/missing/key".into()),
            ("--git-askpass", "/missing/askpass".into()),
            ("--hermes", "/missing/hermes".into()),
            ("--skills-commit-file", "/missing/commit".into()),
        ] {
            args.extend([name.into(), value]);
        }
        let report = pip_control::run_cli(args).unwrap();
        assert_eq!(report["result"], "disabled");
        assert_eq!(store.status(now).unwrap().direct_attempts_complete, 1);
        assert_eq!(store.run_count().unwrap(), 0);
        assert_eq!(store.case(case_key()).unwrap(), case_before);
    }
    assert_eq!(runtime.tasks.borrow().len(), 1);
    drop(store);
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 200, 30, true).unwrap();
    assert_eq!(store.run_count().unwrap(), 1);
    assert_eq!(runtime.tasks.borrow().len(), 1);
}

#[test]
fn retained_result_does_not_hide_later_queue_errors() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder(directory.path());
    let queue = queue(directory.path());
    let policy = active_policy();
    reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 100, 30, true).unwrap();
    execute_direct_queue_once(&runtime(Ok(builder_result())), &queue, 101).unwrap();
    std::fs::write(
        directory.path().join("direct-queue/results/attempt-2.json"),
        b"not JSON",
    )
    .unwrap();
    assert!(
        reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 102, 30, false)
            .is_err()
    );
    assert_eq!(store.status(102).unwrap().direct_attempts_complete, 1);
    assert_eq!(store.run_count().unwrap(), 0);
}

#[test]
fn paused_collection_rejects_misbound_results_and_records_failures_without_dispatch() {
    for malformed in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = queued_builder(directory.path());
        let queue = queue(directory.path());
        let policy = active_policy();
        let result = if malformed {
            let mut result = builder_result();
            if let WorkerResult::Builder(builder) = &mut result {
                builder.common.task_id = "some-other-task".into();
            }
            Ok(result)
        } else {
            Err(DirectWorkerRuntimeError::Failed("reported failure".into()))
        };
        let runtime = runtime(result);
        reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 100, 30, true)
            .unwrap();
        execute_direct_queue_once(&runtime, &queue, 101).unwrap();
        let case_before = store.case(case_key()).unwrap();
        let collected =
            reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 102, 30, false);
        assert_eq!(collected.is_err(), malformed);
        assert_eq!(store.status(102).unwrap().direct_attempts_complete, 0);
        assert_eq!(
            store.status(102).unwrap().direct_attempts_failed,
            u64::from(!malformed)
        );
        assert_eq!(store.run_count().unwrap(), 0);
        assert_eq!(store.case(case_key()).unwrap(), case_before);
        assert_eq!(runtime.tasks.borrow().len(), 1);
        if !malformed {
            assert_eq!(
                reconcile_direct_queue_once(
                    &mut store,
                    &policy,
                    &queue,
                    "controller",
                    103,
                    30,
                    false
                )
                .unwrap(),
                DirectQueueCycle::AuthorizationBlocked
            );
        }
    }
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
        DirectQueueCycle::Retained { attempt_id: 1 }
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
            1_000,
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
fn queued_task_failure_is_recorded_by_controller_and_releases_the_effect() {
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
    let runtime = runtime(Err(DirectWorkerRuntimeError::Failed("outage".into())));
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
    // A failed comparison is preserved, but cannot spend the main case budget.
    let mut policy = active_policy();
    policy.max_provider_failures = 1;
    assert_eq!(
        pip_control::enforce_operational_bounds(&mut store, &policy, 104).unwrap(),
        pip_control::OperationalBoundsCycle::Idle
    );
    assert_eq!(store.case(case_key()).unwrap().unwrap().state, "REVIEWING");
    assert!(
        store
            .immutable_history_for_case(case_key())
            .unwrap()
            .evidence
            .iter()
            .any(|evidence| evidence.kind == "DETACHED_REVIEW_FAILURE")
    );
}

// All execution tests use the production controller/worker message boundary.
fn queue(root: &std::path::Path) -> DirectQueue {
    let root = root.join("direct-queue");
    for child in ["inbox", "results", "archive"] {
        std::fs::create_dir_all(root.join(child)).unwrap();
    }
    DirectQueue::new(root).unwrap()
}

#[test]
fn unavailable_or_occupied_queue_never_leases_new_work() {
    for handoff_exists in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = queued_builder(directory.path());
        let queue = queue(directory.path());
        let inbox = directory.path().join("direct-queue/inbox");
        if handoff_exists {
            // A collision is uncertain: never authorize a second writer.
            std::fs::write(inbox.join("attempt-1.json"), b"existing handoff").unwrap();
        } else {
            std::fs::remove_dir(&inbox).unwrap();
        }
        let result = reconcile_direct_queue_once(
            &mut store,
            &active_policy(),
            &queue,
            "controller",
            100,
            30,
            true,
        );
        if handoff_exists {
            assert_eq!(result.unwrap(), DirectQueueCycle::Idle);
        } else {
            assert!(result.is_err());
        }
        let status = store.status(101).unwrap();
        assert_eq!(status.direct_attempts_running, 0);
        assert_eq!(status.direct_attempts_failed, 0);
        assert_eq!(
            store
                .failed_direct_attempt_count_for_case(case_key())
                .unwrap(),
            0
        );
        if handoff_exists {
            std::fs::remove_file(inbox.join("attempt-1.json")).unwrap();
        } else {
            std::fs::create_dir(&inbox).unwrap();
        }
        assert!(matches!(
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
            DirectQueueCycle::Prepared { attempt_id: 1, .. }
        ));
    }
}

#[test]
fn unavailable_runtime_backs_off_without_spending_the_case_budget() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder(directory.path());
    let queue = queue(directory.path());
    let mut policy = active_policy();
    policy.max_provider_failures = 1;
    let runtime = runtime(Err(DirectWorkerRuntimeError::Unavailable(
        "runtime offline".into(),
    )));
    reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 100, 30, true).unwrap();
    execute_direct_queue_once(&runtime, &queue, 101).unwrap();
    reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 102, 30, true).unwrap();
    assert_eq!(
        pip_control::enforce_operational_bounds(&mut store, &policy, 103).unwrap(),
        pip_control::OperationalBoundsCycle::Idle
    );
    assert_eq!(store.status(103).unwrap().direct_attempts_failed, 1);
    assert_eq!(
        store
            .record_direct_unavailability(1, "controller", 104, "runtime offline")
            .unwrap(),
        162
    );
    assert_eq!(
        store
            .immutable_history_for_case(case_key())
            .unwrap()
            .evidence
            .iter()
            .filter(|e| e.kind == "DIRECT_RUNTIME_UNAVAILABLE")
            .count(),
        1
    );
    assert!(
        store
            .record_direct_unavailability(1, "another-controller", 104, "runtime offline")
            .is_err()
    );
    assert_eq!(
        reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 110, 30, true)
            .unwrap(),
        DirectQueueCycle::Idle
    );
    drop(store);
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    assert!(matches!(
        reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 163, 30, true)
            .unwrap(),
        DirectQueueCycle::Prepared { attempt_id: 2, .. }
    ));
    assert_eq!(runtime.tasks.borrow().len(), 1);
    execute_direct_queue_once(&runtime, &queue, 164).unwrap();
    reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 165, 30, true).unwrap();
    assert_eq!(
        store
            .record_direct_unavailability(2, "controller", 166, "runtime offline")
            .unwrap(),
        285
    );
    assert_eq!(
        store
            .failed_direct_attempt_count_for_case(case_key())
            .unwrap(),
        0
    );
}

#[test]
fn lease_expired_before_execution_is_an_outage_not_a_work_failure() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder(directory.path());
    let queue = queue(directory.path());
    let policy = active_policy();
    let runtime = runtime(Ok(builder_result()));
    reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 100, 30, true).unwrap();
    execute_direct_queue_once(&runtime, &queue, 131).unwrap();
    reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 132, 30, true).unwrap();
    assert!(runtime.tasks.borrow().is_empty());
    assert_eq!(
        store
            .failed_direct_attempt_count_for_case(case_key())
            .unwrap(),
        0
    );
}

#[test]
fn completed_result_survives_controller_restart_without_rerunning_provider() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder(directory.path());
    let queue = queue(directory.path());
    let policy = active_policy();
    let runtime = runtime(Ok(builder_result()));
    reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 100, 30, true).unwrap();
    execute_direct_queue_once(&runtime, &queue, 101).unwrap();
    drop(store);
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    // Even another worker tick does not execute an inbox item with a result.
    assert_eq!(
        execute_direct_queue_once(&runtime, &queue, 102).unwrap(),
        DirectQueueCycle::Idle
    );
    assert_eq!(
        reconcile_direct_queue_once(&mut store, &policy, &queue, "controller", 103, 30, true)
            .unwrap(),
        DirectQueueCycle::Ingested {
            task_id: task_id().into(),
            transition_count: 2
        }
    );
    assert_eq!(runtime.tasks.borrow().len(), 1);
    assert_eq!(
        runtime.tasks.borrow()[0].0.model,
        "cursor-grok-4.6-high-fast"
    );
    assert_eq!(store.status(103).unwrap().direct_attempts_complete, 1);
}

#[test]
fn unavailability_transaction_cannot_change_an_attempt_after_lease_loss() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder(directory.path());
    let queue = queue(directory.path());
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
    let attempt = store.direct_attempt(1).unwrap().unwrap();
    store
        .release_effect(&attempt.effect_id, "controller")
        .unwrap();
    assert!(
        store
            .record_direct_unavailability(1, "controller", 102, "runtime offline")
            .is_err()
    );
    assert_eq!(store.status(102).unwrap().direct_attempts_running, 1);
    assert_eq!(store.status(102).unwrap().direct_attempts_failed, 0);
    assert!(
        !store
            .immutable_history_for_case(case_key())
            .unwrap()
            .evidence
            .iter()
            .any(|e| e.kind == "DIRECT_RUNTIME_UNAVAILABLE")
    );
}

#[test]
fn stale_authorization_never_prepares_a_direct_job() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder(directory.path());
    let queue = queue(directory.path());
    assert_eq!(
        reconcile_direct_queue_once(
            &mut store,
            &active_policy(),
            &queue,
            "controller",
            100,
            30,
            false
        )
        .unwrap(),
        DirectQueueCycle::AuthorizationBlocked
    );
    let status = store.status(100).unwrap();
    assert_eq!(status.outbox_pending, 1);
    assert_eq!(status.outbox_leased, 0);
    assert_eq!(status.direct_attempts_running, 0);
}

#[test]
fn corrupted_direct_job_never_reaches_the_runtime() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder_with(directory.path(), |task| {
        task["model"] = json!("auto");
    });
    let queue = queue(directory.path());
    assert!(matches!(
        reconcile_direct_queue_once(
            &mut store,
            &active_policy(),
            &queue,
            "controller",
            100,
            30,
            true
        ),
        Err(DirectQueueError::InvalidEnvelope)
    ));
    let runtime = runtime(Ok(builder_result()));
    assert_eq!(
        execute_direct_queue_once(&runtime, &queue, 101).unwrap(),
        DirectQueueCycle::Idle
    );
    assert!(runtime.tasks.borrow().is_empty());
    assert_eq!(store.status(101).unwrap().direct_attempts_running, 0);
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
            payload: serde_json::to_value(&policy).unwrap(),
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
    queued_shadow_review_with(root, vec![])
}

fn queued_shadow_review_with(root: &std::path::Path, mut effects: Vec<EffectInput>) -> Store {
    effects.push(EffectInput {
        effect_id: "effect-dispatch-reviewers:direct:secperf-opus".into(),
        effect_type: "RUN_DIRECT_OBSERVER".into(),
        payload: serde_json::to_value(shadow_task()).unwrap(),
    });
    let mut store = Store::open(root.join("ledger.db")).unwrap();
    let policy = active_policy();
    store
        .record_policy(&PolicyInput {
            repository_id: 1_055_628_515,
            revision: policy.revision,
            accepted_at: 1,
            payload: serde_json::to_value(&policy).unwrap(),
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
                effects,
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
