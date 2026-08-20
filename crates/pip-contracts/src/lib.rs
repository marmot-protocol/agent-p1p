//! Stable serialized contracts at the worker/control-plane boundary.

#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CONTRACT_VERSION: u32 = 1;

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
    pub requested_model: String,
    pub plan_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_head_sha: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
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
    pub issue_comment_id: u64,
    pub issue_comment_body_sha256: String,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuilderResult {
    #[serde(flatten)]
    pub common: CommonResult,
    pub outcome: BuilderOutcome,
    pub plan_version: u32,
    pub build_round: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci_head_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_ci_green: Option<bool>,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewResult {
    #[serde(flatten)]
    pub common: CommonResult,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum WorkerResult {
    Planner(PlannerResult),
    Builder(BuilderResult),
    Review(ReviewResult),
    Final(FinalResult),
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
    fn validate(
        &self,
        expected_role: WorkerRole,
        blocked_unexpected: bool,
    ) -> Result<(), ContractError> {
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
        if self.requested_model != self.actual_model && !blocked_unexpected {
            return Err(ContractError::UnexpectedModel);
        }
        if self.requested_model == self.actual_model && blocked_unexpected {
            return Err(ContractError::InvalidOutcomeEvidence);
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
        let (plan_version, pr_number, head_sha) = match self {
            Self::Planner(result) => (result.plan_version, None, None),
            Self::Builder(result) => (
                result.plan_version,
                result.pr_number,
                result.head_sha.as_deref(),
            ),
            Self::Review(result) => (
                result.plan_version,
                Some(result.pr_number),
                Some(result.reviewed_head_sha.as_str()),
            ),
            Self::Final(result) => (
                result.plan_version,
                Some(result.pr_number),
                Some(result.reviewed_head_sha.as_str()),
            ),
        };
        let matches = common.case == binding.case
            && common.task_id == binding.task_id
            && common.role == binding.role
            && common.requested_model == binding.requested_model
            && plan_version == binding.plan_version
            && binding
                .pr_number
                .is_none_or(|expected| pr_number == Some(expected))
            && binding
                .expected_head_sha
                .as_deref()
                .is_none_or(|expected| head_sha == Some(expected));
        if matches {
            Ok(())
        } else {
            Err(ContractError::BindingMismatch)
        }
    }
}

impl PlannerResult {
    fn validate(&self) -> Result<(), ContractError> {
        self.common.validate(
            WorkerRole::Planner,
            self.outcome == PlannerOutcome::BlockedUnexpectedModel,
        )?;
        if self.plan_version == 0
            || self.issue_comment_id == 0
            || !is_hex(&self.planned_base_sha, 40)
        {
            return Err(ContractError::InvalidIdentity);
        }
        if !is_hex(&self.issue_comment_body_sha256, 64) {
            return Err(ContractError::InvalidDigest);
        }
        if !not_blank(&self.authorized_scope) || !not_blank(&self.plan_artifact) {
            return Err(ContractError::EmptyRequiredField);
        }
        if self.dependencies.iter().any(|item| !item.is_object()) {
            return Err(ContractError::InvalidOutcomeEvidence);
        }
        if self.outcome == PlannerOutcome::Proceed
            && (!self.sensitive_scope.is_empty()
                || !self.dependencies.is_empty()
                || !self.open_decisions.is_empty())
        {
            return Err(ContractError::InvalidOutcomeEvidence);
        }
        Ok(())
    }
}

impl BuilderResult {
    fn validate(&self) -> Result<(), ContractError> {
        self.common.validate(
            WorkerRole::Builder,
            self.outcome == BuilderOutcome::BlockedUnexpectedModel,
        )?;
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
            let (Some(pr_number), Some(head), Some(ci_head), Some(true)) = (
                self.pr_number,
                self.head_sha.as_deref(),
                self.ci_head_sha.as_deref(),
                self.required_ci_green,
            ) else {
                return Err(ContractError::InvalidOutcomeEvidence);
            };
            if pr_number == 0 || !is_hex(head, 40) || !is_hex(ci_head, 40) {
                return Err(ContractError::InvalidOutcomeEvidence);
            }
            if head != ci_head {
                return Err(ContractError::CiHeadMismatch);
            }
        } else if self.required_ci_green == Some(true) {
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
        self.common.validate(
            self.common.role,
            self.outcome == ReviewOutcome::BlockedUnexpectedModel,
        )?;
        if self.plan_version == 0
            || self.review_round == 0
            || self.pr_number == 0
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
        self.common.validate(
            WorkerRole::FinalReviewer,
            self.outcome == FinalOutcome::BlockedUnexpectedModel,
        )?;
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
