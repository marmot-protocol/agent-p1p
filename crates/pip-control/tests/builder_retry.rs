use pip_control::{
    BuilderRetryRequest, DispatchCycleContext, OperationalBound, OperationalBoundsCycle,
    RepositoryPolicy, authorize_builder_retry, dispatch_once_with, enforce_operational_bounds,
    load_repository_policy,
};
use pip_hermes::{CommandOutput, CommandRunner, CommandSpec, HermesError};
use pip_store::{
    ApplyResult, EffectInput, EventInput, NewCase, PolicyInput, RunInput, Store, TransitionInput,
};
use serde_json::json;

const CASE: &str = "repo:42#9@3";

#[test]
fn remediation_builder_retry_preserves_existing_pr_head_plan_and_round() {
    let (_dir, mut store, paused, accepted, mut request) = fixture();
    let old = store.claim_effect("worker", 132, 5).unwrap().unwrap();
    let id = store.begin_direct_attempt(&old, "old-task", 132).unwrap();
    store
        .complete_direct_attempt(id, "worker", 133, &json!({"accepted_build":true}))
        .unwrap();
    store
        .acknowledge_effect(&old.effect_id, "worker", 133)
        .unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: CASE.into(),
                expected_revision: 2,
                next_state: "REMEDIATING".into(),
                remediation_round: 1,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some("b".repeat(40)),
                observed_at: 135,
                event: EventInput {
                    event_id: "reviews-published".into(),
                    event_type: "REVIEWS_PUBLISHED".into(),
                    payload: json!({}),
                },
                run: Some(RunInput {
                    run_id: "accepted-build".into(),
                    task_id: "old-task".into(),
                    role: "builder".into(),
                    payload: json!({"head_sha":"b".repeat(40)}),
                }),
                evidence: vec![],
                findings: vec![],
                effects: vec![EffectInput {
                    effect_id: "remediation-builder".into(),
                    effect_type: "RUN_DIRECT_WORKER".into(),
                    payload: json!({"role":"builder","task_id":"remediation-task"}),
                }],
            },
            None,
        )
        .unwrap();
    let effect = store.claim_effect("worker", 136, 5).unwrap().unwrap();
    let id = store
        .begin_direct_attempt(&effect, "remediation-task", 136)
        .unwrap();
    store
        .fail_direct_attempt(id, "worker", 137, "invalid output binding")
        .unwrap();
    store.release_effect(&effect.effect_id, "worker").unwrap();
    enforce_operational_bounds(&mut store, &accepted, 140).unwrap();
    request.expected_revision = 4;
    request.expected_failures = 4;
    request.effect_id = effect.effect_id;
    let history = store.immutable_history_for_case(CASE).unwrap();
    assert_eq!(
        authorize_builder_retry(&mut store, &paused, &request, 141, 0).unwrap(),
        ApplyResult::Applied
    );
    let case = store.case(CASE).unwrap().unwrap();
    assert_eq!(case.state, "READY_TO_BUILD");
    assert_eq!(case.pr_number, Some(77));
    assert_eq!(case.head_sha, Some("b".repeat(40)));
    assert_eq!(case.plan_version, 1);
    assert_eq!(case.remediation_round, 1);
    assert_eq!(store.failed_direct_attempt_count_for_case(CASE).unwrap(), 4);
    assert_eq!(
        store.immutable_history_for_case(CASE).unwrap().runs,
        history.runs
    );
    assert_eq!(
        authorize_builder_retry(&mut store, &paused, &request, 142, 0).unwrap(),
        ApplyResult::Replayed
    );
}

#[test]
fn retry_cannot_replace_a_running_or_completed_target_attempt() {
    for complete in [false, true] {
        let (_dir, mut store, paused, _accepted, request) = fixture();
        let effect = store.claim_effect("worker", 132, 5).unwrap().unwrap();
        let id = store
            .begin_direct_attempt(&effect, "old-task", 132)
            .unwrap();
        if complete {
            store
                .complete_direct_attempt(id, "worker", 133, &json!({"completed":true}))
                .unwrap();
        }
        store.release_effect(&effect.effect_id, "worker").unwrap();
        let before = store.status(140).unwrap();
        assert!(authorize_builder_retry(&mut store, &paused, &request, 140, 0).is_err());
        assert_eq!(store.status(140).unwrap(), before);
    }
}

