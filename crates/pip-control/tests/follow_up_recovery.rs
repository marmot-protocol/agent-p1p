use pip_control::{PublicationRetryRequest, authorize_follow_up_recovery, load_repository_policy};
use pip_store::{
    ApplyResult, EventInput, EvidenceInput, NewCase, PolicyInput, Store, TransitionInput,
};
use serde_json::json;

#[test]
fn only_proven_false_follow_up_takeover_can_be_recovered_once_without_resetting_history() {
    for fault in [
        "none",
        "queued-peer",
        "real-takeover",
        "not-ready",
        "bounded",
        "missing-receipt",
        "foreign-receipt",
        "changed-head",
        "nonroot",
        "stale",
        "active",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("db")).unwrap();
        let mut paused = load_repository_policy(include_bytes!(
            "../../../config/target/repositories/mdk.json"
        ))
        .unwrap();
        paused.github.automation_actor_id = Some(88);
        let mut active = paused.clone();
        active.intake.enabled = true;
        active.intake.paused = false;
        active.dispatch_enabled = true;
        store
            .record_policy(&PolicyInput {
                repository_id: active.repository.id,
                revision: active.revision,
                accepted_at: 1,
                payload: serde_json::to_value(&active).unwrap(),
            })
            .unwrap();
        let key = format!("repo:{}#9@3", active.repository.id);
        store
            .create_case(&NewCase {
                case_key: key.clone(),
                repository_id: active.repository.id,
                issue_number: 9,
                workflow_version: 3,
                policy_revision: active.revision,
                initial_state: "FINAL_REVIEW".into(),
                observed_at: 1,
                event: EventInput {
                    event_id: "start".into(),
                    event_type: "TEST".into(),
                    payload: json!({}),
                },
                effects: vec![],
            })
            .unwrap();
        for (rev, state, event, payload) in [
            (
                1,
                if fault == "not-ready" {
                    "REVIEWING"
                } else {
                    "SHADOW_READY"
                },
                "READY",
                json!({}),
            ),
            (
                2,
                "PLANNING",
                "HUMAN_FEEDBACK_RECEIVED",
                json!({"bounded":fault=="bounded","message_key":"feedback"}),
            ),
            (
                3,
                "TAKEN_OVER",
                "HUMAN_TOOK_OVER",
                json!({"blockers":if fault=="real-takeover" {vec!["FOREIGN_HEAD_COMMIT"]} else {vec!["PR_LEFT_DRAFT_STATE"]}}),
            ),
        ] {
            store.apply_transition(&TransitionInput {
                case_key:key.clone(),expected_revision:rev,next_state:state.into(),remediation_round:u32::from(rev>1),plan_version:1,pr_number:Some(77),head_sha:Some(if rev==3 && fault=="changed-head" {"c"} else {"b"}.repeat(40)),observed_at:rev+1,
                event:EventInput{event_id:format!("event-{rev}"),event_type:event.into(),payload},run:None,findings:vec![],effects:vec![],
                evidence:if rev==1 && fault!="missing-receipt" {vec![EvidenceInput{evidence_id:"ready-receipt".into(),kind:"GITHUB_DISPOSITION_COMMENT".into(),source:"github-issue-77".into(),payload:json!({"effect_type":"NOTIFY_SHADOW_READY","actor_id":if fault=="foreign-receipt" {99} else {88},"target_number":77,"ready_for_review":{"pull_request_number":77,"head_sha":"b".repeat(40),"mutation":"updated"}})}]} else {vec![]},
            },None).unwrap();
        }
        let before = store.immutable_history_for_case(&key).unwrap();
        let peer = format!("repo:{}#10@3", active.repository.id);
        if fault == "queued-peer" {
            store
                .create_case(&NewCase {
                    case_key: peer.clone(),
                    repository_id: active.repository.id,
                    issue_number: 10,
                    workflow_version: 3,
                    policy_revision: active.revision,
                    initial_state: "READY_TO_BUILD".into(),
                    observed_at: 1,
                    event: EventInput {
                        event_id: "peer-intake".into(),
                        event_type: "ISSUE_AUTHORIZED".into(),
                        payload: json!({}),
                    },
                    effects: vec![pip_store::EffectInput {
                        effect_id: "peer-builder".into(),
                        effect_type: "RUN_DIRECT_WORKER".into(),
                        payload: json!({}),
                    }],
                })
                .unwrap();
            let effect = store.claim_effect("peer-worker", 5, 100).unwrap().unwrap();
            store.begin_direct_attempt(&effect, "peer-task", 5).unwrap();
        }
        let request = PublicationRetryRequest {
            case_key: key.clone(),
            expected_revision: if fault == "stale" { 3 } else { 4 },
            expected_head: "b".repeat(40),
            request_id: "repair".into(),
            reason: "Confirmed false takeover; exact owned PR returned to draft".into(),
        };
        let result = authorize_follow_up_recovery(
            &mut store,
            if fault == "active" { &active } else { &paused },
            &request,
            10,
            if fault == "nonroot" { 1000 } else { 0 },
        );
        if matches!(fault, "none" | "queued-peer") {
            assert_eq!(result.unwrap(), ApplyResult::Applied);
            let case = store.case(&key).unwrap().unwrap();
            assert_eq!(case.state, "PLANNING");
            assert_eq!(case.remediation_round, 1);
            assert_eq!(case.plan_version, 1);
            assert_eq!(case.state_revision, 5);
            assert_eq!(
                authorize_follow_up_recovery(&mut store, &paused, &request, 11, 0).unwrap(),
                ApplyResult::Replayed
            );
            let after = store.immutable_history_for_case(&key).unwrap();
            assert_eq!(
                &after.events[..before.events.len()],
                before.events.as_slice()
            );
            assert_eq!(after.evidence, before.evidence);
            assert_eq!(
                store.status(11).unwrap().outbox_pending,
                if fault == "queued-peer" { 2 } else { 1 }
            );
            if fault == "queued-peer" {
                assert_eq!(store.running_direct_attempts_for_case(&peer).unwrap(), 1);
                assert_eq!(store.case(&peer).unwrap().unwrap().state_revision, 1);
            }
            assert_eq!(
                store
                    .claim_effect("test", 11, 30)
                    .unwrap()
                    .unwrap()
                    .effect_type,
                "DISPATCH_PLANNER"
            );
        } else {
            assert!(result.is_err(), "{fault}");
            assert_eq!(
                store.immutable_history_for_case(&key).unwrap(),
                before,
                "{fault}"
            );
        }
    }
}
