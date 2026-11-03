//! Simulated PSP with deterministic ID generation and configurable fault injection.

use crate::traits::{
    ChargeRequest, FailureClass, PaymentProvider, ProviderError, ProviderPayment, ProviderRefund,
    ProviderStatus, RefundRequest, WebhookError, WebhookTrigger,
};
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use reqwest::header::HeaderMap;
use serde_json::Value;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct SimulatedPsp {
    charge_count: Arc<AtomicUsize>,
    retrieve_count: Arc<AtomicUsize>,
    refund_count: Arc<AtomicUsize>,
    latency: Option<Duration>,
    force_timeout: Arc<AtomicBool>,
    force_5xx: Arc<AtomicBool>,
    force_status: Arc<RwLock<Option<ProviderStatus>>>,
    payments: Arc<RwLock<HashMap<String, ProviderPayment>>>,
    webhook_secret: String,
}

impl Default for SimulatedPsp {
    fn default() -> Self {
        Self::new()
    }
}

impl SimulatedPsp {
    pub fn new() -> Self {
        Self {
            charge_count: Arc::new(AtomicUsize::new(0)),
            retrieve_count: Arc::new(AtomicUsize::new(0)),
            refund_count: Arc::new(AtomicUsize::new(0)),
            latency: None,
            force_timeout: Arc::new(AtomicBool::new(false)),
            force_5xx: Arc::new(AtomicBool::new(false)),
            force_status: Arc::new(RwLock::new(None)),
            payments: Arc::new(RwLock::new(HashMap::new())),
            webhook_secret: "simulated_webhook_secret_key_12345".to_string(),
        }
    }

    pub fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = Some(latency);
        self
    }

    pub fn with_webhook_secret(mut self, secret: impl Into<String>) -> Self {
        self.webhook_secret = secret.into();
        self
    }

    pub fn set_force_timeout(&self, timeout: bool) {
        self.force_timeout.store(timeout, Ordering::SeqCst);
    }

    pub fn set_force_5xx(&self, is_5xx: bool) {
        self.force_5xx.store(is_5xx, Ordering::SeqCst);
    }

    pub async fn set_force_status(&self, status: Option<ProviderStatus>) {
        let mut lock = self.force_status.write().await;
        *lock = status;
    }

    pub fn charge_counter(&self) -> usize {
        self.charge_count.load(Ordering::SeqCst)
    }

    pub fn retrieve_counter(&self) -> usize {
        self.retrieve_count.load(Ordering::SeqCst)
    }

    pub fn refund_counter(&self) -> usize {
        self.refund_count.load(Ordering::SeqCst)
    }

    /// Derive a deterministic provider payment ID from tenant ID and idempotency key (UUIDv5).
    pub fn deterministic_payment_id(tenant_id: &str, idempotency_key: &str) -> String {
        let name = format!("{}:{}", tenant_id, idempotency_key);
        format!(
            "sim_{}",
            Uuid::new_v5(&Uuid::NAMESPACE_OID, name.as_bytes())
        )
    }

    /// Helper to sign webhook payloads for testing.
    pub fn sign_webhook_payload(&self, payload: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(self.webhook_secret.as_bytes())
            .expect("HMAC can take any key length");
        mac.update(payload);
        hex::encode(mac.finalize().into_bytes())
    }

    /// Helper to seed or record a payment in memory for testing and reconciler verification.
    pub async fn record_payment(&self, payment: ProviderPayment) {
        let mut lock = self.payments.write().await;
        lock.insert(payment.provider_payment_id.clone(), payment);
    }
}

