//! Pip control-plane process and release lifecycle boundary.

#![forbid(unsafe_code)]

mod authorization;
mod ci;
mod cli;
mod direct_worker;
mod dispatch;
mod disposition;
mod draft_pr;
mod final_preflight;
mod install;
mod intake;
mod merge;
mod plans;
mod policy;
mod release;
mod results;
mod reviews;
mod runtime;
mod shadow;
mod takeover;
mod workspace;

pub use authorization::{
    ActiveAuthorization, AuthorizationBlock, AuthorizationError, reconcile_active_authorization,
    verify_active_authorization,
};
pub use ci::{CiCycle, CiCycleError, PullRequestSource, reconcile_ci_once};
pub use cli::{CliError, run_cli, run_git_askpass};
pub use direct_worker::{
    CursorDirectRuntime, DirectWorkerCycle, DirectWorkerCycleContext, DirectWorkerError,
    DirectWorkerRuntime, DirectWorkerRuntimeError, recommended_direct_lease_seconds,
    run_direct_worker_once_with,
};
pub use dispatch::{
    DispatchCycleContext, DispatchCycleError, DispatchCycleResult, dispatch_once,
    dispatch_once_with, dispatch_once_with_workspace,
};
pub use disposition::{
    DispositionCycle, DispositionError, DispositionWriter, consume_disposition_once,
};
pub use draft_pr::{
    BranchPublicationRequest, BranchPublisher, DraftPullRequestCycle, DraftPullRequestError,
    DraftPullRequestWriter, publish_draft_pull_request_once, publish_draft_pull_request_once_with,
};
pub use final_preflight::{
    FinalPreflightCycle, FinalPreflightError, FinalPreflightSource, reconcile_final_preflight_once,
};
pub use install::{
    HostInstallOptions, InstallError, InstallFault, InstallLayout, InstallOutcome, InstallResult,
    install_host_release, install_release, install_release_pinned,
};
pub use intake::{ActiveIntakeError, ActiveIntakeReport, IntakeCandidateResult, reconcile_intake};
pub use merge::{MergeCycle, MergeCycleError, MergeSource, MergeWriter, reconcile_merge_once};
pub use plans::{PlanPublicationCycle, PlanPublicationError, PlanWriter, publish_plan_once};
pub use policy::{
    GitHubConfiguration, IntakeConfiguration, MergeConfiguration, PolicyError, RepositoryIdentity,
    RepositoryPolicy, RoleConfiguration, load_repository_policy,
};
pub use release::{
    ArtifactManifest, ReleaseError, ReleaseManifest, ReleaseMetadata, VerifiedRelease,
    create_release_manifest, resource_set_digest, sign_manifest, verify_release, verifying_key,
};
pub use results::{
    ResultCycle, ResultCycleError, ingest_completed_once, ingest_completed_once_with,
};
pub use reviews::{
    ReviewPublicationCycle, ReviewPublicationError, ReviewWriter, publish_reviews_once,
};
pub use runtime::{RuntimeBootstrapError, bootstrap_hermes_runtime_with};
pub use shadow::{IntakeSource, ShadowCandidate, ShadowError, ShadowReport, reconcile_read_only};
pub use takeover::{TakeoverCycle, TakeoverError, reconcile_takeover_once};
pub use workspace::{GitWorkspacePreparer, WorkspaceError, WorkspacePreparer};
