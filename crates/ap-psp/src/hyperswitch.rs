//! Hyperswitch payment adapter targeting Juspay's open-source payment switch sandbox.

use crate::traits::{
    ChargeRequest, FailureClass, PaymentProvider, ProviderError, ProviderPayment, ProviderRefund,
    ProviderStatus, RefundRequest, WebhookError, WebhookTrigger,
};
use ap_core::money::Currency;
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use reqwest::header::HeaderMap;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Sha256, Sha512};
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

type HmacSha512 = Hmac<Sha512>;
type HmacSha256 = Hmac<Sha256>;

pub const DEFAULT_HYPERSWITCH_BASE_URL: &str = "https://sandbox.hyperswitch.io";

#[derive(Clone)]
pub struct HyperswitchPsp {
    client: Client,
    base_url: String,
    api_key: String,
    webhook_secret: Option<String>,
}

#[derive(Serialize)]
struct HyperswitchPaymentRequest<'a> {
    amount: i64,
    currency: &'a str,
    confirm: bool,
    capture_method: &'a str,
    payment_id: &'a str,
    customer_id: &'a str,
    metadata: Value,
}

#[derive(Deserialize)]
struct HyperswitchPaymentResponse {
    payment_id: Option<String>,
    amount: Option<i64>,
    currency: Option<String>,
    status: Option<String>,
    error_code: Option<String>,
    error_message: Option<String>,
}

#[derive(Serialize)]
struct HyperswitchRefundRequest<'a> {
    payment_id: &'a str,
    amount: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
}

#[derive(Deserialize)]
struct HyperswitchRefundResponse {
    refund_id: Option<String>,
    payment_id: Option<String>,
    amount: Option<i64>,
    status: Option<String>,
}

impl HyperswitchPsp {
    /// Create new adapter from environment variables (`HYPERSWITCH_API_KEY` and optional `HYPERSWITCH_WEBHOOK_SECRET`).
    pub fn from_env() -> Result<Self, ProviderError> {
        let api_key = std::env::var("HYPERSWITCH_API_KEY").map_err(|_| {
            ProviderError::InvalidRequest(
                "HYPERSWITCH_API_KEY environment variable not set".to_string(),
            )
        })?;
        let webhook_secret = std::env::var("HYPERSWITCH_WEBHOOK_SECRET").ok();
        Ok(Self::new(
            DEFAULT_HYPERSWITCH_BASE_URL,
            api_key,
            webhook_secret,
        ))
    }

    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        webhook_secret: Option<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            webhook_secret,
        }
    }

    /// Derive deterministic payment ID using UUIDv5 as specified in section 8.3
    pub fn deterministic_payment_id(tenant_id: &str, idempotency_key: &str) -> String {
        let name = format!("{}:{}", tenant_id, idempotency_key);
        format!(
            "pay_{}",
            Uuid::new_v5(&Uuid::NAMESPACE_OID, name.as_bytes())
        )
    }

    /// Total mapping function from Hyperswitch status string to ProviderStatus.
    /// Every status in the Hyperswitch documentation is mapped; all unknown strings map to Unknown.
    pub fn map_status(status_str: &str, error_code: Option<&str>) -> ProviderStatus {
        match status_str.to_ascii_lowercase().as_str() {
            "succeeded" => ProviderStatus::Succeeded,
            "failed" => {
                let failure_class = match error_code.unwrap_or("").to_ascii_lowercase().as_str() {
                    "insufficient_funds" | "he_02" => FailureClass::InsufficientFunds,
                    "timeout" | "gateway_timeout" => FailureClass::Timeout,
                    "internal_server_error" | "provider_down" => FailureClass::ProviderDown,
                    "invalid_request" | "bad_request" => FailureClass::InvalidRequest,
                    _ => FailureClass::Declined,
                };
                ProviderStatus::Failed(failure_class)
            }
            "cancelled" => ProviderStatus::Failed(FailureClass::Declined),
            "processing" => ProviderStatus::Processing,
            "requires_payment_method" => ProviderStatus::ActionRequired,
            "requires_confirmation" => ProviderStatus::ActionRequired,
            "requires_customer_action" => ProviderStatus::ActionRequired,
            "requires_merchant_action" => ProviderStatus::ActionRequired,
            "requires_capture" => ProviderStatus::Processing,
            "partially_captured" => ProviderStatus::Processing,
            // Unknown input => Unknown (never default to Allow/Succeeded)
            _ => ProviderStatus::Unknown,
        }
    }
}

