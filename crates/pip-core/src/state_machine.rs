use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CaseState {
    Planning,
    WaitingHuman,
    ReadyToBuild,
    Building,
    WaitingCi,
    Reviewing,
    Remediating,
    FinalReview,
    ShadowReady,
    ReadyToMerge,
    Merging,
    Completed,
    Blocked,
    Escalated,
    Abandoned,
    TakenOver,
}

impl CaseState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Abandoned | Self::TakenOver)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Event {
    PlanRecorded,
    Proceed,
    WaitingForIssueCreator,
    NeedsHumanScopeDecision,
    RootCauseDifferentScope,
    CrossRepoDependency,
    AlreadyFixed,
    NotReproducible,
    Duplicate,
    Abandon,
    HumanNarrowedScope,
    HumanApprovedStaleBase,
    HumanClarified,
    BuilderDispatched,
    BuilderRetryAuthorized,
    BuildRecorded,
    ReviewReady,
    ReturnToPlanning,
    Blocked,
    HumanReaffirmedScope,
    CiAccepted,
    CiFailed,
    ReviewRecorded,
    ReviewsApproved,
    ReviewsPublished,
    FinalPreflightAccepted,
    RequestChanges,
    Ready,
    ReturnToBuild,
    ReturnToReview,
    WaitForIssueCreator,
    HumanMerged,
    PrerequisiteResolved,
    BlockedUnexpectedModel,
    MergeStarted,
    MergeVerified,
    HumanTookOver,
    AuthorizationRemoved,
    OperationalBoundReached,
}

impl Event {
    pub const ALL: [Self; 39] = [
        Self::PlanRecorded,
        Self::Proceed,
        Self::WaitingForIssueCreator,
        Self::NeedsHumanScopeDecision,
        Self::RootCauseDifferentScope,
        Self::CrossRepoDependency,
        Self::AlreadyFixed,
        Self::NotReproducible,
        Self::Duplicate,
        Self::Abandon,
        Self::HumanNarrowedScope,
        Self::HumanApprovedStaleBase,
        Self::HumanClarified,
        Self::BuilderDispatched,
        Self::BuilderRetryAuthorized,
        Self::BuildRecorded,
        Self::ReviewReady,
        Self::ReturnToPlanning,
        Self::Blocked,
        Self::HumanReaffirmedScope,
        Self::CiAccepted,
        Self::CiFailed,
        Self::ReviewRecorded,
        Self::ReviewsApproved,
        Self::ReviewsPublished,
        Self::FinalPreflightAccepted,
        Self::RequestChanges,
        Self::Ready,
        Self::ReturnToBuild,
        Self::ReturnToReview,
        Self::WaitForIssueCreator,
        Self::HumanMerged,
        Self::PrerequisiteResolved,
        Self::BlockedUnexpectedModel,
        Self::MergeStarted,
        Self::MergeVerified,
        Self::HumanTookOver,
        Self::AuthorizationRemoved,
        Self::OperationalBoundReached,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseDomainValueError {
    kind: &'static str,
    value: String,
}

impl ParseDomainValueError {
    fn new(kind: &'static str, value: &str) -> Self {
        Self {
            kind,
            value: value.to_owned(),
        }
    }
}

impl fmt::Display for ParseDomainValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown {}: {:?}", self.kind, self.value)
    }
}

impl std::error::Error for ParseDomainValueError {}

macro_rules! string_enum {
    ($type:ty, $kind:literal, {$($wire:literal => $variant:ident),+ $(,)?}) => {
        impl FromStr for $type {
            type Err = ParseDomainValueError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($wire => Ok(Self::$variant),)+
                    _ => Err(ParseDomainValueError::new($kind, value)),
                }
            }
        }

        impl fmt::Display for $type {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                let value = match self {
                    $(Self::$variant => $wire,)+
                };
                formatter.write_str(value)
            }
        }
    };
}

