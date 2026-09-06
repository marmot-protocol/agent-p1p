use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pip_control::{
    HostInstallOptions, InstallFault, InstallLayout, InstallResult, ReleaseManifest,
    ReleaseMetadata, create_release_manifest, install_host_release, install_release,
    install_release_pinned, sign_manifest, verifying_key,
};
use pip_store::Store;
use rusqlite::Connection;

#[test]
fn host_state_root_allows_worker_group_traversal_without_directory_listing() {
    let installer = include_str!("../../../scripts/install-rust-control-plane.sh");
    assert!(installer.contains("chmod 0710 \"$state_root\""));
    assert!(installer.contains("ensure_directory /var/lib/pip pip-control pip-control 710"));
    assert!(!installer.contains("ensure_directory /var/lib/pip pip-control pip-control 700"));
}

#[test]
fn installed_release_permissions_are_independent_of_umask() {
    const CHILD: &str = "PIP_INSTALL_UMASK_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // umask is process-global: isolate each mask from parallel Rust tests.
        for mask in ["077", "000"] {
            let output = Command::new("sh")
                .args([
                    "-c",
                    "umask \"$1\"; shift; exec \"$@\"",
                    "pip-umask-test",
                    mask,
                ])
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "installed_release_permissions_are_independent_of_umask",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "umask {mask}:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    }

    let sandbox = tempfile::tempdir().unwrap();
    let layout = layout(sandbox.path());
    prepare_layout(&layout);
    fs::set_permissions(&layout.state_root, fs::Permissions::from_mode(0o700)).unwrap();
    let key = STANDARD.encode([47_u8; 32]);
    let public = verifying_key(&key).unwrap();
    for (version, source) in [("v1", "a"), ("v2", "b")] {
        let cohort = cohort(sandbox.path(), version, version.as_bytes(), source, &key);
        let installed = install_release(&cohort, &public, &layout, None).unwrap();
        assert_eq!(installed.result, InstallResult::Installed);
        assert_public_release_directories(&layout.install_root.join("releases"));
        let release = fs::read_link(layout.install_root.join("current")).unwrap();
        assert_eq!(mode(&release.join("bin/pip-control")), 0o555);
        assert_eq!(mode(&release.join("SOURCE.COMMIT")), 0o444);
        assert_eq!(
            mode(&release.join("share/pip/config/repositories/mdk.json")),
            0o444
        );
        assert_eq!(mode(&layout.state_root), 0o700);
        assert_eq!(mode(&layout.state_root.join("ledger.db")), 0o600);
        assert_eq!(
            install_release(&cohort, &public, &layout, None)
                .unwrap()
                .result,
            InstallResult::Existing
        );
    }
}

#[test]
fn reinstall_rejects_directory_permission_drift_without_mutation() {
    for relative in ["..", "", "bin", "share/pip/config/repositories"] {
        for bad_mode in [0o700, 0o777] {
            let sandbox = tempfile::tempdir().unwrap();
            let layout = layout(sandbox.path());
            prepare_layout(&layout);
            let key = STANDARD.encode([49_u8; 32]);
            let public = verifying_key(&key).unwrap();
            let cohort = cohort(sandbox.path(), "v1", b"binary-v1", "a", &key);
            install_release(&cohort, &public, &layout, None).unwrap();
            let release = fs::read_link(layout.install_root.join("current")).unwrap();
            let directory = release.join(relative);
            fs::set_permissions(&directory, fs::Permissions::from_mode(bad_mode)).unwrap();
            let ledger = fs::read(layout.state_root.join("ledger.db")).unwrap();
            assert!(
                install_release(&cohort, &public, &layout, None).is_err(),
                "accepted {relative:?} mode {bad_mode:o}"
            );
            assert_eq!(
                fs::read_link(layout.install_root.join("current")).unwrap(),
                release
            );
            assert_eq!(
                fs::read(layout.state_root.join("ledger.db")).unwrap(),
                ledger
            );
            assert_eq!(mode(&directory), bad_mode);
        }
    }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

fn assert_public_release_directories(path: &Path) {
    assert_eq!(mode(path), 0o755, "{}", path.display());
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            assert_public_release_directories(&entry.path());
        }
    }
}

