use pip_control::{
    PublicationRetryRequest, RepositoryPolicy, authorize_infrastructure_recovery,
    enforce_operational_bounds, load_repository_policy,
};
use pip_store::{ApplyResult, EventInput, NewCase, PolicyInput, RunInput, Store, TransitionInput};
use serde_json::json;
const CASE: &str = "repo:42#9@3";

fn fixture() -> (
    tempfile::TempDir,
    Store,
    RepositoryPolicy,
    RepositoryPolicy,
    PublicationRetryRequest,
    u64,
) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("ledger.db")).unwrap();
    let mut paused = load_repository_policy(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    paused.repository.id = 42;
    paused.revision = 7;
    let mut active = paused.clone();
    active.intake.enabled = true;
    active.intake.paused = false;
    active.dispatch_enabled = true;
    store
        .record_policy(&PolicyInput {
            repository_id: 42,
            revision: 7,
            accepted_at: 100,
            payload: serde_json::to_value(&active).unwrap(),
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
            observed_at: 100,
            event: EventInput {
                event_id: "intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: vec![],
        })
        .unwrap();
    for (revision, state, event, role) in [
        (1, "READY_TO_BUILD", "PROCEED", "planner"),
        (2, "REVIEWING", "CI_ACCEPTED", "builder"),
    ] {
        store
            .apply_transition(
                &TransitionInput {
                    case_key: CASE.into(),
                    expected_revision: revision,
                    next_state: state.into(),
                    remediation_round: 0,
                    plan_version: 1,
                    pr_number: if revision == 2 { Some(77) } else { None },
                    head_sha: if revision == 2 {
                        Some("b".repeat(40))
                    } else {
                        None
                    },
                    observed_at: 100 + revision,
                    event: EventInput {
                        event_id: event.into(),
                        event_type: event.into(),
                        payload: json!({}),
                    },
                    run: Some(RunInput {
                        run_id: role.into(),
                        task_id: role.into(),
                        role: role.into(),
                        payload: json!({"plan_version":1,"head_sha":"b".repeat(40)}),
                    }),
                    evidence: vec![],
                    findings: vec![],
                    effects: vec![],
                },
                None,
            )
            .unwrap();
    }
    let now = 101 + active.max_case_elapsed_seconds;
    enforce_operational_bounds(&mut store, &active, now).unwrap();
    let request = PublicationRetryRequest {
        case_key: CASE.into(),
        expected_revision: 4,
        expected_head: "b".repeat(40),
        request_id: "repair-1".into(),
        reason: "Sandbox permission bug repaired; reconcile current target before fresh reviews"
            .into(),
    };
    (dir, store, paused, active, request, now)
}

#[test]
fn infrastructure_recovery_is_bounded_audited_and_preserves_work() {
    let (_dir, mut store, paused, active, request, now) = fixture();
    let before = store.immutable_history_for_case(CASE).unwrap();
    assert_eq!(
        authorize_infrastructure_recovery(&mut store, &paused, &request, now + 10, 0).unwrap(),
        ApplyResult::Applied
    );
    let case = store.case(CASE).unwrap().unwrap();
    assert_eq!(case.state, "REMEDIATING");
    assert_eq!(case.remediation_round, 1);
    assert_eq!(case.policy_revision, 7);
    assert_eq!(case.head_sha, Some(request.expected_head.clone()));
    assert_eq!(case.plan_version, 1);
    assert_eq!(store.case_authorized_at(CASE).unwrap(), Some(100));
    let after = store.immutable_history_for_case(CASE).unwrap();
    assert_eq!(after.runs, before.runs);
    assert_eq!(
        &after.events[..before.events.len()],
        before.events.as_slice()
    );
    assert_eq!(after.events.len(), before.events.len() + 1);
    assert_eq!(
        authorize_infrastructure_recovery(&mut store, &paused, &request, now + 11, 0).unwrap(),
        ApplyResult::Replayed
    );
    enforce_operational_bounds(&mut store, &active, now + 12).unwrap();
    assert_eq!(store.case(CASE).unwrap().unwrap().state, "REMEDIATING");
    enforce_operational_bounds(
        &mut store,
        &active,
        now + 10 + active.max_case_elapsed_seconds,
    )
    .unwrap();
    assert_eq!(store.case(CASE).unwrap().unwrap().state, "ESCALATED");
}

#[test]
fn infrastructure_recovery_rejects_stale_head_revision_nonroot_and_active_policy() {
    let (_dir, mut store, paused, active, request, now) = fixture();
    let before = store.status(now).unwrap();
    assert!(authorize_infrastructure_recovery(&mut store, &paused, &request, now, 1000).is_err());
    assert!(authorize_infrastructure_recovery(&mut store, &active, &request, now, 0).is_err());
    let mut changed = request.clone();
    changed.expected_head = "c".repeat(40);
    assert!(authorize_infrastructure_recovery(&mut store, &paused, &changed, now, 0).is_err());
    changed = request.clone();
    changed.expected_revision = 3;
    assert!(authorize_infrastructure_recovery(&mut store, &paused, &changed, now, 0).is_err());
    assert_eq!(store.status(now).unwrap(), before);
}

#[test]
fn store_rejects_forged_recovery_deadline_and_non_elapsed_holds() {
    for wrong_deadline in [true, false] {
        let (_dir, mut store, _paused, active, request, now) = fixture();
        let mut input = TransitionInput {
            case_key: CASE.into(),
            expected_revision: 4,
            next_state: "REMEDIATING".into(),
            remediation_round: 1,
            plan_version: 1,
            pr_number: Some(77),
            head_sha: Some("b".repeat(40)),
            observed_at: now + 10,
            event: EventInput {
                event_id: request.request_id.clone(),
                event_type: "INFRASTRUCTURE_RECOVERY_AUTHORIZED".into(),
                payload: json!({"schema_version":1,"operator_uid":0,"request":request,"deadline":now+10+active.max_case_elapsed_seconds+u64::from(wrong_deadline)}),
            },
            run: None,
            evidence: vec![],
            findings: vec![],
            effects: vec![pip_store::EffectInput {
                effect_id: "new-builder".into(),
                effect_type: "DISPATCH_BUILDER".into(),
                payload: json!({}),
            }],
        };
        if !wrong_deadline {
            let mut hold = input.clone();
            hold.next_state = "ESCALATED".into();
            hold.remediation_round = 0;
            hold.event = EventInput {
                event_id: "other-hold".into(),
                event_type: "OPERATIONAL_BOUND_REACHED".into(),
                payload: json!({"bound":"PROVIDER_FAILURES"}),
            };
            hold.effects.clear();
            store.apply_transition(&hold, None).unwrap();
            input.expected_revision = 5;
            input.event.payload["request"]["expected_revision"] = json!(5);
        }
        let before = store.status(now + 10).unwrap();
        assert!(store.apply_transition(&input, None).is_err());
        assert_eq!(store.status(now + 10).unwrap(), before);
    }
}
