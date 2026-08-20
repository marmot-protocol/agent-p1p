use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pip_control::{
    InstallFault, InstallLayout, InstallResult, ReleaseMetadata, create_release_manifest,
    install_release, sign_manifest, verifying_key,
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
