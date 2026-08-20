use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pip_control::{
    HostInstallOptions, InstallFault, InstallLayout, InstallResult, ReleaseManifest,
    ReleaseMetadata, create_release_manifest, install_host_release, install_release,
    install_release_pinned, sign_manifest, verifying_key,
};
use pip_store::Store;

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
    assert!(
        layout
            .unit_root
            .join("pip-v2-controller@.service")
            .is_file()
    );
    assert!(layout.unit_root.join("pip-v2-controller@.timer").is_file());

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
                .join("pip-v2-shadow-reconcile.service")
                .exists()
        );
        assert!(!layout.unit_root.join("pip-v2-controller@.service").exists());
        assert!(!layout.unit_root.join("pip-v2-controller@.timer").exists());
        assert!(
            !layout
                .unit_root
                .join("pip-v2-shadow-reconcile.timer")
                .exists()
        );
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

fn layout(root: &Path) -> InstallLayout {
    InstallLayout {
        install_root: root.join("opt/pip-v2"),
        config_root: root.join("etc/pip-v2"),
        unit_root: root.join("etc/systemd/system"),
        state_root: root.join("var/lib/pip-v2"),
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
    fs::create_dir_all(root.join("share/pip-v2/config/repositories")).unwrap();
    fs::create_dir_all(root.join("share/pip-v2/systemd")).unwrap();
    fs::write(root.join("bin/pip-control"), binary).unwrap();
    fs::write(
        root.join("share/pip-v2/config/repositories/mdk.json"),
        include_bytes!("../../../config/target/repositories/mdk.json"),
    )
    .unwrap();
    fs::write(
        root.join("share/pip-v2/systemd/pip-v2-shadow-reconcile.service"),
        include_bytes!("../../../packaging/systemd/pip-v2-shadow-reconcile.service"),
    )
    .unwrap();
    fs::write(
        root.join("share/pip-v2/systemd/pip-v2-shadow-reconcile.timer"),
        include_bytes!("../../../packaging/systemd/pip-v2-shadow-reconcile.timer"),
    )
    .unwrap();
    fs::write(
        root.join("share/pip-v2/systemd/pip-v2-controller@.service"),
        include_bytes!("../../../packaging/systemd/pip-v2-controller@.service"),
    )
    .unwrap();
    fs::write(
        root.join("share/pip-v2/systemd/pip-v2-controller@.timer"),
        include_bytes!("../../../packaging/systemd/pip-v2-controller@.timer"),
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
            workflow_version: 2,
            contract_version: 1,
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
