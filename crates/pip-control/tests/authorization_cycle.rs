use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use pip_control::{
    ActiveAuthorization, IntakeSource, load_repository_policy, reconcile_active_authorization,
    verify_active_authorization,
};
use pip_github::{
    GitHubError, IntakeSnapshot, IssueContentSnapshot, IssueSnapshot, LabelEvent,
    RepositorySnapshot,
};
use pip_store::{EventInput, NewCase, Store};
use serde_json::{Value, json};

#[derive(Default)]
struct FakeSource {
    snapshots: RefCell<BTreeMap<u64, IntakeSnapshot>>,
}

impl IntakeSource for FakeSource {
    fn discover(
        &self,
        _owner: &str,
        _repository: &str,
        _label: &str,
    ) -> Result<Vec<IssueSnapshot>, GitHubError> {
        panic!("active authorization must not use broad discovery")
    }

    fn intake(
        &self,
        _owner: &str,
        _repository: &str,
        issue_number: u64,
    ) -> Result<IntakeSnapshot, GitHubError> {
        self.snapshots
            .borrow()
            .get(&issue_number)
            .cloned()
            .ok_or(GitHubError::InvalidIdentity)
    }
}

#[test]
fn active_cases_require_a_current_trusted_authorization_event() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    let policy = active_policy();
    seed_case(&mut store, 1240, policy.revision);
    let source = FakeSource::default();
    source
        .snapshots
        .borrow_mut()
        .insert(1240, authorized_snapshot(1240));

    assert_eq!(
        verify_active_authorization(&source, &policy, &store).unwrap(),
        ActiveAuthorization::Authorized { case_count: 1 }
    );

    let mut removed = authorized_snapshot(1240);
    removed.label_events.push(LabelEvent {
        id: 12,
        labeled: false,
        actor_id: 202880,
        label: "pip-ok".into(),
        created_at: "2026-08-20T12:01:00Z".into(),
    });
    removed.issue.labels.clear();
    source.snapshots.borrow_mut().insert(1240, removed);
    let ActiveAuthorization::Blocked { cases } =
        verify_active_authorization(&source, &policy, &store).unwrap()
    else {
        panic!("removed authorization must block")
    };
    assert_eq!(cases[0].case_key, "repo:1055628515#1240@2");
    assert_eq!(
        cases[0].blockers,
        ["REQUIRED_LABEL_MISSING", "LATEST_AUTHORIZATION_REMOVED"]
    );
}

#[test]
fn repository_policy_issue_and_actor_drift_fail_closed_for_every_active_case() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    let policy = active_policy();
    seed_case(&mut store, 1240, policy.revision);
    seed_case(&mut store, 1241, policy.revision + 1);
    let source = FakeSource::default();
    let mut first = authorized_snapshot(1240);
    first.repository.id = 99;
    first.issue.open = false;
    let mut second = authorized_snapshot(1241);
    second.label_events[0].actor_id = 999;
    source.snapshots.borrow_mut().insert(1240, first);
    source.snapshots.borrow_mut().insert(1241, second);

    let ActiveAuthorization::Blocked { cases } =
        verify_active_authorization(&source, &policy, &store).unwrap()
    else {
        panic!("drift must block")
    };
    assert_eq!(cases.len(), 2);
    assert_eq!(
        cases[0].blockers,
        ["REPOSITORY_IDENTITY_MISMATCH", "ISSUE_CLOSED"]
    );
    assert_eq!(
        cases[1].blockers,
        ["POLICY_REVISION_MISMATCH", "UNTRUSTED_LABEL_ACTOR"]
    );
}

#[test]
fn removed_authorization_is_committed_and_supersedes_pending_dispatch_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    let policy = active_policy();
    seed_case_with_dispatch(&mut store, 1240, policy.revision);
    let source = FakeSource::default();
    let mut removed = authorized_snapshot(1240);
    removed.issue.labels.clear();
    removed.label_events.push(LabelEvent {
        id: 12,
        labeled: false,
        actor_id: 202880,
        label: "pip-ok".into(),
        created_at: "2026-08-20T12:01:00Z".into(),
    });
    source.snapshots.borrow_mut().insert(1240, removed);

    let ActiveAuthorization::Blocked { cases } =
        reconcile_active_authorization(&source, &policy, &mut store, 100).unwrap()
    else {
        panic!("removed authorization must block")
    };
    assert!(cases[0].revoked);
    assert_eq!(
        store.case(&cases[0].case_key).unwrap().unwrap().state,
        "ABANDONED"
    );
    let status = store.status(101).unwrap();
    assert_eq!(status.outbox_pending, 1);
    assert_eq!(status.outbox_superseded, 1);
    assert_eq!(
        store
            .claim_effect("controller", 101, 30)
            .unwrap()
            .unwrap()
            .effect_type,
        "RECORD_ABANDONMENT"
    );

    assert_eq!(
        reconcile_active_authorization(&source, &policy, &mut store, 102).unwrap(),
        ActiveAuthorization::Authorized { case_count: 0 }
    );
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
    load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn seed_case(store: &mut Store, issue_number: u64, policy_revision: u64) {
    store
        .create_case(&NewCase {
            case_key: format!("repo:1055628515#{issue_number}@2"),
            repository_id: 1_055_628_515,
            issue_number,
            workflow_version: 2,
            policy_revision,
            initial_state: "PLANNING".into(),
            observed_at: 1,
            event: EventInput {
                event_id: format!("event-intake-{issue_number}"),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({"issue_number": issue_number}),
            },
            effects: Vec::new(),
        })
        .unwrap();
}

fn seed_case_with_dispatch(store: &mut Store, issue_number: u64, policy_revision: u64) {
    let mut case = NewCase {
        case_key: format!("repo:1055628515#{issue_number}@2"),
        repository_id: 1_055_628_515,
        issue_number,
        workflow_version: 2,
        policy_revision,
        initial_state: "PLANNING".into(),
        observed_at: 1,
        event: EventInput {
            event_id: format!("event-intake-{issue_number}"),
            event_type: "ISSUE_AUTHORIZED".into(),
            payload: json!({"issue_number": issue_number}),
        },
        effects: Vec::new(),
    };
    case.effects.push(pip_store::EffectInput {
        effect_id: format!("effect-intake-{issue_number}-planner"),
        effect_type: "DISPATCH_PLANNER".into(),
        payload: json!({"case_key": case.case_key}),
    });
    store.create_case(&case).unwrap();
}

fn authorized_snapshot(issue_number: u64) -> IntakeSnapshot {
    IntakeSnapshot {
        repository: RepositorySnapshot {
            id: 1_055_628_515,
            full_name: "marmot-protocol/mdk".into(),
            default_branch: "master".into(),
        },
        issue: IssueSnapshot {
            id: 5_000_000_000 + issue_number,
            number: issue_number,
            open: true,
            is_pull_request: false,
            labels: BTreeSet::from(["pip-ok".into()]),
        },
        issue_content: IssueContentSnapshot {
            author_id: 1001,
            title: format!("Fixture issue #{issue_number}"),
            body: "Fixture body".into(),
            created_at: "2026-08-19T00:00:00Z".into(),
            updated_at: "2026-08-20T00:00:00Z".into(),
        },
        label_events: vec![LabelEvent {
            id: 11,
            labeled: true,
            actor_id: 202880,
            label: "pip-ok".into(),
            created_at: "2026-08-20T12:00:00Z".into(),
        }],
        comments: Vec::new(),
    }
}
