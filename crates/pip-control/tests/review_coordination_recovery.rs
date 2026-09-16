use pip_control::{
    PublicationRetryRequest, authorize_review_coordination_recovery, load_repository_policy,
};
use pip_store::{
    ApplyResult, EffectInput, EventInput, NewCase, PolicyInput, RunInput, Store, TransitionInput,
};
use serde_json::json;

#[test]
fn recovery_only_requeues_proven_coordination_stalls_without_resetting_budgets() {
    for cause in ["lost-peer", "feedback"] {
        for variant in [
            "valid",
            "head",
            "revision",
            "root",
            "active",
            "deadline",
            "reason",
            "wrong-event",
            "running",
            "repeat",
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
            store
                .record_policy(&PolicyInput {
                    repository_id: 42,
                    revision: active.revision,
                    accepted_at: 100,
                    payload: serde_json::to_value(&active).unwrap(),
                })
                .unwrap();
            let case = "repo:42#9@3";
            let head = "b".repeat(40);
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
            let review = json!({"body":{"role":"reviewer-secperf","review_mode":"required","expected_head_sha":head,"plan_version":1,"remediation_round":2,"pr_number":77}});
            let mut transition = TransitionInput {
                case_key: case.into(),
                expected_revision: 1,
                next_state: "REVIEWING".into(),
                remediation_round: 2,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some(head.clone()),
                observed_at: 101,
                event: EventInput {
                    event_id: "reviewing".into(),
                    event_type: "CI_ACCEPTED".into(),
                    payload: json!({}),
                },
                run: Some(RunInput {
                    run_id: "builder".into(),
                    task_id: "builder".into(),
                    role: "builder".into(),
                    payload: json!({}),
                }),
                evidence: vec![],
                findings: vec![],
                effects: vec![EffectInput {
                    effect_id: "required-review".into(),
                    effect_type: "RUN_DIRECT_WORKER".into(),
                    payload: review,
                }],
            };
            store.apply_transition(&transition, None).unwrap();
            if variant == "running" {
                let claimed = store.claim_effect("worker", 102, 300).unwrap().unwrap();
                store.begin_direct_attempt(&claimed, "review", 102).unwrap();
            }
            transition.expected_revision = 2;
            transition.observed_at = 103;
            transition.run = Some(RunInput {
                run_id: "old-general".into(),
                task_id: "old-general".into(),
                role: "reviewer-general".into(),
                payload: json!({}),
            });
            transition.effects.clear();
            transition.event = EventInput {
                event_id: "stopped".into(),
                event_type: if variant == "wrong-event" {
                    "FIXTURE"
                } else if cause == "lost-peer" {
                    "REVIEW_RECORDED"
                } else {
                    "OPERATIONAL_BOUND_REACHED"
                }
                .into(),
                payload: json!({"reason":"REVIEW_FEEDBACK_ALREADY_ATTEMPTED","head_sha":head,"pull_request_number":77,"blockers":["UNRESOLVED_REVIEW_THREAD:thread"]}),
            };
            if cause == "feedback" {
                // The escalation must have come from final review, not a provider failure.
                transition.next_state = "FINAL_REVIEW".into();
                let mut final_transition = transition.clone();
                final_transition.event = EventInput {
                    event_id: "final".into(),
                    event_type: "BOTH_APPROVED".into(),
                    payload: json!({}),
                };
                store.apply_transition(&final_transition, None).unwrap();
                transition.run = None;
                transition.expected_revision = 3;
                transition.next_state = "ESCALATED".into();
            }
            store.apply_transition(&transition, None).unwrap();
            if cause == "lost-peer" {
                // Reproduce the pre-fix projection, retaining its real superseding event.
                rusqlite::Connection::open(store.path()).unwrap().execute("UPDATE outbox SET superseded_at=103,superseded_by_event_id='stopped',lease_owner=NULL,lease_until=NULL WHERE effect_id='required-review'",[]).unwrap();
            }
            let current = store.case(case).unwrap().unwrap();
            let mut request = PublicationRetryRequest {
                case_key: case.into(),
                expected_revision: current.state_revision,
                expected_head: head,
                request_id: "recover-review".into(),
                reason: "Repaired the coordinator and verified the unchanged authorized draft PR"
                    .into(),
            };
            let mut now = 104;
            let mut uid = 0;
            match variant {
                "head" => request.expected_head = "c".repeat(40),
                "revision" => request.expected_revision -= 1,
                "root" => uid = 1000,
                "active" => paused.dispatch_enabled = true,
                "deadline" => now = 100 + active.max_case_elapsed_seconds,
                "reason" => request.reason.clear(),
                _ => (),
            }
            let before = store.immutable_history_for_case(case).unwrap();
            let result =
                authorize_review_coordination_recovery(&mut store, &paused, &request, now, uid);
            if !matches!(variant, "valid" | "repeat") {
                assert!(result.is_err(), "{cause}/{variant}: {result:?}");
                assert_eq!(store.immutable_history_for_case(case).unwrap(), before);
                continue;
            }
            assert_eq!(result.unwrap(), ApplyResult::Applied);
            assert_eq!(
                authorize_review_coordination_recovery(&mut store, &paused, &request, 105, 0)
                    .unwrap(),
                ApplyResult::Replayed
            );
            let after = store.immutable_history_for_case(case).unwrap();
            assert_eq!(
                &after.events[..before.events.len()],
                before.events.as_slice()
            );
            assert_eq!(after.runs, before.runs);
            assert_eq!(before.runs.len(), 2);
            assert!(store.current_review_runs_for_case(case).unwrap().is_empty());
            let recovered = store.case(case).unwrap().unwrap();
            assert_eq!(recovered.state, "WAITING_CI");
            assert_eq!(recovered.remediation_round, 2);
            assert_eq!(recovered.head_sha, current.head_sha);
            assert_eq!(recovered.plan_version, current.plan_version);
            let effect = store.claim_effect("controller", 106, 30).unwrap().unwrap();
            assert_eq!(effect.effect_type, "OBSERVE_CI");
            request.request_id = "second-recovery".into();
            request.expected_revision = recovered.state_revision;
            assert!(
                authorize_review_coordination_recovery(&mut store, &paused, &request, 107, 0)
                    .is_err()
            );
            if variant == "valid" {
                transition.expected_revision = recovered.state_revision;
                transition.next_state = "REVIEWING".into();
                transition.observed_at = 108;
                transition.run = None;
                transition.event = EventInput {
                    event_id: "fresh-ci".into(),
                    event_type: "CI_ACCEPTED".into(),
                    payload: json!({}),
                };
                store.apply_transition(&transition, None).unwrap();
                let fixture: serde_json::Value = serde_json::from_str(include_str!(
                    "../../../migration/target-v1/worker-results.json"
                ))
                .unwrap();
                for (index, expected_state) in [(2, "REVIEWING"), (3, "FINAL_REVIEW")] {
                    let mut value = fixture["results"][index].clone();
                    value["case"] =
                        json!({"repository_id":42,"issue_number":9,"workflow_version":3});
                    value["workflow_version"] = json!(3);
                    value["task_id"] = json!(format!("fresh-review-{index}"));
                    value["review_round"] = json!(3);
                    value["reviewed_head_sha"] = json!(recovered.head_sha);
                    let result: pip_contracts::WorkerResult =
                        serde_json::from_value(value).unwrap();
                    let pip_contracts::WorkerResult::Review(review) = &result else {
                        unreachable!()
                    };
                    let common = &review.common;
                    let binding = pip_contracts::WorkerBinding {
                        case: common.case.clone(),
                        task_id: common.task_id.clone(),
                        role: common.role,
                        reviewer_id: Some(review.reviewer_id.clone()),
                        review_mode: Some(pip_contracts::ReviewMode::Required),
                        requested_model: common.requested_model.clone(),
                        skills_repository_commit: common.skills_repository_commit.clone(),
                        plan_version: review.plan_version,
                        pr_number: Some(review.pr_number),
                        expected_head_sha: Some(review.reviewed_head_sha.clone()),
                    };
                    pip_controller::ingest_worker_result(
                        &mut store,
                        &active.case_policy(),
                        &binding,
                        &result,
                    )
                    .unwrap();
                    assert_eq!(store.case(case).unwrap().unwrap().state, expected_state);
                }
                assert_eq!(store.current_review_runs_for_case(case).unwrap().len(), 2);
                assert_eq!(store.runs_for_case(case).unwrap().len(), 4);
            }
        }
    }
}
