//! Fail-closed bootstrap for the service-owned Hermes dispatcher root.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{CommandRunner, CommandSpec, HermesError, HermesReader, valid_id};

const ROOT_MARKER: &str = ".pip-v2-root.json";
const PROFILE_MARKER: &str = ".pip-v2-profile.json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileBootstrapSpec {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub reasoning_effort: String,
    pub skills: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeBootstrapSpec {
    pub root: PathBuf,
    pub skills_root: PathBuf,
    pub auth_source: PathBuf,
    pub board: String,
    pub board_name: String,
    pub board_description: String,
    pub profiles: Vec<ProfileBootstrapSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BootstrapOutcome {
    pub hermes_version: String,
    pub board_created: bool,
    pub profiles_created: u32,
    pub profiles_reconciled: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BootstrapError {
    InvalidConfiguration,
    UnsafePath(PathBuf),
    UnmanagedPath(PathBuf),
    ManagedPathDrift(PathBuf),
    Filesystem(String),
    Serialization(String),
    Hermes(HermesError),
    BoardUnavailable,
    IncompatibleCli(String),
}

impl fmt::Display for BootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => formatter.write_str("invalid Hermes bootstrap spec"),
            Self::UnsafePath(path) => write!(formatter, "unsafe Hermes path: {}", path.display()),
            Self::UnmanagedPath(path) => {
                write!(
                    formatter,
                    "refusing unmanaged Hermes path: {}",
                    path.display()
                )
            }
            Self::ManagedPathDrift(path) => {
                write!(formatter, "managed Hermes path drift: {}", path.display())
            }
            Self::Filesystem(error) => {
                write!(formatter, "Hermes bootstrap filesystem error: {error}")
            }
            Self::Serialization(error) => {
                write!(formatter, "Hermes bootstrap serialization error: {error}")
            }
            Self::Hermes(error) => write!(formatter, "Hermes bootstrap command failed: {error}"),
            Self::BoardUnavailable => {
                formatter.write_str("Hermes board was not durable after creation")
            }
            Self::IncompatibleCli(surface) => {
                write!(
                    formatter,
                    "Hermes CLI is missing required {surface} capabilities"
                )
            }
        }
    }
}

impl std::error::Error for BootstrapError {}

impl From<HermesError> for BootstrapError {
    fn from(error: HermesError) -> Self {
        Self::Hermes(error)
    }
}

pub struct HermesBootstrap<R> {
    runner: R,
    program: String,
    timeout: Duration,
    max_output_bytes: usize,
}

