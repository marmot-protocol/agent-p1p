use pip_control::{PolicyError, load_repository_policy};

#[test]
fn target_mdk_policy_is_generic_paused_numeric_and_shadow_only() {
    let bytes = include_bytes!("../../../config/target/repositories/mdk.json");
    let policy = load_repository_policy(bytes).unwrap();
    assert_eq!(policy.policy_format, 2);
    assert_eq!(policy.revision, 3);
    assert_eq!(policy.workflow_version, 3);
    assert_eq!(policy.repository.id, 1_055_628_515);
    assert_eq!(policy.repository.full_name(), "marmot-protocol/mdk");
    assert_eq!(policy.repository.default_branch, "master");
    assert_eq!(policy.checkout, "/var/lib/pip/repositories/mdk");
    assert_eq!(policy.workspace, "/var/lib/pip/worktrees/mdk");
    assert_eq!(policy.artifacts, "/var/lib/pip/artifacts/mdk");
    assert!(policy.workspace_storage.require_distinct_filesystem);
    assert_eq!(policy.workspace_storage.minimum_free_bytes, 536_870_912_000);
    assert_eq!(policy.workspace_storage.terminal_retention_seconds, 86_400);
    assert!(!policy.intake.enabled);
    assert!(policy.intake.paused);
    assert!(!policy.dispatch_enabled);
    assert_eq!(policy.github.automation_actor_id, Some(292_420_120));
    assert_eq!(policy.github.reviewer_general_actor_id, Some(323_997_422));
    assert_eq!(policy.github.reviewer_secperf_actor_id, Some(323_998_100));
    assert_eq!(
        policy.intake.trusted_actor_ids,
        [202880, 258432291, 292420120]
    );
    assert!(policy.merge.is_shadow());
    assert!(!policy.merge.autonomous);
    assert_eq!(policy.merge.method, "squash");
    assert_eq!(policy.max_case_elapsed_seconds, 86_400);
    assert_eq!(policy.max_provider_failures, 3);
    assert_eq!(policy.max_repeated_finding_fingerprint, 2);
    assert_eq!(policy.required_ci_contexts, ["Required CI"]);
    assert_eq!(policy.sensitive_scope_categories.len(), 7);
    assert!(policy.intake.held_issue_numbers.is_empty());
    let workflow = policy.workflow_policy().unwrap();
    assert_eq!(workflow.roles().len(), 6);
    assert_eq!(workflow.required_reviewers().count(), 2);
    assert_eq!(workflow.reviewers().count(), 3);
    assert_eq!(policy.roles[0].reasoning_effort.as_deref(), Some("xhigh"));
    assert_eq!(policy.roles[1].reasoning_effort, None);
    assert_eq!(policy.intake_policy(false).trusted_actor_ids.len(), 3);
    let raw: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    assert!(raw.get("canary_issue").is_none());
}

#[test]
fn phase9_mdk_policy_changes_only_revision_and_activation_controls() {
    let target_bytes = include_bytes!("../../../config/target/repositories/mdk.json");
    let active_bytes = include_bytes!("../../../config/activation/repositories/mdk-phase9.json");
    let target = load_repository_policy(target_bytes).unwrap();
    let active = load_repository_policy(active_bytes).unwrap();

    assert_eq!(target.revision, 3);
    assert_eq!(active.revision, 4);
    assert!(active.intake.enabled);
    assert!(!active.intake.paused);
    assert!(active.dispatch_enabled);
    assert_eq!(active.intake.repository_active_limit, 1);
    assert_eq!(active.intake.global_active_limit, 1);
    assert!(active.merge.is_shadow());
    assert!(!active.merge.autonomous);

    let mut target: serde_json::Value = serde_json::from_slice(target_bytes).unwrap();
    let active: serde_json::Value = serde_json::from_slice(active_bytes).unwrap();
    target["revision"] = active["revision"].clone();
    target["intake"]["enabled"] = active["intake"]["enabled"].clone();
    target["intake"]["paused"] = active["intake"]["paused"].clone();
    target["dispatch_enabled"] = active["dispatch_enabled"].clone();
    assert_eq!(active, target);
    assert!(active.get("canary_issue").is_none());
}

