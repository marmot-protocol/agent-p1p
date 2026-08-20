use pip_contracts::{ContractError, WorkerResult};
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