impl<R: CommandRunner + Clone> HermesBootstrap<R> {
    pub fn new(
        runner: R,
        program: impl Into<String>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<Self, BootstrapError> {
        let program = program.into();
        if program.trim().is_empty() || timeout.is_zero() || max_output_bytes == 0 {
            return Err(BootstrapError::InvalidConfiguration);
        }
        Ok(Self {
            runner,
            program,
            timeout,
            max_output_bytes,
        })
    }

    pub fn apply(&self, spec: &RuntimeBootstrapSpec) -> Result<BootstrapOutcome, BootstrapError> {
        let prepared = PreparedSpec::new(spec)?;
        prepared.preflight()?;

        let before = self.capabilities()?;
        self.probe_contract()?;
        let board_created = if before.boards.iter().any(|board| board == &prepared.board) {
            false
        } else {
            self.create_board(&prepared)?;
            let after = self.capabilities()?;
            if !after.boards.iter().any(|board| board == &prepared.board) {
                return Err(BootstrapError::BoardUnavailable);
            }
            true
        };

        prepared.reconcile_root()?;
        let mut profiles_created = 0_u32;
        let mut profiles_reconciled = 0_u32;
        for profile in &prepared.profiles {
            if prepared.reconcile_profile(profile)? {
                profiles_created += 1;
            } else {
                profiles_reconciled += 1;
            }
            self.verify_profile(profile)?;
        }
        Ok(BootstrapOutcome {
            hermes_version: before.version,
            board_created,
            profiles_created,
            profiles_reconciled,
        })
    }

    fn capabilities(&self) -> Result<super::Capabilities, BootstrapError> {
        HermesReader::new(
            self.runner.clone(),
            &self.program,
            self.timeout,
            self.max_output_bytes,
        )?
        .capabilities()
        .map_err(Into::into)
    }

    fn create_board(&self, spec: &PreparedSpec) -> Result<(), BootstrapError> {
        let output = self.runner.run(&CommandSpec {
            program: self.program.clone(),
            args: vec![
                "kanban".into(),
                "boards".into(),
                "create".into(),
                spec.board.clone(),
                "--name".into(),
                spec.board_name.clone(),
                "--description".into(),
                spec.board_description.clone(),
            ],
            timeout: self.timeout,
            max_output_bytes: self.max_output_bytes,
        })?;
        if output.timed_out {
            return Err(HermesError::TimedOut.into());
        }
        if output.stdout.len() > self.max_output_bytes
            || output.stderr.len() > self.max_output_bytes
        {
            return Err(HermesError::OutputTooLarge.into());
        }
        if output.status != 0 {
            return Err(HermesError::CommandFailed(output.status).into());
        }
        Ok(())
    }

    fn probe_contract(&self) -> Result<(), BootstrapError> {
        self.require_help(
            vec!["kanban".into(), "create".into(), "--help".into()],
            &[
                "--workspace",
                "--idempotency-key",
                "--created-by",
                "--max-runtime",
                "--max-retries",
                "--skill",
                "--model",
                "--provider",
                "--initial-status",
            ],
            "Kanban task creation",
        )?;
        self.require_help(
            vec!["gateway".into(), "run".into(), "--help".into()],
            &["--no-supervise"],
            "foreground gateway",
        )
    }

    fn verify_profile(&self, profile: &ProfileBootstrapSpec) -> Result<(), BootstrapError> {
        for (key, expected) in [
            ("model", profile.model.as_str()),
            ("provider", profile.provider.as_str()),
            ("agent.reasoning_effort", profile.reasoning_effort.as_str()),
            ("terminal.home_mode", "profile"),
        ] {
            let output = self.runner.run(&CommandSpec {
                program: self.program.clone(),
                args: vec![
                    "-p".into(),
                    profile.name.clone(),
                    "config".into(),
                    "get".into(),
                    key.into(),
                ],
                timeout: self.timeout,
                max_output_bytes: self.max_output_bytes,
            })?;
            if output.timed_out {
                return Err(HermesError::TimedOut.into());
            }
            if output.stdout.len() > self.max_output_bytes
                || output.stderr.len() > self.max_output_bytes
            {
                return Err(HermesError::OutputTooLarge.into());
            }
            if output.status != 0 {
                return Err(HermesError::CommandFailed(output.status).into());
            }
            let actual = std::str::from_utf8(&output.stdout)
                .map_err(|_| HermesError::InvalidUtf8)?
                .trim();
            if actual != expected {
                return Err(BootstrapError::IncompatibleCli(format!(
                    "effective profile configuration ({}/{key})",
                    profile.name
                )));
            }
        }
        Ok(())
    }

    fn require_help(
        &self,
        args: Vec<String>,
        required: &[&str],
        surface: &str,
    ) -> Result<(), BootstrapError> {
        let output = self.runner.run(&CommandSpec {
            program: self.program.clone(),
            args,
            timeout: self.timeout,
            max_output_bytes: self.max_output_bytes,
        })?;
        if output.timed_out {
            return Err(HermesError::TimedOut.into());
        }
        if output.stdout.len() > self.max_output_bytes
            || output.stderr.len() > self.max_output_bytes
        {
            return Err(HermesError::OutputTooLarge.into());
        }
        if output.status != 0 {
            return Err(HermesError::CommandFailed(output.status).into());
        }
        let stdout = std::str::from_utf8(&output.stdout).map_err(|_| HermesError::InvalidUtf8)?;
        if required.iter().any(|flag| !stdout.contains(flag)) {
            return Err(BootstrapError::IncompatibleCli(surface.into()));
        }
        Ok(())
    }
}

struct PreparedSpec {
    root: PathBuf,
    skills_root: PathBuf,
    auth_source: PathBuf,
    board: String,
    board_name: String,
    board_description: String,
    profiles: Vec<ProfileBootstrapSpec>,
}

impl PreparedSpec {
    fn new(spec: &RuntimeBootstrapSpec) -> Result<Self, BootstrapError> {
        if !valid_id(&spec.board)
            || !valid_text(&spec.board_name, 256)
            || !valid_text(&spec.board_description, 1024)
            || spec.profiles.is_empty()
        {
            return Err(BootstrapError::InvalidConfiguration);
        }
        let root = real_directory(&spec.root)?;
        let skills_root = real_directory(&spec.skills_root)?;
        let auth_source = regular_file(&spec.auth_source, 1024 * 1024)?;
        if auth_source != root.join("auth.json") {
            return Err(BootstrapError::UnsafePath(auth_source));
        }
        let auth_mode = fs::metadata(&auth_source)
            .map_err(fs_error)?
            .permissions()
            .mode()
            & 0o777;
        if auth_mode & 0o077 != 0 {
            return Err(BootstrapError::UnsafePath(auth_source));
        }
        let names = spec
            .profiles
            .iter()
            .map(|profile| &profile.name)
            .collect::<BTreeSet<_>>();
        if names.len() != spec.profiles.len() || !spec.profiles.iter().all(valid_profile) {
            return Err(BootstrapError::InvalidConfiguration);
        }
        Ok(Self {
            root,
            skills_root,
            auth_source,
            board: spec.board.clone(),
            board_name: spec.board_name.clone(),
            board_description: spec.board_description.clone(),
            profiles: spec.profiles.clone(),
        })
    }

