use std::fs;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hmac::{Hmac, KeyInit, Mac};
use pip_control::{WebhookIngressState, WebhookSpool, webhook_ingress_router};
use sha2::Sha256;
use tower::ServiceExt as _;

type HmacSha256 = Hmac<Sha256>;

#[tokio::test]
async fn http_ingress_accepts_only_signed_bounded_github_issue_deliveries() {
    let directory = tempfile::tempdir().unwrap();
    for child in ["receipts", "pending", "processed"] {
        fs::create_dir(directory.path().join(child)).unwrap();
    }
    let state = WebhookIngressState::new(
        WebhookSpool::open(directory.path()).unwrap(),
        b"webhook-secret".to_vec(),
    )
    .unwrap();
    let app = webhook_ingress_router(state);
    let payload = b"{\"action\":\"labeled\"}";
    let signature = signature(b"webhook-secret", payload);
    let request = Request::builder()
        .method("POST")
        .uri("/github")
        .header("content-type", "application/json")
        .header("x-github-delivery", "01234567-89ab-cdef-0123-456789abcdef")
        .header("x-github-event", "issues")
        .header("x-hub-signature-256", signature)
        .body(Body::from(payload.as_slice()))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(
        directory
            .path()
            .join("pending/01234567-89ab-cdef-0123-456789abcdef.json")
            .is_file()
    );

    let missing_signature = Request::builder()
        .method("POST")
        .uri("/github")
        .header("content-type", "application/json")
        .header("x-github-delivery", "11234567-89ab-cdef-0123-456789abcdef")
        .header("x-github-event", "issues")
        .body(Body::from(payload.as_slice()))
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(missing_signature)
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );

    let wrong_path = Request::builder()
        .method("POST")
        .uri("/")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(wrong_path).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );

    let oversized = Request::builder()
        .method("POST")
        .uri("/github")
        .header("content-type", "application/json")
        .header("x-github-delivery", "21234567-89ab-cdef-0123-456789abcdef")
        .header("x-github-event", "issues")
        .header("x-hub-signature-256", "sha256=00")
        .body(Body::from(vec![b'x'; 4 * 1024 * 1024 + 1]))
        .unwrap();
    assert_eq!(
        app.oneshot(oversized).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn http_ingress_authenticates_github_ping_without_spooling_it() {
    let directory = tempfile::tempdir().unwrap();
    for child in ["receipts", "pending", "processed"] {
        fs::create_dir(directory.path().join(child)).unwrap();
    }
    let state = WebhookIngressState::new(
        WebhookSpool::open(directory.path()).unwrap(),
        b"webhook-secret".to_vec(),
    )
    .unwrap();
    let app = webhook_ingress_router(state);
    let payload = b"{\"zen\":\"Keep it logically awesome.\"}";
    let valid_ping = Request::builder()
        .method("POST")
        .uri("/github")
        .header("content-type", "application/json")
        .header("x-github-delivery", "31234567-89ab-cdef-0123-456789abcdef")
        .header("x-github-event", "ping")
        .header("x-hub-signature-256", signature(b"webhook-secret", payload))
        .body(Body::from(payload.as_slice()))
        .unwrap();

    assert_eq!(
        app.clone().oneshot(valid_ping).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    for child in ["receipts", "pending", "processed"] {
        assert_eq!(
            fs::read_dir(directory.path().join(child)).unwrap().count(),
            0
        );
    }

    let invalid_ping = Request::builder()
        .method("POST")
        .uri("/github")
        .header("content-type", "application/json")
        .header("x-github-delivery", "41234567-89ab-cdef-0123-456789abcdef")
        .header("x-github-event", "ping")
        .header("x-hub-signature-256", "sha256=00")
        .body(Body::from(payload.as_slice()))
        .unwrap();
    assert_eq!(
        app.oneshot(invalid_ping).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
}

fn signature(secret: &[u8], payload: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).unwrap();
    mac.update(payload);
    format!(
        "sha256={}",
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}
