//! Durable, token-free GitHub webhook ingress spool.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENVELOPE_BYTES: usize = 6 * 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
pub struct WebhookSpoolInput<'a> {
    pub delivery_id: &'a str,
    pub event_name: &'a str,
    pub signature: &'a str,
    pub payload: &'a [u8],
    pub received_at: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpoolApplyResult {
    Stored,
    Replayed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpooledWebhook {
    pub delivery_id: String,
    pub event_name: String,
    pub signature: String,
    pub payload: Vec<u8>,
    pub received_at: u64,
}

#[derive(Debug)]
pub enum WebhookSpoolError {
    InvalidInput(&'static str),
    UnsafePath(PathBuf),
    DeliveryConflict,
    Filesystem(String),
    Serialization(String),
}

impl fmt::Display for WebhookSpoolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(formatter, "invalid webhook: {message}"),
            Self::UnsafePath(path) => write!(formatter, "unsafe spool path: {}", path.display()),
            Self::DeliveryConflict => formatter.write_str("webhook delivery conflict"),
            Self::Filesystem(error) => write!(formatter, "webhook spool filesystem error: {error}"),
            Self::Serialization(error) => {
                write!(formatter, "webhook spool serialization error: {error}")
            }
        }
    }
}

impl std::error::Error for WebhookSpoolError {}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SpoolEnvelope {
    spool_format: u32,
    delivery_id: String,
    event_name: String,
    signature: String,
    payload_sha256: String,
    payload_base64: String,
    received_at: u64,
}

pub struct WebhookSpool {
    root: PathBuf,
}

impl WebhookSpool {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, WebhookSpoolError> {
        let root = root.as_ref().to_path_buf();
        validate_directory(&root)?;
        for child in ["receipts", "pending", "processed"] {
            validate_directory(&root.join(child))?;
        }
        Ok(Self { root })
    }

    pub fn store(
        &self,
        input: WebhookSpoolInput<'_>,
        secret: &[u8],
    ) -> Result<SpoolApplyResult, WebhookSpoolError> {
        validate_input(input, secret)?;
        let digest = hex(&Sha256::digest(input.payload));
        let envelope = SpoolEnvelope {
            spool_format: 1,
            delivery_id: input.delivery_id.into(),
            event_name: input.event_name.into(),
            signature: input.signature.into(),
            payload_sha256: digest,
            payload_base64: STANDARD.encode(input.payload),
            received_at: input.received_at,
        };
        let encoded = serde_json::to_vec(&envelope)
            .map_err(|error| WebhookSpoolError::Serialization(error.to_string()))?;
        if encoded.len() > MAX_ENVELOPE_BYTES {
            return Err(WebhookSpoolError::InvalidInput(
                "encoded payload is too large",
            ));
        }

        let receipt = self
            .root
            .join("receipts")
            .join(format!("{}.json", input.delivery_id));
        let pending = self
            .root
            .join("pending")
            .join(format!("{}.json", input.delivery_id));
        let processed = self
            .root
            .join("processed")
            .join(format!("{}.json", input.delivery_id));
        if receipt.exists() {
            compare_existing(&receipt, &envelope)?;
            ensure_queued(&receipt, &pending, &processed, &envelope)?;
            return Ok(SpoolApplyResult::Replayed);
        }

        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = self.root.join("receipts").join(format!(
            ".{}.{}.{}.tmp",
            input.delivery_id,
            std::process::id(),
            sequence
        ));
        let write_result =
            write_temporary(&temporary, &encoded).and_then(|()| {
                match fs::hard_link(&temporary, &receipt) {
                    Ok(()) => Ok(true),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                    Err(error) => Err(WebhookSpoolError::Filesystem(error.to_string())),
                }
            });
        let _ = fs::remove_file(&temporary);
        let created = write_result?;
        if !created {
            compare_existing(&receipt, &envelope)?;
            ensure_queued(&receipt, &pending, &processed, &envelope)?;
            return Ok(SpoolApplyResult::Replayed);
        }
        sync_directory(&self.root.join("receipts"))?;
        ensure_queued(&receipt, &pending, &processed, &envelope)?;
        Ok(SpoolApplyResult::Stored)
    }