#[test]
fn upgrades_and_reinstalls_preserve_valid_operator_policy() {
    let sandbox = tempfile::tempdir().unwrap();
    let layout = layout(sandbox.path());
    prepare_layout(&layout);
    let key = STANDARD.encode([19_u8; 32]);
    let public = verifying_key(&key).unwrap();
    let v1 = cohort(sandbox.path(), "v1", b"binary-v1\n", "a", &key);
    let v2 = cohort(sandbox.path(), "v2", b"binary-v2\n", "b", &key);
    install_release(&v1, &public, &layout, None).unwrap();
    let path = layout.config_root.join("repositories/mdk.json");
    let mut policy: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    policy["revision"] = serde_json::json!(123);
    policy["intake"]["enabled"] = serde_json::json!(true);
    policy["intake"]["paused"] = serde_json::json!(false);
    policy["dispatch_enabled"] = serde_json::json!(true);
    policy["github"]["automation_actor_id"] = serde_json::json!(1);
    policy["github"]["reviewer_general_actor_id"] = serde_json::json!(2);
    policy["github"]["reviewer_secperf_actor_id"] = serde_json::json!(3);
    let accepted = serde_json::to_vec(&policy).unwrap();
    pip_control::load_repository_policy(&accepted).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&path, &accepted).unwrap();
    for release in [&v1, &v2, &v2] {
        install_release(release, &public, &layout, None).unwrap();
        assert!(
            fs::read(&path).unwrap() == accepted,
            "operator policy was overwritten"
        );
    }
    // Invalid existing state is an explicit error, never silently replaced by
    // packaged defaults. Historical state and the installed release stay put.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&path, b"invalid policy").unwrap();
    assert!(install_release(&v1, &public, &layout, None).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"invalid policy");
    assert_eq!(current_source(&layout), "b".repeat(40));
}

