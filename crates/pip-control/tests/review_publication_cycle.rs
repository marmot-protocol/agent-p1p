use std::cell::RefCell;

use pip_control::{
    ReviewPublicationCycle, ReviewWriter, load_repository_policy, publish_reviews_once,
};
use pip_github::{GitHubError, MutationResult, ReviewEvent, ReviewMutationSpec};
use pip_store::{EffectInput, EventInput, NewCase, RunInput, Store, TransitionInput};
use serde_json::{Value, json};

#[derive(Default)]
struct FixtureWriter {
    reviews: RefCell<Vec<ReviewMutationSpec>>,
    fail: bool,
}

impl ReviewWriter for FixtureWriter {
    fn ensure_review(&self, spec: &ReviewMutationSpec) -> Result<MutationResult, GitHubError> {
        self.reviews.borrow_mut().push(spec.clone());
        if self.fail {
            Err(GitHubError::Transport("fixture outage".into()))
        } else {
            Ok(MutationResult::Created(spec.expected_actor_id))
        }
    }
}

#[test]
fn publication_preserves_attributed_suggestions_and_verification_limits() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = review_store_with(directory.path().join("ledger.db"), false, |reviews| {
        reviews[0]["suggestions"] = json!([{"summary":"Bound the subscription timeout", "rationale":"Avoid hanging checks"}]);
        reviews[0]["evidence"] =
            json!({"local_checks":["13 tests passed"], "limitations":["Full suite not run"]});
        reviews[1]["evidence"] =
            json!({"limitations":["Shell commands denied; source inspection only"]});
    });
    let general = FixtureWriter::default();
    let secperf = FixtureWriter::default();
    publish_reviews_once(
        &general,
        &secperf,
        &active_policy(),
        &mut store,
        100,
        "publisher",
        30,
        true,
    )
    .unwrap();
    let general = &general.reviews.borrow()[0];
    let secperf = &secperf.reviews.borrow()[0];
    for expected in [
        "general-sol",
        "Bound the subscription timeout",
        "13 tests passed",
        "Full suite not run",
    ] {
        assert!(general.body.contains(expected), "missing {expected}");
    }
    assert!(
        secperf
            .body
            .contains("Shell commands denied; source inspection only")
    );
    assert!(!secperf.body.contains("13 tests passed"));
    assert_eq!(secperf.event, ReviewEvent::Approve);
}