    pub fn next_pending(&self) -> Result<Option<SpooledWebhook>, WebhookSpoolError> {
        let pending_root = self.root.join("pending");
        validate_directory(&pending_root)?;
        let mut entries = fs::read_dir(&pending_root)
            .map_err(|error| WebhookSpoolError::Filesystem(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| WebhookSpoolError::Filesystem(error.to_string()))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        let Some(entry) = entries.first() else {
            return Ok(None);
        };
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| WebhookSpoolError::UnsafePath(entry.path()))?;
        let delivery_id = name
            .strip_suffix(".json")
            .filter(|value| valid_delivery_id(value))
            .ok_or_else(|| WebhookSpoolError::UnsafePath(entry.path()))?;
        let envelope = read_envelope(&entry.path(), delivery_id)?;
        Ok(Some(SpooledWebhook {
            delivery_id: envelope.delivery_id,
            event_name: envelope.event_name,
            signature: envelope.signature,
            payload: STANDARD
                .decode(&envelope.payload_base64)
                .map_err(|_| WebhookSpoolError::InvalidInput("invalid payload encoding"))?,
            received_at: envelope.received_at,
        }))
    }

    pub fn mark_processed(&self, delivery_id: &str) -> Result<(), WebhookSpoolError> {
        if !valid_delivery_id(delivery_id) {
            return Err(WebhookSpoolError::InvalidInput("invalid delivery ID"));
        }
        let pending_root = self.root.join("pending");
        let processed_root = self.root.join("processed");
        validate_directory(&pending_root)?;
        validate_directory(&processed_root)?;
        let name = format!("{delivery_id}.json");
        let pending = pending_root.join(&name);
        let processed = processed_root.join(name);
        if processed.exists() {
            if pending.exists() {
                compare_files(&pending, &processed)?;
                fs::remove_file(&pending)
                    .map_err(|error| WebhookSpoolError::Filesystem(error.to_string()))?;
                sync_directory(&pending_root)?;
            }
            return Ok(());
        }
        match fs::hard_link(&pending, &processed) {
            Ok(()) => sync_directory(&processed_root)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                compare_files(&pending, &processed)?;
            }
            Err(error) => return Err(WebhookSpoolError::Filesystem(error.to_string())),
        }
        fs::remove_file(&pending)
            .map_err(|error| WebhookSpoolError::Filesystem(error.to_string()))?;
        sync_directory(&pending_root)
    }
}

fn validate_input(input: WebhookSpoolInput<'_>, secret: &[u8]) -> Result<(), WebhookSpoolError> {
    validate_webhook_authentication(input.delivery_id, input.signature, input.payload, secret)?;
    if input.event_name != "issues" {
        return Err(WebhookSpoolError::InvalidInput("unsupported event"));
    }
    if input.received_at == 0 {
        return Err(WebhookSpoolError::InvalidInput("invalid receive time"));
    }
    Ok(())
}

pub(crate) fn validate_webhook_authentication(
    delivery_id: &str,
    signature: &str,
    payload: &[u8],
    secret: &[u8],
) -> Result<(), WebhookSpoolError> {
    if !valid_delivery_id(delivery_id) {
        return Err(WebhookSpoolError::InvalidInput("invalid delivery ID"));
    }
    if payload.is_empty() || payload.len() > MAX_PAYLOAD_BYTES {
        return Err(WebhookSpoolError::InvalidInput("invalid payload size"));
    }
    if secret.is_empty()
        || secret.len() > 1024
        || !pip_github::verify_webhook(secret, payload, signature)
    {
        return Err(WebhookSpoolError::InvalidInput("signature rejected"));
    }
    Ok(())
}

fn ensure_queued(
    receipt: &Path,
    pending: &Path,
    processed: &Path,
    expected: &SpoolEnvelope,
) -> Result<(), WebhookSpoolError> {
    if processed.exists() {
        compare_existing(processed, expected)?;
        if pending.exists() {
            compare_existing(pending, expected)?;
            return Err(WebhookSpoolError::DeliveryConflict);
        }
        return Ok(());
    }
    match fs::hard_link(receipt, pending) {
        Ok(()) => sync_directory(
            pending
                .parent()
                .ok_or_else(|| WebhookSpoolError::UnsafePath(pending.to_path_buf()))?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            compare_existing(pending, expected)
        }
        Err(error) => Err(WebhookSpoolError::Filesystem(error.to_string())),
    }
}