    fn preflight(&self) -> Result<(), BootstrapError> {
        preflight_root(&self.root)?;
        let expected_profiles = self
            .profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<BTreeSet<_>>();
        let profiles_root = self.root.join("profiles");
        if profiles_root.exists() {
            real_directory(&profiles_root)?;
            for entry in fs::read_dir(&profiles_root).map_err(fs_error)? {
                let entry = entry.map_err(fs_error)?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| BootstrapError::UnsafePath(entry.path()))?;
                if !expected_profiles.contains(name.as_str()) {
                    return Err(BootstrapError::UnmanagedPath(entry.path()));
                }
            }
        }
        for profile in &self.profiles {
            let profile_root = self.root.join("profiles").join(&profile.name);
            preflight_managed_directory(&profile_root, PROFILE_MARKER, &profile_marker(profile))?;
            let auth = profile_root.join("auth.json");
            if auth.exists() || fs::symlink_metadata(&auth).is_ok() {
                ensure_symlink(&auth, &self.auth_source)?;
            }
            let profile_skills = profile_root.join("skills");
            if profile_skills.exists() {
                real_directory(&profile_skills)?;
                let expected = profile
                    .skills
                    .iter()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>();
                for entry in fs::read_dir(&profile_skills).map_err(fs_error)? {
                    let entry = entry.map_err(fs_error)?;
                    let name = entry
                        .file_name()
                        .into_string()
                        .map_err(|_| BootstrapError::UnsafePath(entry.path()))?;
                    if !expected.contains(name.as_str()) {
                        return Err(BootstrapError::UnmanagedPath(entry.path()));
                    }
                }
            }
            for skill in &profile.skills {
                let source = self.skill_source(skill)?;
                let marker = regular_file(&source.join("SKILL.md"), 256 * 1024)?;
                if marker.parent() != Some(source.as_path()) {
                    return Err(BootstrapError::UnsafePath(marker));
                }
            }
        }
        Ok(())
    }

    fn reconcile_root(&self) -> Result<(), BootstrapError> {
        write_marker(&self.root, ROOT_MARKER, &root_marker())?;
        write_managed(&self.root.join("config.yaml"), &root_config()?, 0o600)?;
        ensure_directory(&self.root.join("home"), 0o700)
    }

    fn reconcile_profile(&self, profile: &ProfileBootstrapSpec) -> Result<bool, BootstrapError> {
        let profiles = self.root.join("profiles");
        ensure_directory(&profiles, 0o700)?;
        let profile_root = profiles.join(&profile.name);
        let created = if profile_root.exists() {
            false
        } else {
            ensure_directory(&profile_root, 0o700)?;
            true
        };
        write_marker(&profile_root, PROFILE_MARKER, &profile_marker(profile))?;
        write_managed(
            &profile_root.join("config.yaml"),
            &profile_config(profile)?,
            0o600,
        )?;
        write_managed(
            &profile_root.join(".env"),
            b"# Pip v2 profile secrets are provisioned through the shared auth link.\n",
            0o600,
        )?;
        ensure_symlink(&profile_root.join("auth.json"), &self.auth_source)?;
        ensure_directory(&profile_root.join("home"), 0o700)?;
        let skills = profile_root.join("skills");
        ensure_directory(&skills, 0o700)?;
        for skill in &profile.skills {
            reconcile_managed_symlink(&skills.join(skill), &self.skill_source(skill)?)?;
        }
        Ok(created)
    }