string_enum!(CaseState, "case state", {
    "PLANNING" => Planning,
    "WAITING_HUMAN" => WaitingHuman,
    "READY_TO_BUILD" => ReadyToBuild,
    "BUILDING" => Building,
    "WAITING_CI" => WaitingCi,
    "REVIEWING" => Reviewing,
    "REMEDIATING" => Remediating,
    "FINAL_REVIEW" => FinalReview,
    "SHADOW_READY" => ShadowReady,
    "READY_TO_MERGE" => ReadyToMerge,
    "MERGING" => Merging,
    "COMPLETED" => Completed,
    "BLOCKED" => Blocked,
    "ESCALATED" => Escalated,
    "ABANDONED" => Abandoned,
    "TAKEN_OVER" => TakenOver,
});

string_enum!(Event, "event", {
    "PLAN_RECORDED" => PlanRecorded,
    "PROCEED" => Proceed,
    "WAITING_FOR_ISSUE_CREATOR" => WaitingForIssueCreator,
    "NEEDS_HUMAN_SCOPE_DECISION" => NeedsHumanScopeDecision,
    "ROOT_CAUSE_DIFFERENT_SCOPE" => RootCauseDifferentScope,
    "CROSS_REPO_DEPENDENCY" => CrossRepoDependency,
    "ALREADY_FIXED" => AlreadyFixed,
    "NOT_REPRODUCIBLE" => NotReproducible,
    "DUPLICATE" => Duplicate,
    "ABANDON" => Abandon,
    "HUMAN_NARROWED_SCOPE" => HumanNarrowedScope,
    "HUMAN_APPROVED_STALE_BASE" => HumanApprovedStaleBase,
    "HUMAN_CLARIFIED" => HumanClarified,
    "BUILDER_DISPATCHED" => BuilderDispatched,
    "BUILDER_RETRY_AUTHORIZED" => BuilderRetryAuthorized,
    "BUILD_RECORDED" => BuildRecorded,
    "REVIEW_READY" => ReviewReady,
    "RETURN_TO_PLANNING" => ReturnToPlanning,
    "BLOCKED" => Blocked,
    "HUMAN_REAFFIRMED_SCOPE" => HumanReaffirmedScope,
    "CI_ACCEPTED" => CiAccepted,
    "CI_FAILED" => CiFailed,
    "REVIEW_RECORDED" => ReviewRecorded,
    "REVIEWS_APPROVED" => ReviewsApproved,
    "REVIEWS_PUBLISHED" => ReviewsPublished,
    "FINAL_PREFLIGHT_ACCEPTED" => FinalPreflightAccepted,
    "REQUEST_CHANGES" => RequestChanges,
    "READY" => Ready,
    "RETURN_TO_BUILD" => ReturnToBuild,
    "RETURN_TO_REVIEW" => ReturnToReview,
    "WAIT_FOR_ISSUE_CREATOR" => WaitForIssueCreator,
    "HUMAN_MERGED" => HumanMerged,
    "PREREQUISITE_RESOLVED" => PrerequisiteResolved,
    "BLOCKED_UNEXPECTED_MODEL" => BlockedUnexpectedModel,
    "MERGE_STARTED" => MergeStarted,
    "MERGE_VERIFIED" => MergeVerified,
    "HUMAN_TOOK_OVER" => HumanTookOver,
    "AUTHORIZATION_REMOVED" => AuthorizationRemoved,
    "OPERATIONAL_BOUND_REACHED" => OperationalBoundReached,
});

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MergeMode {
    #[default]
    Shadow,
    Guarded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransitionContext {
    pub remediation_round: u32,
    pub max_remediation_rounds: u32,
    pub merge_mode: MergeMode,
}

impl Default for TransitionContext {
    fn default() -> Self {
        Self {
            remediation_round: 0,
            max_remediation_rounds: 3,
            merge_mode: MergeMode::Shadow,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Effect {
    DispatchPlanner,
    PublishPlan,
    DispatchBuilder,
    PublishDraftPullRequest,
    ObserveCi,
    DispatchReviewers,
    PublishReviews,
    ObserveFinalPreflight,
    DispatchFinalReviewer,
    HoldForHuman,
    NotifyShadowReady,
    BeginMerge,
    ExecuteMerge,
    RecordCompletion,
    RecordAbandonment,
    RecordBlock,
    RecordTakeover,
    Escalate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionDecision {
    pub next_state: CaseState,
    pub effects: Vec<Effect>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionError {
    InvalidLoopBound,
    TerminalState(CaseState),
    InvalidTransition { state: CaseState, event: Event },
}

impl fmt::Display for TransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLoopBound => {
                formatter.write_str("remediation loop bound must be positive")
            }
            Self::TerminalState(state) => {
                write!(formatter, "terminal state cannot transition: {state}")
            }
            Self::InvalidTransition { state, event } => {
                write!(formatter, "invalid transition: {state} + {event}")
            }
        }
    }
}

impl std::error::Error for TransitionError {}

fn decision(
    next_state: CaseState,
    effects: &[Effect],
) -> Result<TransitionDecision, TransitionError> {
    Ok(TransitionDecision {
        next_state,
        effects: effects.to_vec(),
    })
}

pub fn transition(
    state: CaseState,
    event: Event,
    context: TransitionContext,
) -> Result<TransitionDecision, TransitionError> {
    if context.max_remediation_rounds == 0 {
        return Err(TransitionError::InvalidLoopBound);
    }
    if state.is_terminal() {
        return Err(TransitionError::TerminalState(state));
    }
    if event == Event::HumanTookOver {
        return decision(CaseState::TakenOver, &[Effect::RecordTakeover]);
    }
    if event == Event::AuthorizationRemoved {
        return decision(CaseState::Abandoned, &[Effect::RecordAbandonment]);
    }
    if event == Event::BlockedUnexpectedModel {
        return decision(CaseState::Blocked, &[Effect::RecordBlock]);
    }
    if event == Event::OperationalBoundReached {
        return decision(CaseState::Escalated, &[Effect::Escalate]);
    }

    use CaseState as State;
    use Effect as Fx;
    use Event as Ev;

    match (state, event) {
        (State::Planning, Ev::PlanRecorded) => decision(State::Planning, &[Fx::PublishPlan]),
        (State::Planning, Ev::Proceed) => decision(State::ReadyToBuild, &[Fx::DispatchBuilder]),
        (
            State::Planning,
            Ev::WaitingForIssueCreator | Ev::NeedsHumanScopeDecision | Ev::RootCauseDifferentScope,
        ) => decision(State::WaitingHuman, &[Fx::HoldForHuman]),
        (State::Planning, Ev::CrossRepoDependency | Ev::Blocked) => {
            decision(State::Blocked, &[Fx::RecordBlock])
        }
        (State::Planning, Ev::AlreadyFixed | Ev::NotReproducible | Ev::Duplicate) => {
            decision(State::Completed, &[Fx::RecordCompletion])
        }
        (State::Planning, Ev::Abandon) => decision(State::Abandoned, &[Fx::RecordAbandonment]),
        (State::Planning, Ev::HumanNarrowedScope | Ev::HumanApprovedStaleBase)
        | (State::WaitingHuman | State::Escalated, Ev::HumanClarified) => {
            decision(State::Planning, &[Fx::DispatchPlanner])
        }
        (State::ReadyToBuild, Ev::BuilderDispatched) => decision(State::Building, &[]),
        (State::ReadyToBuild | State::Escalated, Ev::BuilderRetryAuthorized) => {
            decision(State::ReadyToBuild, &[Fx::DispatchBuilder])
        }
        (State::Building | State::Remediating, Ev::BuildRecorded) => {
            decision(state, &[Fx::PublishDraftPullRequest])
        }
        (State::Building | State::Remediating, Ev::ReviewReady) => {
            decision(State::WaitingCi, &[Fx::ObserveCi])
        }
        (State::Building | State::Remediating, Ev::ReturnToPlanning) => {
            decision(State::Planning, &[Fx::DispatchPlanner])
        }
        (State::Building, Ev::HumanReaffirmedScope) => {
            decision(State::ReadyToBuild, &[Fx::DispatchBuilder])
        }
        (State::WaitingCi, Ev::CiAccepted) => decision(State::Reviewing, &[Fx::DispatchReviewers]),
        (State::WaitingCi, Ev::CiFailed)
            if context.remediation_round >= context.max_remediation_rounds =>
        {
            decision(State::Escalated, &[Fx::Escalate])
        }
        (State::WaitingCi, Ev::CiFailed) => decision(State::Remediating, &[Fx::DispatchBuilder]),
        (State::Reviewing, Ev::ReviewRecorded) => decision(State::Reviewing, &[]),
        (State::Reviewing, Ev::ReviewsApproved) => {
            decision(State::FinalReview, &[Fx::PublishReviews])
        }
        (State::FinalReview, Ev::ReviewsPublished) => {
            decision(State::FinalReview, &[Fx::ObserveFinalPreflight])
        }
        (State::Remediating, Ev::ReviewsPublished) => {
            decision(State::Remediating, &[Fx::DispatchBuilder])
        }
        (State::FinalReview, Ev::FinalPreflightAccepted) => {
            decision(State::FinalReview, &[Fx::DispatchFinalReviewer])
        }
        (State::Reviewing, Ev::RequestChanges)
            if context.remediation_round >= context.max_remediation_rounds =>
        {
            decision(State::Escalated, &[Fx::PublishReviews, Fx::Escalate])
        }
        (State::Reviewing, Ev::RequestChanges) => {
            decision(State::Remediating, &[Fx::PublishReviews])
        }
        (State::FinalReview, Ev::Ready) if context.merge_mode == MergeMode::Shadow => {
            decision(State::ShadowReady, &[Fx::NotifyShadowReady])
        }
        (State::FinalReview, Ev::Ready) => decision(State::ReadyToMerge, &[Fx::BeginMerge]),
        (
            State::FinalReview,
            Ev::ReturnToBuild | Ev::ReturnToReview | Ev::ReturnToPlanning | Ev::WaitForIssueCreator,
        ) if context.remediation_round >= context.max_remediation_rounds => {
            decision(State::Escalated, &[Fx::Escalate])
        }
        (State::FinalReview, Ev::ReturnToBuild) => {
            decision(State::Remediating, &[Fx::DispatchBuilder])
        }
        (State::FinalReview, Ev::ReturnToReview) => {
            decision(State::Reviewing, &[Fx::DispatchReviewers])
        }
        (State::FinalReview, Ev::ReturnToPlanning) => {
            decision(State::Planning, &[Fx::DispatchPlanner])
        }
        (State::FinalReview, Ev::WaitForIssueCreator) => {
            decision(State::WaitingHuman, &[Fx::HoldForHuman])
        }
        (State::ShadowReady, Ev::HumanMerged) => {
            decision(State::Completed, &[Fx::RecordCompletion])
        }
        (State::ReadyToMerge, Ev::MergeStarted) if context.merge_mode == MergeMode::Guarded => {
            decision(State::Merging, &[Fx::ExecuteMerge])
        }
        (State::Merging, Ev::MergeVerified) if context.merge_mode == MergeMode::Guarded => {
            decision(State::Completed, &[Fx::RecordCompletion])
        }
        (State::Blocked, Ev::PrerequisiteResolved) => {
            decision(State::Planning, &[Fx::DispatchPlanner])
        }
        (
            State::Building
            | State::WaitingCi
            | State::Reviewing
            | State::Remediating
            | State::FinalReview
            | State::ShadowReady
            | State::ReadyToMerge
            | State::Merging,
            Ev::Blocked,
        ) => decision(State::Blocked, &[Fx::RecordBlock]),
        (State::Building | State::Remediating | State::FinalReview, Ev::Abandon) => {
            decision(State::Abandoned, &[Fx::RecordAbandonment])
        }
        _ => Err(TransitionError::InvalidTransition { state, event }),
    }
}
