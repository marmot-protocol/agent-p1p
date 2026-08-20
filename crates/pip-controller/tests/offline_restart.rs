use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_controller::{LedgerController, WorkflowCommand};
use pip_core::{
    CaseId, CasePolicy, CaseState, Event, EventId, GitSha, IssueNumber, MergeMode, ObservedAt,
    PlanVersion, PolicyRevision, PullRequestNumber, RepositoryId, StateRevision, WorkflowVersion,
};
use pip_store::{ApplyResult, EffectInput, EventInput, NewCase, PolicyInput, Store};
use serde_json::json;

fn case_id() -> CaseId {
    CaseId::new(
        RepositoryId::new(NonZeroU64::new(984_321).unwrap()),
        IssueNumber::new(NonZeroU64::new(1240).unwrap()),
        WorkflowVersion::new(NonZeroU32::new(1).unwrap()),
    )
}

fn policy() -> CasePolicy {
    CasePolicy {
        revision: PolicyRevision::new(NonZeroU64::new(1).unwrap()),
        merge_mode: MergeMode::Shadow,
        max_remediation_rounds: NonZeroU32::new(3).unwrap(),
    }
}

fn command(
    store: &Store,
    sequence: u64,
    event: Event,
    accepted_plan_version: Option<u32>,
    next_pr: Option<u64>,
    next_head: Option<&str>,
) -> WorkflowCommand {
    let current = store.case(&case_id().to_string()).unwrap().unwrap();
    WorkflowCommand {
        case_id: case_id(),
        event_id: EventId::from_str(&format!("event-{sequence}")).unwrap(),
        observed_at: ObservedAt::new(1_787_000_000 + sequence),
        expected_state: CaseState::from_str(&current.state).unwrap(),
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(current.state_revision).unwrap(),
        ),
        accepted_policy_revision: PolicyRevision::new(
            NonZeroU64::new(current.policy_revision).unwrap(),
        ),
        remediation_round: current.remediation_round,
        plan_version: NonZeroU32::new(current.plan_version).map(PlanVersion::new),
        pr_number: current
            .pr_number
            .and_then(NonZeroU64::new)
            .map(PullRequestNumber::new),
        head_sha: current
            .head_sha
            .as_deref()
            .map(GitSha::from_str)
            .transpose()
            .unwrap(),
        event,
        accepted_plan_version: accepted_plan_version
            .and_then(NonZeroU32::new)
            .map(PlanVersion::new),
        next_pr_number: next_pr
            .and_then(NonZeroU64::new)
            .map(PullRequestNumber::new),
        next_head_sha: next_head.map(GitSha::from_str).transpose().unwrap(),
        event_payload: json!({"sequence": sequence}),
        run: None,
        evidence: Vec::new(),
        findings: Vec::new(),
    }
}

#[test]
fn full_shadow_workflow_survives_restart_and_multiple_remediation_rounds() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ledger.db");
    let mut store = Store::open(&path).unwrap();
    store
        .record_policy(&PolicyInput {
            repository_id: 984_321,
            revision: 1,
            accepted_at: 1_787_000_000,
            payload: json!({"merge_mode": "shadow", "max_remediation_rounds": 3}),
        })
        .unwrap();
    store
        .create_case(&NewCase {
            case_key: case_id().to_string(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "PLANNING".into(),
            observed_at: 1_787_000_000,
            event: EventInput {
                event_id: "event-intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({"label": "pip-ok"}),
            },
            effects: vec![EffectInput {
                effect_id: "effect-intake-planner".into(),
                effect_type: "DISPATCH_PLANNER".into(),
                payload: json!({"case_key": case_id().to_string()}),
            }],
        })
        .unwrap();

    let steps = [
        (Event::Proceed, Some(1), None, None),
        (Event::BuilderDispatched, None, None, None),
        (Event::ReviewReady, None, Some(77), Some("b".repeat(40))),
        (Event::CiAccepted, None, None, None),
        (Event::RequestChanges, None, None, None),
        (Event::ReviewsPublished, None, None, None),
        (Event::ReviewReady, None, None, Some("c".repeat(40))),
        (Event::CiAccepted, None, None, None),
        (Event::RequestChanges, None, None, None),
        (Event::ReviewsPublished, None, None, None),
        (Event::ReviewReady, None, None, Some("d".repeat(40))),
        (Event::CiAccepted, None, None, None),
        (Event::ReviewsApproved, None, None, None),
        (Event::ReviewsPublished, None, None, None),
        (Event::FinalPreflightAccepted, None, None, None),
        (Event::Ready, None, None, None),
    ];
    let mut replay = None;
    for (index, (event, plan, pr, head)) in steps.into_iter().enumerate() {
        let workflow = command(
            &store,
            u64::try_from(index + 1).unwrap(),
            event,
            plan,
            pr,
            head.as_deref(),
        );
        assert_eq!(
            LedgerController::apply(&mut store, &policy(), &workflow).unwrap(),
            ApplyResult::Applied
        );
        if index == 4 {
            replay = Some(workflow.clone());
        }
        drop(store);
        store = Store::open(&path).unwrap();
    }

    let final_case = store.case(&case_id().to_string()).unwrap().unwrap();
    assert_eq!(final_case.state, "SHADOW_READY");
    assert_eq!(final_case.remediation_round, 2);
    assert_eq!(final_case.plan_version, 1);
    assert_eq!(final_case.pr_number, Some(77));
    assert_eq!(final_case.head_sha, Some("d".repeat(40)));
    assert!(
        store
            .projection_matches_history(&case_id().to_string())
            .unwrap()
    );
    assert_eq!(store.event_count().unwrap(), 17);

    assert_eq!(
        LedgerController::apply(&mut store, &policy(), &replay.unwrap()).unwrap(),
        ApplyResult::Replayed
    );
    assert_eq!(store.event_count().unwrap(), 17);
}
