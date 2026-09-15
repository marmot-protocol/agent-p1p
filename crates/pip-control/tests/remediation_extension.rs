use pip_control::{
    PublicationRetryRequest, RepositoryPolicy, authorize_remediation_extension,
    load_repository_policy,
};
use pip_store::{ApplyResult, EventInput, NewCase, PolicyInput, Store, TransitionInput};
use serde_json::json;

const CASE: &str = "repo:42#9@3";

fn fixture(
    cause: &str,
) -> (
    tempfile::TempDir,
    Store,
    RepositoryPolicy,
    PublicationRetryRequest,
) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("ledger.db")).unwrap();
    let mut policy = load_repository_policy(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    policy.repository.id = 42;
    policy.revision = 8;
    policy.max_remediation_rounds = 3;
    policy.intake.enabled = true;
    policy.intake.paused = false;
    policy.dispatch_enabled = true;
    store
        .record_policy(&PolicyInput {
            repository_id: 42,
            revision: 8,
            accepted_at: 100,
            payload: serde_json::to_value(&policy).unwrap(),
        })
        .unwrap();
    store
        .create_case(&NewCase {
            case_key: CASE.into(),
            repository_id: 42,
            issue_number: 9,
            workflow_version: 3,
            policy_revision: 8,
            initial_state: "PLANNING".into(),
            observed_at: 100,
            event: EventInput {
                event_id: "intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: vec![],
        })
        .unwrap();
    for (revision, state, event) in [(1, "WAITING_CI", "REVIEW_READY"), (2, "ESCALATED", cause)] {
        store
            .apply_transition(
                &TransitionInput {
                    case_key: CASE.into(),
                    expected_revision: revision,
                    next_state: state.into(),
                    remediation_round: 3,
                    plan_version: 1,
                    pr_number: Some(77),
                    head_sha: Some("b".repeat(40)),
                    observed_at: 100 + revision,
                    event: EventInput {
                        event_id: format!("event-{revision}"),
                        event_type: event.into(),
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
    }
    policy.revision = 9;
    policy.max_remediation_rounds = 10;
    policy.intake.enabled = false;
    policy.intake.paused = true;
    policy.dispatch_enabled = false;
    policy.conversations_enabled = true;
    let request = PublicationRetryRequest {
        case_key: CASE.into(),
        expected_revision: 3,
        expected_head: "b".repeat(40),
        request_id: "operator-extend".into(),
        reason: "Allow further CI corrections without resetting history".into(),
    };
    (dir, store, policy, request)
}

#[test]
fn extension_preserves_spent_rounds_old_policy_history_and_deadline_and_replays() {
    let (_dir, mut store, paused, request) = fixture("CI_FAILED");
    let old = store.immutable_history_for_case(CASE).unwrap();
    assert_eq!(
        authorize_remediation_extension(&mut store, &paused, &request, 200, 0).unwrap(),
        ApplyResult::Applied
    );
    let case = store.case(CASE).unwrap().unwrap();
    assert_eq!(
        (
            case.state.as_str(),
            case.policy_revision,
            case.remediation_round,
            case.plan_version
        ),
        ("WAITING_CI", 9, 3, 1)
    );
    assert_eq!(case.pr_number, Some(77));
    assert_eq!(case.head_sha, Some("b".repeat(40)));
    assert_eq!(store.case_authorized_at(CASE).unwrap(), Some(100));
    assert_eq!(store.infrastructure_recovery_deadline(CASE).unwrap(), None);
    assert_eq!(
        store.accepted_policy(42, 8).unwrap()["max_remediation_rounds"],
        3
    );
    assert_eq!(
        store.accepted_policy(42, 9).unwrap()["max_remediation_rounds"],
        10
    );
    let history = store.immutable_history_for_case(CASE).unwrap();
    assert_eq!(&history.events[..old.events.len()], old.events.as_slice());
    assert!(store.projection_matches_history(CASE).unwrap());
    assert_eq!(
        authorize_remediation_extension(&mut store, &paused, &request, 201, 0).unwrap(),
        ApplyResult::Replayed
    );
    assert_eq!(
        store.immutable_history_for_case(CASE).unwrap().events.len(),
        history.events.len()
    );
    let mut changed = paused.clone();
    changed.max_remediation_rounds = 11;
    assert!(authorize_remediation_extension(&mut store, &changed, &request, 202, 0).is_err());
}

#[test]
fn store_rejects_counter_reset_without_recording_the_new_policy() {
    let (_dir, mut store, mut policy, request) = fixture("CI_FAILED");
    policy.intake.enabled = true;
    policy.intake.paused = false;
    policy.dispatch_enabled = true;
    let before = store.immutable_history_for_case(CASE).unwrap();
    let input = TransitionInput {
        case_key: CASE.into(),
        expected_revision: 3,
        next_state: "WAITING_CI".into(),
        remediation_round: 0,
        plan_version: 1,
        pr_number: Some(77),
        head_sha: Some("b".repeat(40)),
        observed_at: 200,
        event: EventInput {
            event_id: request.request_id.clone(),
            event_type: "REMEDIATION_BUDGET_EXTENDED".into(),
            payload: json!({"schema_version":1,"operator_uid":0,"request":request,"policy":policy}),
        },
        run: None,
        evidence: vec![],
        findings: vec![],
        effects: vec![pip_store::EffectInput {
            effect_id: "new-observation".into(),
            effect_type: "OBSERVE_CI".into(),
            payload: json!({"case_key":CASE}),
        }],
    };
    assert!(store.apply_transition(&input, None).is_err());
    assert_eq!(
        store.immutable_history_for_case(CASE).unwrap().events,
        before.events
    );
    assert!(store.accepted_policy(42, 9).is_err());
}

#[test]
fn extension_rejects_unrelated_changes_stale_binding_other_holds_and_nonroot() {
    for fault in [
        "model",
        "time",
        "provider",
        "scope",
        "same-revision",
        "same-limit",
        "active",
        "head",
        "revision",
        "reason",
        "uid",
        "elapsed",
        "other-cause",
    ] {
        let (_dir, mut store, mut paused, mut request) = fixture(if fault == "other-cause" {
            "OPERATIONAL_BOUND_REACHED"
        } else {
            "CI_FAILED"
        });
        let mut uid = 0;
        let mut now = 200;
        match fault {
            "model" => paused.roles[0].model = "other-model".into(),
            "time" => paused.max_case_elapsed_seconds += 1,
            "provider" => paused.max_provider_failures += 1,
            "scope" => paused.intake.trusted_actor_ids.push(123),
            "same-revision" => paused.revision = 8,
            "same-limit" => paused.max_remediation_rounds = 3,
            "active" => paused.dispatch_enabled = true,
            "head" => request.expected_head = "c".repeat(40),
            "revision" => request.expected_revision = 2,
            "reason" => request.reason.clear(),
            "uid" => uid = 1000,
            "elapsed" => now = 100 + paused.max_case_elapsed_seconds,
            _ => (),
        }
        let before = store.immutable_history_for_case(CASE).unwrap();
        assert!(
            authorize_remediation_extension(&mut store, &paused, &request, now, uid).is_err(),
            "{fault}"
        );
        assert_eq!(
            store.immutable_history_for_case(CASE).unwrap().events,
            before.events,
            "{fault}"
        );
        assert_eq!(
            store.case(CASE).unwrap().unwrap().policy_revision,
            8,
            "{fault}"
        );
    }
}
