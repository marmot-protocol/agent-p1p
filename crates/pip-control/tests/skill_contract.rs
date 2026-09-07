const ROLE_SKILLS: [(&str, &str); 6] = [
    (
        "workflow-contract",
        include_str!("../../../skills/shared/workflow-contract/SKILL.md"),
    ),
    ("planner", include_str!("../../../skills/planner/SKILL.md")),
    (
        "builder-grok",
        include_str!("../../../skills/builder-grok/SKILL.md"),
    ),
    (
        "reviewer-general",
        include_str!("../../../skills/reviewer-general/SKILL.md"),
    ),
    (
        "reviewer-secperf",
        include_str!("../../../skills/reviewer-secperf/SKILL.md"),
    ),
    (
        "final-reviewer",
        include_str!("../../../skills/final-reviewer/SKILL.md"),
    ),
];

#[test]
fn canonical_skills_name_the_rust_worker_contract_and_reject_legacy_final_outcome() {
    for (name, skill) in ROLE_SKILLS {
        assert!(
            skill.contains("references/worker-result-contracts.md"),
            "{name} does not name the canonical Rust result contract"
        );
        assert!(
            !skill.contains("HUMAN_REVIEW_REQUIRED"),
            "{name} still instructs workers to emit the legacy final outcome"
        );
    }
}

#[test]
fn worker_field_guide_is_a_packaged_shared_skill_resource() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let guide = std::fs::read_to_string(
        root.join("skills/shared/workflow-contract/references/worker-result-contracts.md"),
    )
    .expect("field guide must travel with the shared skill");
    for required in [
        "## Common fields",
        "## Planner",
        "## Builder",
        "## Reviewers",
        "## Final reviewer",
        "kanban_complete",
    ] {
        assert!(guide.contains(required), "missing {required}");
    }
    let release = std::fs::read_to_string(root.join("scripts/build-rust-release.sh")).unwrap();
    assert!(release.contains("cp -R skills"));
    assert!(
        release
            .contains("cp skills/shared/workflow-contract/references/worker-result-contracts.md")
    );
}

#[test]
fn shared_skill_requires_every_immutable_common_binding() {
    let shared = ROLE_SKILLS[0].1;
    for field in [
        "contract_version",
        "workflow_version",
        "repository_id",
        "issue_number",
        "task_id",
        "role",
        "requested_model",
        "actual_model",
        "skills_repository_commit",
        "started_at_unix",
        "completed_at_unix",
        "evidence",
    ] {
        assert!(
            shared.contains(&format!("`{field}`")),
            "shared skill omits required common field {field}"
        );
    }
    for field in [
        "immutable_evidence_bundle",
        "bound_state_revision",
        "sha256",
    ] {
        assert!(
            shared.contains(&format!("`{field}`")),
            "shared skill omits immutable task evidence field {field}"
        );
    }
}

#[test]
fn final_reviewer_requires_the_controller_preflight_from_the_bound_bundle() {
    let final_reviewer = ROLE_SKILLS
        .iter()
        .find_map(|(name, skill)| (*name == "final-reviewer").then_some(*skill))
        .unwrap();
    assert!(final_reviewer.contains("`immutable_evidence_bundle`"));
    assert!(final_reviewer.contains("`GITHUB_FINAL_PREFLIGHT`"));
}

#[test]
fn builder_uses_the_assigned_worktree_and_never_pushes_directly() {
    let builder = ROLE_SKILLS
        .iter()
        .find_map(|(name, skill)| (*name == "builder-grok").then_some(*skill))
        .unwrap();
    assert!(builder.contains("`assigned_worktree`"));
    assert!(builder.contains("`assigned_branch`"));
    assert!(builder.contains("Do not push"));
    assert!(builder.contains("controller publishes the branch"));
}

#[test]
fn reviewers_leave_hidden_publication_metadata_to_the_controller() {
    for name in ["reviewer-general", "reviewer-secperf"] {
        let skill = ROLE_SKILLS
            .iter()
            .find_map(|(candidate, skill)| (*candidate == name).then_some(*skill))
            .unwrap();
        assert!(!skill.contains("Pip reviewer role:"));
        assert!(skill.contains("controller derives"));
        assert!(skill.contains("hidden"));
    }
}
