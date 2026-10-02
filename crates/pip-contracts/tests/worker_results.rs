use pip_contracts::{
    BindingFill, ContractError, ReviewMode, WorkerBinding, WorkerResult, WorkerRole, fill_binding,
};
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
fn a_self_reported_model_name_neither_proves_nor_breaks_a_result() {
    // Providers do not attest which model ran; substitution is enforced by
    // the pinned provider flag and model probe, not by the model's own claim.
    let mut value = fixture()["results"][1].clone();
    value["actual_model"] = json!("cursor/auto");
    let result: WorkerResult = serde_json::from_value(value).unwrap();
    assert_eq!(result.validate(), Ok(()));
}

#[test]
fn review_ready_builder_requires_an_exact_commit_head() {
    let mut value = fixture()["results"][1].clone();
    value["head_sha"] = json!("not-a-sha");
    let result: WorkerResult = serde_json::from_value(value).unwrap();
    assert_eq!(
        result.validate(),
        Err(ContractError::InvalidOutcomeEvidence)
    );
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
fn unknown_fields_are_ignored_rather_than_failing_a_run() {
    let mut value = fixture()["results"][0].clone();
    value["surprise"] = json!(true);
    let result: WorkerResult = serde_json::from_value(value).unwrap();
    assert_eq!(result.validate(), Ok(()));
    assert!(
        serde_json::to_value(&result)
            .unwrap()
            .get("surprise")
            .is_none()
    );
}

#[test]
fn proceeding_plans_may_name_sensitive_scope_but_not_open_decisions() {
    let mut value = fixture()["results"][0].clone();
    value["sensitive_scope"] = json!(["MLS_CGKA", "CRYPTOGRAPHY"]);
    let result: WorkerResult = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(result.validate(), Ok(()));
    value["open_decisions"] = json!(["which group epoch policy applies"]);
    let result: WorkerResult = serde_json::from_value(value).unwrap();
    assert_eq!(
        result.validate(),
        Err(ContractError::InvalidOutcomeEvidence)
    );
}

fn review_binding() -> WorkerBinding {
    WorkerBinding {
        case: pip_contracts::CaseIdentity {
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
        },
        task_id: "review-secperf-1".into(),
        role: WorkerRole::ReviewerSecperf,
        reviewer_id: Some("secperf-kimi".into()),
        review_mode: Some(ReviewMode::Required),
        requested_model: "cursor/kimi-k3-max".into(),
        skills_repository_commit: "a".repeat(40),
        plan_version: 1,
        pr_number: Some(77),
        expected_head_sha: Some("b".repeat(40)),
    }
}

fn task_body() -> Value {
    json!({"review_round": 2, "build_round": 3, "final_review_round": 1})
}

#[test]
fn the_controller_fills_every_field_it_already_knows() {
    let binding = review_binding();
    // A worker returns only its judgement; bookkeeping is absent or wrong.
    let mut value = json!({
        "role": "reviewer-secperf",
        "task_id": "whatever-the-model-guessed",
        "requested_model": "cursor/auto",
        "outcome": "APPROVE",
        "blocking_findings": [],
        "suggestions": [],
        "finding_confirmations": [],
    });
    fill_binding(
        &mut value,
        &BindingFill {
            binding: &binding,
            task_body: &task_body(),
            run_times: Some((1_787_000_000, 1_787_000_100)),
            now: 1_787_000_200,
            reviewed_head_from_binding: true,
        },
    )
    .unwrap();
    let result = WorkerResult::decode(value).unwrap();
    result.validate_binding(&binding).unwrap();
    let WorkerResult::Review(review) = result else {
        panic!("expected a review");
    };
    assert_eq!(review.common.task_id, "review-secperf-1");
    assert_eq!(review.common.requested_model, "cursor/kimi-k3-max");
    assert_eq!(review.common.actual_model, "cursor/kimi-k3-max");
    assert_eq!(review.review_round, 2);
    assert_eq!(review.reviewed_head_sha, "b".repeat(40));
    assert_eq!(review.common.started_at_unix, 1_787_000_000);
    assert_eq!(review.common.completed_at_unix, 1_787_000_100);
}

#[test]
fn a_native_reviewer_must_still_attest_the_head_it_reviewed() {
    let binding = review_binding();
    let fill = |head: Value| {
        let mut value = json!({
            "role": "reviewer-secperf", "outcome": "APPROVE", "reviewed_head_sha": head,
            "blocking_findings": [], "suggestions": [], "finding_confirmations": [],
            "started_at_unix": 1_787_000_000_u64, "completed_at_unix": 1_787_000_050_u64,
        });
        fill_binding(
            &mut value,
            &BindingFill {
                binding: &binding,
                task_body: &task_body(),
                run_times: None,
                now: 1_787_000_100,
                reviewed_head_from_binding: false,
            },
        )
        .unwrap();
        WorkerResult::decode(value).unwrap()
    };
    let attested = fill(json!("b".repeat(40)));
    attested.validate_binding(&binding).unwrap();
    // Valid worker times are kept when the transport does not measure them.
    assert_eq!(attested.common().completed_at_unix, 1_787_000_050);
    assert_eq!(
        fill(json!("c".repeat(40))).validate_binding(&binding),
        Err(ContractError::BindingMismatch)
    );
}

#[test]
fn implausible_worker_times_are_clamped_to_the_observation() {
    let binding = review_binding();
    let mut value = json!({
        "role": "reviewer-secperf", "outcome": "APPROVE", "reviewed_head_sha": "b".repeat(40),
        "blocking_findings": [], "suggestions": [], "finding_confirmations": [],
        "started_at_unix": 9_999_999_999_u64, "completed_at_unix": "soon",
    });
    fill_binding(
        &mut value,
        &BindingFill {
            binding: &binding,
            task_body: &task_body(),
            run_times: None,
            now: 1_787_000_100,
            reviewed_head_from_binding: false,
        },
    )
    .unwrap();
    let result = WorkerResult::decode(value).unwrap();
    assert_eq!(result.common().started_at_unix, 1_787_000_100);
    assert_eq!(result.common().completed_at_unix, 1_787_000_100);
}

#[test]
fn planning_and_building_bind_the_job_not_the_previous_pr_output() {
    for index in [0, 1] {
        let result: WorkerResult =
            serde_json::from_value(fixture()["results"][index].clone()).unwrap();
        let common = result.common();
        let binding = WorkerBinding {
            case: common.case.clone(),
            task_id: common.task_id.clone(),
            role: common.role,
            reviewer_id: None,
            review_mode: None,
            requested_model: common.requested_model.clone(),
            skills_repository_commit: common.skills_repository_commit.clone(),
            plan_version: 1,
            pr_number: Some(77),
            expected_head_sha: Some("c".repeat(40)),
        };
        result.validate_binding(&binding).unwrap();
        let wrong_job = WorkerBinding {
            task_id: "another-job".into(),
            ..binding
        };
        assert_eq!(
            result.validate_binding(&wrong_job),
            Err(ContractError::BindingMismatch)
        );
    }
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
        reviewer_id: Some("secperf-kimi".into()),
        review_mode: Some(ReviewMode::Required),
        requested_model: "cursor/kimi-k3-max".into(),
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
