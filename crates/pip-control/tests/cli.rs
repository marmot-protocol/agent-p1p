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
    assert_eq!(value["ledger"]["schema_version"], 8);
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
    let manifest_sha256 = digest(&manifest);
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
    assert_eq!(value["manifest_sha256"], manifest_sha256);
    assert_eq!(value["binary_sha256"], digest(b"fixture binary\n"));
}

#[test]
fn privileged_install_command_requires_verified_digests_and_host_identity() {
    let error = pip_control::run_cli(
        [
            "install-release",
            "--cohort",
            "/cohort",
            "--public-key",
            "/key",
            "--install-root",
            "/opt/pip",
            "--config-root",
            "/etc/pip",
            "--unit-root",
            "/etc/systemd/system",
            "--state-root",
            "/var/lib/pip",
        ]
        .into_iter()
        .map(str::to_owned),
    )
    .unwrap_err();
    assert!(matches!(error, pip_control::CliError::Usage(_)));
}

#[test]
fn derive_public_key_reads_a_private_mode_signing_key() {
    let directory = tempfile::tempdir().unwrap();
    let signing_key = directory.path().join("release-signing.key");
    fs::write(&signing_key, STANDARD.encode([17_u8; 32])).unwrap();
    fs::set_permissions(&signing_key, fs::Permissions::from_mode(0o600)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_pip-control"))
        .args([
            "derive-public-key",
            "--signing-key",
            signing_key.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(
        value["public_key"],
        pip_control::verifying_key(&STANDARD.encode([17_u8; 32])).unwrap()
    );
}

#[test]
fn controller_cycle_is_inert_before_credentials_database_or_hermes_when_policy_is_paused() {
    let directory = tempfile::tempdir().unwrap();
    let policy = directory.path().join("policy.json");
    fs::write(
        &policy,
        include_bytes!("../../../config/target/repositories/mdk.json"),
    )
    .unwrap();
    let database = directory.path().join("must-not-exist.db");
    let output = Command::new(env!("CARGO_BIN_EXE_pip-control"))
        .args([
            "controller-cycle",
            "--policy",
            policy.to_str().unwrap(),
            "--database",
            database.to_str().unwrap(),
            "--github-token",
            directory.path().join("missing-token").to_str().unwrap(),
            "--github-reviewer-general-app",
            directory
                .path()
                .join("missing-general-app")
                .to_str()
                .unwrap(),
            "--github-reviewer-general-key",
            directory
                .path()
                .join("missing-general-key")
                .to_str()
                .unwrap(),
            "--github-reviewer-secperf-app",
            directory
                .path()
                .join("missing-secperf-app")
                .to_str()
                .unwrap(),
            "--github-reviewer-secperf-key",
            directory
                .path()
                .join("missing-secperf-key")
                .to_str()
                .unwrap(),
            "--git-askpass",
            directory.path().join("missing-askpass").to_str().unwrap(),
            "--hermes",
            "/missing/hermes",
            "--owner",
            "pip-controller",
            "--skills-commit-file",
            directory.path().join("missing-source").to_str().unwrap(),
            "--direct-queue",
            directory.path().join("missing-queue").to_str().unwrap(),
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
    assert_eq!(value["result"], "disabled");
    assert!(!database.exists());
}

#[test]
fn controller_cycle_rejects_duplicate_reviewer_apps_before_network_or_ledger() {
    let directory = tempfile::tempdir().unwrap();
    let policy_path = directory.path().join("policy.json");
    let mut policy: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    policy["intake"]["enabled"] = serde_json::json!(true);
    policy["intake"]["paused"] = serde_json::json!(false);
    policy["dispatch_enabled"] = serde_json::json!(true);
    policy["github"]["automation_actor_id"] = serde_json::json!(202880);
    policy["github"]["reviewer_general_actor_id"] = serde_json::json!(202881);
    policy["github"]["reviewer_secperf_actor_id"] = serde_json::json!(202882);
    fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();

    let controller_token = directory.path().join("github.token");
    let general_app = directory.path().join("general.app.json");
    let secperf_app = directory.path().join("secperf.app.json");
    let general_key = directory.path().join("general.pem");
    let secperf_key = directory.path().join("secperf.pem");
    let duplicate_app = serde_json::json!({
        "app_id": 123456,
        "installation_id": 987654,
        "repository_id": 1_055_628_515_u64,
    });
    fs::write(&controller_token, b"controller-token\n").unwrap();
    fs::write(&general_app, serde_json::to_vec(&duplicate_app).unwrap()).unwrap();
    fs::write(&secperf_app, serde_json::to_vec(&duplicate_app).unwrap()).unwrap();
    fs::write(
        &general_key,
        include_bytes!("../../pip-github/tests/fixtures/github-app-test-key.pem"),
    )
    .unwrap();
    fs::write(
        &secperf_key,
        include_bytes!("../../pip-github/tests/fixtures/github-app-test-key.pem"),
    )
    .unwrap();
    for secret in [&controller_token, &general_key, &secperf_key] {
        fs::set_permissions(secret, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let database = directory.path().join("ledger.db");

    let result = pip_control::run_cli(
        [
            "controller-cycle",
            "--policy",
            policy_path.to_str().unwrap(),
            "--database",
            database.to_str().unwrap(),
            "--github-token",
            controller_token.to_str().unwrap(),
            "--github-reviewer-general-app",
            general_app.to_str().unwrap(),
            "--github-reviewer-general-key",
            general_key.to_str().unwrap(),
            "--github-reviewer-secperf-app",
            secperf_app.to_str().unwrap(),
            "--github-reviewer-secperf-key",
            secperf_key.to_str().unwrap(),
            "--git-askpass",
            "/missing/askpass",
            "--hermes",
            "/missing/hermes",
            "--owner",
            "pip-controller-mdk",
            "--skills-commit-file",
            "/missing/source",
            "--direct-queue",
            "/missing/queue",
            "--now",
            "1788290400",
        ]
        .into_iter()
        .map(str::to_owned),
    );

    assert!(matches!(
        result,
        Err(pip_control::CliError::InvalidArgument(argument))
            if argument == "--github-reviewer-apps"
    ));
    assert!(!database.exists());
}

#[test]
fn webhook_intake_rejects_an_invalid_signature_without_recording_a_delivery() {
    let directory = tempfile::tempdir().unwrap();
    let policy_path = directory.path().join("policy.json");
    let mut policy: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    policy["intake"]["enabled"] = serde_json::json!(true);
    policy["intake"]["paused"] = serde_json::json!(false);
    policy["dispatch_enabled"] = serde_json::json!(true);
    policy["github"]["automation_actor_id"] = serde_json::json!(202880);
    policy["github"]["reviewer_general_actor_id"] = serde_json::json!(202881);
    policy["github"]["reviewer_secperf_actor_id"] = serde_json::json!(202882);
    fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let database = directory.path().join("ledger.db");
    let token = directory.path().join("github.token");
    let secret = directory.path().join("webhook.secret");
    let payload = directory.path().join("payload.json");
    fs::write(&token, b"not-used\n").unwrap();
    fs::write(&secret, b"webhook-secret\n").unwrap();
    fs::write(&payload, b"{}\n").unwrap();
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();

    let result = pip_control::run_cli(
        [
            "webhook-intake",
            "--policy",
            policy_path.to_str().unwrap(),
            "--database",
            database.to_str().unwrap(),
            "--github-token",
            token.to_str().unwrap(),
            "--webhook-secret",
            secret.to_str().unwrap(),
            "--payload",
            payload.to_str().unwrap(),
            "--delivery-id",
            "01234567-89ab-cdef-0123-456789abcdef",
            "--event",
            "issues",
            "--signature",
            "sha256=00",
            "--now",
            "1787220000",
        ]
        .into_iter()
        .map(str::to_owned),
    );

    assert!(matches!(
        result,
        Err(pip_control::CliError::Reconciliation(_))
    ));
    assert_eq!(
        Store::open(&database)
            .unwrap()
            .status(0)
            .unwrap()
            .webhook_deliveries,
        0
    );
}

#[test]
fn git_askpass_reads_the_systemd_credential_without_json_or_token_environment() {
    let directory = tempfile::tempdir().unwrap();
    let credential = directory.path().join("github.token");
    fs::write(&credential, b"github-secret-token\n").unwrap();
    fs::set_permissions(&credential, fs::Permissions::from_mode(0o400)).unwrap();

    let username = Command::new(env!("CARGO_BIN_EXE_pip-control"))
        .env_clear()
        .env("PIP_GIT_ASKPASS", "1")
        .env("PIP_GIT_TOKEN_FILE", &credential)
        .arg("Username for 'https://github.com': ")
        .output()
        .unwrap();
    assert!(username.status.success());
    assert_eq!(username.stdout, b"x-access-token\n");

    let password = Command::new(env!("CARGO_BIN_EXE_pip-control"))
        .env_clear()
        .env("PIP_GIT_ASKPASS", "1")
        .env("PIP_GIT_TOKEN_FILE", &credential)
        .arg("Password for 'https://x-access-token@github.com': ")
        .output()
        .unwrap();
    assert!(password.status.success());
    assert_eq!(password.stdout, b"github-secret-token\n");

    let rejected = Command::new(env!("CARGO_BIN_EXE_pip-control"))
        .env_clear()
        .env("PIP_GIT_ASKPASS", "1")
        .env("PIP_GIT_TOKEN_FILE", &credential)
        .arg("Unexpected prompt")
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
}

fn digest(bytes: &[u8]) -> String {
    let value = Sha256::digest(bytes);
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}