fn compare_existing(path: &Path, expected: &SpoolEnvelope) -> Result<(), WebhookSpoolError> {
    let bytes = read_bounded_file(path, MAX_ENVELOPE_BYTES)?;
    let existing: SpoolEnvelope = serde_json::from_slice(&bytes)
        .map_err(|error| WebhookSpoolError::Serialization(error.to_string()))?;
    if existing.spool_format != 1
        || existing.delivery_id != expected.delivery_id
        || existing.event_name != expected.event_name
        || existing.signature != expected.signature
        || existing.payload_sha256 != expected.payload_sha256
        || existing.payload_base64 != expected.payload_base64
    {
        return Err(WebhookSpoolError::DeliveryConflict);
    }
    Ok(())
}

fn read_envelope(
    path: &Path,
    expected_delivery_id: &str,
) -> Result<SpoolEnvelope, WebhookSpoolError> {
    let bytes = read_bounded_file(path, MAX_ENVELOPE_BYTES)?;
    let envelope: SpoolEnvelope = serde_json::from_slice(&bytes)
        .map_err(|error| WebhookSpoolError::Serialization(error.to_string()))?;
    let payload = STANDARD
        .decode(&envelope.payload_base64)
        .map_err(|_| WebhookSpoolError::InvalidInput("invalid payload encoding"))?;
    let digest = hex(&Sha256::digest(&payload));
    if envelope.spool_format != 1
        || envelope.delivery_id != expected_delivery_id
        || envelope.event_name != "issues"
        || envelope.signature.is_empty()
        || !envelope.signature.is_ascii()
        || payload.is_empty()
        || payload.len() > MAX_PAYLOAD_BYTES
        || STANDARD.encode(&payload) != envelope.payload_base64
        || envelope.payload_sha256 != digest
        || envelope.received_at == 0
    {
        return Err(WebhookSpoolError::InvalidInput("invalid spool envelope"));
    }
    Ok(envelope)
}

fn compare_files(left: &Path, right: &Path) -> Result<(), WebhookSpoolError> {
    let left_bytes = read_bounded_file(left, MAX_ENVELOPE_BYTES)?;
    let right_bytes = read_bounded_file(right, MAX_ENVELOPE_BYTES)?;
    if left_bytes != right_bytes {
        return Err(WebhookSpoolError::DeliveryConflict);
    }
    Ok(())
}

fn write_temporary(path: &Path, bytes: &[u8]) -> Result<(), WebhookSpoolError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o640)
        .open(path)
        .map_err(|error| WebhookSpoolError::Filesystem(error.to_string()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| WebhookSpoolError::Filesystem(error.to_string()))
}

fn read_bounded_file(path: &Path, max_bytes: usize) -> Result<Vec<u8>, WebhookSpoolError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| WebhookSpoolError::Filesystem(error.to_string()))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX)
    {
        return Err(WebhookSpoolError::UnsafePath(path.to_path_buf()));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(max_bytes));
    File::open(path)
        .and_then(|mut file| {
            Read::by_ref(&mut file)
                .take(u64::try_from(max_bytes.saturating_add(1)).unwrap_or(u64::MAX))
                .read_to_end(&mut bytes)
        })
        .map_err(|error| WebhookSpoolError::Filesystem(error.to_string()))?;
    if bytes.len() > max_bytes {
        return Err(WebhookSpoolError::UnsafePath(path.to_path_buf()));
    }
    Ok(bytes)
}

fn validate_directory(path: &Path) -> Result<(), WebhookSpoolError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| WebhookSpoolError::UnsafePath(path.to_path_buf()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(WebhookSpoolError::UnsafePath(path.to_path_buf()));
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), WebhookSpoolError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| WebhookSpoolError::Filesystem(error.to_string()))
}

fn valid_delivery_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