#[test]
fn review_retry_preserves_the_build_and_requires_fresh_ci() {
    review_retry_with_payload(false, "reviewer-secperf", "required");
}

#[test]
fn review_retry_resolves_frozen_dispatch_references() {
    review_retry_with_payload(true, "reviewer-secperf", "required");
}

#[test]
fn review_retry_rejects_frozen_wrong_role_and_observer_intents() {
    review_retry_with_payload(true, "builder", "required");
    review_retry_with_payload(true, "reviewer-secperf", "observer");
}

fn review_retry_with_payload(frozen: bool, role: &str, mode: &str) {
    let (_dir, mut store, paused, accepted, mut request) = fixture();
    store.apply_transition(&TransitionInput {
        case_key: CASE.into(), expected_revision: 2, next_state: "REVIEWING".into(),
        remediation_round: 0, plan_version: 1, pr_number: Some(77), head_sha: Some("b".repeat(40)),
        observed_at: 135, event: EventInput { event_id:"ci-accepted".into(), event_type:"CI_ACCEPTED".into(), payload:json!({}) },
        run:Some(RunInput { run_id:"build-run".into(),task_id:"builder".into(),role:"builder".into(),payload:json!({"head_sha":"b".repeat(40)}) }),
        evidence:vec![], findings:vec![], effects:vec![EffectInput {
            effect_id:if frozen {"review-dispatch"} else {"old-review"}.into(),effect_type:if frozen {"DISPATCH_REVIEWERS"} else {"RUN_DIRECT_WORKER"}.into(),
                payload:json!({"role":"reviewer-secperf","body":{"review_mode":"required"},"task_id":"reviewer"}),
        }],
    },None).unwrap();
    if frozen {
        let dispatch = store.claim_effect("dispatcher", 135, 5).unwrap().unwrap();
        let desired = json!({"source_effect_id":"review-dispatch","role":role,"body":{"review_mode":mode},"task_id":"reviewer"});
        store
            .freeze_dispatch_intents(
                &dispatch,
                &[pip_store::DispatchIntent {
                    intent_id: "reviewer".into(),
                    transport: pip_store::DispatchTransport::Direct,
                    desired: desired.clone(),
                }],
                135,
            )
            .unwrap();
        store
            .complete_dispatch_outputs(
                "review-dispatch",
                &[],
                &[EffectInput {
                    effect_id: "old-review".into(),
                    effect_type: "RUN_DIRECT_WORKER".into(),
                    payload: desired,
                }],
                "dispatcher",
                135,
                None,
            )
            .unwrap();
    }
    let effect = store.claim_effect("worker", 136, 5).unwrap().unwrap();
    let attempt = store
        .begin_direct_attempt(&effect, "reviewer", 136)
        .unwrap();
    store
        .fail_direct_attempt(attempt, "worker", 137, "workspace trust required")
        .unwrap();
    store.release_effect(&effect.effect_id, "worker").unwrap();
    enforce_operational_bounds(&mut store, &accepted, 138).unwrap();
    request.expected_revision = 4;
    request.effect_id = "old-review".into();
    request.expected_failures = 4;
    let before = store.immutable_history_for_case(CASE).unwrap();
    let status = store.status(139).unwrap();
    if role != "reviewer-secperf" || mode != "required" {
        assert!(
            pip_control::authorize_review_retry(&mut store, &paused, &request, 139, 0).is_err()
        );
        assert_eq!(store.status(139).unwrap(), status);
        assert_eq!(store.immutable_history_for_case(CASE).unwrap(), before);
        return;
    }
    for field in ["revision", "failures", "effect", "reason"] {
        let mut invalid = request.clone();
        match field {
            "revision" => invalid.expected_revision += 1,
            "failures" => invalid.expected_failures += 1,
            "effect" => invalid.effect_id = "old-builder".into(),
            "reason" => invalid.reason.clear(),
            _ => unreachable!(),
        }
        assert!(
            pip_control::authorize_review_retry(&mut store, &paused, &invalid, 139, 0).is_err(),
            "accepted {field}"
        );
        assert_eq!(store.status(139).unwrap(), status);
    }
    assert!(pip_control::authorize_review_retry(&mut store, &accepted, &request, 139, 0).is_err());
    assert!(
        pip_control::authorize_review_retry(
            &mut store,
            &paused,
            &request,
            100 + accepted.max_case_elapsed_seconds,
            0
        )
        .is_err()
    );
    assert!(pip_control::authorize_review_retry(&mut store, &paused, &request, 139, 1000).is_err());
    assert_eq!(
        pip_control::authorize_review_retry(&mut store, &paused, &request, 139, 0).unwrap(),
        ApplyResult::Applied
    );
    let case = store.case(CASE).unwrap().unwrap();
    assert_eq!(case.state, "WAITING_CI");
    assert_eq!(case.pr_number, Some(77));
    assert_eq!(case.head_sha, Some("b".repeat(40)));
    assert_eq!(case.plan_version, 1);
    assert_eq!(
        store.immutable_history_for_case(CASE).unwrap().runs,
        before.runs
    );
    assert_eq!(store.failed_direct_attempt_count_for_case(CASE).unwrap(), 4);
    assert_eq!(store.effective_provider_failure_limit(CASE, 3).unwrap(), 5);
    assert_eq!(
        pip_control::authorize_review_retry(&mut store, &paused, &request, 140, 0).unwrap(),
        ApplyResult::Replayed
    );
    let mut conflicting = request.clone();
    conflicting.reason.push_str(" altered");
    assert!(
        pip_control::authorize_review_retry(&mut store, &paused, &conflicting, 140, 0).is_err()
    );
    let effect = store.claim_effect("controller", 141, 5).unwrap().unwrap();
    assert_eq!(effect.effect_type, "OBSERVE_CI");
}

