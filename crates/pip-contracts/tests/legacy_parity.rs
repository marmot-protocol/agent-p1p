use pip_contracts::WorkerResult;
use serde_json::{Value, json};

const REPOSITORY_ID: u64 = 1_055_628_515;

#[test]
fn frozen_python_happy_path_maps_to_valid_target_worker_contracts() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../migration/legacy-v1/happy-path.json")).unwrap();
    let results = fixture["results"].as_object().unwrap();
    let mut adapted = Vec::new();
    for name in [
        "planner",
        "builder",
        "general_review",
        "secperf_review",
        "final_review",
    ] {
        let result = adapt(name, &results[name]).unwrap();
        result.validate().unwrap();
        adapted.push(result);
    }

    let expected_head = fixture["join"]["head_sha"].as_str().unwrap();
    for result in &adapted[1..] {
        let value = serde_json::to_value(result).unwrap();
        let observed = value
            .get("head_sha")
            .or_else(|| value.get("reviewed_head_sha"))
            .and_then(Value::as_str)
            .unwrap();
        assert_eq!(observed, expected_head);
    }
    assert_eq!(fixture["expected"]["state"], "SHADOW_READY");
    assert_eq!(fixture["expected"]["merge_performed"], false);
}

#[test]
fn frozen_python_invalid_recipes_still_fail_closed_after_mapping() {
    let happy: Value =
        serde_json::from_str(include_str!("../../../migration/legacy-v1/happy-path.json")).unwrap();
    let recipes: Value = serde_json::from_str(include_str!(
        "../../../migration/legacy-v1/invalid-contracts.json"
    ))
    .unwrap();
    for recipe in recipes["cases"].as_array().unwrap() {
        let source = recipe["source"].as_str().unwrap();
        let mut value = happy["results"][source].clone();
        for field in recipe
            .get("remove")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            value
                .as_object_mut()
                .unwrap()
                .remove(field.as_str().unwrap());
        }
        if let Some(fields) = recipe.get("set").and_then(Value::as_object) {
            value.as_object_mut().unwrap().extend(fields.clone());
        }
        let observed = adapt(source, &value).and_then(|result| {
            result
                .validate()
                .map(|()| result)
                .map_err(|error| error.to_string())
        });
        assert!(observed.is_err(), "{}", recipe["name"]);
    }
}

fn adapt(name: &str, source: &Value) -> Result<WorkerResult, String> {
    let (target_role, legacy_role) = match name {
        "planner" => ("planner", "planner"),
        "builder" => ("builder", "builder-grok"),
        "general_review" => ("reviewer-general", "reviewer-general"),
        "secperf_review" => ("reviewer-secperf", "reviewer-secperf"),
        "final_review" => ("final-reviewer", "final-reviewer"),
        _ => return Err("unknown frozen result".into()),
    };
    if source.get("role").and_then(Value::as_str) != Some(legacy_role) {
        return Err("legacy role is missing or unexpected".into());
    }
    let mut target = common(source, target_role)?;
    let fields = target.as_object_mut().unwrap();
    match name {
        "planner" => {
            copy(source, fields, "outcome", "outcome")?;
            copy(source, fields, "plan_version", "plan_version")?;
            copy(source, fields, "planned_base_sha", "planned_base_sha")?;
            copy(source, fields, "root_cause", "root_cause")?;
            copy(source, fields, "authorized_scope", "authorized_scope")?;
            copy(source, fields, "sensitive_scope", "sensitive_scope")?;
            copy(source, fields, "dependencies", "dependencies")?;
            copy(source, fields, "open_decisions", "open_decisions")?;
            copy(source, fields, "plan_file", "plan_artifact")?;
        }
        "builder" => {
            for field in [
                "outcome",
                "plan_version",
                "build_round",
                "head_sha",
                "local_checks",
                "finding_resolutions",
            ] {
                copy(source, fields, field, field)?;
            }
        }
        "general_review" | "secperf_review" => {
            for field in [
                "outcome",
                "plan_version",
                "review_round",
                "pr_number",
                "reviewed_head_sha",
                "blocking_findings",
                "suggestions",
                "finding_confirmations",
            ] {
                copy(source, fields, field, field)?;
            }
        }
        "final_review" => {
            fields.insert("outcome".into(), json!("READY"));
            for field in [
                "plan_version",
                "final_review_round",
                "pr_number",
                "reviewed_head_sha",
                "residual_uncertainties",
                "decision_rationale",
            ] {
                copy(source, fields, field, field)?;
            }
        }
        _ => unreachable!(),
    }
    serde_json::from_value(target).map_err(|error| error.to_string())
}

fn common(source: &Value, role: &str) -> Result<Value, String> {
    let case_id = required(source, "case_id")?
        .as_str()
        .ok_or("invalid case")?;
    let issue_number = case_id
        .split_once('#')
        .and_then(|(_, number)| number.parse::<u64>().ok())
        .ok_or("invalid case")?;
    let started = required(source, "started_at")?
        .as_str()
        .ok_or("invalid time")?;
    let completed = required(source, "completed_at")?
        .as_str()
        .ok_or("invalid time")?;
    if completed < started {
        return Err("completion precedes start".into());
    }
    Ok(json!({
        "contract_version": 1,
        "workflow_version": 2,
        "case": {
            "repository_id": REPOSITORY_ID,
            "issue_number": issue_number,
            "workflow_version": 2
        },
        "task_id": required(source, "task_id")?,
        "role": role,
        "requested_model": required(source, "requested_model")?,
        "actual_model": required(source, "actual_model")?,
        "skills_repository_commit": required(source, "skills_repository_commit")?,
        "started_at_unix": 1,
        "completed_at_unix": 2,
        "evidence": required(source, "evidence")?
    }))
}

fn copy(
    source: &Value,
    target: &mut serde_json::Map<String, Value>,
    source_name: &str,
    target_name: &str,
) -> Result<(), String> {
    target.insert(target_name.into(), required(source, source_name)?.clone());
    Ok(())
}

fn required<'a>(source: &'a Value, field: &str) -> Result<&'a Value, String> {
    source.get(field).ok_or_else(|| format!("missing {field}"))
}
