//! Stable serialized contracts at the worker/control-plane boundary.

#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CONTRACT_VERSION: u32 = 2;

/// Full case history travels as a digest-bound file, not inline model context.
/// Keep transport limits distinct from prompt and provider-output limits.
pub const MAX_EVIDENCE_BUNDLE_BYTES: usize = 8 * 1024 * 1024;
/// Pretty JSON can expand the compact bundle; still bound the on-disk input.
pub const MAX_EVIDENCE_ARTIFACT_BYTES: usize = 32 * 1024 * 1024;
/// Leave room for the frozen task, policy, and binding around its evidence.
pub const MAX_WORK_ENVELOPE_BYTES: usize = 12 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContractError {
    UnsupportedContractVersion,
    InvalidIdentity,
    WorkflowVersionMismatch,
    RoleMismatch,
    UnexpectedModel,
    InvalidCommit,
    InvalidDigest,
    InvalidTimestampOrder,
    EmptyRequiredField,
    InvalidOutcomeEvidence,
    CiHeadMismatch,
    ApprovalHasBlockingFindings,
    ApprovalHasOpenConfirmation,
    BindingMismatch,
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedContractVersion => "unsupported contract version",
            Self::InvalidIdentity => "invalid case or run identity",
            Self::WorkflowVersionMismatch => "case workflow version mismatch",
            Self::RoleMismatch => "payload fields do not match the declared role",
            Self::UnexpectedModel => "actual model does not match requested model",
            Self::InvalidCommit => "invalid lowercase Git commit SHA",
            Self::InvalidDigest => "invalid lowercase SHA-256 digest",
            Self::InvalidTimestampOrder => "completion precedes start time",
            Self::EmptyRequiredField => "required string or collection is empty",
            Self::InvalidOutcomeEvidence => "outcome does not have its required evidence",
            Self::CiHeadMismatch => "CI head does not match the reported PR head",
            Self::ApprovalHasBlockingFindings => "approval retains blocking findings",
            Self::ApprovalHasOpenConfirmation => "approval retains an open finding confirmation",
            Self::BindingMismatch => "worker result does not match its immutable task binding",
        })
    }
}