#[derive(Clone)]
struct NoHermes;
impl CommandRunner for NoHermes {
    fn run(&self, _: &CommandSpec) -> Result<CommandOutput, HermesError> {
        panic!("builder retry must not invoke Hermes")
    }
}

fn fixture() -> (
    tempfile::TempDir,
    Store,
    RepositoryPolicy,
    RepositoryPolicy,
    BuilderRetryRequest,
) {
    fixture_at(100)
}

fn fixture_at(
    created: u64,
) -> (
    tempfile::TempDir,
    Store,
    RepositoryPolicy,
    RepositoryPolicy,
    BuilderRetryRequest,
) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("ledger.db")).unwrap();
    let mut paused = load_repository_policy(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    paused.repository.id = 42;
    let mut accepted = paused.clone();
    accepted.revision = 7;
    accepted.intake.enabled = true;
    accepted.intake.paused = false;
    accepted.dispatch_enabled = true;
    store
        .record_policy(&PolicyInput {
            repository_id: 42,
            revision: 7,
            accepted_at: created,
            payload: serde_json::to_value(&accepted).unwrap(),
        })
        .unwrap();
    store
        .create_case(&NewCase {
            case_key: CASE.into(),
            repository_id: 42,
            issue_number: 9,
            workflow_version: 3,
            policy_revision: 7,
            initial_state: "PLANNING".into(),
            observed_at: created,
            event: EventInput {
                event_id: "intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: vec![],
        })
        .unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: CASE.into(),
                expected_revision: 1,
                next_state: "READY_TO_BUILD".into(),
                remediation_round: 0,
                plan_version: 1,
                pr_number: None,
                head_sha: None,
                observed_at: created + 1,
                event: EventInput {
                    event_id: "plan".into(),
                    event_type: "PROCEED".into(),
                    payload: json!({}),
                },
                run: Some(RunInput {
                    run_id: "plan-run".into(),
                    task_id: "planner".into(),
                    role: "planner".into(),
                    payload: json!({"plan_version":1,"plan_sha256":"a".repeat(64)}),
                }),
                evidence: vec![],
                findings: vec![],
                effects: vec![EffectInput {
                    effect_id: "old-builder".into(),
                    effect_type: "RUN_DIRECT_WORKER".into(),
                    payload: json!({"role":"builder","task_id":"old-task"}),
                }],
            },
            None,
        )
        .unwrap();
    for now in [created + 10, created + 20, created + 30] {
        let claimed = store.claim_effect("worker", now, 5).unwrap().unwrap();
        let id = store
            .begin_direct_attempt(&claimed, "old-task", now)
            .unwrap();
        store
            .fail_direct_attempt(id, "worker", now + 1, "startup failure")
            .unwrap();
        store.release_effect(&claimed.effect_id, "worker").unwrap();
    }
    let request = BuilderRetryRequest {
        case_key: CASE.into(),
        expected_revision: 2,
        effect_id: "old-builder".into(),
        expected_failures: 3,
        request_id: "operator-retry-1".into(),
        reason: "Workspace boundary repaired".into(),
    };
    (dir, store, paused, accepted, request)
}

