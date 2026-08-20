//! Strict operator command parsing and machine-readable output.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use pip_store::Store;
use serde_json::{Value, json};

use crate::{ReleaseError, verify_release};

#[derive(Debug)]
pub enum CliError {
    Usage(&'static str),
    InvalidArgument(String),
    UnsafeInput(PathBuf),
    InputTooLarge(PathBuf),
    Filesystem(String),
    Ledger(String),
    Release(ReleaseError),
    Clock,
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => formatter.write_str(message),
            Self::InvalidArgument(argument) => write!(formatter, "invalid argument: {argument}"),
            Self::UnsafeInput(path) => write!(
                formatter,
                "input is not a regular non-symlink file: {}",
                path.display()
            ),
            Self::InputTooLarge(path) => write!(
                formatter,
                "input exceeded its size bound: {}",
                path.display()
            ),
            Self::Filesystem(error) => write!(formatter, "input filesystem error: {error}"),
            Self::Ledger(error) => write!(formatter, "ledger status failed: {error}"),
            Self::Release(error) => error.fmt(formatter),
            Self::Clock => formatter.write_str("system clock is before the Unix epoch"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<ReleaseError> for CliError {
    fn from(error: ReleaseError) -> Self {
        Self::Release(error)
    }
}

pub fn run_cli(arguments: impl IntoIterator<Item = String>) -> Result<Value, CliError> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let Some(command) = arguments.first().map(String::as_str) else {
        return Err(CliError::Usage("a pip-control command is required"));
    };
    match command {
        "status" => status(&arguments[1..]),
        "verify-release" => verify(&arguments[1..]),
        _ => Err(CliError::InvalidArgument(command.into())),
    }
}

fn status(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(arguments, &["--database"], &["--now"])?;
    let database = required(&options, "--database")?;
    let now = options
        .get("--now")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| CliError::InvalidArgument("--now".into()))
        })
        .transpose()?
        .map_or_else(current_time, Ok)?;
    let store =
        Store::open_read_only(database).map_err(|error| CliError::Ledger(error.to_string()))?;
    let ledger = store
        .status(now)
        .map_err(|error| CliError::Ledger(error.to_string()))?;
    Ok(json!({
        "ok": true,
        "observed_at": now,
        "ledger": ledger,
    }))
}

fn verify(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(
        arguments,
        &[
            "--release-root",
            "--manifest",
            "--signature",
            "--public-key",
        ],
        &[],
    )?;
    let release_root = required(&options, "--release-root")?;
    let manifest = read_bounded(Path::new(required(&options, "--manifest")?), 1024 * 1024)?;
    let signature = read_bounded(Path::new(required(&options, "--signature")?), 1024)?;
    let public_key = read_bounded(Path::new(required(&options, "--public-key")?), 1024)?;
    let signature = std::str::from_utf8(&signature)
        .map_err(|_| CliError::InvalidArgument("--signature".into()))?;
    let public_key = std::str::from_utf8(&public_key)
        .map_err(|_| CliError::InvalidArgument("--public-key".into()))?;
    let verified = verify_release(release_root, &manifest, signature, public_key)?;
    Ok(json!({
        "ok": true,
        "source_commit": verified.source_commit(),
        "binary_path": verified.binary_path(),
        "artifact_count": verified.artifact_count(),
    }))
}

fn options(
    arguments: &[String],
    required_names: &[&str],
    optional_names: &[&str],
) -> Result<BTreeMap<String, String>, CliError> {
    if !arguments.len().is_multiple_of(2) {
        return Err(CliError::Usage("command options must be name/value pairs"));
    }
    let allowed = required_names
        .iter()
        .chain(optional_names)
        .copied()
        .collect::<Vec<_>>();
    let mut result = BTreeMap::new();
    for pair in arguments.chunks_exact(2) {
        if !allowed.contains(&pair[0].as_str()) || pair[1].is_empty() {
            return Err(CliError::InvalidArgument(pair[0].clone()));
        }
        if result.insert(pair[0].clone(), pair[1].clone()).is_some() {
            return Err(CliError::InvalidArgument(pair[0].clone()));
        }
    }
    if required_names
        .iter()
        .any(|name| !result.contains_key(*name))
    {
        return Err(CliError::Usage("one or more required options are missing"));
    }
    Ok(result)
}

fn required<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, CliError> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or(CliError::Usage("required option is missing"))
}

fn current_time() -> Result<u64, CliError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| CliError::Clock)
}

fn read_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, CliError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| CliError::Filesystem(error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CliError::UnsafeInput(path.to_owned()));
    }
    if metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
        return Err(CliError::InputTooLarge(path.to_owned()));
    }
    let mut file = File::open(path).map_err(|error| CliError::Filesystem(error.to_string()))?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(max_bytes));
    file.by_ref()
        .take(u64::try_from(max_bytes.saturating_add(1)).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|error| CliError::Filesystem(error.to_string()))?;
    if bytes.len() > max_bytes {
        return Err(CliError::InputTooLarge(path.to_owned()));
    }
    Ok(bytes)
}
