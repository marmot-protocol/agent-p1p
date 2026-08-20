//! Signed, source-bound release cohort verification.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_ARTIFACTS: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactManifest {
    pub path: String,
    pub sha256: String,
    pub mode: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub release_format: u32,
    pub version: String,
    pub source_commit: String,
    pub cargo_lock_sha256: String,
    pub target: String,
    pub rust_toolchain: String,
    pub binary_path: String,
    pub binary_sha256: String,
    pub resources_sha256: String,
    pub workflow_version: u32,
    pub contract_version: u32,
    pub built_at: String,
    pub builder_identity: String,
    pub artifacts: Vec<ArtifactManifest>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseMetadata {
    pub version: String,
    pub source_commit: String,
    pub cargo_lock_sha256: String,
    pub target: String,
    pub rust_toolchain: String,
    pub built_at: String,
    pub builder_identity: String,
    pub workflow_version: u32,
    pub contract_version: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedRelease {
    source_commit: String,
    binary_path: PathBuf,
    binary_sha256: String,
    manifest_sha256: String,
    artifact_count: usize,
}

impl VerifiedRelease {
    #[must_use]
    pub fn source_commit(&self) -> &str {
        &self.source_commit
    }

    #[must_use]
    pub fn binary_path(&self) -> &Path {
        &self.binary_path
    }

    #[must_use]
    pub fn binary_sha256(&self) -> &str {
        &self.binary_sha256
    }

    #[must_use]
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    #[must_use]
    pub const fn artifact_count(&self) -> usize {
        self.artifact_count
    }
}

#[derive(Debug)]
pub enum ReleaseError {
    ManifestTooLarge,
    MalformedManifest(String),
    InvalidSignature,
    InvalidManifest(&'static str),
    DuplicateArtifact {
        path: String,
    },
    UnsafeArtifact {
        path: String,
    },
    MissingArtifact {
        path: String,
    },
    ArtifactDigest {
        path: String,
    },
    ArtifactMode {
        path: String,
        expected: u32,
        actual: u32,
    },
    Filesystem(String),
}

impl fmt::Display for ReleaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ManifestTooLarge => formatter.write_str("release manifest exceeded its bound"),
            Self::MalformedManifest(error) => {
                write!(formatter, "malformed release manifest: {error}")
            }
            Self::InvalidSignature => formatter.write_str("release manifest signature is invalid"),
            Self::InvalidManifest(message) => {
                write!(formatter, "invalid release manifest: {message}")
            }
            Self::DuplicateArtifact { path } => {
                write!(formatter, "duplicate release artifact: {path}")
            }
            Self::UnsafeArtifact { path } => write!(formatter, "unsafe release artifact: {path}"),
            Self::MissingArtifact { path } => {
                write!(formatter, "missing release artifact: {path}")
            }
            Self::ArtifactDigest { path } => {
                write!(formatter, "release artifact digest mismatch: {path}")
            }
            Self::ArtifactMode {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "release artifact mode mismatch for {path}: expected {expected:o}, got {actual:o}"
            ),
            Self::Filesystem(error) => write!(formatter, "release filesystem error: {error}"),
        }
    }
}

impl std::error::Error for ReleaseError {}

