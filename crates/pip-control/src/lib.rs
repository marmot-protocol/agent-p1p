//! Pip control-plane process and release lifecycle boundary.

#![forbid(unsafe_code)]

mod authorization;
mod ci;
mod cli;
mod dispatch;
mod final_preflight;
mod install;
mod intake;
mod policy;
mod release;
mod results;
mod shadow;
mod takeover;

pub use authorization::{
    ActiveAuthorization, AuthorizationBlock, AuthorizationError, reconcile_active_authorization,
    verify_active_authorization,
};
pub use ci::{CiCycle, CiCycleError, PullRequestSource, reconcile_ci_once};
pub use cli::{CliError, run_cli};
pub use dispatch::{
    DispatchCycleContext, DispatchCycleError, DispatchCycleResult, dispatch_once,
    dispatch_once_with,
};
pub use final_preflight::{
    FinalPreflightCycle, FinalPreflightError, FinalPreflightSource, reconcile_final_preflight_once,
};
pub use install::{
    HostInstallOptions, InstallError, InstallFault, InstallLayout, InstallOutcome, InstallResult,
    install_host_release, install_release, install_release_pinned,
};
pub use intake::{ActiveIntakeError, ActiveIntakeReport, IntakeCandidateResult, reconcile_intake};
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
pub use shadow::{IntakeSource, ShadowCandidate, ShadowError, ShadowReport, reconcile_read_only};
pub use takeover::{TakeoverCycle, TakeoverError, reconcile_takeover_once};
