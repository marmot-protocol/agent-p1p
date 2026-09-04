use std::collections::BTreeSet;
use std::fmt;
use std::num::NonZeroU32;

use crate::{ActorId, PolicyRevision};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntakePolicy {
    pub revision: PolicyRevision,
    pub intake_enabled: bool,
    pub dispatch_enabled: bool,
    pub global_paused: bool,
    pub repository_paused: bool,
    pub required_label: String,
    pub trusted_actor_ids: BTreeSet<ActorId>,
    pub repository_active_limit: NonZeroU32,
    pub global_active_limit: NonZeroU32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueObservation {
    pub open: bool,
    pub is_pull_request: bool,
    pub labels: BTreeSet<String>,
    pub latest_label_actor_id: Option<ActorId>,
    pub excluded: bool,
    pub held: bool,
    pub already_owned: bool,
    pub repository_active_cases: u32,
    pub global_active_cases: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum IntakeBlocker {
    IntakeDisabled,
    GlobalPaused,
    RepositoryPaused,
    DispatchDisabled,
    IssueClosed,
    PullRequest,
    RequiredLabelMissing,
    UntrustedLabelActor,
    Excluded,
    Held,
    AlreadyOwned,
    RepositoryLimitReached,
    GlobalLimitReached,
}

impl IntakeBlocker {
    pub const ALL: [Self; 13] = [
        Self::IntakeDisabled,
        Self::GlobalPaused,
        Self::RepositoryPaused,
        Self::DispatchDisabled,
        Self::IssueClosed,
        Self::PullRequest,
        Self::RequiredLabelMissing,
        Self::UntrustedLabelActor,
        Self::Excluded,
        Self::Held,
        Self::AlreadyOwned,
        Self::RepositoryLimitReached,
        Self::GlobalLimitReached,
    ];
}

impl fmt::Display for IntakeBlocker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::IntakeDisabled => "INTAKE_DISABLED",
            Self::GlobalPaused => "GLOBAL_PAUSED",
            Self::RepositoryPaused => "REPOSITORY_PAUSED",
            Self::DispatchDisabled => "DISPATCH_DISABLED",
            Self::IssueClosed => "ISSUE_CLOSED",
            Self::PullRequest => "PULL_REQUEST",
            Self::RequiredLabelMissing => "REQUIRED_LABEL_MISSING",
            Self::UntrustedLabelActor => "UNTRUSTED_LABEL_ACTOR",
            Self::Excluded => "EXCLUDED",
            Self::Held => "HELD",
            Self::AlreadyOwned => "ALREADY_OWNED",
            Self::RepositoryLimitReached => "REPOSITORY_LIMIT_REACHED",
            Self::GlobalLimitReached => "GLOBAL_LIMIT_REACHED",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntakeDecision {
    Eligible,
    Ineligible(Vec<IntakeBlocker>),
}

#[must_use]
pub fn evaluate_intake(policy: &IntakePolicy, observation: &IssueObservation) -> IntakeDecision {
    let mut blockers = Vec::new();
    let mut block_if = |condition, blocker| {
        if condition {
            blockers.push(blocker);
        }
    };

    block_if(!policy.intake_enabled, IntakeBlocker::IntakeDisabled);
    block_if(policy.global_paused, IntakeBlocker::GlobalPaused);
    block_if(policy.repository_paused, IntakeBlocker::RepositoryPaused);
    block_if(!policy.dispatch_enabled, IntakeBlocker::DispatchDisabled);
    block_if(!observation.open, IntakeBlocker::IssueClosed);
    block_if(observation.is_pull_request, IntakeBlocker::PullRequest);
    let label_present = observation.labels.contains(&policy.required_label);
    block_if(!label_present, IntakeBlocker::RequiredLabelMissing);
    block_if(
        label_present
            && observation
                .latest_label_actor_id
                .is_none_or(|actor| !policy.trusted_actor_ids.contains(&actor)),
        IntakeBlocker::UntrustedLabelActor,
    );
    block_if(observation.excluded, IntakeBlocker::Excluded);
    block_if(observation.held, IntakeBlocker::Held);
    block_if(observation.already_owned, IntakeBlocker::AlreadyOwned);
    block_if(
        observation.repository_active_cases >= policy.repository_active_limit.get(),
        IntakeBlocker::RepositoryLimitReached,
    );
    block_if(
        observation.global_active_cases >= policy.global_active_limit.get(),
        IntakeBlocker::GlobalLimitReached,
    );

    if blockers.is_empty() {
        IntakeDecision::Eligible
    } else {
        IntakeDecision::Ineligible(blockers)
    }
}
