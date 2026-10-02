use pip_store::{
    EffectInput, EventInput, NewCase, ProjectionRejection, Store, TaskProjectionInput,
};
use serde_json::json;

const CASE: &str = "repo:984321#1240@1";

fn open_with_projection() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    store
        .create_case(&NewCase {
            case_key: CASE.into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "PLANNING".into(),
            observed_at: 100,
            event: EventInput {
                event_id: "event-intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: vec![EffectInput {
                effect_id: "dispatch-planner".into(),
                effect_type: "DISPATCH_PLANNER".into(),
                payload: json!({}),
            }],
        })
        .unwrap();
    let effect = store.claim_effect("projector", 100, 30).unwrap().unwrap();
    store
        .complete_task_projection(
            &TaskProjectionInput {
                projection_id: "plan-key".into(),
                effect_id: effect.effect_id,
                board: "pip-mdk".into(),
                task_id: "t_original".into(),
                desired: json!({
                    "effect_id": "dispatch-planner:worker",
                    "projection_key": "plan-key",
                    "body": {"case_key": CASE, "role": "planner"},
                }),
                observed: json!({"id": "t_original"}),
            },
            "projector",
            101,
            None,
        )
        .unwrap();
    (directory, store)
}

fn unconsumed(store: &Store) -> Vec<String> {
    store
        .unconsumed_task_projections_in(984_321, Some(CASE))
        .unwrap()
        .into_iter()
        .map(|projection| projection.projection_id)
        .collect()
}

#[test]
fn a_rejected_result_is_retried_once_cooled_down_with_the_validation_error() {
    let (_directory, mut store) = open_with_projection();
    assert_eq!(unconsumed(&store), ["plan-key"]);

    let rejection = store
        .reject_task_projection("plan-key", 200, "unknown field `extra`", 3, 120)
        .unwrap();
    assert_eq!(
        rejection,
        ProjectionRejection::Retry {
            projection_id: "plan-key:attempt:2".into(),
            attempt: 2,
        }
    );
    // Replaying the same rejection is idempotent.
    assert_eq!(
        store
            .reject_task_projection("plan-key", 205, "unknown field `extra`", 3, 120)
            .unwrap(),
        rejection
    );
    // The rejected projection is no longer awaited, and the retry is not due yet.
    assert!(unconsumed(&store).is_empty());
    assert!(
        store
            .pending_projection_retries(984_321, Some(CASE), 319)
            .unwrap()
            .is_empty()
    );

    let pending = store
        .pending_projection_retries(984_321, Some(CASE), 320)
        .unwrap();
    assert_eq!(pending.len(), 1);
    let retry = &pending[0];
    assert_eq!(retry.projection_id, "plan-key:attempt:2");
    assert_eq!(retry.effect_id, "dispatch-planner");
    assert_eq!(retry.board, "pip-mdk");
    assert_eq!(retry.desired["projection_key"], "plan-key:attempt:2");
    assert_eq!(
        retry.desired["effect_id"],
        "dispatch-planner:worker:attempt:2"
    );
    assert_eq!(retry.desired["body"]["role"], "planner");
    assert_eq!(
        retry.desired["body"]["previous_result_error"],
        "unknown field `extra`"
    );

    store
        .record_projection_task(
            "plan-key:attempt:2",
            "t_retry",
            &json!({"id": "t_retry"}),
            321,
        )
        .unwrap();
    assert!(
        store
            .pending_projection_retries(984_321, Some(CASE), 400)
            .unwrap()
            .is_empty()
    );
    let awaited = store
        .unconsumed_task_projections_in(984_321, Some(CASE))
        .unwrap();
    assert_eq!(awaited.len(), 1);
    assert_eq!(awaited[0].task_id, "t_retry");
    assert_eq!(awaited[0].desired, retry.desired);
}

#[test]
fn retries_stop_when_the_stage_attempt_budget_is_spent() {
    let (_directory, mut store) = open_with_projection();
    store
        .reject_task_projection("plan-key", 200, "bad", 2, 0)
        .unwrap();
    store
        .record_projection_task("plan-key:attempt:2", "t_retry", &json!({}), 201)
        .unwrap();
    assert_eq!(
        store
            .reject_task_projection("plan-key:attempt:2", 300, "bad again", 2, 0)
            .unwrap(),
        ProjectionRejection::Exhausted { attempts: 2 }
    );
    assert!(unconsumed(&store).is_empty());
    assert!(
        store
            .pending_projection_retries(984_321, Some(CASE), 400)
            .unwrap()
            .is_empty()
    );
}
