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
fn workers_can_validate_results_without_ledger_or_provider_access() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("result.json");
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap();
    let run = |value: &serde_json::Value| {
        let bytes = serde_json::to_vec(value).unwrap();
        fs::write(&path, &bytes).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_pip-control"))
            .args(["validate-worker-result", "--input", path.to_str().unwrap()])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        output
    };
    for value in fixtures["results"].as_array().unwrap() {
        let output = run(value);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["workflow_authorized"], false);
    }
    let mut wrong = fixtures["results"][1].clone();
    wrong["local_checks"] = serde_json::json!({"passed":true});
    let output = run(&wrong);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("expected a sequence"));
    let mut wrong = fixtures["results"][1].clone();
    wrong["actual_model"] = serde_json::json!("cursor/auto");
    assert!(!run(&wrong).status.success());
}

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
    assert_eq!(value["ledger"]["schema_version"], 13);
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
fn status_can_inspect_one_case_or_attempt_without_creating_a_ledger() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("ledger.db");
    let mut store = Store::open(&database).unwrap();
    store
        .record_policy(&pip_store::PolicyInput {
            repository_id: 17,
            revision: 1,
            accepted_at: 1,
            payload: serde_json::json!({"fixture": true}),
        })
        .unwrap();
    store
        .create_case(&pip_store::NewCase {
            case_key: "repo:17#2@1".into(),
            repository_id: 17,
            issue_number: 2,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "PLANNING".into(),
            observed_at: 1,
            event: pip_store::EventInput {
                event_id: "authorized".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: serde_json::json!({}),
            },
            effects: vec![pip_store::EffectInput {
                effect_id: "worker".into(),
                effect_type: "RUN_DIRECT_WORKER".into(),
                payload: serde_json::json!({}),
            }],
        })
        .unwrap();
    let claimed = store.claim_effect("operator-test", 2, 60).unwrap().unwrap();
    let attempt = store.begin_direct_attempt(&claimed, "task-1", 2).unwrap();
    store
        .fail_direct_attempt(attempt, "operator-test", 3, "provider exited with 1")
        .unwrap();
    drop(store);
    let before = fs::read(&database).unwrap();
    let run = |selector: &str, value: &str| {
        pip_control::run_cli(
            [
                "status",
                "--database",
                database.to_str().unwrap(),
                selector,
                value,
            ]
            .map(str::to_owned),
        )
    };
    let case = run("--case", "repo:17#2@1").unwrap();
    assert_eq!(case["case"]["state"], "PLANNING");
    assert_eq!(case["history"]["events"][0]["event_id"], "authorized");
    let failed = run("--attempt", &attempt.to_string()).unwrap();
    assert_eq!(failed["attempt"]["status"], "FAILED");
    assert_eq!(failed["attempt"]["error"], "provider exited with 1");
    assert!(
        pip_control::run_cli(
            [
                "status",
                "--database",
                database.to_str().unwrap(),
                "--case",
                "repo:17#2@1",
                "--attempt",
                "1"
            ]
            .map(str::to_owned)
        )
        .is_err()
    );
    for (selector, value) in [
        ("--case", "missing"),
        ("--attempt", "0"),
        ("--attempt", "-1"),
        ("--attempt", "999"),
    ] {
        assert!(run(selector, value).is_err());
    }
    assert_eq!(fs::read(&database).unwrap(), before);
    let missing = directory.path().join("missing.db");
    assert!(
        pip_control::run_cli(
            [
                "status",
                "--database",
                missing.to_str().unwrap(),
                "--attempt",
                "1"
            ]
            .map(str::to_owned)
        )
        .is_err()
    );
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
fn paused_controller_reports_collection_failures_without_reading_credentials() {
    for initialized in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let policy = directory.path().join("policy.json");
        fs::write(
            &policy,
            include_bytes!("../../../config/target/repositories/mdk.json"),
        )
        .unwrap();
        let database = directory.path().join("must-not-exist.db");
        if initialized {
            Store::open(&database).unwrap();
        }
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

        assert_eq!(
            output.status.success(),
            !initialized,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["ok"], !initialized);
        assert_eq!(value["result"], "disabled");
        assert_eq!(database.exists(), initialized);
        if initialized {
            assert_eq!(value["direct_worker"]["result"], "error");
            assert_eq!(value["worker_result"]["result"], "idle");
        }
    }
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

#[test]
fn publication_retry_is_a_strict_offline_operator_command() {
    let args = [
        "authorize-publication-retry",
        "--policy",
        "/missing/policy",
        "--database",
        "/missing/ledger",
        "--direct-queue",
        "/missing/queue",
        "--case",
        "repo:42#1@1",
        "--expected-revision",
        "2",
        "--expected-head",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "--request-id",
        "sign-build",
        "--reason",
        "Sign accepted build",
    ];
    let error = pip_control::run_cli(args.into_iter().map(String::from))
        .unwrap_err()
        .to_string();
    if rustix::process::geteuid().as_raw() != 0 {
        assert!(error.contains("requires root"), "{error}");
    } else {
        assert!(error.contains("filesystem"), "{error}");
    }
}