    fn skill_source(&self, skill: &str) -> Result<PathBuf, BootstrapError> {
        let relative = if skill == "workflow-contract" {
            PathBuf::from("shared/workflow-contract")
        } else {
            PathBuf::from(skill)
        };
        let source = real_directory(&self.skills_root.join(relative))?;
        if !source.starts_with(&self.skills_root) {
            return Err(BootstrapError::UnsafePath(source));
        }
        Ok(source)
    }
}

#[derive(Deserialize, Serialize)]
struct OwnershipMarker {
    schema_version: u32,
    owner: String,
    kind: String,
    name: String,
}

fn root_marker() -> OwnershipMarker {
    OwnershipMarker {
        schema_version: 1,
        owner: "pip-v2".into(),
        kind: "hermes-root".into(),
        name: "dispatcher".into(),
    }
}

fn profile_marker(profile: &ProfileBootstrapSpec) -> OwnershipMarker {
    OwnershipMarker {
        schema_version: 1,
        owner: "pip-v2".into(),
        kind: "hermes-profile".into(),
        name: profile.name.clone(),
    }
}

fn root_config() -> Result<Vec<u8>, BootstrapError> {
    encoded(&json!({
        "max_concurrent_sessions": 1,
        "platform_toolsets": {"cli": []},
        "agent": {"disabled_toolsets": ["delegation", "memory", "messaging", "web"]},
        "kanban": {
            "auto_decompose": false,
            "auto_subscribe_on_create": false,
            "dispatch_in_gateway": true,
            "dispatch_interval_seconds": 15,
            "failure_limit": 1,
            "max_in_progress": 1,
            "max_in_progress_per_profile": 1,
            "review_dispatch": false
        }
    }))
}

fn profile_config(profile: &ProfileBootstrapSpec) -> Result<Vec<u8>, BootstrapError> {
    encoded(&json!({
        "model": profile.model,
        "provider": profile.provider,
        "fallback_providers": [],
        "max_concurrent_sessions": 1,
        "agent": {
            "disabled_toolsets": ["browser", "cronjob", "delegation", "image_gen", "memory", "messaging", "tts", "vision", "web"],
            "reasoning_effort": profile.reasoning_effort
        },
        "terminal": {
            "backend": "local",
            "cwd": ".",
            "env_passthrough": [],
            "home_mode": "profile",
            "timeout": 300
        },
        "platform_toolsets": {"cli": ["file", "terminal"]},
        "kanban": {"dispatch_in_gateway": false}
    }))
}

