use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer as _, SigningKey};
use pip_control::{
    ArtifactManifest, ReleaseError, ReleaseManifest, ReleaseMetadata, create_release_manifest,
    resource_set_digest, sign_manifest, verify_release, verifying_key,
};
use sha2::{Digest, Sha256};

#[test]
fn signed_source_bound_release_verifies_every_regular_artifact() {
    let fixture = release_fixture();
    let verified = verify_release(
        fixture.root.path(),
        &fixture.manifest,
        &fixture.signature,
        &fixture.public_key,
    )
    .unwrap();
    assert_eq!(verified.source_commit(), "a".repeat(40));
    assert_eq!(
        verified.binary_path(),
        fixture
            .root
            .path()
            .canonicalize()
            .unwrap()
            .join("bin/pip-control")
    );
    assert_eq!(verified.artifact_count(), 2);
    assert_eq!(
        verified.manifest_sha256(),
        hex_digest(&Sha256::digest(&fixture.manifest))
    );
    let manifest: ReleaseManifest = serde_json::from_slice(&fixture.manifest).unwrap();
    assert_eq!(verified.binary_sha256(), manifest.binary_sha256);
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn signature_tampering_artifact_drift_and_symlinks_fail_closed() {
    let fixture = release_fixture();
    let mut changed_manifest = fixture.manifest.clone();
    let index = changed_manifest
        .windows("0.1.0".len())
        .position(|window| window == b"0.1.0")
        .unwrap();
    changed_manifest[index] = b'9';
    assert!(matches!(
        verify_release(
            fixture.root.path(),
            &changed_manifest,
            &fixture.signature,
            &fixture.public_key,
        ),
        Err(ReleaseError::InvalidSignature)
    ));

    let skill = fixture
        .root
        .path()
        .join("share/pip-v2/skills/planner/SKILL.md");
    fs::set_permissions(&skill, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(&skill, "drift\n").unwrap();
    fs::set_permissions(&skill, fs::Permissions::from_mode(0o444)).unwrap();
    assert!(matches!(
        verify_release(
            fixture.root.path(),
            &fixture.manifest,
            &fixture.signature,
            &fixture.public_key,
        ),
        Err(ReleaseError::ArtifactDigest { .. })
    ));

    let fixture = release_fixture();
    let skill = fixture
        .root
        .path()
        .join("share/pip-v2/skills/planner/SKILL.md");
    fs::remove_file(&skill).unwrap();
    symlink("../../../../bin/pip-control", &skill).unwrap();
    assert!(matches!(
        verify_release(
            fixture.root.path(),
            &fixture.manifest,
            &fixture.signature,
            &fixture.public_key,
        ),
        Err(ReleaseError::UnsafeArtifact { .. })
    ));
}

#[test]
fn duplicate_paths_unknown_fields_and_aggregate_resource_drift_are_rejected() {
    let fixture = release_fixture();
    let mut value: serde_json::Value = serde_json::from_slice(&fixture.manifest).unwrap();
    value["unexpected"] = serde_json::json!(true);
    assert!(matches!(
        verify_release(
            fixture.root.path(),
            &serde_json::to_vec(&value).unwrap(),
            &fixture.signature,
            &fixture.public_key,
        ),
        Err(ReleaseError::MalformedManifest(_)) | Err(ReleaseError::InvalidSignature)
    ));

    let artifacts = vec![
        ArtifactManifest {
            path: "share/a".into(),
            sha256: "b".repeat(64),
            mode: 0o444,
        },
        ArtifactManifest {
            path: "share/a".into(),
            sha256: "c".repeat(64),
            mode: 0o444,
        },
    ];
    assert!(matches!(
        resource_set_digest(&artifacts, "bin/pip-control"),
        Err(ReleaseError::DuplicateArtifact { .. })
    ));
}

#[test]
fn deterministic_manifest_creation_and_offline_signing_form_a_verifiable_cohort() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("bin")).unwrap();
    fs::create_dir_all(root.path().join("share/pip-v2/contracts")).unwrap();
    fs::write(root.path().join("bin/pip-control"), b"binary\n").unwrap();
    fs::write(
        root.path().join("share/pip-v2/contracts/planner.json"),
        b"{}\n",
    )
    .unwrap();
    fs::set_permissions(
        root.path().join("bin/pip-control"),
        fs::Permissions::from_mode(0o555),
    )
    .unwrap();
    fs::set_permissions(
        root.path().join("share/pip-v2/contracts/planner.json"),
        fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    let metadata = ReleaseMetadata {
        version: "0.1.0".into(),
        source_commit: "a".repeat(40),
        cargo_lock_sha256: "b".repeat(64),
        target: "x86_64-unknown-linux-gnu".into(),
        rust_toolchain: "rustc 1.96.1".into(),
        built_at: "2026-08-20T12:00:00Z".into(),
        builder_identity: "github-actions:pip-release".into(),
        workflow_version: 2,
        contract_version: 1,
    };
    let first = create_release_manifest(root.path(), &metadata).unwrap();
    let second = create_release_manifest(root.path(), &metadata).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        first
            .artifacts
            .iter()
            .map(|artifact| artifact.path.as_str())
            .collect::<Vec<_>>(),
        ["bin/pip-control", "share/pip-v2/contracts/planner.json"]
    );

    let bytes = serde_json::to_vec(&first).unwrap();
    let signing_key = STANDARD.encode([13_u8; 32]);
    let signature = sign_manifest(&bytes, &signing_key).unwrap();
    let public_key = verifying_key(&signing_key).unwrap();
    verify_release(root.path(), &bytes, &signature, &public_key).unwrap();

    let unsafe_file = root.path().join("share/pip-v2/contracts/unsafe.json");
    fs::write(&unsafe_file, b"{}\n").unwrap();
    fs::set_permissions(&unsafe_file, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        create_release_manifest(root.path(), &metadata),
        Err(ReleaseError::UnsafeArtifact { .. })
    ));
}