#[test]
fn exhausted_builder_can_be_retried_after_automatic_escalation_without_erasing_history() {
    let (_dir, mut store, paused, accepted, mut request) = fixture();
    assert!(matches!(
        enforce_operational_bounds(&mut store, &accepted, 140).unwrap(),
        OperationalBoundsCycle::Escalated {
            bound: OperationalBound::ProviderFailures,
            ..
        }
    ));
    request.expected_revision = 3;
    let before = store.immutable_history_for_case(CASE).unwrap();
    assert_eq!(
        authorize_builder_retry(&mut store, &paused, &request, 141, 0).unwrap(),
        ApplyResult::Applied
    );
    let case = store.case(CASE).unwrap().unwrap();
    assert_eq!(case.state, "READY_TO_BUILD");
    assert_eq!(case.state_revision, 4);
    assert_eq!(store.failed_direct_attempt_count_for_case(CASE).unwrap(), 3);
    assert_eq!(store.effective_provider_failure_limit(CASE, 3).unwrap(), 4);
    assert_eq!(
        store.immutable_history_for_case(CASE).unwrap().runs,
        before.runs
    );
    assert_eq!(
        authorize_builder_retry(&mut store, &paused, &request, 142, 0).unwrap(),
        ApplyResult::Replayed
    );
    assert_eq!(
        enforce_operational_bounds(&mut store, &accepted, 143).unwrap(),
        OperationalBoundsCycle::Idle
    );
}

#[test]
fn retry_cannot_reopen_an_unrelated_escalation() {
    for (bound, source) in [
        ("ELAPSED_TIME", "direct-worker"),
        ("PROVIDER_FAILURES", "hermes-circuit-breaker"),
        ("REPEATED_FINDINGS", "direct-worker"),
    ] {
        let (_dir, mut store, paused, _accepted, mut request) = fixture();
        store.apply_transition(&TransitionInput {
            case_key: CASE.into(), expected_revision: 2, next_state: "ESCALATED".into(),
            remediation_round: 0, plan_version: 1, pr_number: None, head_sha: None, observed_at: 140,
            event: EventInput { event_id: "different-escalation".into(), event_type: "OPERATIONAL_BOUND_REACHED".into(),
                payload: json!({"bound":bound,"details":{"source":source},"observed":3,"limit":3}) },
            run: None, evidence: vec![], findings: vec![], effects: vec![],
        }, None).unwrap();
        request.expected_revision = 3;
        let before = store.status(141).unwrap();
        assert!(authorize_builder_retry(&mut store, &paused, &request, 141, 0).is_err());
        assert_eq!(store.status(141).unwrap(), before);
    }
}