impl std::error::Error for ContractError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkerRole {
    Planner,
    Builder,
    ReviewerGeneral,
    ReviewerSecperf,
    FinalReviewer,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewMode {
    Required,
    Advisory,
    Shadow,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaseIdentity {
    pub repository_id: u64,
    pub issue_number: u64,
    pub workflow_version: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerBinding {
    pub case: CaseIdentity,
    pub task_id: String,
    pub role: WorkerRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_mode: Option<ReviewMode>,
    pub requested_model: String,
    pub skills_repository_commit: String,
    pub plan_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_head_sha: Option<String>,
}

/// Unknown fields are ignored: a stray key from a model must not discard its work.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CommonResult {
    pub contract_version: u32,
    pub workflow_version: u32,
    pub case: CaseIdentity,
    pub task_id: String,
    pub role: WorkerRole,
    pub requested_model: String,
    pub actual_model: String,
    pub skills_repository_commit: String,
    pub started_at_unix: u64,
    pub completed_at_unix: u64,
    pub evidence: Map<String, Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PlannerOutcome {
    Proceed,
    AlreadyFixed,
    NotReproducible,
    Duplicate,
    RootCauseDifferentScope,
    CrossRepoDependency,
    WaitingForIssueCreator,
    NeedsHumanScopeDecision,
    Abandon,
    Blocked,
    BlockedUnexpectedModel,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SensitiveScope {
    Cryptography,
    MlsCgka,
    KeyHandling,
    TrustAnchor,
    MembershipAuthorization,
    AdminAuthorization,
    PushPayloadContext,
}

/// Unknown fields are ignored: a stray key from a model must not discard its work.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PlannerResult {
    #[serde(flatten)]
    pub common: CommonResult,
    pub outcome: PlannerOutcome,
    pub plan_version: u32,
    pub planned_base_sha: String,
    pub root_cause: String,
    pub authorized_scope: String,
    pub sensitive_scope: Vec<SensitiveScope>,
    pub dependencies: Vec<Value>,
    pub open_decisions: Vec<String>,
    pub plan_artifact: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BuilderOutcome {
    ReviewReady,
    ReturnToPlanning,
    Blocked,
    Abandon,
    BlockedUnexpectedModel,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FindingResolution {
    pub finding_id: String,
    pub resolution_commit: String,
    pub resolved_head_sha: String,
    pub resolution_summary: String,
    pub tests: Vec<String>,
}

/// Unknown fields are ignored: a stray key from a model must not discard its work.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BuilderResult {
    #[serde(flatten)]
    pub common: CommonResult,
    pub outcome: BuilderOutcome,
    pub plan_version: u32,
    pub build_round: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    pub local_checks: Vec<String>,
    pub finding_resolutions: Vec<FindingResolution>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewOutcome {
    Approve,
    RequestChanges,
    Blocked,
    BlockedUnexpectedModel,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlockingFinding {
    pub id: String,
    pub summary: String,
    pub defect: String,
    pub consequence: String,
    pub corrective_direction: String,
    pub required_evidence: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Suggestion {
    pub summary: String,
    pub rationale: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConfirmationStatus {
    ConfirmedResolved,
    StillOpen,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FindingConfirmation {
    pub finding_id: String,
    pub status: ConfirmationStatus,
    pub reviewed_fix_sha: String,
    pub evidence: Vec<String>,
}

/// Unknown fields are ignored: a stray key from a model must not discard its work.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReviewResult {
    #[serde(flatten)]
    pub common: CommonResult,
    pub reviewer_id: String,
    pub outcome: ReviewOutcome,
    pub plan_version: u32,
    pub review_round: u32,
    pub pr_number: u64,
    pub reviewed_head_sha: String,
    pub blocking_findings: Vec<BlockingFinding>,
    pub suggestions: Vec<Suggestion>,
    pub finding_confirmations: Vec<FindingConfirmation>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FinalOutcome {
    Ready,
    ReturnToBuild,
    ReturnToReview,
    ReturnToPlanning,
    WaitForIssueCreator,
    Blocked,
    Abandon,
    BlockedUnexpectedModel,
}

/// Unknown fields are ignored: a stray key from a model must not discard its work.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FinalResult {
    #[serde(flatten)]
    pub common: CommonResult,
    pub outcome: FinalOutcome,
    pub plan_version: u32,
    pub final_review_round: u32,
    pub pr_number: u64,
    pub reviewed_head_sha: String,
    pub residual_uncertainties: Vec<String>,
    pub decision_rationale: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum WorkerResult {
    Planner(PlannerResult),
    Builder(BuilderResult),
    Review(ReviewResult),
    Final(FinalResult),
}

/// Results decode by their declared role, never by guessing which shape fits.
impl<'de> Deserialize<'de> for WorkerResult {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::decode(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Everything the controller already knows about a job, used to complete a
/// worker's result so the model only has to report its actual work.
pub struct BindingFill<'a> {
    pub binding: &'a WorkerBinding,
    /// The frozen task body; supplies review, build, and final-review rounds.
    pub task_body: &'a Value,
    /// Start and end times measured by the executor, when it measures them.
    pub run_times: Option<(u64, u64)>,
    /// When the controller observed the result; an upper bound for worker times.
    pub now: u64,
    /// Fill `reviewed_head_sha` from the binding. Only for executors that
    /// verify the reviewed checkout themselves; otherwise the worker attests it.
    pub reviewed_head_from_binding: bool,
}

/// Overwrites or inserts the controller-owned fields of a worker result.
/// Worker-produced data (outcomes, findings, a builder's new head) is kept.
pub fn fill_binding(value: &mut Value, fill: &BindingFill<'_>) -> Result<(), ContractError> {
    let binding = fill.binding;
    let object = value.as_object_mut().ok_or(ContractError::RoleMismatch)?;
    let case = serde_json::to_value(&binding.case).map_err(|_| ContractError::InvalidIdentity)?;
    let role = serde_json::to_value(binding.role).map_err(|_| ContractError::RoleMismatch)?;
    object.insert("contract_version".into(), CONTRACT_VERSION.into());
    object.insert(
        "workflow_version".into(),
        binding.case.workflow_version.into(),
    );
    object.insert("case".into(), case);
    object.insert("task_id".into(), binding.task_id.clone().into());
    object.insert("role".into(), role);
    object.insert(
        "requested_model".into(),
        binding.requested_model.clone().into(),
    );
    object.insert(
        "actual_model".into(),
        binding.requested_model.clone().into(),
    );
    object.insert(
        "skills_repository_commit".into(),
        binding.skills_repository_commit.clone().into(),
    );
    object.insert("plan_version".into(), binding.plan_version.into());
    object
        .entry("evidence")
        .or_insert_with(|| Value::Object(Map::new()));
    let reviewed = matches!(
        binding.role,
        WorkerRole::ReviewerGeneral | WorkerRole::ReviewerSecperf | WorkerRole::FinalReviewer
    );
    if reviewed {
        if let Some(pr_number) = binding.pr_number {
            object.insert("pr_number".into(), pr_number.into());
        }
        if fill.reviewed_head_from_binding
            && let Some(head) = &binding.expected_head_sha
        {
            object.insert("reviewed_head_sha".into(), head.clone().into());
        }
    }
    if let Some(reviewer_id) = &binding.reviewer_id {
        object.insert("reviewer_id".into(), reviewer_id.clone().into());
    }
    let round = |key: &str| fill.task_body.get(key).and_then(Value::as_u64);
    let rounds: &[&str] = match binding.role {
        WorkerRole::Builder => &["build_round"],
        WorkerRole::ReviewerGeneral | WorkerRole::ReviewerSecperf => &["review_round"],
        WorkerRole::FinalReviewer => &["final_review_round"],
        WorkerRole::Planner => &[],
    };
    for key in rounds {
        if let Some(value) = round(key) {
            object.insert((*key).into(), value.into());
        }
    }
    let (started, completed) = fill.run_times.unwrap_or_else(|| {
        let completed = object
            .get("completed_at_unix")
            .and_then(Value::as_u64)
            .filter(|completed| *completed > 0 && *completed <= fill.now)
            .unwrap_or(fill.now);
        let started = object
            .get("started_at_unix")
            .and_then(Value::as_u64)
            .filter(|started| *started > 0 && *started <= completed)
            .unwrap_or(completed);
        (started, completed)
    });
    object.insert("started_at_unix".into(), started.into());
    object.insert("completed_at_unix".into(), completed.into());
    Ok(())
}

fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn not_blank(value: &str) -> bool {
    !value.trim().is_empty()
}

impl CommonResult {
    fn validate(&self, expected_role: WorkerRole) -> Result<(), ContractError> {
        if self.contract_version != CONTRACT_VERSION {
            return Err(ContractError::UnsupportedContractVersion);
        }
        if self.workflow_version == 0
            || self.case.repository_id == 0
            || self.case.issue_number == 0
            || self.case.workflow_version == 0
        {
            return Err(ContractError::InvalidIdentity);
        }
        if self.case.workflow_version != self.workflow_version {
            return Err(ContractError::WorkflowVersionMismatch);
        }
        if self.role != expected_role {
            return Err(ContractError::RoleMismatch);
        }
        if !not_blank(&self.task_id)
            || !not_blank(&self.requested_model)
            || !not_blank(&self.actual_model)
        {
            return Err(ContractError::EmptyRequiredField);
        }
        if !is_hex(&self.skills_repository_commit, 40) {
            return Err(ContractError::InvalidCommit);
        }
        if self.completed_at_unix < self.started_at_unix {
            return Err(ContractError::InvalidTimestampOrder);
        }
        Ok(())
    }
}

impl WorkerResult {
    /// Decode the declared role directly so a malformed field produces its
    /// actual type error, not an opaque failure of the untagged union.
    pub fn decode(value: Value) -> Result<Self, serde_json::Error> {
        let role: WorkerRole =
            serde_json::from_value(value.get("role").cloned().unwrap_or(Value::Null))?;
        match role {
            WorkerRole::Planner => serde_json::from_value(value).map(Self::Planner),
            WorkerRole::Builder => serde_json::from_value(value).map(Self::Builder),
            WorkerRole::ReviewerGeneral | WorkerRole::ReviewerSecperf => {
                serde_json::from_value(value).map(Self::Review)
            }
            WorkerRole::FinalReviewer => serde_json::from_value(value).map(Self::Final),
        }
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        match self {
            Self::Planner(result) => result.validate(),
            Self::Builder(result) => result.validate(),
            Self::Review(result) => result.validate(),
            Self::Final(result) => result.validate(),
        }
    }

    #[must_use]
    pub fn common(&self) -> &CommonResult {
        match self {
            Self::Planner(result) => &result.common,
            Self::Builder(result) => &result.common,
            Self::Review(result) => &result.common,
            Self::Final(result) => &result.common,
        }
    }

    pub fn validate_binding(&self, binding: &WorkerBinding) -> Result<(), ContractError> {
        self.validate()?;
        let common = self.common();
        let (plan_version, pr_number, head_sha, reviewer_id) = match self {
            Self::Planner(result) => (result.plan_version, None, None, None),
            Self::Builder(result) => (result.plan_version, None, result.head_sha.as_deref(), None),
            Self::Review(result) => (
                result.plan_version,
                Some(result.pr_number),
                Some(result.reviewed_head_sha.as_str()),
                Some(result.reviewer_id.as_str()),
            ),
            Self::Final(result) => (
                result.plan_version,
                Some(result.pr_number),
                Some(result.reviewed_head_sha.as_str()),
                None,
            ),
        };
        // PR/head are reviewed outputs only for review roles. For planning
        // and building they describe incoming context: remediation must be
        // allowed to produce a different commit, and neither result contract
        // owns the PR number. Publication validates that new commit separately.
        let reviewed_output = matches!(self, Self::Review(_) | Self::Final(_));
        let matches = common.case == binding.case
            && common.task_id == binding.task_id
            && common.role == binding.role
            && reviewer_id == binding.reviewer_id.as_deref()
            && binding.review_mode.is_some() == reviewer_id.is_some()
            && common.requested_model == binding.requested_model
            && common.skills_repository_commit == binding.skills_repository_commit
            && plan_version == binding.plan_version
            && (!reviewed_output
                || (binding
                    .pr_number
                    .is_none_or(|expected| pr_number == Some(expected))
                    && binding
                        .expected_head_sha
                        .as_deref()
                        .is_none_or(|expected| head_sha == Some(expected))));
        if matches {
            Ok(())
        } else {
            Err(ContractError::BindingMismatch)
        }
    }
}

impl PlannerResult {
    fn validate(&self) -> Result<(), ContractError> {
        self.common.validate(WorkerRole::Planner)?;
        if self.plan_version == 0 || !is_hex(&self.planned_base_sha, 40) {
            return Err(ContractError::InvalidIdentity);
        }
        if !not_blank(&self.authorized_scope) || !not_blank(&self.plan_artifact) {
            return Err(ContractError::EmptyRequiredField);
        }
        if self.dependencies.iter().any(|item| !item.is_object()) {
            return Err(ContractError::InvalidOutcomeEvidence);
        }
        // Sensitive scope is published and focuses review; it does not stop
        // a plan. Unresolved dependencies or decisions do.
        if self.outcome == PlannerOutcome::Proceed
            && (!self.dependencies.is_empty() || !self.open_decisions.is_empty())
        {
            return Err(ContractError::InvalidOutcomeEvidence);
        }
        Ok(())
    }
}

impl BuilderResult {
    fn validate(&self) -> Result<(), ContractError> {
        self.common.validate(WorkerRole::Builder)?;
        if self.plan_version == 0 || self.build_round == 0 {
            return Err(ContractError::InvalidIdentity);
        }
        for resolution in &self.finding_resolutions {
            if !is_hex(&resolution.resolution_commit, 40)
                || !is_hex(&resolution.resolved_head_sha, 40)
                || !not_blank(&resolution.finding_id)
                || !not_blank(&resolution.resolution_summary)
                || resolution.tests.is_empty()
                || resolution.tests.iter().any(|item| !not_blank(item))
            {
                return Err(ContractError::InvalidOutcomeEvidence);
            }
        }
        if self.outcome == BuilderOutcome::ReviewReady {
            let Some(head) = self.head_sha.as_deref() else {
                return Err(ContractError::InvalidOutcomeEvidence);
            };
            if !is_hex(head, 40) {
                return Err(ContractError::InvalidOutcomeEvidence);
            }
        } else if self.head_sha.is_some() {
            return Err(ContractError::InvalidOutcomeEvidence);
        }
        Ok(())
    }
}

impl ReviewResult {
    fn validate(&self) -> Result<(), ContractError> {
        if !matches!(
            self.common.role,
            WorkerRole::ReviewerGeneral | WorkerRole::ReviewerSecperf
        ) {
            return Err(ContractError::RoleMismatch);
        }
        self.common.validate(self.common.role)?;
        if self.plan_version == 0
            || self.review_round == 0
            || self.pr_number == 0
            || !not_blank(&self.reviewer_id)
            || !is_hex(&self.reviewed_head_sha, 40)
        {
            return Err(ContractError::InvalidIdentity);
        }
        if self.outcome == ReviewOutcome::Approve && !self.blocking_findings.is_empty() {
            return Err(ContractError::ApprovalHasBlockingFindings);
        }
        if self.outcome == ReviewOutcome::Approve
            && self
                .finding_confirmations
                .iter()
                .any(|item| item.status != ConfirmationStatus::ConfirmedResolved)
        {
            return Err(ContractError::ApprovalHasOpenConfirmation);
        }
        Ok(())
    }
}

impl FinalResult {
    fn validate(&self) -> Result<(), ContractError> {
        self.common.validate(WorkerRole::FinalReviewer)?;
        if self.plan_version == 0
            || self.final_review_round == 0
            || self.pr_number == 0
            || !is_hex(&self.reviewed_head_sha, 40)
        {
            return Err(ContractError::InvalidIdentity);
        }
        if !not_blank(&self.decision_rationale) {
            return Err(ContractError::EmptyRequiredField);
        }
        Ok(())
    }
}
