use std::fs;
use std::os::unix::fs::MetadataExt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use pip_control::{SpoolApplyResult, WebhookSpool, WebhookSpoolInput};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[test]
fn verified_delivery_is_spooled_atomically_with_exact_raw_body() {
    let directory = tempfile::tempdir().unwrap();
    prepare(directory.path());
    let spool = WebhookSpool::open(directory.path()).unwrap();
    let payload = b"{\"raw\":\"bytes\\nremain exact\"}";
    let signature = signature(b"webhook-secret", payload);

    let result = spool
        .store(
            WebhookSpoolInput {
                delivery_id: "01234567-89ab-cdef-0123-456789abcdef",
                event_name: "issues",
                signature: &signature,
                payload,
                received_at: 1_788_290_400,
            },
            b"webhook-secret",
        )
        .unwrap();

    assert_eq!(result, SpoolApplyResult::Stored);
    let path = directory
        .path()
        .join("pending/01234567-89ab-cdef-0123-456789abcdef.json");
    assert!(path.is_file());
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o640);
    assert!(
        fs::read_dir(directory.path().join("pending"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
    );
    let envelope: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(envelope["spool_format"], 1);
    assert_eq!(
        envelope["delivery_id"],
        "01234567-89ab-cdef-0123-456789abcdef"
    );
    assert_eq!(envelope["event_name"], "issues");
    assert_eq!(envelope["signature"], signature);
    assert_eq!(envelope["received_at"], 1_788_290_400_u64);
    assert_eq!(
        STANDARD
            .decode(envelope["payload_base64"].as_str().unwrap())
            .unwrap(),
        payload
    );
}

#[test]
fn exact_replay_is_idempotent_and_delivery_conflict_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    prepare(directory.path());
    let spool = WebhookSpool::open(directory.path()).unwrap();
    let delivery_id = "01234567-89ab-cdef-0123-456789abcdef";
    let first = b"first";
    let first_signature = signature(b"secret", first);
    let input = WebhookSpoolInput {
        delivery_id,
        event_name: "issues",
        signature: &first_signature,
        payload: first,
        received_at: 100,
    };

    assert_eq!(
        spool.store(input, b"secret").unwrap(),
        SpoolApplyResult::Stored
    );
    assert_eq!(
        spool.store(input, b"secret").unwrap(),
        SpoolApplyResult::Replayed
    );
    let conflicting = b"different";
    let conflicting_signature = signature(b"secret", conflicting);
    let error = spool
        .store(
            WebhookSpoolInput {
                delivery_id,
                event_name: "issues",
                signature: &conflicting_signature,
                payload: conflicting,
                received_at: 101,
            },
            b"secret",
        )
        .unwrap_err();
    assert!(error.to_string().contains("delivery conflict"));
    assert_eq!(
        fs::read_dir(directory.path().join("pending"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn invalid_headers_signature_size_or_filesystem_never_create_a_spool_item() {
    let directory = tempfile::tempdir().unwrap();
    prepare(directory.path());
    let spool = WebhookSpool::open(directory.path()).unwrap();
    let valid_payload = b"{}";
    let valid_signature = signature(b"secret", valid_payload);

    for input in [
        WebhookSpoolInput {
            delivery_id: "../../escape",
            event_name: "issues",
            signature: &valid_signature,
            payload: valid_payload,
            received_at: 1,
        },
        WebhookSpoolInput {
            delivery_id: "01234567-89ab-cdef-0123-456789abcdef",
            event_name: "pull_request",
            signature: &valid_signature,
            payload: valid_payload,
            received_at: 1,
        },
        WebhookSpoolInput {
            delivery_id: "01234567-89ab-cdef-0123-456789abcdef",
            event_name: "issues",
            signature: "sha256=00",
            payload: valid_payload,
            received_at: 1,
        },
    ] {
        assert!(spool.store(input, b"secret").is_err());
    }
    assert_eq!(
        fs::read_dir(directory.path().join("pending"))
            .unwrap()
            .count(),
        0
    );

    let unsafe_root = tempfile::tempdir().unwrap();
    fs::create_dir(unsafe_root.path().join("real-pending")).unwrap();
    fs::create_dir(unsafe_root.path().join("processed")).unwrap();
    std::os::unix::fs::symlink(
        unsafe_root.path().join("real-pending"),
        unsafe_root.path().join("pending"),
    )
    .unwrap();
    assert!(WebhookSpool::open(unsafe_root.path()).is_err());
}

#[test]
fn processing_moves_the_ingress_owned_inode_without_creating_a_new_hard_link() {
    let implementation = include_str!("../src/webhook_spool.rs");

    assert!(implementation.contains("fs::rename(&pending, &processed)"));
    assert!(!implementation.contains("fs::hard_link(&pending, &processed)"));
}

fn prepare(root: &std::path::Path) {
    fs::create_dir(root.join("receipts")).unwrap();
    fs::create_dir(root.join("pending")).unwrap();
    fs::create_dir(root.join("processed")).unwrap();
}

fn signature(secret: &[u8], payload: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).unwrap();
    mac.update(payload);
    format!("sha256={}", hex(mac.finalize().into_bytes().as_slice()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