pub fn create_release_manifest(
    release_root: impl AsRef<Path>,
    metadata: &ReleaseMetadata,
) -> Result<ReleaseManifest, ReleaseError> {
    let root_input = release_root.as_ref();
    let root_metadata = fs::symlink_metadata(root_input)
        .map_err(|error| ReleaseError::Filesystem(error.to_string()))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(ReleaseError::InvalidManifest(
            "release root is not a real directory",
        ));
    }
    let root = root_input
        .canonicalize()
        .map_err(|error| ReleaseError::Filesystem(error.to_string()))?;
    let mut paths = Vec::new();
    collect_artifacts(&root, &root, &mut paths)?;
    paths.sort();
    if paths.is_empty() || paths.len() > MAX_ARTIFACTS {
        return Err(ReleaseError::InvalidManifest("invalid artifact cohort"));
    }
    let mut artifacts = Vec::with_capacity(paths.len());
    for path in paths {
        let relative = path
            .strip_prefix(&root)
            .map_err(|error| ReleaseError::Filesystem(error.to_string()))?;
        let relative = relative
            .to_str()
            .ok_or_else(|| ReleaseError::UnsafeArtifact {
                path: relative.to_string_lossy().into_owned(),
            })?
            .replace(std::path::MAIN_SEPARATOR, "/");
        let file_metadata = fs::symlink_metadata(&path)
            .map_err(|error| ReleaseError::Filesystem(error.to_string()))?;
        let mode = file_metadata.permissions().mode() & 0o7777;
        let artifact = ArtifactManifest {
            path: relative,
            sha256: digest_file(&path)?,
            mode,
        };
        validate_artifact_entry(&artifact)?;
        artifacts.push(artifact);
    }
    let binary_path = "bin/pip-control".to_owned();
    let binary_sha256 = artifacts
        .iter()
        .find(|artifact| artifact.path == binary_path)
        .map(|artifact| artifact.sha256.clone())
        .ok_or(ReleaseError::InvalidManifest(
            "release binary is missing from the cohort",
        ))?;
    let resources_sha256 = resource_set_digest(&artifacts, &binary_path)?;
    let manifest = ReleaseManifest {
        release_format: 1,
        version: metadata.version.clone(),
        source_commit: metadata.source_commit.clone(),
        cargo_lock_sha256: metadata.cargo_lock_sha256.clone(),
        target: metadata.target.clone(),
        rust_toolchain: metadata.rust_toolchain.clone(),
        binary_path,
        binary_sha256,
        resources_sha256,
        workflow_version: metadata.workflow_version,
        contract_version: metadata.contract_version,
        built_at: metadata.built_at.clone(),
        builder_identity: metadata.builder_identity.clone(),
        artifacts,
    };
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub fn sign_manifest(
    manifest_bytes: &[u8],
    signing_key_base64: &str,
) -> Result<String, ReleaseError> {
    if manifest_bytes.is_empty() || manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ReleaseError::ManifestTooLarge);
    }
    let signing_key = decode_signing_key(signing_key_base64)?;
    Ok(STANDARD.encode(signing_key.sign(manifest_bytes).to_bytes()))
}

pub fn verifying_key(signing_key_base64: &str) -> Result<String, ReleaseError> {
    let signing_key = decode_signing_key(signing_key_base64)?;
    Ok(STANDARD.encode(signing_key.verifying_key().to_bytes()))
}

pub fn verify_release(
    release_root: impl AsRef<Path>,
    manifest_bytes: &[u8],
    signature_base64: &str,
    public_key_base64: &str,
) -> Result<VerifiedRelease, ReleaseError> {
    if manifest_bytes.is_empty() || manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ReleaseError::ManifestTooLarge);
    }
    verify_signature(manifest_bytes, signature_base64, public_key_base64)?;
    let manifest: ReleaseManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|error| ReleaseError::MalformedManifest(error.to_string()))?;
    validate_manifest(&manifest)?;

    let root_input = release_root.as_ref();
    let root_metadata = fs::symlink_metadata(root_input)
        .map_err(|error| ReleaseError::Filesystem(error.to_string()))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(ReleaseError::InvalidManifest(
            "release root is not a real directory",
        ));
    }
    let root = root_input
        .canonicalize()
        .map_err(|error| ReleaseError::Filesystem(error.to_string()))?;

    let resources = resource_set_digest(&manifest.artifacts, &manifest.binary_path)?;
    if resources != manifest.resources_sha256 {
        return Err(ReleaseError::InvalidManifest(
            "aggregate resource digest does not match",
        ));
    }

    let mut binary_count = 0_usize;
    for artifact in &manifest.artifacts {
        if artifact.path == manifest.binary_path {
            binary_count += 1;
            if artifact.sha256 != manifest.binary_sha256 {
                return Err(ReleaseError::InvalidManifest(
                    "binary digest disagrees with its artifact entry",
                ));
            }
        }
        verify_artifact(&root, artifact)?;
    }
    if binary_count != 1 {
        return Err(ReleaseError::InvalidManifest(
            "manifest must contain exactly one binary entry",
        ));
    }

    Ok(VerifiedRelease {
        source_commit: manifest.source_commit,
        binary_path: root.join(manifest.binary_path),
        binary_sha256: manifest.binary_sha256,
        manifest_sha256: hex_digest(&Sha256::digest(manifest_bytes)),
        artifact_count: manifest.artifacts.len(),
    })
}