#[async_trait]
impl PaymentProvider for HyperswitchPsp {
    async fn charge(&self, req: ChargeRequest) -> Result<ProviderPayment, ProviderError> {
        let url = format!("{}/payments", self.base_url);
        let curr_str = req.currency.to_string();

        let payload = HyperswitchPaymentRequest {
            amount: req.amount_minor,
            currency: &curr_str,
            confirm: true,
            capture_method: "automatic",
            payment_id: &req.payment_id,
            customer_id: &req.customer_id,
            metadata: req.metadata.unwrap_or_else(|| {
                serde_json::json!({
                    "task_id": req.task_id,
                    "purchase_id": req.purchase_id
                })
            }),
        };

        let resp = self
            .client
            .post(&url)
            .header("api-key", &self.api_key)
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ProviderError::Timeout(e.to_string())
                } else if e.is_connect() {
                    ProviderError::ProviderDown(e.to_string())
                } else {
                    ProviderError::Network(e.to_string())
                }
            })?;

        let status_code = resp.status();
        let body_bytes = resp
            .bytes()
            .await
            .map_err(|e| ProviderError::Network(format!("Failed to read response body: {e}")))?;

        if status_code.is_server_error() {
            return Err(ProviderError::ProviderDown(format!(
                "Hyperswitch server error ({status_code}): {}",
                String::from_utf8_lossy(&body_bytes)
            )));
        }

        let raw_json: Value = serde_json::from_slice(&body_bytes).unwrap_or(Value::Null);

        let parsed: HyperswitchPaymentResponse =
            serde_json::from_slice(&body_bytes).map_err(|e| {
                ProviderError::Serialization(format!(
                    "Failed to parse Hyperswitch response: {e}; body: {}",
                    String::from_utf8_lossy(&body_bytes)
                ))
            })?;

        let payment_id = parsed.payment_id.unwrap_or(req.payment_id);
        let status = Self::map_status(
            parsed.status.as_deref().unwrap_or("unknown"),
            parsed.error_code.as_deref(),
        );

        let amount_minor = parsed.amount.unwrap_or(req.amount_minor);
        let currency = parsed
            .currency
            .and_then(|c| Currency::from_str(&c).ok())
            .unwrap_or(req.currency);

        Ok(ProviderPayment {
            provider_payment_id: payment_id,
            amount_minor,
            currency,
            status,
            error_message: parsed.error_message,
            raw_response: Some(raw_json),
        })
    }

    async fn retrieve(
        &self,
        provider_payment_id: &str,
    ) -> Result<Option<ProviderPayment>, ProviderError> {
        let url = format!("{}/payments/{}", self.base_url, provider_payment_id);

        let resp = self
            .client
            .get(&url)
            .header("api-key", &self.api_key)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ProviderError::Timeout(e.to_string())
                } else if e.is_connect() {
                    ProviderError::ProviderDown(e.to_string())
                } else {
                    ProviderError::Network(e.to_string())
                }
            })?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let status_code = resp.status();
        let body_bytes = resp
            .bytes()
            .await
            .map_err(|e| ProviderError::Network(format!("Failed to read response body: {e}")))?;

        if status_code.is_server_error() {
            return Err(ProviderError::ProviderDown(format!(
                "Hyperswitch server error ({status_code})"
            )));
        }

        let raw_json: Value = serde_json::from_slice(&body_bytes).unwrap_or(Value::Null);
        let parsed: HyperswitchPaymentResponse =
            serde_json::from_slice(&body_bytes).map_err(|e| {
                ProviderError::Serialization(format!("Failed to parse retrieve response: {e}"))
            })?;

        let payment_id = parsed
            .payment_id
            .unwrap_or_else(|| provider_payment_id.to_string());
        let status = Self::map_status(
            parsed.status.as_deref().unwrap_or("unknown"),
            parsed.error_code.as_deref(),
        );

        let amount_minor = parsed.amount.unwrap_or(0);
        let currency = parsed
            .currency
            .and_then(|c| Currency::from_str(&c).ok())
            .unwrap_or(Currency::INR);

        Ok(Some(ProviderPayment {
            provider_payment_id: payment_id,
            amount_minor,
            currency,
            status,
            error_message: parsed.error_message,
            raw_response: Some(raw_json),
        }))
    }

    async fn refund(&self, req: RefundRequest) -> Result<ProviderRefund, ProviderError> {
        let url = format!("{}/refunds", self.base_url);

        let payload = HyperswitchRefundRequest {
            payment_id: &req.provider_payment_id,
            amount: req.amount_minor,
            reason: req.reason.as_deref(),
        };

        let resp = self
            .client
            .post(&url)
            .header("api-key", &self.api_key)
            .json(&payload)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;

        let status_code = resp.status();
        let body_bytes = resp
            .bytes()
            .await
            .map_err(|e| ProviderError::Network(format!("Failed to read refund body: {e}")))?;

        if status_code.is_server_error() {
            return Err(ProviderError::ProviderDown(format!(
                "Hyperswitch refund server error ({status_code})"
            )));
        }

        let parsed: HyperswitchRefundResponse =
            serde_json::from_slice(&body_bytes).map_err(|e| {
                ProviderError::Serialization(format!("Failed to parse refund response: {e}"))
            })?;

        Ok(ProviderRefund {
            refund_id: parsed.refund_id.unwrap_or(req.refund_id),
            provider_payment_id: parsed.payment_id.unwrap_or(req.provider_payment_id),
            amount_minor: parsed.amount.unwrap_or(req.amount_minor),
            status: parsed.status.unwrap_or_else(|| "succeeded".to_string()),
        })
    }

    fn verify_webhook(
        &self,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<WebhookTrigger, WebhookError> {
        let secret = self
            .webhook_secret
            .as_ref()
            .ok_or(WebhookError::MissingSignature)?;

        // Check for signature header: x-webhook-signature-512 or x-webhook-signature-256
        let sig_512 = headers.get("x-webhook-signature-512");
        let sig_256 = headers.get("x-webhook-signature-256");

        if let Some(sig_hdr) = sig_512 {
            let sig_str = sig_hdr
                .to_str()
                .map_err(|_| WebhookError::InvalidSignature)?;
            let sig_bytes = hex::decode(sig_str).map_err(|_| WebhookError::InvalidSignature)?;

            let mut mac = HmacSha512::new_from_slice(secret.as_bytes())
                .map_err(|_| WebhookError::InvalidSignature)?;
            mac.update(body);

            mac.verify_slice(&sig_bytes)
                .map_err(|_| WebhookError::InvalidSignature)?;
        } else if let Some(sig_hdr) = sig_256 {
            let sig_str = sig_hdr
                .to_str()
                .map_err(|_| WebhookError::InvalidSignature)?;
            let sig_bytes = hex::decode(sig_str).map_err(|_| WebhookError::InvalidSignature)?;

            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .map_err(|_| WebhookError::InvalidSignature)?;
            mac.update(body);

            mac.verify_slice(&sig_bytes)
                .map_err(|_| WebhookError::InvalidSignature)?;
        } else {
            return Err(WebhookError::MissingSignature);
        }

        let v: Value = serde_json::from_slice(body)
            .map_err(|e| WebhookError::MalformedPayload(e.to_string()))?;

        let event_id = v
            .get("event_id")
            .and_then(|x| x.as_str())
            .unwrap_or("hs_evt_default")
            .to_string();

        let event_type = v
            .get("event_type")
            .and_then(|x| x.as_str())
            .unwrap_or("payment_succeeded")
            .to_string();

        let provider_payment_id = v
            .get("payment_id")
            .or_else(|| v.pointer("/content/payment_id"))
            .or_else(|| v.pointer("/data/payment_id"))
            .and_then(|x| x.as_str())
            .ok_or_else(|| {
                WebhookError::MalformedPayload(
                    "Missing payment_id in Hyperswitch webhook payload".to_string(),
                )
            })?
            .to_string();

        Ok(WebhookTrigger {
            provider_payment_id,
            event_id,
            event_type,
        })
    }
}
