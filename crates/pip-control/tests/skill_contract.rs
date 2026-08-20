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
            skill.contains("docs/worker-result-contracts.md"),
            "{name} does not name the canonical Rust result contract"
        );
        assert!(
            !skill.contains("HUMAN_REVIEW_REQUIRED"),
            "{name} still instructs workers to emit the legacy final outcome"
        );
    }
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
}