#[test]
fn clean_install_reinstall_and_upgrade_are_content_addressed_and_paused() {
    let sandbox = tempfile::tempdir().unwrap();
    let layout = layout(sandbox.path());
    prepare_layout(&layout);
    let key = STANDARD.encode([19_u8; 32]);
    let public = verifying_key(&key).unwrap();
    let v1 = cohort(sandbox.path(), "v1", b"binary-v1\n", "a", &key);
    let v2 = cohort(sandbox.path(), "v2", b"binary-v2\n", "b", &key);

    let first = install_release(&v1, &public, &layout, None).unwrap();
    assert_eq!(first.result, InstallResult::Installed);
    assert_eq!(current_source(&layout), "a".repeat(40));
    let policy = fs::read_to_string(layout.config_root.join("repositories/mdk.json")).unwrap();
    assert!(policy.contains(r#""enabled": false"#));
    assert!(policy.contains(r#""dispatch_enabled": false"#));
    assert!(!layout.unit_root.join("timers.target.wants").exists());
    assert!(layout.unit_root.join("pip-controller@.service").is_file());
    assert!(layout.unit_root.join("pip-controller@.timer").is_file());
    assert!(
        layout
            .unit_root
            .join("pip-hermes-gateway.service")
            .is_file()
    );
    assert!(
        layout
            .unit_root
            .join("pip-webhook-ingress.service")
            .is_file()
    );
    assert!(
        layout
            .unit_root
            .join("pip-webhook-consumer@.service")
            .is_file()
    );
    assert!(
        layout
            .unit_root
            .join("pip-webhook-consumer@.timer")
            .is_file()
    );

    let replay = install_release(&v1, &public, &layout, None).unwrap();
    assert_eq!(replay.result, InstallResult::Existing);
    assert_eq!(replay.release_id, first.release_id);

    let upgraded = install_release(&v2, &public, &layout, None).unwrap();
    assert_eq!(upgraded.result, InstallResult::Installed);
    assert_ne!(upgraded.release_id, first.release_id);
    assert_eq!(current_source(&layout), "b".repeat(40));
    assert!(
        layout
            .install_root
            .join("releases")
            .join(first.release_id)
            .is_dir()
    );
}

#[test]
fn upgrade_snapshots_schema_seven_before_migrating_to_eight() {
    let sandbox = tempfile::tempdir().unwrap();
    let layout = layout(sandbox.path());
    prepare_layout(&layout);
    let key = STANDARD.encode([43_u8; 32]);
    let public = verifying_key(&key).unwrap();
    let v1 = cohort(sandbox.path(), "v1", b"binary-v1\n", "a", &key);
    let v2 = cohort(sandbox.path(), "v2", b"binary-v2\n", "b", &key);
    install_release(&v1, &public, &layout, None).unwrap();

    let ledger = layout.state_root.join("ledger.db");
    let connection = Connection::open(&ledger).unwrap();
    connection
        .execute_batch(
            "DROP TABLE dispatch_create_attempts;
             DROP TABLE dispatch_batches;
             DELETE FROM schema_migrations WHERE version = 8;
             PRAGMA user_version = 7;",
        )
        .unwrap();
    drop(connection);

    let upgraded = install_release(&v2, &public, &layout, None).unwrap();
    assert_eq!(upgraded.result, InstallResult::Installed);
    assert_eq!(current_source(&layout), "b".repeat(40));
    assert_eq!(
        Store::open_read_only(&ledger)
            .unwrap()
            .schema_version()
            .unwrap(),
        8
    );
}

#[test]
fn every_injected_install_failure_restores_the_complete_preinstall_snapshot() {
    for fault in InstallFault::ALL {
        let sandbox = tempfile::tempdir().unwrap();
        let layout = layout(sandbox.path());
        prepare_layout(&layout);
        let key = STANDARD.encode([23_u8; 32]);
        let public = verifying_key(&key).unwrap();
        let v1 = cohort(sandbox.path(), "v1", b"binary-v1\n", "a", &key);
        assert!(install_release(&v1, &public, &layout, Some(fault)).is_err());
        assert!(!layout.install_root.join("current").exists());
        assert!(!layout.config_root.join("repositories/mdk.json").exists());
        assert!(
            !layout
                .unit_root
                .join("pip-shadow-reconcile.service")
                .exists()
        );
        assert!(!layout.unit_root.join("pip-controller@.service").exists());
        assert!(!layout.unit_root.join("pip-controller@.timer").exists());
        assert!(!layout.unit_root.join("pip-hermes-gateway.service").exists());
        assert!(
            !layout
                .unit_root
                .join("pip-webhook-ingress.service")
                .exists()
        );
        assert!(
            !layout
                .unit_root
                .join("pip-webhook-consumer@.service")
                .exists()
        );
        assert!(
            !layout
                .unit_root
                .join("pip-webhook-consumer@.timer")
                .exists()
        );
        assert!(!layout.unit_root.join("pip-shadow-reconcile.timer").exists());
        assert!(!layout.state_root.join("ledger.db").exists());
    }
}

#[test]
fn every_injected_upgrade_failure_restores_the_previous_release_and_ledger() {
    for fault in InstallFault::ALL {
        let sandbox = tempfile::tempdir().unwrap();
        let layout = layout(sandbox.path());
        prepare_layout(&layout);
        let key = STANDARD.encode([29_u8; 32]);
        let public = verifying_key(&key).unwrap();
        let v1 = cohort(sandbox.path(), "v1", b"binary-v1\n", "a", &key);
        let v2 = cohort(sandbox.path(), "v2", b"binary-v2\n", "b", &key);
        let installed = install_release(&v1, &public, &layout, None).unwrap();
        let policy_before = fs::read(layout.config_root.join("repositories/mdk.json")).unwrap();
        let ledger_before = Store::open_read_only(layout.state_root.join("ledger.db"))
            .unwrap()
            .status(1_787_220_000)
            .unwrap();

        assert!(install_release(&v2, &public, &layout, Some(fault)).is_err());
        assert_eq!(current_source(&layout), "a".repeat(40));
        assert_eq!(
            fs::read(layout.config_root.join("repositories/mdk.json")).unwrap(),
            policy_before
        );
        let ledger_after = Store::open_read_only(layout.state_root.join("ledger.db"))
            .unwrap()
            .status(1_787_220_000)
            .unwrap();
        assert_eq!(ledger_after, ledger_before);
        let releases = fs::read_dir(layout.install_root.join("releases"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(
            releases[0].file_name().to_string_lossy(),
            installed.release_id
        );
    }
}

#[test]
fn pinned_install_rejects_a_digest_not_obtained_from_signed_verification() {
    let sandbox = tempfile::tempdir().unwrap();
    let layout = layout(sandbox.path());
    prepare_layout(&layout);
    let key = STANDARD.encode([31_u8; 32]);
    let public = verifying_key(&key).unwrap();
    let release = cohort(sandbox.path(), "v1", b"binary-v1\n", "a", &key);
    let manifest: ReleaseManifest =
        serde_json::from_slice(&fs::read(release.join("release-manifest.json")).unwrap()).unwrap();

    assert!(
        install_release_pinned(
            &release,
            &public,
            &layout,
            &"0".repeat(64),
            &manifest.binary_sha256,
        )
        .is_err()
    );
    assert!(!layout.install_root.join("current").exists());
}

#[test]
fn failed_host_finalization_restores_files_and_prior_timer_state() {
    let sandbox = tempfile::tempdir().unwrap();
    let layout = layout(sandbox.path());
    prepare_layout(&layout);
    let key = STANDARD.encode([37_u8; 32]);
    let public = verifying_key(&key).unwrap();
    let v1 = cohort(sandbox.path(), "v1", b"binary-v1\n", "a", &key);
    let v2 = cohort(sandbox.path(), "v2", b"binary-v2\n", "b", &key);
    install_release(&v1, &public, &layout, None).unwrap();
    let systemctl = fake_systemctl(sandbox.path(), true, true, true);
    let owner = fs::metadata(&layout.state_root).unwrap();
    let (manifest_sha256, binary_sha256) = cohort_digests(&v2);

    assert!(
        install_host_release(
            &v2,
            &public,
            &layout,
            &manifest_sha256,
            &binary_sha256,
            &HostInstallOptions {
                systemctl,
                state_uid: owner.uid(),
                state_gid: owner.gid(),
            },
        )
        .is_err()
    );
    assert_eq!(current_source(&layout), "a".repeat(40));
    assert_eq!(
        fs::read_to_string(sandbox.path().join("enabled")).unwrap(),
        "1"
    );
    assert_eq!(
        fs::read_to_string(sandbox.path().join("active")).unwrap(),
        "1"
    );
}

#[test]
fn failed_fresh_host_finalization_does_not_restore_absent_units() {
    let sandbox = tempfile::tempdir().unwrap();
    let layout = layout(sandbox.path());
    prepare_layout(&layout);
    let key = STANDARD.encode([39_u8; 32]);
    let public = verifying_key(&key).unwrap();
    let release = cohort(sandbox.path(), "v1", b"binary-v1\n", "a", &key);
    let systemctl = fake_systemctl(sandbox.path(), false, false, true);
    let owner = fs::metadata(&layout.state_root).unwrap();
    let (manifest_sha256, binary_sha256) = cohort_digests(&release);

    let error = install_host_release(
        &release,
        &public,
        &layout,
        &manifest_sha256,
        &binary_sha256,
        &HostInstallOptions {
            systemctl,
            state_uid: owner.uid(),
            state_gid: owner.gid(),
        },
    )
    .unwrap_err();

    assert!(!matches!(error, pip_control::InstallError::Rollback(_)));
    assert!(!layout.install_root.join("current").exists());
    assert!(
        !layout
            .unit_root
            .join("pip-webhook-ingress.service")
            .exists()
    );
}

#[test]
fn successful_host_install_preserves_a_fresh_disabled_timer() {
    let sandbox = tempfile::tempdir().unwrap();
    let layout = layout(sandbox.path());
    prepare_layout(&layout);
    let key = STANDARD.encode([41_u8; 32]);
    let public = verifying_key(&key).unwrap();
    let release = cohort(sandbox.path(), "v1", b"binary-v1\n", "a", &key);
    let systemctl = fake_systemctl(sandbox.path(), false, false, false);
    let owner = fs::metadata(&layout.state_root).unwrap();
    let (manifest_sha256, binary_sha256) = cohort_digests(&release);

    let outcome = install_host_release(
        &release,
        &public,
        &layout,
        &manifest_sha256,
        &binary_sha256,
        &HostInstallOptions {
            systemctl,
            state_uid: owner.uid(),
            state_gid: owner.gid(),
        },
    )
    .unwrap();
    assert_eq!(outcome.result, InstallResult::Installed);
    assert_eq!(
        fs::read_to_string(sandbox.path().join("enabled")).unwrap(),
        "0"
    );
    assert_eq!(
        fs::read_to_string(sandbox.path().join("active")).unwrap(),
        "0"
    );
    let ledger = fs::metadata(layout.state_root.join("ledger.db")).unwrap();
    assert_eq!((ledger.uid(), ledger.gid()), (owner.uid(), owner.gid()));
}

#[test]
fn host_upgrade_quiesces_and_restores_every_runtime_entrypoint() {
    let sandbox = tempfile::tempdir().unwrap();
    let layout = layout(sandbox.path());
    prepare_layout(&layout);
    let key = STANDARD.encode([43_u8; 32]);
    let public = verifying_key(&key).unwrap();
    let v1 = cohort(sandbox.path(), "v1", b"binary-v1\n", "a", &key);
    let v2 = cohort(sandbox.path(), "v2", b"binary-v2\n", "b", &key);
    install_release(&v1, &public, &layout, None).unwrap();
    let systemctl = fake_systemctl(sandbox.path(), true, true, false);
    let owner = fs::metadata(&layout.state_root).unwrap();
    let (manifest_sha256, binary_sha256) = cohort_digests(&v2);

    install_host_release(
        &v2,
        &public,
        &layout,
        &manifest_sha256,
        &binary_sha256,
        &HostInstallOptions {
            systemctl,
            state_uid: owner.uid(),
            state_gid: owner.gid(),
        },
    )
    .unwrap();

    let calls = fs::read_to_string(sandbox.path().join("systemctl.calls")).unwrap();
    for unit in [
        "pip-shadow-reconcile.timer",
        "pip-controller@mdk.timer",
        "pip-direct-worker@mdk.timer",
        "pip-hermes-gateway.service",
        "pip-webhook-ingress.service",
        "pip-webhook-consumer@mdk.timer",
    ] {
        for command in ["is-enabled", "is-active", "stop", "enable", "start"] {
            assert!(
                calls
                    .lines()
                    .any(|line| line == format!("{command} {unit}")),
                "missing {command} lifecycle call for {unit}:\n{calls}"
            );
        }
    }
}

fn layout(root: &Path) -> InstallLayout {
    InstallLayout {
        install_root: root.join("opt/pip"),
        config_root: root.join("etc/pip"),
        unit_root: root.join("etc/systemd/system"),
        state_root: root.join("var/lib/pip"),
    }
}

fn prepare_layout(layout: &InstallLayout) {
    for path in [
        &layout.install_root,
        &layout.config_root,
        &layout.unit_root,
        &layout.state_root,
    ] {
        fs::create_dir_all(path).unwrap();
    }
}

fn cohort(parent: &Path, name: &str, binary: &[u8], source: &str, key: &str) -> PathBuf {
    let cohort = parent.join(name);
    let root = cohort.join("root");
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::create_dir_all(root.join("share/pip/config/repositories")).unwrap();
    fs::create_dir_all(root.join("share/pip/systemd")).unwrap();
    fs::write(root.join("bin/pip-control"), binary).unwrap();
    fs::write(
        root.join("share/pip/config/repositories/mdk.json"),
        include_bytes!("../../../config/target/repositories/mdk.json"),
    )
    .unwrap();
    fs::write(
        root.join("share/pip/systemd/pip-shadow-reconcile.service"),
        include_bytes!("../../../packaging/systemd/pip-shadow-reconcile.service"),
    )
    .unwrap();
    fs::write(
        root.join("share/pip/systemd/pip-shadow-reconcile.timer"),
        include_bytes!("../../../packaging/systemd/pip-shadow-reconcile.timer"),
    )
    .unwrap();
    fs::write(
        root.join("share/pip/systemd/pip-controller@.service"),
        include_bytes!("../../../packaging/systemd/pip-controller@.service"),
    )
    .unwrap();
    fs::write(
        root.join("share/pip/systemd/pip-controller@.timer"),
        include_bytes!("../../../packaging/systemd/pip-controller@.timer"),
    )
    .unwrap();
    fs::write(
        root.join("share/pip/systemd/pip-direct-worker@.service"),
        include_bytes!("../../../packaging/systemd/pip-direct-worker@.service"),
    )
    .unwrap();
    fs::write(
        root.join("share/pip/systemd/pip-direct-worker@.timer"),
        include_bytes!("../../../packaging/systemd/pip-direct-worker@.timer"),
    )
    .unwrap();
    fs::write(
        root.join("share/pip/systemd/pip-hermes-gateway.service"),
        include_bytes!("../../../packaging/systemd/pip-hermes-gateway.service"),
    )
    .unwrap();
    fs::write(
        root.join("share/pip/systemd/pip-webhook-ingress.service"),
        include_bytes!("../../../packaging/systemd/pip-webhook-ingress.service"),
    )
    .unwrap();
    fs::write(
        root.join("share/pip/systemd/pip-webhook-consumer@.service"),
        include_bytes!("../../../packaging/systemd/pip-webhook-consumer@.service"),
    )
    .unwrap();
    fs::write(
        root.join("share/pip/systemd/pip-webhook-consumer@.timer"),
        include_bytes!("../../../packaging/systemd/pip-webhook-consumer@.timer"),
    )
    .unwrap();
    for entry in walk_files(&root) {
        let mode = if entry.ends_with("bin/pip-control") {
            0o555
        } else {
            0o444
        };
        fs::set_permissions(entry, fs::Permissions::from_mode(mode)).unwrap();
    }
    let manifest = create_release_manifest(
        &root,
        &ReleaseMetadata {
            version: name.into(),
            source_commit: source.repeat(40),
            cargo_lock_sha256: "c".repeat(64),
            target: "fixture-linux".into(),
            rust_toolchain: "rustc fixture".into(),
            built_at: "2026-08-20T12:00:00Z".into(),
            builder_identity: "fixture".into(),
            workflow_version: 3,
            contract_version: 2,
        },
    )
    .unwrap();
    let bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(cohort.join("release-manifest.json"), &bytes).unwrap();
    fs::write(
        cohort.join("release-manifest.sig"),
        sign_manifest(&bytes, key).unwrap(),
    )
    .unwrap();
    cohort
}

fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_owned()];
    let mut files = Vec::new();
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap().path();
            if entry.is_dir() {
                pending.push(entry);
            } else {
                files.push(entry);
            }
        }
    }
    files
}

fn current_source(layout: &InstallLayout) -> String {
    let current = fs::read_link(layout.install_root.join("current")).unwrap();
    let descriptor = fs::read_to_string(current.join("SOURCE.COMMIT")).unwrap();
    descriptor.trim().into()
}

fn cohort_digests(cohort: &Path) -> (String, String) {
    use sha2::{Digest, Sha256};
    let bytes = fs::read(cohort.join("release-manifest.json")).unwrap();
    let manifest: ReleaseManifest = serde_json::from_slice(&bytes).unwrap();
    let digest = Sha256::digest(bytes);
    (
        digest.iter().map(|byte| format!("{byte:02x}")).collect(),
        manifest.binary_sha256,
    )
}

fn fake_systemctl(root: &Path, enabled: bool, active: bool, fail_reload_once: bool) -> PathBuf {
    let script = root.join("systemctl");
    fs::write(root.join("enabled"), if enabled { "1" } else { "0" }).unwrap();
    fs::write(root.join("active"), if active { "1" } else { "0" }).unwrap();
    fs::write(root.join("reloads"), "0").unwrap();
    let body = format!(
        r#"#!/bin/sh
root={root:?}
printf '%s %s\n' "$1" "${{2-}}" >>"$root/systemctl.calls"
unit=${{2-}}
case "$unit" in
  pip-controller@*.timer) definition=pip-controller@.timer ;;
  pip-direct-worker@*.timer) definition=pip-direct-worker@.timer ;;
  pip-webhook-consumer@*.timer) definition=pip-webhook-consumer@.timer ;;
  *) definition=$unit ;;
esac
if [ "$1" != daemon-reload ] && [ -n "$definition" ] && [ ! -f "$root/etc/systemd/system/$definition" ]; then
  case "$1" in
    is-enabled) echo not-found; exit 1 ;;
    is-active) echo inactive; exit 3 ;;
    *) exit 5 ;;
  esac
fi
case "$1" in
  is-enabled)
    if [ "$(cat "$root/enabled")" = 1 ]; then echo enabled; else echo disabled; exit 1; fi
    ;;
  is-active)
    if [ "$(cat "$root/active")" = 1 ]; then echo active; else echo inactive; exit 3; fi
    ;;
  stop) printf 0 >"$root/active" ;;
  start) printf 1 >"$root/active" ;;
  disable) printf 0 >"$root/enabled" ;;
  enable) printf 1 >"$root/enabled" ;;
  daemon-reload)
    count=$(cat "$root/reloads")
    count=$((count + 1))
    printf %s "$count" >"$root/reloads"
    if [ {fail} = 1 ] && [ "$count" = 1 ]; then exit 1; fi
    ;;
  *) exit 2 ;;
esac
"#,
        root = root,
        fail = u8::from(fail_reload_once)
    );
    fs::write(&script, body).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    script
}