#[test]
fn retry_redispatches_exact_model_with_fresh_skills_and_stops_after_another_failure() {
    let (_dir, mut store, paused, accepted, request) = fixture();
    let history = store.immutable_history_for_case(CASE).unwrap();
    assert_eq!(
        authorize_builder_retry(&mut store, &paused, &request, 140, 0).unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        authorize_builder_retry(&mut store, &paused, &request, 141, 0).unwrap(),
        ApplyResult::Replayed
    );
    assert_eq!(
        enforce_operational_bounds(&mut store, &accepted, 142).unwrap(),
        OperationalBoundsCycle::Idle
    );
    dispatch_once_with(
        &mut store,
        &accepted,
        NoHermes,
        DispatchCycleContext {
            skills_repository_commit: &"b".repeat(40),
            hermes_program: "unused",
            owner: "controller",
            now: 143,
            lease_seconds: 30,
            authorization_valid: true,
        },
    )
    .unwrap();
    let mut stacked = request.clone();
    stacked.request_id = "operator-retry-2".into();
    stacked.expected_revision = 3;
    let before = store.status(143).unwrap();
    assert!(authorize_builder_retry(&mut store, &paused, &stacked, 143, 0).is_err());
    assert_eq!(store.status(143).unwrap(), before);
    assert_eq!(
        store
            .effective_provider_failure_limit("repo:42#10@3", 3)
            .unwrap(),
        3
    );
    let claimed = store
        .claim_effect_matching("worker", 144, 30, &["RUN_DIRECT_WORKER"])
        .unwrap()
        .unwrap();
    assert_ne!(claimed.effect_id, "old-builder");
    assert_eq!(
        claimed.payload["body"]["skills_repository_commit"],
        "b".repeat(40)
    );
    assert_eq!(claimed.payload["body"]["plan_version"], 1);
    assert_eq!(
        claimed.payload["model"],
        accepted
            .roles
            .iter()
            .find(|r| r.role == pip_contracts::WorkerRole::Builder)
            .unwrap()
            .model
    );
    assert_eq!(claimed.state_revision, 3);
    let id = store
        .begin_direct_attempt(&claimed, claimed.payload["task_id"].as_str().unwrap(), 144)
        .unwrap();
    store
        .fail_direct_attempt(id, "worker", 145, "new failure")
        .unwrap();
    store.release_effect(&claimed.effect_id, "worker").unwrap();
    assert_eq!(store.status(145).unwrap().direct_attempts_failed, 4);
    assert_eq!(
        store.immutable_history_for_case(CASE).unwrap().runs,
        history.runs
    );
    assert!(matches!(
        enforce_operational_bounds(&mut store, &accepted, 146).unwrap(),
        OperationalBoundsCycle::Escalated {
            bound: OperationalBound::ProviderFailures,
            observed: 4,
            limit: 4,
            ..
        }
    ));
    let after = store.status(147).unwrap();
    assert_eq!(
        authorize_builder_retry(&mut store, &paused, &request, 147, 0).unwrap(),
        ApplyResult::Replayed
    );
    assert_eq!(store.status(147).unwrap(), after);
}

#[test]
fn retry_requires_root_inert_runtime_bindings_and_exact_request() {
    for scenario in [
        "uid", "active", "model", "revision", "effect", "reason", "time",
    ] {
        let (_dir, mut store, mut paused, _accepted, mut request) = fixture();
        let mut uid = 0;
        let mut now = 140;
        match scenario {
            "uid" => uid = 1000,
            "active" => paused.dispatch_enabled = true,
            "model" => paused.roles[1].model = "different".into(),
            "revision" => request.expected_revision = 1,
            "effect" => request.effect_id = "foreign".into(),
            "reason" => request.reason.clear(),
            "time" => now = 86500,
            _ => unreachable!(),
        }
        let before = store.status(140).unwrap();
        assert!(
            authorize_builder_retry(&mut store, &paused, &request, now, uid).is_err(),
            "{scenario}"
        );
        assert_eq!(store.status(140).unwrap(), before);
    }
    let (_dir, mut store, paused, _accepted, mut request) = fixture();
    authorize_builder_retry(&mut store, &paused, &request, 140, 0).unwrap();
    request.reason = "changed authorization".into();
    assert!(authorize_builder_retry(&mut store, &paused, &request, 141, 0).is_err());
}

#[test]
fn retry_never_extends_the_case_deadline() {
    let (_dir, mut store, paused, accepted, request) = fixture();
    authorize_builder_retry(&mut store, &paused, &request, 140, 0).unwrap();
    assert!(matches!(
        enforce_operational_bounds(&mut store, &accepted, 86500).unwrap(),
        OperationalBoundsCycle::Escalated {
            bound: OperationalBound::ElapsedTime,
            ..
        }
    ));
}

