//! Controller-owned transfer from the verified ingress spool to the ledger.

use std::fmt;

use pip_store::Store;
use serde::Serialize;

use crate::{
    ActiveIntakeError, IntakeSource, RepositoryPolicy, WebhookEnvelope, WebhookIntakeReport,
    WebhookSpool, WebhookSpoolError, ingest_webhook,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WebhookSpoolCycle {
    pub report_format: u32,
    pub result: String,
    pub delivery_id: Option<String>,
    pub intake: Option<WebhookIntakeReport>,
}

#[derive(Debug)]
pub enum WebhookSpoolConsumerError {
    Spool(WebhookSpoolError),
    Intake(ActiveIntakeError),
}

impl fmt::Display for WebhookSpoolConsumerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spool(error) => error.fmt(formatter),
            Self::Intake(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for WebhookSpoolConsumerError {}

impl From<WebhookSpoolError> for WebhookSpoolConsumerError {
    fn from(error: WebhookSpoolError) -> Self {
        Self::Spool(error)
    }
}

impl From<ActiveIntakeError> for WebhookSpoolConsumerError {
    fn from(error: ActiveIntakeError) -> Self {
        Self::Intake(error)
    }
}

pub fn consume_webhook_spool_once<S: IntakeSource>(
    source: &S,
    policy: &RepositoryPolicy,
    store: &mut Store,
    spool: &WebhookSpool,
    secret: &[u8],
    observed_at: u64,
    global_paused: bool,
) -> Result<WebhookSpoolCycle, WebhookSpoolConsumerError> {
    let Some(pending) = spool.next_pending()? else {
        return Ok(WebhookSpoolCycle {
            report_format: 1,
            result: "EMPTY".into(),
            delivery_id: None,
            intake: None,
        });
    };
    let intake = ingest_webhook(
        source,
        policy,
        store,
        WebhookEnvelope {
            delivery_id: &pending.delivery_id,
            event_name: &pending.event_name,
            signature: &pending.signature,
            payload: &pending.payload,
            received_at: pending.received_at,
        },
        secret,
        observed_at,
        global_paused,
    )?;
    spool.mark_processed(&pending.delivery_id)?;
    Ok(WebhookSpoolCycle {
        report_format: 1,
        result: "PROCESSED".into(),
        delivery_id: Some(pending.delivery_id),
        intake: Some(intake),
    })
}