#[test]
fn policy_rejects_unknown_fields_model_fallback_and_shadow_merge_authority() {
    let bytes = include_bytes!("../../../config/target/repositories/mdk.json");
    let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value["intake"]["enabled"] = serde_json::json!(true);
    value["intake"]["paused"] = serde_json::json!(false);
    value["dispatch_enabled"] = serde_json::json!(true);
    assert!(load_repository_policy(&serde_json::to_vec(&value).unwrap()).is_ok());

    value["github"]["reviewer_secperf_actor_id"] =
        value["github"]["reviewer_general_actor_id"].clone();
    assert!(matches!(
        load_repository_policy(&serde_json::to_vec(&value).unwrap()),
        Err(PolicyError::Invalid)
    ));

    let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value["intake"]["enabled"] = serde_json::json!(true);
    value["intake"]["paused"] = serde_json::json!(false);
    value["dispatch_enabled"] = serde_json::json!(true);
    value["github"]["automation_actor_id"] = serde_json::Value::Null;
    assert!(matches!(
        load_repository_policy(&serde_json::to_vec(&value).unwrap()),
        Err(PolicyError::Invalid)
    ));

    let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value["canary_issue"] = serde_json::json!(1240);
    assert!(matches!(
        load_repository_policy(&serde_json::to_vec(&value).unwrap()),
        Err(PolicyError::Malformed(_))
    ));

    let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value["roles"][0]["model"] = serde_json::json!("auto");
    assert!(matches!(
        load_repository_policy(&serde_json::to_vec(&value).unwrap()),
        Err(PolicyError::Invalid)
    ));

    let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value["roles"][0]["reasoning_effort"] = serde_json::Value::Null;
    assert!(matches!(
        load_repository_policy(&serde_json::to_vec(&value).unwrap()),
        Err(PolicyError::Invalid)
    ));

    let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value["roles"][1]["reasoning_effort"] = serde_json::json!("high");
    assert!(matches!(
        load_repository_policy(&serde_json::to_vec(&value).unwrap()),
        Err(PolicyError::Invalid)
    ));

    let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value["merge"]["autonomous"] = serde_json::json!(true);
    assert!(matches!(
        load_repository_policy(&serde_json::to_vec(&value).unwrap()),
        Err(PolicyError::Invalid)
    ));

    let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value["merge"]["method"] = serde_json::json!("auto");
    assert!(matches!(
        load_repository_policy(&serde_json::to_vec(&value).unwrap()),
        Err(PolicyError::Invalid)
    ));

    for field in [
        "max_case_elapsed_seconds",
        "max_provider_failures",
        "max_repeated_finding_fingerprint",
    ] {
        let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        value[field] = serde_json::json!(0);
        assert!(matches!(
            load_repository_policy(&serde_json::to_vec(&value).unwrap()),
            Err(PolicyError::Invalid)
        ));
    }

    for field in ["checkout", "workspace", "artifacts"] {
        let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        value[field] = serde_json::json!("relative/path");
        assert!(matches!(
            load_repository_policy(&serde_json::to_vec(&value).unwrap()),
            Err(PolicyError::Invalid)
        ));
    }

    let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value["checkout"] = value["workspace"].clone();
    assert!(matches!(
        load_repository_policy(&serde_json::to_vec(&value).unwrap()),
        Err(PolicyError::Invalid)
    ));

    for field in ["minimum_free_bytes", "terminal_retention_seconds"] {
        let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        value["workspace_storage"][field] = serde_json::json!(0);
        assert!(matches!(
            load_repository_policy(&serde_json::to_vec(&value).unwrap()),
            Err(PolicyError::Invalid)
        ));
    }
}
