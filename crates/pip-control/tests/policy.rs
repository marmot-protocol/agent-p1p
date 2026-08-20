use pip_control::{PolicyError, load_repository_policy};

#[test]
fn target_mdk_policy_is_generic_paused_numeric_and_shadow_only() {
    let bytes = include_bytes!("../../../config/target/repositories/mdk.json");
    let policy = load_repository_policy(bytes).unwrap();
    assert_eq!(policy.repository.id, 1_055_628_515);
    assert_eq!(policy.repository.full_name(), "marmot-protocol/mdk");
    assert_eq!(policy.repository.default_branch, "master");
    assert!(!policy.intake.enabled);
    assert!(policy.intake.paused);
    assert!(!policy.dispatch_enabled);
    assert_eq!(policy.github.automation_actor_id, None);
    assert_eq!(
        policy.intake.trusted_actor_ids,
        [202880, 258432291, 292420120]
    );
    assert!(policy.merge.is_shadow());
    assert!(!policy.merge.autonomous);
    assert_eq!(policy.workflow_policy().unwrap().roles().len(), 5);
    assert_eq!(policy.intake_policy(false).trusted_actor_ids.len(), 3);
    let raw: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    assert!(raw.get("canary_issue").is_none());
}

#[test]
fn policy_rejects_unknown_fields_model_fallback_and_shadow_merge_authority() {
    let bytes = include_bytes!("../../../config/target/repositories/mdk.json");
    let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value["intake"]["enabled"] = serde_json::json!(true);
    value["intake"]["paused"] = serde_json::json!(false);
    value["dispatch_enabled"] = serde_json::json!(true);
    assert!(matches!(
        load_repository_policy(&serde_json::to_vec(&value).unwrap()),
        Err(PolicyError::Invalid)
    ));
    value["github"]["automation_actor_id"] = serde_json::json!(202880);
    assert!(load_repository_policy(&serde_json::to_vec(&value).unwrap()).is_ok());

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
    value["merge"]["autonomous"] = serde_json::json!(true);
    assert!(matches!(
        load_repository_policy(&serde_json::to_vec(&value).unwrap()),
        Err(PolicyError::Invalid)
    ));
}