#[test]
fn distinct_role_identities_publish_exact_head_approvals_before_preflight() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = review_store(directory.path().join("ledger.db"), false);
    let general = FixtureWriter::default();
    let secperf = FixtureWriter::default();

    let result = publish_reviews_once(
        &general,
        &secperf,
        &active_policy(),
        &mut store,
        100,
        "review-publisher",
        30,
        true,
    )
    .unwrap();
    assert_eq!(
        result,
        ReviewPublicationCycle::Published {
            case_key: "repo:984321#1240@1".into(),
            general_review_id: 202_881,
            secperf_review_id: 202_882,
        }
    );
    let general = general.reviews.borrow();
    let secperf = secperf.reviews.borrow();
    assert_eq!(general[0].expected_actor_id, 202_881);
    assert_eq!(secperf[0].expected_actor_id, 202_882);
    assert_eq!(general[0].event, ReviewEvent::Approve);
    assert_eq!(secperf[0].event, ReviewEvent::Approve);
    assert!(
        general[0]
            .body
            .contains("Pip reviewer role: reviewer-general")
    );
    assert!(
        secperf[0]
            .body
            .contains("Pip reviewer role: reviewer-secperf")
    );
    assert_eq!(general[0].expected_head_sha, "b".repeat(40));
    assert_eq!(
        store.case("repo:984321#1240@1").unwrap().unwrap().state,
        "FINAL_REVIEW"
    );
    assert!(
        store
            .claim_effect_matching("preflight", 100, 30, &["OBSERVE_FINAL_PREFLIGHT"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn request_changes_are_published_before_the_remediation_builder_is_released() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = review_store(directory.path().join("ledger.db"), true);
    let general = FixtureWriter::default();
    let secperf = FixtureWriter::default();
    publish_reviews_once(
        &general,
        &secperf,
        &active_policy(),
        &mut store,
        100,
        "review-publisher",
        30,
        true,
    )
    .unwrap();
    assert_eq!(
        general.reviews.borrow()[0].event,
        ReviewEvent::RequestChanges
    );
    assert_eq!(secperf.reviews.borrow()[0].event, ReviewEvent::Approve);
    assert!(
        store
            .claim_effect_matching("dispatcher", 100, 30, &["DISPATCH_BUILDER"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn reviewer_credential_outage_releases_the_effect_without_partial_ledger_evidence() {
    for (general_failed, secperf_failed) in [(true, false), (false, true), (true, true)] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = review_store(directory.path().join("ledger.db"), false);
        let general = FixtureWriter {
            fail: general_failed,
            ..FixtureWriter::default()
        };
        let secperf = FixtureWriter {
            fail: secperf_failed,
            ..FixtureWriter::default()
        };
        assert!(
            publish_reviews_once(
                &general,
                &secperf,
                &active_policy(),
                &mut store,
                100,
                "review-publisher",
                30,
                true,
            )
            .is_err()
        );
        // Publishing one accepted lane must not depend on the other App being up.
        assert_eq!(general.reviews.borrow().len(), 1);
        assert_eq!(secperf.reviews.borrow().len(), 1);
        assert!(
            store
                .claim_effect_matching("retry", 100, 30, &["PUBLISH_REVIEWS"])
                .unwrap()
                .is_some()
        );
        assert_eq!(store.evidence_count().unwrap(), 0);
    }
}

#[test]
fn multiple_required_instances_in_one_lane_publish_one_aggregate_lane_verdict() {
    let directory = tempfile::tempdir().unwrap();
    let policy = policy_with_required_opus();
    let mut store = review_store_with_policy(directory.path().join("ledger.db"), &policy);
    let general = FixtureWriter::default();
    let secperf = FixtureWriter::default();

    publish_reviews_once(
        &general,
        &secperf,
        &policy,
        &mut store,
        100,
        "review-publisher",
        30,
        true,
    )
    .unwrap();
    assert_eq!(general.reviews.borrow().len(), 1);
    assert_eq!(secperf.reviews.borrow().len(), 1);
    let published = &secperf.reviews.borrow()[0];
    assert_eq!(published.event, ReviewEvent::Approve);
    assert!(published.body.contains("secperf-kimi"));
    assert!(published.body.contains("secperf-opus"));
    let history = store
        .immutable_history_for_case("repo:984321#1240@1")
        .unwrap();
    let publication = history
        .evidence
        .iter()
        .find(|evidence| evidence.kind == "GITHUB_REVIEW_PUBLICATION")
        .unwrap();
    assert_eq!(
        publication.payload["secperf"]["reviewer_ids"],
        json!(["secperf-kimi", "secperf-opus"])
    );
}

fn review_store(path: std::path::PathBuf, request_changes: bool) -> Store {
    review_store_with(path, request_changes, |_| {})
}

fn review_store_with(
    path: std::path::PathBuf,
    request_changes: bool,
    mutate: impl FnOnce(&mut Vec<Value>),
) -> Store {
    let mut store = Store::open(path).unwrap();
    store
        .create_case(&NewCase {
            case_key: "repo:984321#1240@1".into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: active_policy().revision,
            initial_state: "REVIEWING".into(),
            observed_at: 1,
            event: EventInput {
                event_id: "event-reviewing".into(),
                event_type: "CI_ACCEPTED".into(),
                payload: json!({"head_sha":"b".repeat(40)}),
            },
            effects: Vec::new(),
        })
        .unwrap();
    let mut reviews = fixture_reviews();
    mutate(&mut reviews);
    if request_changes {
        reviews[0]["outcome"] = json!("REQUEST_CHANGES");
        reviews[0]["blocking_findings"] = json!([{
            "id":"GENERAL-R1-001",
            "summary":"unsafe edge",
            "defect":"unchecked edge",
            "consequence":"wrong state",
            "corrective_direction":"validate it",
            "required_evidence":["regression test"]
        }]);
    }
    store
        .apply_transition(
            &review_transition(1, "REVIEWING", &reviews[0], Vec::new()),
            None,
        )
        .unwrap();
    let next_state = if request_changes {
        "REMEDIATING"
    } else {
        "FINAL_REVIEW"
    };
    store
        .apply_transition(
            &review_transition(
                2,
                next_state,
                &reviews[1],
                vec![EffectInput {
                    effect_id: "effect-publish-reviews".into(),
                    effect_type: "PUBLISH_REVIEWS".into(),
                    payload: json!({"case_key":"repo:984321#1240@1"}),
                }],
            ),
            None,
        )
        .unwrap();
    store
}

fn review_transition(
    expected_revision: u64,
    next_state: &str,
    result: &Value,
    effects: Vec<EffectInput>,
) -> TransitionInput {
    TransitionInput {
        case_key: "repo:984321#1240@1".into(),
        expected_revision,
        next_state: next_state.into(),
        remediation_round: u32::from(next_state == "REMEDIATING"),
        plan_version: 1,
        pr_number: Some(77),
        head_sha: Some("b".repeat(40)),
        observed_at: expected_revision + 1,
        event: EventInput {
            event_id: format!("event-review-{expected_revision}"),
            event_type: "REVIEW_RESULT".into(),
            payload: result.clone(),
        },
        run: Some(RunInput {
            run_id: format!("run-review-{expected_revision}"),
            task_id: result["task_id"].as_str().unwrap().into(),
            role: result["role"].as_str().unwrap().into(),
            payload: result.clone(),
        }),
        evidence: Vec::new(),
        findings: Vec::new(),
        effects,
    }
}

fn fixture_reviews() -> Vec<Value> {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap();
    fixture["results"].as_array().unwrap()[2..4].to_vec()
}

fn policy_with_required_opus() -> pip_control::RepositoryPolicy {
    let mut value = serde_json::to_value(active_policy()).unwrap();
    value["roles"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|role| role["reviewer_id"] == "secperf-opus")
        .unwrap()["review_mode"] = json!("required");
    load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn review_store_with_policy(
    path: std::path::PathBuf,
    policy: &pip_control::RepositoryPolicy,
) -> Store {
    let mut store = review_store(path, false);
    let mut opus = fixture_reviews()[1].clone();
    opus["task_id"] = json!("review-secperf-opus-1");
    opus["reviewer_id"] = json!("secperf-opus");
    opus["requested_model"] = json!("cursor/claude-opus-5-thinking-high");
    opus["actual_model"] = json!("cursor/claude-opus-5-thinking-high");
    let current = store.case("repo:984321#1240@1").unwrap().unwrap();
    store
        .apply_transition(
            &review_transition(
                current.state_revision,
                "FINAL_REVIEW",
                &opus,
                vec![EffectInput {
                    effect_id: "effect-publish-reviews-required-opus".into(),
                    effect_type: "PUBLISH_REVIEWS".into(),
                    payload: json!({"case_key":"repo:984321#1240@1"}),
                }],
            ),
            None,
        )
        .unwrap();
    assert_eq!(
        store
            .case("repo:984321#1240@1")
            .unwrap()
            .unwrap()
            .policy_revision,
        policy.revision
    );
    store
}

fn active_policy() -> pip_control::RepositoryPolicy {
    let mut value: Value = serde_json::from_slice(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    value["repository"]["id"] = json!(984321);
    value["workflow_version"] = json!(1);
    value["intake"]["enabled"] = json!(true);
    value["intake"]["paused"] = json!(false);
    value["dispatch_enabled"] = json!(true);
    value["github"]["automation_actor_id"] = json!(202880);
    value["github"]["reviewer_general_actor_id"] = json!(202881);
    value["github"]["reviewer_secperf_actor_id"] = json!(202882);
    load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap()
}
