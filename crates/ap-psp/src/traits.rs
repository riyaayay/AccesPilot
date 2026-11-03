//! Payment provider abstraction and lifecycle types.

use ap_core::money::Currency;
use async_trait::async_trait;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureClass {
    Declined,
    InsufficientFunds,
    Timeout,
    ProviderDown,
    InvalidRequest,
}

impl FailureClass {
    /// Pure function: returns true if the error class is safe to retry with the same idempotency key.
    pub fn is_safe_to_retry(&self) -> bool {
        match self {
            FailureClass::Timeout | FailureClass::ProviderDown => true,
            FailureClass::Declined
            | FailureClass::InsufficientFunds
            | FailureClass::InvalidRequest => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderStatus {
    Succeeded,
    Failed(FailureClass),
    Processing,
    ActionRequired,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChargeRequest {
    pub payment_id: String,
    pub amount_minor: i64,
    pub currency: Currency,
    pub customer_id: String,
    pub task_id: Uuid,
    pub purchase_id: Uuid,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderPayment {
    pub provider_payment_id: String,
    pub amount_minor: i64,
    pub currency: Currency,
    pub status: ProviderStatus,
    pub error_message: Option<String>,
    pub raw_response: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefundRequest {
    pub refund_id: String,
    pub provider_payment_id: String,
    pub amount_minor: i64,
    pub currency: Currency,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRefund {
    pub refund_id: String,
    pub provider_payment_id: String,
    pub amount_minor: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookTrigger {
    pub provider_payment_id: String,
    pub event_id: String,
    pub event_type: String,
}

#[derive(Error, Debug)]
pub enum ProviderError {
    #[error("Network error: {0}")]
    Network(String),
    #[error("Timeout error: {0}")]
    Timeout(String),
    #[error("API error ({status}): {body}")]
    Api { status: u16, body: String },
    #[error("Provider down ({0})")]
    ProviderDown(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Unknown provider error: {0}")]
    Unknown(String),
}

#[derive(Error, Debug)]
pub enum WebhookError {
    #[error("Missing signature header")]
    MissingSignature,
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Malformed webhook payload: {0}")]
    MalformedPayload(String),
}

#[async_trait]
pub trait PaymentProvider: Send + Sync {
    async fn charge(&self, req: ChargeRequest) -> Result<ProviderPayment, ProviderError>;
    async fn retrieve(
        &self,
        provider_payment_id: &str,
    ) -> Result<Option<ProviderPayment>, ProviderError>;
    async fn refund(&self, req: RefundRequest) -> Result<ProviderRefund, ProviderError>;
    fn verify_webhook(
        &self,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<WebhookTrigger, WebhookError>;
}
