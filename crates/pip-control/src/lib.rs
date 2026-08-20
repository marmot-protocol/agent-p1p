//! Pip control-plane process and release lifecycle boundary.

#![forbid(unsafe_code)]

mod cli;
mod release;

pub use cli::{CliError, run_cli};
pub use release::{
    ArtifactManifest, ReleaseError, ReleaseManifest, VerifiedRelease, resource_set_digest,
    verify_release,
};