#[test]
#[ignore = "requires root and disposable systemd; run by the lifecycle harness"]
fn root_cli_retry_checks_real_uid_stopped_units_and_empty_queue() {
    use std::{fs, process::Command};
    assert!(rustix::process::geteuid().is_root());
    assert!(std::path::Path::new("/run/systemd/system").is_dir());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let (dir, store, paused, _accepted, _request) = fixture_at(now - 40);
    let policy = dir.path().join("paused.json");
    fs::write(&policy, serde_json::to_vec(&paused).unwrap()).unwrap();
    let queue = dir.path().join("queue");
    for name in ["inbox", "results", "archive"] {
        fs::create_dir_all(queue.join(name)).unwrap();
    }
    let args = [
        "authorize-builder-retry",
        "--policy",
        policy.to_str().unwrap(),
        "--database",
        store.path().to_str().unwrap(),
        "--direct-queue",
        queue.to_str().unwrap(),
        "--case",
        CASE,
        "--expected-revision",
        "2",
        "--effect-id",
        "old-builder",
        "--expected-failures",
        "3",
        "--request-id",
        "operator-retry-1",
        "--reason",
        "Workspace repaired",
    ];
    let binary = env!("CARGO_BIN_EXE_pip-control");
    let run = || Command::new(binary).args(args).output().unwrap();
    let mut review_args = args;
    review_args[0] = "authorize-review-retry";
    let publication_args = [
        "authorize-publication-retry",
        "--policy",
        policy.to_str().unwrap(),
        "--database",
        store.path().to_str().unwrap(),
        "--direct-queue",
        queue.to_str().unwrap(),
        "--case",
        CASE,
        "--expected-revision",
        "2",
        "--expected-head",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "--request-id",
        "sign-accepted-build",
        "--reason",
        "Sign the accepted build",
    ];
    let publication_denied = Command::new("runuser")
        .args(["-u", "pip-worker", "--", binary])
        .args(publication_args)
        .output()
        .unwrap();
    assert!(!publication_denied.status.success());
    assert!(String::from_utf8_lossy(&publication_denied.stderr).contains("requires root"));
    let review_denied = Command::new("runuser")
        .args(["-u", "pip-worker", "--", binary])
        .args(review_args)
        .output()
        .unwrap();
    assert!(!review_denied.status.success());
    assert!(String::from_utf8_lossy(&review_denied.stderr).contains("requires root"));
    let denied = Command::new("runuser")
        .args(["-u", "pip-worker", "--", binary])
        .args(args)
        .output()
        .unwrap();
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("requires root"));
    fs::write(queue.join("inbox/stale.json"), b"{}").unwrap();
    let publication_stale = Command::new(binary)
        .args(publication_args)
        .output()
        .unwrap();
    assert!(!publication_stale.status.success());
    assert!(String::from_utf8_lossy(&publication_stale.stderr).contains("queue must be drained"));
    let stale = run();
    assert!(!stale.status.success());
    assert!(
        String::from_utf8_lossy(&stale.stderr).contains("queue must be drained"),
        "{}",
        String::from_utf8_lossy(&stale.stderr)
    );
    fs::remove_file(queue.join("inbox/stale.json")).unwrap();
    assert!(
        Command::new("systemctl")
            .args(["enable", "--runtime", "pip-controller@mdk.timer"])
            .status()
            .unwrap()
            .success()
    );
    let active = run();
    let publication_active = Command::new(binary)
        .args(publication_args)
        .output()
        .unwrap();
    assert!(!publication_active.status.success());
    assert!(String::from_utf8_lossy(&publication_active.stderr).contains("execution units"));
    assert!(
        Command::new("systemctl")
            .args(["disable", "--runtime", "pip-controller@mdk.timer"])
            .status()
            .unwrap()
            .success()
    );
    assert!(!active.status.success());
    assert!(String::from_utf8_lossy(&active.stderr).contains("execution units"));
    assert_eq!(store.case(CASE).unwrap().unwrap().state_revision, 2);
    for expected in ["Applied", "Replayed"] {
        let output = run();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["result"], expected);
        assert_eq!(value["runtime_activated"], false);
    }
    assert_eq!(store.status(140).unwrap().direct_attempts_running, 0);
    assert_eq!(store.case(CASE).unwrap().unwrap().state_revision, 3);
}