pub fn resource_set_digest(
    artifacts: &[ArtifactManifest],
    binary_path: &str,
) -> Result<String, ReleaseError> {
    if artifacts.is_empty() || artifacts.len() > MAX_ARTIFACTS || !valid_artifact_path(binary_path)
    {
        return Err(ReleaseError::InvalidManifest("invalid artifact cohort"));
    }
    let mut seen = BTreeSet::new();
    let mut resources = Vec::new();
    for artifact in artifacts {
        validate_artifact_entry(artifact)?;
        if !seen.insert(artifact.path.clone()) {
            return Err(ReleaseError::DuplicateArtifact {
                path: artifact.path.clone(),
            });
        }
        if artifact.path != binary_path {
            resources.push(artifact);
        }
    }
    resources.sort_by(|left, right| left.path.cmp(&right.path));
    let mut digest = Sha256::new();
    for artifact in resources {
        update_field(&mut digest, artifact.path.as_bytes());
        update_field(&mut digest, &artifact.mode.to_be_bytes());
        update_field(&mut digest, artifact.sha256.as_bytes());
    }
    Ok(hex_digest(&digest.finalize()))
}

fn verify_signature(
    manifest: &[u8],
    signature_base64: &str,
    public_key_base64: &str,
) -> Result<(), ReleaseError> {
    let signature = STANDARD
        .decode(signature_base64.trim())
        .map_err(|_| ReleaseError::InvalidSignature)?;
    let public_key = STANDARD
        .decode(public_key_base64.trim())
        .map_err(|_| ReleaseError::InvalidSignature)?;
    let signature =
        Signature::from_slice(&signature).map_err(|_| ReleaseError::InvalidSignature)?;
    let public_key: [u8; 32] = public_key
        .try_into()
        .map_err(|_| ReleaseError::InvalidSignature)?;
    let verifying_key =
        VerifyingKey::from_bytes(&public_key).map_err(|_| ReleaseError::InvalidSignature)?;
    verifying_key
        .verify_strict(manifest, &signature)
        .map_err(|_| ReleaseError::InvalidSignature)
}

fn decode_signing_key(value: &str) -> Result<SigningKey, ReleaseError> {
    let bytes = STANDARD
        .decode(value.trim())
        .map_err(|_| ReleaseError::InvalidSignature)?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| ReleaseError::InvalidSignature)?;
    Ok(SigningKey::from_bytes(&bytes))
}

fn collect_artifacts(
    root: &Path,
    directory: &Path,
    artifacts: &mut Vec<PathBuf>,
) -> Result<(), ReleaseError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| ReleaseError::Filesystem(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ReleaseError::Filesystem(error.to_string()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| ReleaseError::Filesystem(error.to_string()))?;
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        if metadata.file_type().is_symlink() {
            return Err(ReleaseError::UnsafeArtifact { path: relative });
        }
        if metadata.is_dir() {
            collect_artifacts(root, &path, artifacts)?;
        } else if metadata.is_file() {
            artifacts.push(path);
        } else {
            return Err(ReleaseError::UnsafeArtifact { path: relative });
        }
    }
    Ok(())
}

