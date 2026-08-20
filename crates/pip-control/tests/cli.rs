use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer as _, SigningKey};
use pip_control::{ArtifactManifest, ReleaseManifest, resource_set_digest};
use pip_store::Store;
use sha2::{Digest, Sha256};

#[test]
fn status_reads_an_existing_ledger_without_mutating_it() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("ledger.db");
    let store = Store::open(&database).unwrap();
    drop(store);

    let output = Command::new(env!("CARGO_BIN_EXE_pip-control"))
        .args([
            "status",
            "--database",
            database.to_str().unwrap(),
            "--now",
            "1787220000",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["ledger"]["schema_version"], 1);
    assert_eq!(value["ledger"]["cases"], serde_json::json!([]));

    let missing = directory.path().join("missing.db");
    let output = Command::new(env!("CARGO_BIN_EXE_pip-control"))
        .args(["status", "--database", missing.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!missing.exists());
}

#[test]
fn verify_release_command_emits_machine_readable_provenance() {
    let directory = tempfile::tempdir().unwrap();
    let release = directory.path().join("release");
    fs::create_dir_all(release.join("bin")).unwrap();
    let binary = release.join("bin/pip-control");
    fs::write(&binary, b"fixture binary\n").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o555)).unwrap();
    let artifacts = vec![ArtifactManifest {
        path: "bin/pip-control".into(),
        sha256: digest(b"fixture binary\n"),
        mode: 0o555,
    }];
    let manifest = ReleaseManifest {
        release_format: 1,
        version: "0.1.0".into(),
        source_commit: "a".repeat(40),
        cargo_lock_sha256: "b".repeat(64),
        target: "x86_64-unknown-linux-gnu".into(),
        rust_toolchain: "rustc 1.96.1".into(),
        binary_path: "bin/pip-control".into(),
        binary_sha256: artifacts[0].sha256.clone(),
        resources_sha256: resource_set_digest(&artifacts, "bin/pip-control").unwrap(),
        workflow_version: 2,
        contract_version: 1,
        built_at: "2026-08-20T12:00:00Z".into(),
        builder_identity: "github-actions:pip-release".into(),
        artifacts,
    };
    let manifest = serde_json::to_vec(&manifest).unwrap();
    let signing_key = SigningKey::from_bytes(&[11_u8; 32]);
    let signature = STANDARD.encode(signing_key.sign(&manifest).to_bytes());
    let public_key = STANDARD.encode(signing_key.verifying_key().to_bytes());
    let manifest_path = directory.path().join("release-manifest.json");
    let signature_path = directory.path().join("release-manifest.sig");
    let public_key_path = directory.path().join("release-public.key");
    fs::write(&manifest_path, manifest).unwrap();
    fs::write(&signature_path, signature).unwrap();
    fs::write(&public_key_path, public_key).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_pip-control"))
        .args([
            "verify-release",
            "--release-root",
            release.to_str().unwrap(),
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--signature",
            signature_path.to_str().unwrap(),
            "--public-key",
            public_key_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["source_commit"], "a".repeat(40));
    assert_eq!(value["artifact_count"], 1);
}

fn digest(bytes: &[u8]) -> String {
    let value = Sha256::digest(bytes);
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}