#[async_trait]
impl PaymentProvider for SimulatedPsp {
    async fn charge(&self, req: ChargeRequest) -> Result<ProviderPayment, ProviderError> {
        self.charge_count.fetch_add(1, Ordering::SeqCst);

        if let Some(lat) = self.latency {
            tokio::time::sleep(lat).await;
        }

        if self.force_timeout.load(Ordering::SeqCst) {
            return Err(ProviderError::Timeout(
                "Simulated PSP network timeout".to_string(),
            ));
        }

        if self.force_5xx.load(Ordering::SeqCst) {
            return Err(ProviderError::ProviderDown(
                "Simulated PSP 500 Internal Server Error".to_string(),
            ));
        }

        let forced_status = {
            let lock = self.force_status.read().await;
            lock.clone()
        };

        let status = forced_status.unwrap_or(ProviderStatus::Succeeded);
        let error_message = match &status {
            ProviderStatus::Failed(FailureClass::Declined) => Some("Card declined".to_string()),
            ProviderStatus::Failed(FailureClass::InsufficientFunds) => {
                Some("Insufficient funds".to_string())
            }
            ProviderStatus::Failed(_) => Some("Payment failed".to_string()),
            _ => None,
        };

        let payment = ProviderPayment {
            provider_payment_id: req.payment_id.clone(),
            amount_minor: req.amount_minor,
            currency: req.currency,
            status,
            error_message,
            raw_response: Some(serde_json::json!({
                "simulated": true,
                "payment_id": req.payment_id,
                "amount": req.amount_minor,
                "customer_id": req.customer_id
            })),
        };

        let mut payments = self.payments.write().await;
        payments.insert(req.payment_id, payment.clone());

        Ok(payment)
    }

    async fn retrieve(
        &self,
        provider_payment_id: &str,
    ) -> Result<Option<ProviderPayment>, ProviderError> {
        self.retrieve_count.fetch_add(1, Ordering::SeqCst);

        if let Some(lat) = self.latency {
            tokio::time::sleep(lat).await;
        }

        if self.force_timeout.load(Ordering::SeqCst) {
            return Err(ProviderError::Timeout(
                "Simulated PSP retrieve timeout".to_string(),
            ));
        }

        if self.force_5xx.load(Ordering::SeqCst) {
            return Err(ProviderError::ProviderDown(
                "Simulated PSP 500 retrieve error".to_string(),
            ));
        }

        let payments = self.payments.read().await;
        let mut payment_opt = payments.get(provider_payment_id).cloned();
        if let Some(payment) = payment_opt.as_mut() {
            let forced = self.force_status.read().await;
            if let Some(st) = forced.as_ref() {
                payment.status = st.clone();
            }
        }
        Ok(payment_opt)
    }

    async fn refund(&self, req: RefundRequest) -> Result<ProviderRefund, ProviderError> {
        self.refund_count.fetch_add(1, Ordering::SeqCst);

        if self.force_5xx.load(Ordering::SeqCst) {
            return Err(ProviderError::ProviderDown(
                "Simulated PSP refund failed".to_string(),
            ));
        }

        Ok(ProviderRefund {
            refund_id: req.refund_id,
            provider_payment_id: req.provider_payment_id,
            amount_minor: req.amount_minor,
            status: "succeeded".to_string(),
        })
    }

    fn verify_webhook(
        &self,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<WebhookTrigger, WebhookError> {
        let sig_header = headers
            .get("x-webhook-signature-256")
            .or_else(|| headers.get("x-webhook-signature-512"))
            .ok_or(WebhookError::MissingSignature)?;

        let sig_str = sig_header
            .to_str()
            .map_err(|_| WebhookError::InvalidSignature)?;

        let sig_bytes = hex::decode(sig_str).map_err(|_| WebhookError::InvalidSignature)?;

        let mut mac = HmacSha256::new_from_slice(self.webhook_secret.as_bytes())
            .map_err(|_| WebhookError::InvalidSignature)?;
        mac.update(body);

        mac.verify_slice(&sig_bytes)
            .map_err(|_| WebhookError::InvalidSignature)?;

        let v: Value = serde_json::from_slice(body)
            .map_err(|e| WebhookError::MalformedPayload(e.to_string()))?;

        let event_id = v
            .get("event_id")
            .and_then(|x| x.as_str())
            .unwrap_or("sim_evt_default")
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
                WebhookError::MalformedPayload("Missing payment_id in webhook".to_string())
            })?
            .to_string();

        Ok(WebhookTrigger {
            provider_payment_id,
            event_id,
            event_type,
        })
    }
}
