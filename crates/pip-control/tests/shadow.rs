use std::cell::Cell;
use std::collections::BTreeSet;

use pip_control::{IntakeSource, load_repository_policy, reconcile_read_only};
use pip_github::{
    GitHubError, IntakeSnapshot, IssueContentSnapshot, IssueSnapshot, LabelEvent,
    RepositorySnapshot,
};

struct FixtureSource {
    writes: Cell<u64>,
}

impl IntakeSource for FixtureSource {
    fn discover(
        &self,
        _owner: &str,
        _repository: &str,
        _label: &str,
    ) -> Result<Vec<IssueSnapshot>, GitHubError> {
        Ok(vec![issue()])
    }

    fn intake(
        &self,
        _owner: &str,
        _repository: &str,
        _issue_number: u64,
    ) -> Result<IntakeSnapshot, GitHubError> {
        Ok(IntakeSnapshot {
            repository: RepositorySnapshot {
                id: 1_055_628_515,
                full_name: "marmot-protocol/mdk".into(),
                default_branch: "master".into(),
            },
            issue: issue(),
            issue_content: IssueContentSnapshot {
                author_id: 1001,
                title: "Fixture issue".into(),
                body: "Fixture body".into(),
                created_at: "2026-08-19T00:00:00Z".into(),
                updated_at: "2026-08-20T00:00:00Z".into(),
            },
            label_events: vec![LabelEvent {
                id: 91,
                labeled: true,
                actor_id: 202_880,
                label: "pip-ok".into(),
                created_at: "2026-08-20T12:00:00Z".into(),
            }],
            comments: Vec::new(),
        })
    }
}

#[test]
fn paused_shadow_reconciliation_is_deterministic_and_never_mutates() {
    let policy = load_repository_policy(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    let source = FixtureSource {
        writes: Cell::new(0),
    };
    let first = reconcile_read_only(&source, &policy, 1_787_220_000, 0, 0).unwrap();
    let second = reconcile_read_only(&source, &policy, 1_787_220_000, 0, 0).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.mutation_count, 0);
    assert_eq!(source.writes.get(), 0);
    assert_eq!(first.candidates[0].decision, "INELIGIBLE");
    assert_eq!(
        first.candidates[0].blockers,
        ["INTAKE_DISABLED", "REPOSITORY_PAUSED", "DISPATCH_DISABLED"]
    );
}

fn issue() -> IssueSnapshot {
    IssueSnapshot {
        id: 555,
        number: 1240,
        open: true,
        is_pull_request: false,
        labels: BTreeSet::from(["pip-ok".into()]),
    }
}
