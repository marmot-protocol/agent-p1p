//! Pip control-plane process and release lifecycle boundary.

#![forbid(unsafe_code)]

mod cli;
mod dispatch;
mod install;
mod intake;
mod policy;
mod release;
mod results;
mod shadow;

pub use cli::{CliError, run_cli};
pub use dispatch::{DispatchCycleError, DispatchCycleResult, dispatch_once, dispatch_once_with};
pub use install::{
    HostInstallOptions, InstallError, InstallFault, InstallLayout, InstallOutcome, InstallResult,
    install_host_release, install_release, install_release_pinned,
};
pub use intake::{ActiveIntakeError, ActiveIntakeReport, IntakeCandidateResult, reconcile_intake};
pub use policy::{
    IntakeConfiguration, MergeConfiguration, PolicyError, RepositoryIdentity, RepositoryPolicy,
    RoleConfiguration, load_repository_policy,
};
pub use release::{
    ArtifactManifest, ReleaseError, ReleaseManifest, ReleaseMetadata, VerifiedRelease,
    create_release_manifest, resource_set_digest, sign_manifest, verify_release, verifying_key,
};
pub use results::{
    ResultCycle, ResultCycleError, ingest_completed_once, ingest_completed_once_with,
};
pub use shadow::{IntakeSource, ShadowCandidate, ShadowError, ShadowReport, reconcile_read_only};
