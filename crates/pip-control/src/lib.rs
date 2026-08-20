//! Pip control-plane process and release lifecycle boundary.

#![forbid(unsafe_code)]

mod cli;
mod policy;
mod release;

pub use cli::{CliError, run_cli};
pub use policy::{
    IntakeConfiguration, MergeConfiguration, PolicyError, RepositoryIdentity, RepositoryPolicy,
    RoleConfiguration, load_repository_policy,
};
pub use release::{
    ArtifactManifest, ReleaseError, ReleaseManifest, ReleaseMetadata, VerifiedRelease,
    create_release_manifest, resource_set_digest, sign_manifest, verify_release, verifying_key,
};
