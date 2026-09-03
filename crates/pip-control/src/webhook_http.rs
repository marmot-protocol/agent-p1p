//! Loopback HTTP boundary for the isolated webhook ingress identity.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::timeout::TimeoutLayer;

use crate::webhook_spool::validate_webhook_authentication;
use crate::{SpoolApplyResult, WebhookSpool, WebhookSpoolError, WebhookSpoolInput};

const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct WebhookIngressState {
    spool: Arc<WebhookSpool>,
    secret: Arc<WebhookSecret>,
}

struct WebhookSecret(Vec<u8>);

impl Drop for WebhookSecret {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

impl WebhookIngressState {
    pub fn new(spool: WebhookSpool, secret: Vec<u8>) -> Result<Self, WebhookSpoolError> {
        if secret.is_empty() || secret.len() > 1024 {
            return Err(WebhookSpoolError::InvalidInput("invalid secret"));
        }
        Ok(Self {
            spool: Arc::new(spool),
            secret: Arc::new(WebhookSecret(secret)),
        })
    }
}

pub fn webhook_ingress_router(state: WebhookIngressState) -> Router {
    Router::new()
        .route("/github", post(receive_github_webhook))
        .layer(DefaultBodyLimit::max(MAX_PAYLOAD_BYTES))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(10),
        ))
        .layer(ConcurrencyLimitLayer::new(4))
        .with_state(state)
}

async fn receive_github_webhook(
    State(state): State<WebhookIngressState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let Some(content_type) = one_header(&headers, "content-type") else {
        return StatusCode::BAD_REQUEST;
    };
    if !content_type
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
    {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE;
    }
    let (Some(delivery_id), Some(event_name), Some(signature)) = (
        one_header(&headers, "x-github-delivery"),
        one_header(&headers, "x-github-event"),
        one_header(&headers, "x-hub-signature-256"),
    ) else {
        return StatusCode::BAD_REQUEST;
    };
    if event_name == "ping" {
        return match validate_webhook_authentication(delivery_id, signature, &body, &state.secret.0)
        {
            Ok(()) => StatusCode::NO_CONTENT,
            Err(_) => StatusCode::BAD_REQUEST,
        };
    }
    let Ok(received_at) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    let spool = Arc::clone(&state.spool);
    let secret = Arc::clone(&state.secret);
    let delivery_id = delivery_id.to_owned();
    let event_name = event_name.to_owned();
    let signature = signature.to_owned();
    let payload = body.to_vec();
    let outcome = tokio::task::spawn_blocking(move || {
        spool.store(
            WebhookSpoolInput {
                delivery_id: &delivery_id,
                event_name: &event_name,
                signature: &signature,
                payload: &payload,
                received_at: received_at.as_secs(),
            },
            &secret.0,
        )
    })
    .await;
    match outcome {
        Ok(Ok(SpoolApplyResult::Stored | SpoolApplyResult::Replayed)) => StatusCode::ACCEPTED,
        Ok(Err(WebhookSpoolError::InvalidInput(_))) => StatusCode::BAD_REQUEST,
        Ok(Err(WebhookSpoolError::DeliveryConflict)) => StatusCode::CONFLICT,
        Ok(Err(
            WebhookSpoolError::UnsafePath(_)
            | WebhookSpoolError::Filesystem(_)
            | WebhookSpoolError::Serialization(_),
        ))
        | Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

#[derive(Debug)]
pub enum WebhookIngressError {
    InvalidArgument(String),
    UnsafeInput(PathBuf),
    Filesystem(String),
    Spool(WebhookSpoolError),
    Runtime(String),
}

impl fmt::Display for WebhookIngressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument(argument) => write!(formatter, "invalid argument: {argument}"),
            Self::UnsafeInput(path) => write!(formatter, "unsafe input: {}", path.display()),
            Self::Filesystem(error) => write!(formatter, "filesystem error: {error}"),
            Self::Spool(error) => error.fmt(formatter),
            Self::Runtime(error) => write!(formatter, "runtime error: {error}"),
        }
    }
}

impl std::error::Error for WebhookIngressError {}

impl From<WebhookSpoolError> for WebhookIngressError {
    fn from(error: WebhookSpoolError) -> Self {
        Self::Spool(error)
    }
}

pub fn run_webhook_ingress_cli(arguments: &[String]) -> Result<(), WebhookIngressError> {
    let options = ingress_options(arguments)?;
    let listen = required(&options, "--listen")?
        .parse::<SocketAddr>()
        .map_err(|_| WebhookIngressError::InvalidArgument("--listen".into()))?;
    if !listen.ip().is_loopback() || listen.port() == 0 {
        return Err(WebhookIngressError::InvalidArgument("--listen".into()));
    }
    let secret = read_secret(Path::new(required(&options, "--webhook-secret")?), 1024)?;
    let spool = WebhookSpool::open(required(&options, "--spool")?)?;
    let state = WebhookIngressState::new(spool, secret)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| WebhookIngressError::Runtime(error.to_string()))?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(listen)
            .await
            .map_err(|error| WebhookIngressError::Runtime(error.to_string()))?;
        axum::serve(listener, webhook_ingress_router(state))
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(|error| WebhookIngressError::Runtime(error.to_string()))
    })
}

async fn shutdown_signal() {
    let terminate = async {
        let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        else {
            std::future::pending::<()>().await;
            return;
        };
        signal.recv().await;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        () = terminate => {},
    }
}

fn ingress_options(arguments: &[String]) -> Result<BTreeMap<String, String>, WebhookIngressError> {
    if !arguments.len().is_multiple_of(2) {
        return Err(WebhookIngressError::InvalidArgument("options".into()));
    }
    let allowed = ["--listen", "--spool", "--webhook-secret"];
    let mut options = BTreeMap::new();
    for pair in arguments.chunks_exact(2) {
        if !allowed.contains(&pair[0].as_str())
            || pair[1].is_empty()
            || options.insert(pair[0].clone(), pair[1].clone()).is_some()
        {
            return Err(WebhookIngressError::InvalidArgument(pair[0].clone()));
        }
    }
    if allowed.iter().any(|name| !options.contains_key(*name)) {
        return Err(WebhookIngressError::InvalidArgument("options".into()));
    }
    Ok(options)
}

fn required<'a>(
    options: &'a BTreeMap<String, String>,
    name: &str,
) -> Result<&'a str, WebhookIngressError> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| WebhookIngressError::InvalidArgument(name.into()))
}

fn read_secret(path: &Path, max_bytes: usize) -> Result<Vec<u8>, WebhookIngressError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| WebhookIngressError::Filesystem(error.to_string()))?;
    if !crate::secret_file::metadata_is_safe_secret_file(&metadata)
        || metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX)
    {
        return Err(WebhookIngressError::UnsafeInput(path.to_path_buf()));
    }
    let mut secret = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(max_bytes));
    File::open(path)
        .and_then(|mut file| {
            Read::by_ref(&mut file)
                .take(u64::try_from(max_bytes.saturating_add(1)).unwrap_or(u64::MAX))
                .read_to_end(&mut secret)
        })
        .map_err(|error| WebhookIngressError::Filesystem(error.to_string()))?;
    while matches!(secret.last(), Some(b'\n' | b'\r')) {
        secret.pop();
    }
    if secret.is_empty() || secret.len() > max_bytes {
        secret.fill(0);
        return Err(WebhookIngressError::UnsafeInput(path.to_path_buf()));
    }
    Ok(secret)
}

fn one_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() || value.is_empty() || !value.is_ascii() {
        return None;
    }
    Some(value)
}
