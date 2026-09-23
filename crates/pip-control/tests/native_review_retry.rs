use pip_control::{PublicationRetryRequest, authorize_native_review_retry, load_repository_policy};
use pip_store::{
    ApplyResult, EffectInput, EventInput, NewCase, PolicyInput, RunInput, Store,
    TaskProjectionInput, TransitionInput,
};
use serde_json::json;

#[test]
fn native_retry_is_exact_once_preserves_history_and_all_budgets() {
    for variant in [
        "valid", "source", "bound", "profile", "task", "head", "revision", "root", "active",
        "deadline", "reason", "retained",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("ledger.db")).unwrap();
        let mut paused = load_repository_policy(include_bytes!(
            "../../../config/target/repositories/mdk.json"
        ))
        .unwrap();
        paused.repository.id = 42;
        let mut active = paused.clone();
        active.intake.enabled = true;
        active.intake.paused = false;
        active.dispatch_enabled = true;
        let case = "repo:42#9@3";
        let head = "b".repeat(40);
        store
            .record_policy(&PolicyInput {
                repository_id: 42,
                revision: active.revision,
                accepted_at: 100,
                payload: serde_json::to_value(&active).unwrap(),
            })
            .unwrap();
        store
            .create_case(&NewCase {
                case_key: case.into(),
                repository_id: 42,
                issue_number: 9,
                workflow_version: 3,
                policy_revision: active.revision,
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
                        case_key: case.into(),
                        expected_revision: revision,
                        next_state: state.into(),
                        remediation_round: 10,
                        plan_version: 1,
                        pr_number: Some(77),
                        head_sha: Some(head.clone()),
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
                            payload: json!({}),
                        }),
                        evidence: vec![],
                        findings: vec![],
                        effects: if revision == 2 {
                            vec![EffectInput {
                                effect_id: "dispatch".into(),
                                effect_type: "DISPATCH_REVIEWERS".into(),
                                payload: json!({}),
                            }]
                        } else {
                            vec![]
                        },
                    },
                    None,
                )
                .unwrap();
        }
        let effect = store.claim_effect("dispatcher", 103, 30).unwrap().unwrap();
        let desired = json!({"assignee":"reviewer-general","provider":"openai-codex","model":"gpt-6-sol","max_retries":1,"body":{"role":"reviewer-general","review_mode":"required","state_revision":3,"expected_head_sha":head,"plan_version":1}});
        store
            .freeze_dispatch_intents(
                &effect,
                &[pip_store::DispatchIntent {
                    intent_id: "projection".into(),
                    transport: pip_store::DispatchTransport::Hermes,
                    desired: desired.clone(),
                }],
                103,
            )
            .unwrap();
        store
            .complete_task_projections(
                &[TaskProjectionInput {
                    projection_id: "projection".into(),
                    effect_id: "dispatch".into(),
                    board: "board".into(),
                    task_id: "task".into(),
                    desired,
                    observed: json!({}),
                }],
                "dispatcher",
                104,
                None,
            )
            .unwrap();
        if variant == "retained" {
            store
                .retain_task_result("task", &json!({"completed":true}), 104)
                .unwrap();
        }
        store.apply_transition(&TransitionInput{case_key:case.into(),expected_revision:3,next_state:"ESCALATED".into(),remediation_round:10,plan_version:1,pr_number:Some(77),head_sha:Some(head.clone()),observed_at:105,event:EventInput{event_id:"stopped".into(),event_type:"OPERATIONAL_BOUND_REACHED".into(),payload:json!({"bound":if variant=="bound"{"ELAPSED_TIME"}else{"PROVIDER_FAILURES"},"observed":1,"limit":1,"details":{"source":if variant=="source"{"direct-worker"}else{"hermes-circuit-breaker"},"task_id":if variant=="task"{"foreign"}else{"task"},"profile":if variant=="profile"{"planner"}else{"reviewer-general"},"provider":"openai-codex","model":"gpt-6-sol"}})},run:None,evidence:vec![],findings:vec![],effects:vec![]},None).unwrap();
        let mut request = PublicationRetryRequest {
            case_key: case.into(),
            expected_revision: 4,
            expected_head: head.clone(),
            request_id: "native-retry".into(),
            reason: "Repaired the proven session handoff collision".into(),
        };
        let mut now = 106;
        let mut uid = 0;
        match variant {
            "head" => request.expected_head = "c".repeat(40),
            "revision" => request.expected_revision = 3,
            "root" => uid = 1000,
            "active" => paused.dispatch_enabled = true,
            "deadline" => now = 100 + active.max_case_elapsed_seconds,
            "reason" => request.reason.clear(),
            _ => {}
        }
        let before = store.immutable_history_for_case(case).unwrap();
        let status = store.status(now).unwrap();
        let result = authorize_native_review_retry(&mut store, &paused, &request, now, uid);
        if variant != "valid" {
            assert!(result.is_err(), "accepted {variant}");
            assert_eq!(store.status(now).unwrap(), status);
            assert_eq!(store.immutable_history_for_case(case).unwrap(), before);
            continue;
        }
        assert_eq!(result.unwrap(), ApplyResult::Applied);
        let current = store.case(case).unwrap().unwrap();
        assert_eq!(current.state, "WAITING_CI");
        assert_eq!(current.remediation_round, 10);
        assert_eq!(current.head_sha, Some(head));
        assert_eq!(current.plan_version, 1);
        assert_eq!(store.effective_provider_failure_limit(case, 3).unwrap(), 3);
        let after = store.immutable_history_for_case(case).unwrap();
        assert_eq!(
            &after.events[..before.events.len()],
            before.events.as_slice()
        );
        assert_eq!(after.runs, before.runs);
        assert_eq!(
            authorize_native_review_retry(&mut store, &paused, &request, 107, 0).unwrap(),
            ApplyResult::Replayed
        );
        request.reason.push_str(" different");
        assert!(authorize_native_review_retry(&mut store, &paused, &request, 107, 0).is_err());
        let effect = store.claim_effect("controller", 108, 30).unwrap().unwrap();
        assert_eq!(effect.effect_type, "OBSERVE_CI");
        let mut stacked = request.clone();
        stacked.request_id = "another-grant".into();
        stacked.expected_revision = current.state_revision;
        assert!(authorize_native_review_retry(&mut store, &paused, &stacked, 109, 0).is_err());
    }
}
