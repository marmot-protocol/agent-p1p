//! Policy-driven worker dispatch projection.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_contracts::{ReviewMode, WorkerRole};
use pip_core::{
    CaseId, Effect, GitSha, IssueNumber, PlanVersion, PullRequestNumber, RepositoryId,
    StateRevision, WorkflowVersion,
};
use pip_hermes::TaskCreateSpec;
use pip_store::{ClaimedEffect, Store, StoredCase};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const MAX_EVIDENCE_BUNDLE_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionKind {
    Hermes,
    Direct,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RolePolicy {
    pub role: WorkerRole,
    pub reviewer_id: Option<String>,
    pub review_mode: Option<ReviewMode>,
    pub profile: String,
    pub execution: ExecutionKind,
    pub provider: String,
    pub model: String,
    pub max_runtime: String,
    pub priority: u32,
    pub skills: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowPolicy {
    board: String,
    workspace: String,
    branch_prefix: String,
    sensitive_scope_categories: Vec<String>,
    max_provider_failures: u32,
    max_hermes_attempts: Option<u32>,
    hermes_scratch_root: Option<String>,
    roles: Vec<RolePolicy>,
}

impl WorkflowPolicy {
    pub fn new(
        board: impl Into<String>,
        workspace: impl Into<String>,
        branch_prefix: impl Into<String>,
        roles: Vec<RolePolicy>,
    ) -> Result<Self, DispatchError> {
        let board = board.into();
        let workspace = workspace.into();
        let branch_prefix = branch_prefix.into();
        let fixed = [
            WorkerRole::Planner,
            WorkerRole::Builder,
            WorkerRole::FinalReviewer,
        ];
        let fixed_valid = fixed
            .iter()
            .all(|expected| roles.iter().filter(|role| role.role == *expected).count() == 1);
        let reviewer_ids = roles
            .iter()
            .filter_map(|role| role.reviewer_id.as_deref())
            .collect::<BTreeSet<_>>();
        let reviewer_count = roles.iter().filter(|role| is_reviewer(role.role)).count();
        let required_lanes = [WorkerRole::ReviewerGeneral, WorkerRole::ReviewerSecperf]
            .into_iter()
            .all(|expected| {
                roles.iter().any(|role| {
                    role.role == expected && role.review_mode == Some(ReviewMode::Required)
                })
            });
        let valid_roles = fixed_valid
            && reviewer_count >= 2
            && reviewer_ids.len() == reviewer_count
            && required_lanes;
        let valid_bindings = roles.iter().all(valid_role_policy);
        if !valid_id(&board)
            || !workspace.starts_with('/')
            || !valid_text(&workspace, 4096)
            || !valid_branch_prefix(&branch_prefix)
            || !valid_roles
            || !valid_bindings
        {
            return Err(DispatchError::InvalidPolicy);
        }
        Ok(Self {
            board,
            workspace,
            branch_prefix,
            sensitive_scope_categories: Vec::new(),
            max_provider_failures: 1,
            max_hermes_attempts: None,
            hermes_scratch_root: None,
            roles,
        })
    }

    pub fn with_max_provider_failures(
        mut self,
        max_provider_failures: u32,
    ) -> Result<Self, DispatchError> {
        if max_provider_failures == 0 {
            return Err(DispatchError::InvalidPolicy);
        }
        self.max_provider_failures = max_provider_failures;
        Ok(self)
    }

    pub fn with_max_hermes_attempts(mut self, attempts: u32) -> Result<Self, DispatchError> {
        if attempts == 0 {
            return Err(DispatchError::InvalidPolicy);
        }
        self.max_hermes_attempts = Some(attempts);
        Ok(self)
    }

    pub fn with_hermes_scratch_root(mut self, root: String) -> Result<Self, DispatchError> {
        let path = std::path::Path::new(&root);
        let source = std::path::Path::new(&self.workspace);
        if !path.is_absolute()
            || root.ends_with('/')
            || !valid_text(&root, 4096)
            || path.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
            || path.starts_with(source)
            || source.starts_with(path)
        {
            return Err(DispatchError::InvalidPolicy);
        }
        self.hermes_scratch_root = Some(root);
        Ok(self)
    }

    pub fn with_sensitive_scope_categories(
        mut self,
        categories: Vec<String>,
    ) -> Result<Self, DispatchError> {
        let unique = categories.iter().collect::<BTreeSet<_>>();
        if categories.is_empty()
            || unique.len() != categories.len()
            || categories.iter().any(|category| !valid_text(category, 128))
        {
            return Err(DispatchError::InvalidPolicy);
        }
        self.sensitive_scope_categories = categories;
        Ok(self)
    }

    #[must_use]
    pub fn roles(&self) -> &[RolePolicy] {
        &self.roles
    }

    fn role(&self, role: WorkerRole) -> Result<&RolePolicy, DispatchError> {
        self.roles
            .iter()
            .find(|binding| binding.role == role && binding.reviewer_id.is_none())
            .ok_or(DispatchError::InvalidPolicy)
    }

    pub fn reviewers(&self) -> impl Iterator<Item = &RolePolicy> {
        self.roles.iter().filter(|role| is_reviewer(role.role))
    }

    pub fn required_reviewers(&self) -> impl Iterator<Item = &RolePolicy> {
        self.reviewers()
            .filter(|role| role.review_mode == Some(ReviewMode::Required))
    }

    pub fn reviewer(&self, reviewer_id: &str) -> Result<&RolePolicy, DispatchError> {
        self.reviewers()
            .find(|binding| binding.reviewer_id.as_deref() == Some(reviewer_id))
            .ok_or(DispatchError::InvalidPolicy)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatchContext {
    pub case_id: CaseId,
    pub state_revision: StateRevision,
    pub plan_version: Option<PlanVersion>,
    pub remediation_round: u32,
    pub pr_number: Option<PullRequestNumber>,
    pub head_sha: Option<GitSha>,
    pub skills_repository_commit: GitSha,
    pub immutable_evidence_bundle: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowDispatch {
    pub role: WorkerRole,
    pub worker_projection_key: String,
    pub worker_body: Value,
    worker_effect_id: String,
    worker_title: String,
    source_effect_id: String,
    profile: String,
    workspace: String,
    direct_workspace: String,
    execution: ExecutionKind,
    skills: Vec<String>,
    provider: String,
    model: String,
    max_runtime: String,
    max_retries: u32,
    priority: u32,
    board: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DirectTaskSpec {
    pub schema_version: u32,
    pub source_effect_id: String,
    pub task_id: String,
    pub title: String,
    pub body: Value,
    pub role: WorkerRole,
    pub profile: String,
    pub workspace: String,
    pub skills: Vec<String>,
    pub provider: String,
    pub model: String,
    pub max_runtime: String,
    pub priority: u32,
}

impl WorkflowDispatch {
    #[must_use]
    pub const fn execution(&self) -> ExecutionKind {
        self.execution
    }

    pub fn hermes_task(&self) -> Result<TaskCreateSpec, DispatchError> {
        if self.execution != ExecutionKind::Hermes {
            return Err(DispatchError::WrongExecutor);
        }
        Ok(TaskCreateSpec {
            board: self.board.clone(),
            effect_id: self.worker_effect_id.clone(),
            projection_key: self.worker_projection_key.clone(),
            title: self.worker_title.clone(),
            body: self.worker_body.clone(),
            assignee: self.profile.clone(),
            // Pip has already materialized and owns this worktree/branch.
            // Hermes's worktree: mode can create another task-specific branch.
            workspace: format!("dir:{}", self.workspace),
            skills: self.skills.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            max_runtime: self.max_runtime.clone(),
            max_retries: self.max_retries,
            priority: self.priority,
            parent_task_ids: Vec::new(),
        })
    }

    pub fn direct_task(&self) -> Result<DirectTaskSpec, DispatchError> {
        if self.execution != ExecutionKind::Direct {
            return Err(DispatchError::WrongExecutor);
        }
        Ok(DirectTaskSpec {
            schema_version: 1,
            source_effect_id: self.source_effect_id.clone(),
            task_id: self.worker_projection_key.clone(),
            title: self.worker_title.clone(),
            body: self.worker_body.clone(),
            role: self.role,
            profile: self.profile.clone(),
            workspace: self.direct_workspace.clone(),
            skills: self.skills.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            max_runtime: self.max_runtime.clone(),
            priority: self.priority,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DispatchError {
    InvalidPolicy,
    InvalidEffectId,
    MissingPlan,
    MissingPullRequest,
    MissingExactHead,
    WrongExecutor,
    InvalidStoredCase,
    InvalidEvidenceBundle,
    StaleEffect,
    UnsupportedEffect,
}

impl fmt::Display for DispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPolicy => "invalid workflow role policy",
            Self::InvalidEffectId => "invalid durable effect identity",
            Self::MissingPlan => "worker dispatch requires an active plan",
            Self::MissingPullRequest => "review dispatch requires a pull request",
            Self::MissingExactHead => "review dispatch requires an exact head",
            Self::WrongExecutor => "worker dispatch is bound to a different executor",
            Self::InvalidStoredCase => "ledger case cannot form a dispatch binding",
            Self::InvalidEvidenceBundle => {
                "worker dispatch requires a bounded immutable evidence bundle"
            }
            Self::StaleEffect => "outbox effect does not match the current case revision",
            Self::UnsupportedEffect => "outbox effect is not a worker dispatch",
        })
    }
}

pub fn schedule_claimed_dispatch(
    claimed: &ClaimedEffect,
    case: &StoredCase,
    store: &Store,
    policy: &WorkflowPolicy,
    skills_repository_commit: GitSha,
) -> Result<Vec<WorkflowDispatch>, DispatchError> {
    if claimed.case_key != case.case_key || claimed.state_revision != case.state_revision {
        return Err(DispatchError::StaleEffect);
    }
    let case_id = CaseId::new(
        RepositoryId::new(
            NonZeroU64::new(case.repository_id).ok_or(DispatchError::InvalidStoredCase)?,
        ),
        IssueNumber::new(
            NonZeroU64::new(case.issue_number).ok_or(DispatchError::InvalidStoredCase)?,
        ),
        WorkflowVersion::new(
            NonZeroU32::new(case.workflow_version).ok_or(DispatchError::InvalidStoredCase)?,
        ),
    );
    if case_id.to_string() != case.case_key {
        return Err(DispatchError::InvalidStoredCase);
    }
    let effect = match claimed.effect_type.as_str() {
        "DISPATCH_PLANNER" => Effect::DispatchPlanner,
        "DISPATCH_BUILDER" => Effect::DispatchBuilder,
        "DISPATCH_REVIEWERS" => Effect::DispatchReviewers,
        "DISPATCH_FINAL_REVIEWER" => Effect::DispatchFinalReviewer,
        _ => return Err(DispatchError::UnsupportedEffect),
    };
    let context = DispatchContext {
        case_id,
        state_revision: StateRevision::new(
            NonZeroU64::new(case.state_revision).ok_or(DispatchError::InvalidStoredCase)?,
        ),
        plan_version: NonZeroU32::new(case.plan_version).map(PlanVersion::new),
        remediation_round: case.remediation_round,
        pr_number: case
            .pr_number
            .and_then(NonZeroU64::new)
            .map(PullRequestNumber::new),
        head_sha: case
            .head_sha
            .as_deref()
            .map(GitSha::from_str)
            .transpose()
            .map_err(|_| DispatchError::InvalidStoredCase)?,
        skills_repository_commit,
        immutable_evidence_bundle: immutable_evidence_bundle(store, case)?,
    };
    schedule_effect(&claimed.effect_id, effect, &context, policy)
}

fn immutable_evidence_bundle(store: &Store, case: &StoredCase) -> Result<Value, DispatchError> {
    let history = store
        .immutable_history_for_case(&case.case_key)
        .map_err(|_| DispatchError::InvalidEvidenceBundle)?;
    if history.events.last().map(|event| event.state_revision) != Some(case.state_revision) {
        return Err(DispatchError::InvalidEvidenceBundle);
    }
    let mut records =
        serde_json::to_value(&history).map_err(|_| DispatchError::InvalidEvidenceBundle)?;
    // Accepted results occur in both the event journal and run index. Keep
    // one payload in exported jobs and reference it by its immutable identity
    // and digest. The authoritative ledger remains unchanged and fully readable.
    let runs = history
        .runs
        .iter()
        .map(|run| (run.event_id.as_str(), run))
        .collect::<BTreeMap<_, _>>();
    for event in records["events"]
        .as_array_mut()
        .ok_or(DispatchError::InvalidEvidenceBundle)?
    {
        let event_id = event["event_id"]
            .as_str()
            .ok_or(DispatchError::InvalidEvidenceBundle)?;
        if let Some(run) = runs.get(event_id)
            && event["payload_sha256"].as_str() == Some(run.payload_sha256.as_str())
            && event["payload"] == run.payload
        {
            let event = event
                .as_object_mut()
                .ok_or(DispatchError::InvalidEvidenceBundle)?;
            event.remove("payload");
            event.insert(
                "payload_ref".into(),
                json!({"run_id":run.run_id,"payload_sha256":run.payload_sha256}),
            );
        }
    }
    let unsigned = json!({
        "schema_version": 2,
        "case_key": case.case_key,
        "bound_state_revision": case.state_revision,
        "records": records,
    });
    let encoded =
        serde_json::to_vec(&unsigned).map_err(|_| DispatchError::InvalidEvidenceBundle)?;
    if encoded.len() > MAX_EVIDENCE_BUNDLE_BYTES {
        return Err(DispatchError::InvalidEvidenceBundle);
    }
    let mut bundle = unsigned
        .as_object()
        .cloned()
        .ok_or(DispatchError::InvalidEvidenceBundle)?;
    bundle.insert(
        "sha256".into(),
        Value::String(hex_digest(&Sha256::digest(encoded))),
    );
    Ok(Value::Object(bundle))
}

impl std::error::Error for DispatchError {}

/// A reading index, never workflow authority. Full immutable records remain
/// available in the same artifact; no payload is copied into the prompt.
fn evidence_focus(context: &DispatchContext, binding: &RolePolicy) -> Value {
    let records = &context.immutable_evidence_bundle["records"];
    let rows = |kind: &str| records[kind].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut selected = BTreeSet::new();
    let mut latest = |kind, predicate: &dyn Fn(&Value) -> bool| {
        if let Some(index) = rows(kind).iter().rposition(predicate) {
            selected.insert((kind, index));
        }
    };
    latest("events", &|row| {
        matches!(
            row["event_type"].as_str(),
            Some("ISSUE_AUTHORIZED" | "ISSUE_REAUTHORIZED")
        )
    });
    latest("runs", &|row| {
        row["role"] == "planner"
            && row["payload"]["plan_version"].as_u64()
                == context.plan_version.map(|version| u64::from(version.get()))
    });
    if binding.role != WorkerRole::Planner {
        let head = context.head_sha.map(|sha| sha.to_string());
        latest("runs", &|row| {
            row["role"] == "builder"
                && head.is_some()
                && row["payload"]["head_sha"].as_str() == head.as_deref()
        });
        latest("evidence", &|row| {
            row["kind"] == "GITHUB_CI"
                && head.is_some()
                && row["payload"]["pull_request"]["head_sha"].as_str() == head.as_deref()
        });
    }
    if binding.role == WorkerRole::FinalReviewer {
        latest("evidence", &|row| row["kind"] == "GITHUB_FINAL_PREFLIGHT");
    }
    if binding.role == WorkerRole::Builder {
        latest("evidence", &|row| row["kind"] == "GITHUB_REVIEW_FEEDBACK");
    }
    // Human constraints remain visible through replanning and subsequent
    // builds/reviews, rather than disappearing behind the latest model result.
    for (index, row) in rows("evidence").iter().enumerate() {
        if row["kind"] == "HUMAN_DISCUSSION" {
            selected.insert(("evidence", index));
        }
    }
    let independent_review = matches!(
        binding.role,
        WorkerRole::ReviewerGeneral | WorkerRole::ReviewerSecperf
    );
    for (index, row) in rows("findings").iter().enumerate() {
        if !independent_review || row["origin_role"].as_str() == binding.reviewer_id.as_deref() {
            selected.insert(("findings", index));
        }
    }
    if binding.role != WorkerRole::Planner {
        let head = context.head_sha.map(|sha| sha.to_string());
        let mut reviewers = BTreeSet::new();
        for (index, row) in rows("runs").iter().enumerate().rev() {
            let Some(reviewer) = row["payload"]["reviewer_id"].as_str() else {
                continue;
            };
            if head.is_some()
                && row["payload"]["reviewed_head_sha"].as_str() == head.as_deref()
                && (!independent_review || Some(reviewer) == binding.reviewer_id.as_deref())
                && reviewers.insert(reviewer)
            {
                selected.insert(("runs", index));
            }
        }
    }
    json!({"schema_version":1,"records":selected.into_iter().map(|(kind, index)| {
        json!({"pointer":format!("/records/{kind}/{index}"),"payload_sha256":rows(kind)[index]["payload_sha256"]})
    }).collect::<Vec<_>>()})
}

pub fn schedule_effect(
    effect_id: &str,
    effect: Effect,
    context: &DispatchContext,
    policy: &WorkflowPolicy,
) -> Result<Vec<WorkflowDispatch>, DispatchError> {
    if !valid_opaque(effect_id, 512) {
        return Err(DispatchError::InvalidEffectId);
    }
    match effect {
        Effect::DispatchPlanner => Ok(vec![dispatch(
            effect_id,
            effect_id,
            policy.role(WorkerRole::Planner)?,
            context,
            policy,
        )?]),
        Effect::DispatchBuilder => {
            context.plan_version.ok_or(DispatchError::MissingPlan)?;
            Ok(vec![dispatch(
                effect_id,
                effect_id,
                policy.role(WorkerRole::Builder)?,
                context,
                policy,
            )?])
        }
        Effect::DispatchReviewers => {
            require_review_binding(context)?;
            policy
                .reviewers()
                .map(|binding| {
                    let reviewer_id = binding
                        .reviewer_id
                        .as_deref()
                        .ok_or(DispatchError::InvalidPolicy)?;
                    dispatch(
                        &format!("{effect_id}:{reviewer_id}"),
                        effect_id,
                        binding,
                        context,
                        policy,
                    )
                })
                .collect()
        }
        Effect::DispatchFinalReviewer => {
            require_review_binding(context)?;
            Ok(vec![dispatch(
                effect_id,
                effect_id,
                policy.role(WorkerRole::FinalReviewer)?,
                context,
                policy,
            )?])
        }
        _ => Ok(Vec::new()),
    }
}

fn require_review_binding(context: &DispatchContext) -> Result<(), DispatchError> {
    context.plan_version.ok_or(DispatchError::MissingPlan)?;
    context.pr_number.ok_or(DispatchError::MissingPullRequest)?;
    context.head_sha.ok_or(DispatchError::MissingExactHead)?;
    Ok(())
}

fn dispatch(
    effect_id: &str,
    source_effect_id: &str,
    binding: &RolePolicy,
    context: &DispatchContext,
    policy: &WorkflowPolicy,
) -> Result<WorkflowDispatch, DispatchError> {
    let role = binding.role;
    if !context.immutable_evidence_bundle.is_object() {
        return Err(DispatchError::InvalidEvidenceBundle);
    }
    let role_name = role_name(role);
    let review_round = context.remediation_round.saturating_add(1);
    let round = match role {
        WorkerRole::Planner => context.plan_version.map_or(1, |version| version.get() + 1),
        WorkerRole::Builder => review_round,
        WorkerRole::ReviewerGeneral | WorkerRole::ReviewerSecperf | WorkerRole::FinalReviewer => {
            review_round
        }
    };
    let worker_id = binding.reviewer_id.as_deref().unwrap_or(role_name);
    let projection_base = format!(
        "{}:{worker_id}:round:{round}:revision:{}",
        context.case_id,
        context.state_revision.get()
    );
    let worker_projection_key = format!("{projection_base}:worker");
    let mut body = Map::from_iter([
        ("case_key".into(), json!(context.case_id.to_string())),
        (
            "repository_id".into(),
            json!(context.case_id.repository().get()),
        ),
        ("issue_number".into(), json!(context.case_id.issue().get())),
        (
            "workflow_version".into(),
            json!(context.case_id.workflow().get()),
        ),
        ("state_revision".into(), json!(context.state_revision.get())),
        ("role".into(), json!(role_name)),
        ("remediation_round".into(), json!(context.remediation_round)),
    ]);
    let result_plan_version = if role == WorkerRole::Planner {
        context.plan_version.map_or(1, |version| version.get() + 1)
    } else {
        context
            .plan_version
            .map(PlanVersion::get)
            .unwrap_or_default()
    };
    if result_plan_version > 0 {
        body.insert("plan_version".into(), json!(result_plan_version));
    }
    if let Some(pr_number) = context.pr_number {
        body.insert("pr_number".into(), json!(pr_number.get()));
    }
    if let Some(head) = context.head_sha {
        body.insert("expected_head_sha".into(), json!(head.to_string()));
    }
    if matches!(
        role,
        WorkerRole::ReviewerGeneral | WorkerRole::ReviewerSecperf
    ) {
        body.insert("review_round".into(), json!(review_round));
        body.insert("reviewer_id".into(), json!(worker_id));
        body.insert(
            "review_mode".into(),
            json!(match binding.review_mode {
                Some(ReviewMode::Required) => "required",
                Some(ReviewMode::Advisory) => "advisory",
                Some(ReviewMode::Shadow) => "shadow",
                None => return Err(DispatchError::InvalidPolicy),
            }),
        );
    }
    if role == WorkerRole::FinalReviewer {
        body.insert("final_review_round".into(), json!(review_round));
    }
    if role == WorkerRole::Planner && !policy.sensitive_scope_categories.is_empty() {
        body.insert(
            "sensitive_scope_categories".into(),
            json!(policy.sensitive_scope_categories),
        );
    }
    if role == WorkerRole::Builder {
        body.insert("build_round".into(), json!(round));
        body.insert(
            "assigned_branch".into(),
            json!(format!(
                "{}repo-{}/issue-{}/workflow-{}",
                policy.branch_prefix,
                context.case_id.repository().get(),
                context.case_id.issue().get(),
                context.case_id.workflow().get(),
            )),
        );
        body.insert(
            "assigned_worktree".into(),
            json!(format!(
                "{}/repo-{}-issue-{}-workflow-{}",
                policy.workspace.trim_end_matches('/'),
                context.case_id.repository().get(),
                context.case_id.issue().get(),
                context.case_id.workflow().get(),
            )),
        );
    }
    body.insert(
        "execution".into(),
        json!(match binding.execution {
            ExecutionKind::Hermes => "hermes",
            ExecutionKind::Direct => "direct",
        }),
    );
    body.insert("provider".into(), json!(binding.provider));
    body.insert("model".into(), json!(binding.model));
    body.insert(
        "requested_model".into(),
        json!(format!("{}/{}", binding.provider, binding.model)),
    );
    body.insert(
        "skills_repository_commit".into(),
        json!(context.skills_repository_commit.to_string()),
    );
    body.insert(
        "immutable_evidence_bundle".into(),
        context.immutable_evidence_bundle.clone(),
    );
    body.insert("evidence_focus".into(), evidence_focus(context, binding));
    if binding.execution == ExecutionKind::Hermes
        && let Some(root) = &policy.hermes_scratch_root
    {
        body.insert("projection_key".into(), json!(worker_projection_key));
        let root = format!(
            "{root}/{}",
            &hex_digest(&Sha256::digest(worker_projection_key.as_bytes()))[..16]
        );
        // Include private socket staging, not only the final socket name.
        // Full task identity remains in the checked ownership marker.
        if format!("{root}/t").len() > 56 {
            return Err(DispatchError::InvalidPolicy);
        }
        let evidence = serde_json::to_vec(&context.immutable_evidence_bundle)
            .map_err(|_| DispatchError::InvalidEvidenceBundle)?;
        body.insert(
            "immutable_evidence_ref".into(),
            json!({
                "schema_version": 1,
                "path": format!("{root}/immutable-evidence.json"),
                "sha256": hex_digest(&Sha256::digest(evidence)),
            }),
        );
        body.insert("storage".into(), json!({
            "schema_version": 3,
            "root": root,
            "source": format!("{}/repo-{}-issue-{}-workflow-{}", policy.workspace.trim_end_matches('/'), context.case_id.repository().get(), context.case_id.issue().get(), context.case_id.workflow().get()),
            "cargo_target": format!("{root}/disposable/target"),
            "cargo_home": format!("{root}/disposable/cargo-home"),
            "temporary": format!("{root}/t"),
            "results": format!("{root}/results"),
        }));
    }

    Ok(WorkflowDispatch {
        role,
        worker_projection_key,
        worker_body: Value::Object(body),
        worker_effect_id: format!("{effect_id}:worker"),
        worker_title: format!("Run {worker_id} for {}", context.case_id),
        source_effect_id: source_effect_id.into(),
        profile: binding.profile.clone(),
        workspace: format!(
            "{}/repo-{}-issue-{}-workflow-{}",
            policy.workspace.trim_end_matches('/'),
            context.case_id.repository().get(),
            context.case_id.issue().get(),
            context.case_id.workflow().get(),
        ),
        direct_workspace: format!(
            "{}/repo-{}-issue-{}-workflow-{}",
            policy.workspace.trim_end_matches('/'),
            context.case_id.repository().get(),
            context.case_id.issue().get(),
            context.case_id.workflow().get(),
        ),
        execution: binding.execution,
        skills: binding.skills.clone(),
        provider: binding.provider.clone(),
        model: binding.model.clone(),
        max_runtime: binding.max_runtime.clone(),
        max_retries: policy
            .max_hermes_attempts
            .unwrap_or(policy.max_provider_failures),
        priority: binding.priority,
        board: policy.board.clone(),
    })
}

fn valid_role_policy(role: &RolePolicy) -> bool {
    let execution_valid = match role.role {
        WorkerRole::Builder => role.execution == ExecutionKind::Direct && role.provider == "cursor",
        WorkerRole::Planner | WorkerRole::FinalReviewer => {
            role.execution == ExecutionKind::Hermes && role.provider != "cursor"
        }
        WorkerRole::ReviewerGeneral | WorkerRole::ReviewerSecperf => match role.execution {
            ExecutionKind::Direct => role.provider == "cursor",
            ExecutionKind::Hermes => role.provider != "cursor",
        },
    };
    let review_binding_valid = if is_reviewer(role.role) {
        role.reviewer_id.as_deref().is_some_and(valid_id)
            && role.review_mode.is_some()
            && (role.review_mode == Some(ReviewMode::Required)
                || role.execution == ExecutionKind::Direct)
    } else {
        role.reviewer_id.is_none() && role.review_mode.is_none()
    };
    let model = role.model.to_ascii_lowercase();
    execution_valid
        && review_binding_valid
        && valid_id(&role.profile)
        && valid_id(&role.provider)
        && valid_id(&role.model)
        && !matches!(model.as_str(), "auto" | "default" | "latest")
        && valid_id(&role.max_runtime)
        && role.skills.len() == 2
        && role.skills.iter().all(|skill| valid_id(skill))
        && role.skills.iter().any(|skill| skill == "workflow-contract")
        && role
            .skills
            .iter()
            .any(|skill| skill == role_skill_name(role.role))
}

const fn is_reviewer(role: WorkerRole) -> bool {
    matches!(
        role,
        WorkerRole::ReviewerGeneral | WorkerRole::ReviewerSecperf
    )
}

const fn role_skill_name(role: WorkerRole) -> &'static str {
    match role {
        WorkerRole::Planner => "planner",
        WorkerRole::Builder => "builder-grok",
        WorkerRole::ReviewerGeneral => "reviewer-general",
        WorkerRole::ReviewerSecperf => "reviewer-secperf",
        WorkerRole::FinalReviewer => "final-reviewer",
    }
}

fn role_name(role: WorkerRole) -> &'static str {
    match role {
        WorkerRole::Planner => "planner",
        WorkerRole::Builder => "builder",
        WorkerRole::ReviewerGeneral => "reviewer-general",
        WorkerRole::ReviewerSecperf => "reviewer-secperf",
        WorkerRole::FinalReviewer => "final-reviewer",
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_opaque(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .chars()
            .all(|character| !character.is_control() && !character.is_whitespace())
}

fn valid_text(value: &str, max_len: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max_len
        && value.chars().all(|character| !character.is_control())
}

fn valid_branch_prefix(value: &str) -> bool {
    value.ends_with('/')
        && value.len() <= 128
        && value.trim_end_matches('/').split('/').all(valid_id)
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}
