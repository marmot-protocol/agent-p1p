//! Pip control-plane process and release lifecycle boundary.

#![forbid(unsafe_code)]

mod release;

pub use release::{
    ArtifactManifest, ReleaseError, ReleaseManifest, VerifiedRelease, resource_set_digest,
    verify_release,
};
