use pip_contracts::{ContractError, WorkerBinding, WorkerResult, WorkerRole};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap()
}

#[test]
fn all_target_worker_results_round_trip_and_validate() {
    let fixture = fixture();
    assert_eq!(fixture["fixture_format"], 1);
    for value in fixture["results"].as_array().unwrap() {
        let result: WorkerResult = serde_json::from_value(value.clone()).unwrap();
        result.validate().unwrap();
        assert_eq!(serde_json::to_value(result).unwrap(), *value);
    }
}

#[test]
fn model_substitution_fails_closed() {
    let mut value = fixture()["results"][1].clone();
    value["actual_model"] = json!("cursor/auto");
    let result: WorkerResult = serde_json::from_value(value).unwrap();
    assert_eq!(result.validate(), Err(ContractError::UnexpectedModel));
}

#[test]
fn exact_head_and_ci_claims_must_agree() {
    let mut value = fixture()["results"][1].clone();
    value["ci_head_sha"] = json!("cccccccccccccccccccccccccccccccccccccccc");
    let result: WorkerResult = serde_json::from_value(value).unwrap();
    assert_eq!(result.validate(), Err(ContractError::CiHeadMismatch));
}

#[test]
fn approving_review_cannot_retain_blocking_findings() {
    let mut value = fixture()["results"][2].clone();
    value["blocking_findings"] = json!([{
        "id": "GENERAL-R1-001",
        "summary": "fixture",
        "defect": "fixture defect",
        "consequence": "fixture consequence",
        "corrective_direction": "fix it",
        "required_evidence": ["regression test"]
    }]);
    let result: WorkerResult = serde_json::from_value(value).unwrap();
    assert_eq!(
        result.validate(),
        Err(ContractError::ApprovalHasBlockingFindings)
    );
}

#[test]
fn unknown_fields_are_rejected_by_deserialization() {
    let mut value = fixture()["results"][0].clone();
    value["surprise"] = json!(true);
    assert!(serde_json::from_value::<WorkerResult>(value).is_err());
}

#[test]
fn immutable_worker_binding_covers_case_task_role_plan_model_pr_and_head() {
    let value = fixture()["results"][3].clone();
    let result: WorkerResult = serde_json::from_value(value).unwrap();
    let binding = WorkerBinding {
        case: pip_contracts::CaseIdentity {
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
        },
        task_id: "review-secperf-1".into(),
        role: WorkerRole::ReviewerSecperf,
        requested_model: "cursor/claude-opus-4-8-thinking-high".into(),
        skills_repository_commit: "a".repeat(40),
        plan_version: 1,
        pr_number: Some(77),
        expected_head_sha: Some("b".repeat(40)),
    };
    result.validate_binding(&binding).unwrap();

    for changed in [
        WorkerBinding {
            task_id: "wrong".into(),
            ..binding.clone()
        },
        WorkerBinding {
            requested_model: "cursor/auto".into(),
            ..binding.clone()
        },
        WorkerBinding {
            skills_repository_commit: "c".repeat(40),
            ..binding.clone()
        },
        WorkerBinding {
            pr_number: Some(78),
            ..binding.clone()
        },
        WorkerBinding {
            expected_head_sha: Some("c".repeat(40)),
            ..binding.clone()
        },
    ] {
        assert_eq!(
            result.validate_binding(&changed),
            Err(ContractError::BindingMismatch)
        );
    }
}
