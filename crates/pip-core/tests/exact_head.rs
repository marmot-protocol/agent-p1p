use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_core::{
    BuilderEvidence, CaseId, CiEvidence, ExactHeadObservation, FindingEvidence, FindingId, GitSha,
    IssueNumber, JoinBlocker, PlanVersion, PullRequestNumber, RepositoryId, ReviewEvidence,
    ReviewRole, WorkflowVersion, evaluate_exact_head,
};
use proptest::prelude::*;
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
struct Fixture {
    fixture_format: u32,
    base: Value,
    cases: Vec<Case>,
}

#[derive(Clone, Debug, Deserialize)]
struct Case {
    name: String,
    set: serde_json::Map<String, Value>,
    expected_blockers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Input {
    case: CaseDto,
    pr_number: u64,
    plan_version: u32,
    head_sha: String,
    builder: BuilderDto,
    ci: CiDto,
    general: ReviewDto,
    secperf: ReviewDto,
    findings: Vec<FindingDto>,
    open_blocking_threads: u32,
    pip_owned: bool,
    mergeable: bool,
    authorization_valid: bool,
}

#[derive(Debug, Deserialize)]
struct CaseDto {
    repository_id: u64,
    issue_number: u64,
    workflow_version: u32,
}

#[derive(Debug, Deserialize)]
struct BuilderDto {
    head_sha: String,
    ci_head_sha: String,
    ci_green: bool,
}

#[derive(Debug, Deserialize)]
struct CiDto {
    head_sha: String,
    completed: bool,
    hollow: bool,
    rate_limited: bool,
    required_checks_green: bool,
}

#[derive(Debug, Deserialize)]
struct ReviewDto {
    role: String,
    head_sha: String,
    approved: bool,
    blocking_findings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct FindingDto {
    id: String,
    origin: String,
    reviewed_head_sha: String,
    resolution_head_sha: Option<String>,
    mandatory: bool,
    origin_confirmed: bool,
}

fn set_path(root: &mut Value, path: &str, value: Value) {
    let parts: Vec<_> = path.split('.').collect();
    match parts.as_slice() {
        [field] => root[*field] = value,
        [object, field] => root[*object][*field] = value,
        [array, index, field] => root[*array][index.parse::<usize>().unwrap()][*field] = value,
        _ => panic!("unsupported fixture path: {path}"),
    }
}

fn sha(value: &str) -> GitSha {
    GitSha::from_str(value).unwrap()
}

fn binding(input: &Input, head: GitSha) -> pip_core::HeadBinding {
    pip_core::HeadBinding {
        case_id: CaseId::new(
            RepositoryId::new(NonZeroU64::new(input.case.repository_id).unwrap()),
            IssueNumber::new(NonZeroU64::new(input.case.issue_number).unwrap()),
            WorkflowVersion::new(NonZeroU32::new(input.case.workflow_version).unwrap()),
        ),
        pr_number: PullRequestNumber::new(NonZeroU64::new(input.pr_number).unwrap()),
        plan_version: PlanVersion::new(NonZeroU32::new(input.plan_version).unwrap()),
        head_sha: head,
    }
}

fn evaluate(value: Value) -> Vec<JoinBlocker> {
    let input: Input = serde_json::from_value(value).unwrap();
    let expected = binding(&input, sha(&input.head_sha));
    let review = |dto: &ReviewDto| ReviewEvidence {
        binding: binding(&input, sha(&dto.head_sha)),
        role: ReviewRole::from_str(&dto.role).unwrap(),
        approved: dto.approved,
        blocking_findings: dto
            .blocking_findings
            .iter()
            .map(|id| FindingId::from_str(id).unwrap())
            .collect(),
    };
    let observation = ExactHeadObservation {
        expected,
        builder: BuilderEvidence {
            binding: binding(&input, sha(&input.builder.head_sha)),
            ci_head_sha: sha(&input.builder.ci_head_sha),
            ci_green: input.builder.ci_green,
        },
        ci: CiEvidence {
            head_sha: sha(&input.ci.head_sha),
            completed: input.ci.completed,
            hollow: input.ci.hollow,
            rate_limited: input.ci.rate_limited,
            required_checks_green: input.ci.required_checks_green,
        },
        general_review: review(&input.general),
        secperf_review: review(&input.secperf),
        findings: input
            .findings
            .into_iter()
            .map(|finding| FindingEvidence {
                id: FindingId::from_str(&finding.id).unwrap(),
                origin: ReviewRole::from_str(&finding.origin).unwrap(),
                reviewed_head_sha: sha(&finding.reviewed_head_sha),
                resolution_head_sha: finding.resolution_head_sha.as_deref().map(sha),
                mandatory: finding.mandatory,
                origin_confirmed: finding.origin_confirmed,
            })
            .collect(),
        open_blocking_threads: input.open_blocking_threads,
        pip_owned: input.pip_owned,
        mergeable: input.mergeable,
        authorization_valid: input.authorization_valid,
    };
    evaluate_exact_head(&observation).blockers
}

#[test]
fn language_neutral_exact_head_cases_pass() {
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../../migration/target-v1/exact-head-cases.json"
    ))
    .unwrap();
    assert_eq!(fixture.fixture_format, 1);

    for case in fixture.cases {
        let mut input = fixture.base.clone();
        for (path, value) in case.set {
            set_path(&mut input, &path, value);
        }
        let observed: Vec<_> = evaluate(input)
            .into_iter()
            .map(|blocker| blocker.to_string())
            .collect();
        assert_eq!(observed, case.expected_blockers, "{}", case.name);
    }
}

proptest! {
    #[test]
    fn any_different_current_head_fails_closed(byte in b'c'..=b'f') {
        let fixture: Fixture = serde_json::from_str(include_str!(
            "../../../migration/target-v1/exact-head-cases.json"
        )).unwrap();
        let mut input = fixture.base;
        input["head_sha"] = Value::String(std::iter::repeat_n(char::from(byte), 40).collect());
        prop_assert!(!evaluate(input).is_empty());
    }
}
