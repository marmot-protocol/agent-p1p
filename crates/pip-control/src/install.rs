//! Transactional, content-addressed release installation.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, chown, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use pip_store::Store;
use sha2::{Digest, Sha256};

use crate::{ReleaseManifest, load_repository_policy, verify_release};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallLayout {
    pub install_root: PathBuf,
    pub config_root: PathBuf,
    pub unit_root: PathBuf,
    pub state_root: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallFault {
    AfterRelease,
    AfterPolicy,
    AfterUnits,
    AfterLedger,
    AfterCurrent,
}

impl InstallFault {
    pub const ALL: [Self; 5] = [
        Self::AfterRelease,
        Self::AfterPolicy,
        Self::AfterUnits,
        Self::AfterLedger,
        Self::AfterCurrent,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallResult {
    Installed,
    Existing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallOutcome {
    pub result: InstallResult,
    pub release_id: String,
    pub source_commit: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostInstallOptions {
    pub systemctl: PathBuf,
    pub state_uid: u32,
    pub state_gid: u32,
}

#[derive(Debug)]
pub enum InstallError {
    InvalidLayout,
    InvalidCohort(String),
    ExpectedDigest(&'static str),
    ExistingConflict(PathBuf),
    Filesystem(String),
    Ledger(String),
    HostLifecycle(String),
    Injected(InstallFault),
    Rollback(String),
}

impl fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLayout => formatter.write_str("invalid installation layout"),
            Self::InvalidCohort(error) => write!(formatter, "invalid release cohort: {error}"),
            Self::ExpectedDigest(name) => {
                write!(
                    formatter,
                    "verified release {name} does not match the pinned digest"
                )
            }
            Self::ExistingConflict(path) => {
                write!(
                    formatter,
                    "existing installation path conflicts: {}",
                    path.display()
                )
            }
            Self::Filesystem(error) => write!(formatter, "installation filesystem error: {error}"),
            Self::Ledger(error) => write!(formatter, "ledger initialization failed: {error}"),
            Self::HostLifecycle(error) => write!(formatter, "host lifecycle failed: {error}"),
            Self::Injected(point) => write!(formatter, "injected installation failure: {point:?}"),
            Self::Rollback(error) => write!(formatter, "installation rollback failed: {error}"),
        }
    }
}

impl std::error::Error for InstallError {}

pub fn install_release(
    cohort: impl AsRef<Path>,
    public_key: &str,
    layout: &InstallLayout,
    fault: Option<InstallFault>,
) -> Result<InstallOutcome, InstallError> {
    let mut lifecycle = NoopLifecycle;
    install_release_inner(
        cohort,
        public_key,
        layout,
        fault,
        None,
        None,
        &mut lifecycle,
    )
}

pub fn install_release_pinned(
    cohort: impl AsRef<Path>,
    public_key: &str,
    layout: &InstallLayout,
    expected_manifest_sha256: &str,
    expected_binary_sha256: &str,
) -> Result<InstallOutcome, InstallError> {
    let mut lifecycle = NoopLifecycle;
    install_release_inner(
        cohort,
        public_key,
        layout,
        None,
        Some(expected_manifest_sha256),
        Some(expected_binary_sha256),
        &mut lifecycle,
    )
}

pub fn install_host_release(
    cohort: impl AsRef<Path>,
    public_key: &str,
    layout: &InstallLayout,
    expected_manifest_sha256: &str,
    expected_binary_sha256: &str,
    options: &HostInstallOptions,
) -> Result<InstallOutcome, InstallError> {
    let mut lifecycle = SystemdLifecycle::new(
        options.clone(),
        layout.state_root.join("ledger.db"),
        layout.unit_root.join("pip-v2-shadow-reconcile.timer"),
    );
    install_release_inner(
        cohort,
        public_key,
        layout,
        None,
        Some(expected_manifest_sha256),
        Some(expected_binary_sha256),
        &mut lifecycle,
    )
}

fn install_release_inner(
    cohort: impl AsRef<Path>,
    public_key: &str,
    layout: &InstallLayout,
    fault: Option<InstallFault>,
    expected_manifest_sha256: Option<&str>,
    expected_binary_sha256: Option<&str>,
    lifecycle: &mut impl InstallLifecycle,
) -> Result<InstallOutcome, InstallError> {
    validate_layout(layout)?;
    let cohort = real_directory(cohort.as_ref())?;
    let source_root = real_directory(&cohort.join("root"))?;
    let manifest_bytes = read_regular(&cohort.join("release-manifest.json"), 1024 * 1024)?;
    let signature = read_regular(&cohort.join("release-manifest.sig"), 1024)?;
    let signature = std::str::from_utf8(&signature)
        .map_err(|error| InstallError::InvalidCohort(error.to_string()))?;
    let verified = verify_release(&source_root, &manifest_bytes, signature, public_key)
        .map_err(|error| InstallError::InvalidCohort(error.to_string()))?;
    if expected_manifest_sha256.is_some_and(|expected| expected != verified.manifest_sha256()) {
        return Err(InstallError::ExpectedDigest("manifest SHA-256"));
    }
    if expected_binary_sha256.is_some_and(|expected| expected != verified.binary_sha256()) {
        return Err(InstallError::ExpectedDigest("binary SHA-256"));
    }
    let manifest: ReleaseManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| InstallError::InvalidCohort(error.to_string()))?;
    let policies = cohort_policies(&source_root, &manifest, layout)?;

    let release_id = hex_digest(&Sha256::digest(&manifest_bytes));
    let release_dir = layout.install_root.join("releases").join(&release_id);
    let service_target = layout.unit_root.join("pip-v2-shadow-reconcile.service");
    let timer_target = layout.unit_root.join("pip-v2-shadow-reconcile.timer");
    let controller_service_target = layout.unit_root.join("pip-v2-controller@.service");
    let controller_timer_target = layout.unit_root.join("pip-v2-controller@.timer");
    let direct_service_target = layout.unit_root.join("pip-v2-direct-worker@.service");
    let direct_timer_target = layout.unit_root.join("pip-v2-direct-worker@.timer");
    let gateway_service_target = layout.unit_root.join("pip-v2-hermes-gateway.service");
    let ledger_target = layout.state_root.join("ledger.db");
    let current = layout.install_root.join("current");
    let service_bytes = read_regular(
        &source_root.join("share/pip-v2/systemd/pip-v2-shadow-reconcile.service"),
        1024 * 1024,
    )?;
    let timer_bytes = read_regular(
        &source_root.join("share/pip-v2/systemd/pip-v2-shadow-reconcile.timer"),
        1024 * 1024,
    )?;
    let controller_service_bytes = read_regular(
        &source_root.join("share/pip-v2/systemd/pip-v2-controller@.service"),
        1024 * 1024,
    )?;
    let controller_timer_bytes = read_regular(
        &source_root.join("share/pip-v2/systemd/pip-v2-controller@.timer"),
        1024 * 1024,
    )?;
    let direct_service_bytes = read_regular(
        &source_root.join("share/pip-v2/systemd/pip-v2-direct-worker@.service"),
        1024 * 1024,
    )?;
    let direct_timer_bytes = read_regular(
        &source_root.join("share/pip-v2/systemd/pip-v2-direct-worker@.timer"),
        1024 * 1024,
    )?;
    let gateway_service_bytes = read_regular(
        &source_root.join("share/pip-v2/systemd/pip-v2-hermes-gateway.service"),
        1024 * 1024,
    )?;

    if release_dir.exists() {
        verify_release(&release_dir, &manifest_bytes, signature, public_key)
            .map_err(|error| InstallError::ExistingConflict(release_dir.clone()).with(error))?;
        if current_link(&current).as_deref() == Some(release_dir.as_path())
            && policies
                .iter()
                .all(|policy| exact_file(&policy.target, &policy.bytes))
            && exact_file(&service_target, &service_bytes)
            && exact_file(&timer_target, &timer_bytes)
            && exact_file(&controller_service_target, &controller_service_bytes)
            && exact_file(&controller_timer_target, &controller_timer_bytes)
            && exact_file(&direct_service_target, &direct_service_bytes)
            && exact_file(&direct_timer_target, &direct_timer_bytes)
            && exact_file(&gateway_service_target, &gateway_service_bytes)
            && ledger_target.is_file()
        {
            return Ok(InstallOutcome {
                result: InstallResult::Existing,
                release_id,
                source_commit: verified.source_commit().into(),
            });
        }
    }

    let snapshot_paths = policies
        .iter()
        .map(|policy| &policy.target)
        .chain([
            &service_target,
            &timer_target,
            &controller_service_target,
            &controller_timer_target,
            &direct_service_target,
            &direct_timer_target,
            &gateway_service_target,
            &ledger_target,
        ])
        .collect::<Vec<_>>();
    let snapshot = Snapshot::capture(&current, snapshot_paths)?;
    let release_preexisting = release_dir.exists();
    if let Err(error) = lifecycle.before_mutation() {
        return match lifecycle.rollback() {
            Ok(()) => Err(error),
            Err(rollback) => Err(InstallError::Rollback(format!("{error}; {rollback}"))),
        };
    }
    let result = (|| {
        if !release_preexisting {
            install_release_tree(
                &source_root,
                &release_dir,
                &manifest,
                verified.source_commit(),
            )?;
            verify_release(&release_dir, &manifest_bytes, signature, public_key)
                .map_err(|error| InstallError::InvalidCohort(error.to_string()))?;
        }
        inject(fault, InstallFault::AfterRelease)?;
        for policy in &policies {
            write_atomic(&policy.target, &policy.bytes, 0o444)?;
        }
        inject(fault, InstallFault::AfterPolicy)?;
        write_atomic(&service_target, &service_bytes, 0o444)?;
        write_atomic(&timer_target, &timer_bytes, 0o444)?;
        write_atomic(&controller_service_target, &controller_service_bytes, 0o444)?;
        write_atomic(&controller_timer_target, &controller_timer_bytes, 0o444)?;
        write_atomic(&direct_service_target, &direct_service_bytes, 0o444)?;
        write_atomic(&direct_timer_target, &direct_timer_bytes, 0o444)?;
        write_atomic(&gateway_service_target, &gateway_service_bytes, 0o444)?;
        inject(fault, InstallFault::AfterUnits)?;
        Store::open(&ledger_target).map_err(|error| InstallError::Ledger(error.to_string()))?;
        fs::set_permissions(&ledger_target, fs::Permissions::from_mode(0o600)).map_err(fs_error)?;
        inject(fault, InstallFault::AfterLedger)?;
        replace_symlink(&current, &release_dir)?;
        inject(fault, InstallFault::AfterCurrent)?;
        lifecycle.commit()?;
        Ok(())
    })();
    if let Err(error) = result {
        let filesystem_rollback = snapshot.restore(&current);
        if !release_preexisting
            && release_dir.exists()
            && let Err(rollback) = fs::remove_dir_all(&release_dir).map_err(fs_error)
        {
            return Err(InstallError::Rollback(format!("{error}; {rollback}")));
        }
        let lifecycle_rollback = lifecycle.rollback();
        if let Err(rollback) = filesystem_rollback.and(lifecycle_rollback) {
            return Err(InstallError::Rollback(format!("{error}; {rollback}")));
        }
        return Err(error);
    }
    Ok(InstallOutcome {
        result: InstallResult::Installed,
        release_id,
        source_commit: verified.source_commit().into(),
    })
}

trait InstallLifecycle {
    fn before_mutation(&mut self) -> Result<(), InstallError>;
    fn commit(&mut self) -> Result<(), InstallError>;
    fn rollback(&mut self) -> Result<(), InstallError>;
}

struct NoopLifecycle;

impl InstallLifecycle for NoopLifecycle {
    fn before_mutation(&mut self) -> Result<(), InstallError> {
        Ok(())
    }

    fn commit(&mut self) -> Result<(), InstallError> {
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), InstallError> {
        Ok(())
    }
}

struct SystemdLifecycle {
    options: HostInstallOptions,
    ledger: PathBuf,
    timer_path: PathBuf,
    prior_enabled: Option<bool>,
    prior_active: Option<bool>,
}

impl SystemdLifecycle {
    const TIMER: &'static str = "pip-v2-shadow-reconcile.timer";

    fn new(options: HostInstallOptions, ledger: PathBuf, timer_path: PathBuf) -> Self {
        Self {
            options,
            ledger,
            timer_path,
            prior_enabled: None,
            prior_active: None,
        }
    }

    fn query(&self, command: &str, truthy: &[&str], falsy: &[&str]) -> Result<bool, InstallError> {
        let output = Command::new(&self.options.systemctl)
            .args([command, Self::TIMER])
            .output()
            .map_err(|error| InstallError::HostLifecycle(error.to_string()))?;
        let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if truthy.contains(&value.as_str()) {
            return Ok(true);
        }
        if falsy.contains(&value.as_str()) {
            return Ok(false);
        }
        Err(InstallError::HostLifecycle(format!(
            "systemctl {command} returned status {} and state {value:?}",
            status_label(output.status)
        )))
    }

    fn run(&self, command: &str) -> Result<(), InstallError> {
        let output = Command::new(&self.options.systemctl)
            .args([command, Self::TIMER])
            .output()
            .map_err(|error| InstallError::HostLifecycle(error.to_string()))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(InstallError::HostLifecycle(format!(
                "systemctl {command} failed with status {}: {}",
                status_label(output.status),
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    fn daemon_reload(&self) -> Result<(), InstallError> {
        let output = Command::new(&self.options.systemctl)
            .arg("daemon-reload")
            .output()
            .map_err(|error| InstallError::HostLifecycle(error.to_string()))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(InstallError::HostLifecycle(format!(
                "systemctl daemon-reload failed with status {}: {}",
                status_label(output.status),
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    fn restore_state(&self) -> Result<(), InstallError> {
        match (self.prior_enabled, self.prior_active) {
            (Some(enabled), Some(active)) => {
                self.run(if enabled { "enable" } else { "disable" })?;
                self.run(if active { "start" } else { "stop" })
            }
            _ => Ok(()),
        }
    }
}

impl InstallLifecycle for SystemdLifecycle {
    fn before_mutation(&mut self) -> Result<(), InstallError> {
        if !self.timer_path.exists() {
            self.prior_enabled = Some(false);
            self.prior_active = Some(false);
            return Ok(());
        }
        self.prior_enabled = Some(self.query(
            "is-enabled",
            &["enabled", "enabled-runtime"],
            &[
                "disabled",
                "static",
                "indirect",
                "masked",
                "generated",
                "transient",
                "linked",
                "linked-runtime",
                "alias",
                "not-found",
            ],
        )?);
        self.prior_active = Some(self.query(
            "is-active",
            &["active"],
            &["inactive", "failed", "deactivating", "unknown"],
        )?);
        if self.prior_active == Some(true) {
            self.run("stop")?;
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<(), InstallError> {
        chown(
            &self.ledger,
            Some(self.options.state_uid),
            Some(self.options.state_gid),
        )
        .map_err(|error| InstallError::HostLifecycle(error.to_string()))?;
        self.daemon_reload()?;
        self.restore_state()
    }

    fn rollback(&mut self) -> Result<(), InstallError> {
        if self.prior_enabled.is_none() || self.prior_active.is_none() {
            return Ok(());
        }
        self.daemon_reload()?;
        self.restore_state()
    }
}

fn status_label(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "signal".to_owned(), |code| code.to_string())
}

struct CohortPolicy {
    target: PathBuf,
    bytes: Vec<u8>,
}

fn cohort_policies(
    source_root: &Path,
    manifest: &ReleaseManifest,
    layout: &InstallLayout,
) -> Result<Vec<CohortPolicy>, InstallError> {
    const PREFIX: &str = "share/pip-v2/config/repositories/";
    let mut policies = Vec::new();
    for artifact in &manifest.artifacts {
        let Some(name) = artifact.path.strip_prefix(PREFIX) else {
            continue;
        };
        if name.is_empty() || name.contains('/') || !name.ends_with(".json") {
            return Err(InstallError::InvalidCohort(
                "repository policy path is not canonical".into(),
            ));
        }
        let bytes = read_regular(&source_root.join(&artifact.path), 1024 * 1024)?;
        let policy = load_repository_policy(&bytes)
            .map_err(|error| InstallError::InvalidCohort(error.to_string()))?;
        if policy.intake.enabled || !policy.intake.paused || policy.dispatch_enabled {
            return Err(InstallError::InvalidCohort(format!(
                "fresh-install policy {name} must disable and pause intake and dispatch"
            )));
        }
        policies.push(CohortPolicy {
            target: layout.config_root.join("repositories").join(name),
            bytes,
        });
    }
    if policies.is_empty() {
        return Err(InstallError::InvalidCohort(
            "release has no repository policies".into(),
        ));
    }
    policies.sort_by(|left, right| left.target.cmp(&right.target));
    Ok(policies)
}

trait WithContext {
    fn with(self, error: impl fmt::Display) -> InstallError;
}

impl WithContext for InstallError {
    fn with(self, error: impl fmt::Display) -> InstallError {
        match self {
            Self::ExistingConflict(path) => Self::InvalidCohort(format!(
                "existing release {} failed verification: {error}",
                path.display()
            )),
            other => other,
        }
    }
}

fn validate_layout(layout: &InstallLayout) -> Result<(), InstallError> {
    let mut roots = Vec::new();
    for root in [
        &layout.install_root,
        &layout.config_root,
        &layout.unit_root,
        &layout.state_root,
    ] {
        roots.push(real_directory(root)?);
    }
    if roots
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != roots.len()
    {
        return Err(InstallError::InvalidLayout);
    }
    Ok(())
}

fn install_release_tree(
    source: &Path,
    destination: &Path,
    manifest: &ReleaseManifest,
    source_commit: &str,
) -> Result<(), InstallError> {
    let parent = destination.parent().ok_or(InstallError::InvalidLayout)?;
    fs::create_dir_all(parent).map_err(fs_error)?;
    let staging = parent.join(format!(
        ".installing-{}-{}",
        manifest.binary_sha256,
        std::process::id()
    ));
    if staging.exists() {
        return Err(InstallError::ExistingConflict(staging));
    }
    fs::create_dir(&staging).map_err(fs_error)?;
    let copied = (|| {
        for artifact in &manifest.artifacts {
            let target = staging.join(&artifact.path);
            fs::create_dir_all(target.parent().ok_or(InstallError::InvalidLayout)?)
                .map_err(fs_error)?;
            fs::copy(source.join(&artifact.path), &target).map_err(fs_error)?;
            fs::set_permissions(&target, fs::Permissions::from_mode(artifact.mode))
                .map_err(fs_error)?;
        }
        write_atomic(
            &staging.join("SOURCE.COMMIT"),
            format!("{source_commit}\n").as_bytes(),
            0o444,
        )?;
        Ok(())
    })();
    if let Err(error) = copied {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    fs::rename(&staging, destination).map_err(fs_error)
}

#[derive(Clone)]
struct FileSnapshot {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
    mode: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
}

struct Snapshot {
    current: Option<PathBuf>,
    files: Vec<FileSnapshot>,
}

impl Snapshot {
    fn capture<'a>(
        current: &Path,
        files: impl IntoIterator<Item = &'a PathBuf>,
    ) -> Result<Self, InstallError> {
        let mut snapshots = Vec::new();
        for path in files {
            if path.exists() {
                let metadata = fs::symlink_metadata(path).map_err(fs_error)?;
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(InstallError::ExistingConflict(path.clone()));
                }
                let bytes = if path.file_name().and_then(|name| name.to_str()) == Some("ledger.db")
                {
                    snapshot_ledger(path)?
                } else {
                    read_regular(path, 16 * 1024 * 1024)?
                };
                snapshots.push(FileSnapshot {
                    path: path.clone(),
                    bytes: Some(bytes),
                    mode: Some(metadata.permissions().mode() & 0o7777),
                    uid: Some(metadata.uid()),
                    gid: Some(metadata.gid()),
                });
            } else {
                snapshots.push(FileSnapshot {
                    path: path.clone(),
                    bytes: None,
                    mode: None,
                    uid: None,
                    gid: None,
                });
            }
        }
        Ok(Self {
            current: current_link(current),
            files: snapshots,
        })
    }

    fn restore(&self, current: &Path) -> Result<(), InstallError> {
        for file in &self.files {
            if file.path.file_name().and_then(|name| name.to_str()) == Some("ledger.db") {
                remove_sqlite_sidecars(&file.path)?;
            }
            match (&file.bytes, file.mode, file.uid, file.gid) {
                (Some(bytes), Some(mode), Some(uid), Some(gid)) => {
                    write_atomic(&file.path, bytes, mode)?;
                    chown(&file.path, Some(uid), Some(gid)).map_err(fs_error)?;
                }
                (None, None, None, None) => remove_if_file(&file.path)?,
                _ => return Err(InstallError::InvalidLayout),
            }
        }
        match &self.current {
            Some(target) => replace_symlink(current, target),
            None => remove_if_file(current),
        }
    }
}

fn snapshot_ledger(path: &Path) -> Result<Vec<u8>, InstallError> {
    let parent = path.parent().ok_or(InstallError::InvalidLayout)?;
    let backup = parent.join(format!(".pip-ledger-rollback-{}", std::process::id()));
    remove_if_file(&backup)?;
    let store =
        Store::open_read_only(path).map_err(|error| InstallError::Ledger(error.to_string()))?;
    store
        .backup_to(&backup)
        .map_err(|error| InstallError::Ledger(error.to_string()))?;
    drop(store);
    let bytes = read_regular(&backup, 1024 * 1024 * 1024);
    let removed = fs::remove_file(&backup).map_err(fs_error);
    match (bytes, removed) {
        (Ok(bytes), Ok(())) => Ok(bytes),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn remove_sqlite_sidecars(path: &Path) -> Result<(), InstallError> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err(InstallError::InvalidLayout);
    };
    let parent = path.parent().ok_or(InstallError::InvalidLayout)?;
    for suffix in ["-wal", "-shm"] {
        remove_if_file(&parent.join(format!("{name}{suffix}")))?;
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), InstallError> {
    let parent = path.parent().ok_or(InstallError::InvalidLayout)?;
    fs::create_dir_all(parent).map_err(fs_error)?;
    let temporary = parent.join(format!(
        ".pip-install-{}-{}",
        std::process::id(),
        path.file_name().and_then(|v| v.to_str()).unwrap_or("file")
    ));
    remove_if_file(&temporary)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(fs_error)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(fs_error)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(mode)).map_err(fs_error)?;
    fs::rename(&temporary, path).map_err(fs_error)
}

fn replace_symlink(path: &Path, target: &Path) -> Result<(), InstallError> {
    let parent = path.parent().ok_or(InstallError::InvalidLayout)?;
    let temporary = parent.join(format!(".current-{}", std::process::id()));
    remove_if_file(&temporary)?;
    symlink(target, &temporary).map_err(fs_error)?;
    fs::rename(&temporary, path).map_err(fs_error)
}

fn current_link(path: &Path) -> Option<PathBuf> {
    fs::read_link(path).ok()
}

fn remove_if_file(path: &Path) -> Result<(), InstallError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
            fs::remove_file(path).map_err(fs_error)
        }
        Ok(_) => Err(InstallError::ExistingConflict(path.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(fs_error(error)),
    }
}

fn exact_file(path: &Path, expected: &[u8]) -> bool {
    read_regular(path, expected.len().saturating_add(1)).is_ok_and(|bytes| bytes == expected)
}

fn read_regular(path: &Path, max: usize) -> Result<Vec<u8>, InstallError> {
    let metadata = fs::symlink_metadata(path).map_err(fs_error)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > max as u64 {
        return Err(InstallError::ExistingConflict(path.to_owned()));
    }
    fs::read(path).map_err(fs_error)
}

fn real_directory(path: &Path) -> Result<PathBuf, InstallError> {
    let metadata = fs::symlink_metadata(path).map_err(fs_error)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(InstallError::InvalidLayout);
    }
    path.canonicalize().map_err(fs_error)
}

fn inject(fault: Option<InstallFault>, point: InstallFault) -> Result<(), InstallError> {
    if fault == Some(point) {
        Err(InstallError::Injected(point))
    } else {
        Ok(())
    }
}

fn fs_error(error: std::io::Error) -> InstallError {
    InstallError::Filesystem(error.to_string())
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