fn encoded(value: &Value) -> Result<Vec<u8>, BootstrapError> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| BootstrapError::Serialization(error.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn valid_profile(profile: &ProfileBootstrapSpec) -> bool {
    valid_id(&profile.name)
        && valid_id(&profile.provider)
        && profile.provider != "cursor"
        && valid_id(&profile.model)
        && !matches!(
            profile.model.to_ascii_lowercase().as_str(),
            "auto" | "default" | "latest"
        )
        && matches!(
            profile.reasoning_effort.as_str(),
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
        )
        && profile.skills.len() == 2
        && profile.skills.iter().all(|skill| valid_id(skill))
        && profile
            .skills
            .iter()
            .any(|skill| skill == "workflow-contract")
        && profile.skills.iter().any(|skill| skill == &profile.name)
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max
        && value.chars().all(|character| !character.is_control())
}

fn preflight_managed_directory(
    path: &Path,
    marker_name: &str,
    expected: &OwnershipMarker,
) -> Result<(), BootstrapError> {
    if !path.exists() {
        return Ok(());
    }
    real_directory(path)?;
    let marker = path.join(marker_name);
    if !marker.exists() {
        let mut entries = fs::read_dir(path).map_err(fs_error)?;
        if entries.next().transpose().map_err(fs_error)?.is_some() {
            return Err(BootstrapError::UnmanagedPath(path.to_owned()));
        }
        return Ok(());
    }
    let marker = regular_file(&marker, 16 * 1024)?;
    let actual: OwnershipMarker = serde_json::from_slice(&fs::read(&marker).map_err(fs_error)?)
        .map_err(|_| BootstrapError::ManagedPathDrift(marker.clone()))?;
    if actual.schema_version != expected.schema_version
        || actual.owner != expected.owner
        || actual.kind != expected.kind
        || actual.name != expected.name
    {
        return Err(BootstrapError::ManagedPathDrift(marker));
    }
    Ok(())
}

fn preflight_root(root: &Path) -> Result<(), BootstrapError> {
    real_directory(root)?;
    let marker = root.join(ROOT_MARKER);
    if marker.exists() {
        return preflight_managed_directory(root, ROOT_MARKER, &root_marker());
    }
    let entries = fs::read_dir(root)
        .map_err(fs_error)?
        .map(|entry| {
            entry
                .map_err(fs_error)?
                .file_name()
                .into_string()
                .map_err(|_| BootstrapError::UnsafePath(root.to_owned()))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if entries == BTreeSet::from(["auth.json".to_owned()])
        || entries == BTreeSet::from(["auth.json".to_owned(), "profiles".to_owned()])
    {
        Ok(())
    } else {
        Err(BootstrapError::UnmanagedPath(root.to_owned()))
    }
}

fn write_marker(root: &Path, name: &str, marker: &OwnershipMarker) -> Result<(), BootstrapError> {
    let bytes = encoded(
        &serde_json::to_value(marker)
            .map_err(|error| BootstrapError::Serialization(error.to_string()))?,
    )?;
    write_managed(&root.join(name), &bytes, 0o600)
}

fn write_managed(path: &Path, bytes: &[u8], mode: u32) -> Result<(), BootstrapError> {
    if path.exists() {
        regular_file(path, 4 * 1024 * 1024)?;
        if fs::read(path).map_err(fs_error)? == bytes {
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(fs_error)?;
            return Ok(());
        }
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| BootstrapError::UnsafePath(path.to_owned()))?;
    let temporary = path.with_file_name(format!(".{file_name}.pip-v2-tmp"));
    if temporary.exists() {
        return Err(BootstrapError::ManagedPathDrift(temporary));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temporary)
        .map_err(fs_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(fs_error(error));
    }
    fs::rename(&temporary, path).map_err(fs_error)?;
    Ok(())
}

fn ensure_directory(path: &Path, mode: u32) -> Result<(), BootstrapError> {
    if path.exists() {
        real_directory(path)?;
    } else {
        fs::create_dir(path).map_err(fs_error)?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(fs_error)
}

fn ensure_symlink(path: &Path, target: &Path) -> Result<(), BootstrapError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if fs::read_link(path).map_err(fs_error)? == target {
                Ok(())
            } else {
                Err(BootstrapError::ManagedPathDrift(path.to_owned()))
            }
        }
        Ok(_) => Err(BootstrapError::ManagedPathDrift(path.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            symlink(target, path).map_err(fs_error)
        }
        Err(error) => Err(fs_error(error)),
    }
}

fn reconcile_managed_symlink(path: &Path, target: &Path) -> Result<(), BootstrapError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if fs::read_link(path).map_err(fs_error)? == target {
                return Ok(());
            }
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| BootstrapError::UnsafePath(path.to_owned()))?;
            let temporary = path.with_file_name(format!(".{file_name}.pip-v2-tmp"));
            if fs::symlink_metadata(&temporary).is_ok() {
                return Err(BootstrapError::ManagedPathDrift(temporary));
            }
            symlink(target, &temporary).map_err(fs_error)?;
            fs::rename(&temporary, path).map_err(fs_error)
        }
        Ok(_) => Err(BootstrapError::ManagedPathDrift(path.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            symlink(target, path).map_err(fs_error)
        }
        Err(error) => Err(fs_error(error)),
    }
}

fn real_directory(path: &Path) -> Result<PathBuf, BootstrapError> {
    let metadata = fs::symlink_metadata(path).map_err(fs_error)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(BootstrapError::UnsafePath(path.to_owned()));
    }
    path.canonicalize().map_err(fs_error)
}

fn regular_file(path: &Path, max_bytes: u64) -> Result<PathBuf, BootstrapError> {
    let metadata = fs::symlink_metadata(path).map_err(fs_error)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > max_bytes {
        return Err(BootstrapError::UnsafePath(path.to_owned()));
    }
    path.canonicalize().map_err(fs_error)
}

fn fs_error(error: impl ToString) -> BootstrapError {
    BootstrapError::Filesystem(error.to_string())
}
