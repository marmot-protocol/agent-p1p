use pip_store::{EffectInput, EventInput, NewCase, RunInput, Store, TransitionInput};
use serde_json::json;

const CASE: &str = "repo:984321#1240@1";
const HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn step(
    store: &mut Store,
    revision: u64,
    event_type: &str,
    next_state: &str,
    run_role: Option<&str>,
) {
    store
        .apply_transition(
            &TransitionInput {
                case_key: CASE.into(),
                expected_revision: revision,
                next_state: next_state.into(),
                remediation_round: 0,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some(HEAD.into()),
                observed_at: 1_787_000_000 + revision,
                event: EventInput {
                    event_id: format!("event-{revision}"),
                    event_type: event_type.into(),
                    payload: json!({}),
                },
                run: run_role.map(|role| RunInput {
                    run_id: format!("run-{revision}"),
                    task_id: format!("task-{revision}"),
                    role: role.into(),
                    payload: json!({"role": role}),
                }),
                evidence: vec![],
                findings: vec![],
                effects: vec![],
            },
            None,
        )
        .unwrap();
}

#[test]
fn each_entry_into_review_starts_a_fresh_cohort_on_the_same_head() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    store
        .create_case(&NewCase {
            case_key: CASE.into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "WAITING_CI".into(),
            observed_at: 1_787_000_000,
            event: EventInput {
                event_id: "event-intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: vec![],
        })
        .unwrap();
    step(&mut store, 1, "CI_ACCEPTED", "REVIEWING", None);
    step(
        &mut store,
        2,
        "REVIEW_RECORDED",
        "REVIEWING",
        Some("reviewer-general"),
    );
    assert_eq!(store.current_review_runs_for_case(CASE).unwrap().len(), 1);
    step(
        &mut store,
        3,
        "REVIEWS_APPROVED",
        "FINAL_REVIEW",
        Some("reviewer-secperf"),
    );
    // The completed cohort stays visible to final review.
    assert_eq!(store.current_review_runs_for_case(CASE).unwrap().len(), 2);

    // Final review sends the same head and round back for review.
    step(&mut store, 4, "RETURN_TO_REVIEW", "REVIEWING", None);
    assert!(store.current_review_runs_for_case(CASE).unwrap().is_empty());
    step(
        &mut store,
        5,
        "REVIEW_RECORDED",
        "REVIEWING",
        Some("reviewer-general"),
    );
    let runs = store.current_review_runs_for_case(CASE).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].run_id, "run-5");
}

#[test]
fn prior_builder_dispatch_marks_the_worktree_as_possibly_holding_work() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    store
        .create_case(&NewCase {
            case_key: CASE.into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "READY_TO_BUILD".into(),
            observed_at: 1_787_000_000,
            event: EventInput {
                event_id: "event-intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: vec![EffectInput {
                effect_id: "dispatch-builder-1".into(),
                effect_type: "DISPATCH_BUILDER".into(),
                payload: json!({}),
            }],
        })
        .unwrap();
    assert!(!store.builder_dispatched_before(CASE, 1).unwrap());
    assert!(store.builder_dispatched_before(CASE, 2).unwrap());
    assert!(store.builder_dispatched_before("", 2).is_err());
}