fn validate_manifest(manifest: &ReleaseManifest) -> Result<(), ReleaseError> {
    if manifest.release_format != 1
        || !valid_identifier(&manifest.version, 128)
        || !valid_sha(&manifest.source_commit, 40)
        || !valid_sha(&manifest.cargo_lock_sha256, 64)
        || !valid_identifier(&manifest.target, 256)
        || !valid_text(&manifest.rust_toolchain, 512)
        || !valid_artifact_path(&manifest.binary_path)
        || !valid_sha(&manifest.binary_sha256, 64)
        || !valid_sha(&manifest.resources_sha256, 64)
        || manifest.workflow_version == 0
        || manifest.contract_version == 0
        || !valid_timestamp(&manifest.built_at)
        || !valid_text(&manifest.builder_identity, 512)
        || manifest.artifacts.is_empty()
        || manifest.artifacts.len() > MAX_ARTIFACTS
    {
        return Err(ReleaseError::InvalidManifest(
            "one or more required release identities are invalid",
        ));
    }
    Ok(())
}

fn validate_artifact_entry(artifact: &ArtifactManifest) -> Result<(), ReleaseError> {
    if !valid_artifact_path(&artifact.path)
        || !valid_sha(&artifact.sha256, 64)
        || !matches!(artifact.mode, 0o444 | 0o555)
    {
        return Err(ReleaseError::UnsafeArtifact {
            path: artifact.path.clone(),
        });
    }
    Ok(())
}

fn verify_artifact(root: &Path, artifact: &ArtifactManifest) -> Result<(), ReleaseError> {
    validate_artifact_entry(artifact)?;
    let mut current = root.to_owned();
    for component in Path::new(&artifact.path).components() {
        let Component::Normal(segment) = component else {
            return Err(ReleaseError::UnsafeArtifact {
                path: artifact.path.clone(),
            });
        };
        current.push(segment);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ReleaseError::MissingArtifact {
                    path: artifact.path.clone(),
                });
            }
            Err(error) => return Err(ReleaseError::Filesystem(error.to_string())),
        };
        if metadata.file_type().is_symlink() {
            return Err(ReleaseError::UnsafeArtifact {
                path: artifact.path.clone(),
            });
        }
    }
    let metadata = fs::symlink_metadata(&current)
        .map_err(|error| ReleaseError::Filesystem(error.to_string()))?;
    if !metadata.is_file() {
        return Err(ReleaseError::UnsafeArtifact {
            path: artifact.path.clone(),
        });
    }
    let actual_mode = metadata.permissions().mode() & 0o7777;
    if actual_mode != artifact.mode {
        return Err(ReleaseError::ArtifactMode {
            path: artifact.path.clone(),
            expected: artifact.mode,
            actual: actual_mode,
        });
    }
    if digest_file(&current)? != artifact.sha256 {
        return Err(ReleaseError::ArtifactDigest {
            path: artifact.path.clone(),
        });
    }
    Ok(())
}

fn digest_file(path: &Path) -> Result<String, ReleaseError> {
    let mut file = File::open(path).map_err(|error| ReleaseError::Filesystem(error.to_string()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| ReleaseError::Filesystem(error.to_string()))?;
        if read == 0 {
            return Ok(hex_digest(&digest.finalize()));
        }
        digest.update(&buffer[..read]);
    }
}

fn valid_artifact_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && Path::new(value).components().all(|component| {
            matches!(component, Component::Normal(segment) if {
                let segment = segment.as_encoded_bytes();
                !segment.is_empty()
                    && segment.iter().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                    })
            })
        })
}

fn valid_sha(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max
        && value.chars().all(|character| !character.is_control())
}

fn valid_timestamp(value: &str) -> bool {
    valid_text(value, 64)
        && value.ends_with('Z')
        && value.as_bytes().get(10) == Some(&b'T')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'-' | b':' | b'.' | b'T' | b'Z'))
}

fn update_field(digest: &mut Sha256, value: &[u8]) {
    digest.update(value.len().to_be_bytes());
    digest.update(value);
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(char::from(HEX[usize::from(byte >> 4)]));
        result.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    result
}