struct Fixture {
    root: tempfile::TempDir,
    manifest: Vec<u8>,
    signature: String,
    public_key: String,
}

fn release_fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("bin")).unwrap();
    fs::create_dir_all(root.path().join("share/pip-v2/skills/planner")).unwrap();
    let binary = root.path().join("bin/pip-control");
    let skill = root.path().join("share/pip-v2/skills/planner/SKILL.md");
    fs::write(&binary, b"fixture rust binary\n").unwrap();
    fs::write(&skill, b"# Planner fixture\n").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o555)).unwrap();
    fs::set_permissions(&skill, fs::Permissions::from_mode(0o444)).unwrap();

    let artifacts = vec![
        ArtifactManifest {
            path: "bin/pip-control".into(),
            sha256: digest(b"fixture rust binary\n"),
            mode: 0o555,
        },
        ArtifactManifest {
            path: "share/pip-v2/skills/planner/SKILL.md".into(),
            sha256: digest(b"# Planner fixture\n"),
            mode: 0o444,
        },
    ];
    let resources_sha256 = resource_set_digest(&artifacts, "bin/pip-control").unwrap();
    let manifest = ReleaseManifest {
        release_format: 1,
        version: "0.1.0".into(),
        source_commit: "a".repeat(40),
        cargo_lock_sha256: "b".repeat(64),
        target: "x86_64-unknown-linux-gnu".into(),
        rust_toolchain: "rustc 1.96.1".into(),
        binary_path: "bin/pip-control".into(),
        binary_sha256: artifacts[0].sha256.clone(),
        resources_sha256,
        workflow_version: 2,
        contract_version: 1,
        built_at: "2026-08-20T12:00:00Z".into(),
        builder_identity: "github-actions:pip-release".into(),
        artifacts,
    };
    let manifest = serde_json::to_vec(&manifest).unwrap();
    let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
    let signature = STANDARD.encode(signing_key.sign(&manifest).to_bytes());
    let public_key = STANDARD.encode(signing_key.verifying_key().to_bytes());
    Fixture {
        root,
        manifest,
        signature,
        public_key,
    }
}

fn digest(bytes: &[u8]) -> String {
    let value = Sha256::digest(bytes);
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}
