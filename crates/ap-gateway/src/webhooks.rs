//! Webhook handler implementing Section 9.3 (signature verification, deduplication, retrieve-then-settle).

use ap_psp::traits::{PaymentProvider, ProviderStatus, WebhookError};
use ap_store::{release, settle_captured, StoreError};
use axum::http::HeaderMap;
use chrono::Utc;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug)]
pub enum WebhookHandlerResult {
    Processed(String),
    Duplicate,
    SignatureInvalid,
    InternalError(String),
}

/// Process an incoming webhook: verify signature, deduplicate, retrieve from provider, and apply state change.
pub async fn process_webhook(
    pool: &PgPool,
    provider: Arc<dyn PaymentProvider>,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<WebhookHandlerResult, StoreError> {
    // 1. Verify signature
    let trigger = match provider.verify_webhook(headers, body) {
        Ok(t) => t,
        Err(WebhookError::MissingSignature | WebhookError::InvalidSignature) => {
            return Ok(WebhookHandlerResult::SignatureInvalid);
        }
        Err(e) => {
            return Ok(WebhookHandlerResult::InternalError(e.to_string()));
        }
    };

    // 2. Deduplicate by provider event ID
    let raw_json: serde_json::Value =
        serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    let inserted = sqlx::query(
        r#"
        INSERT INTO webhook_events (provider_event_id, provider, payload)
        VALUES ($1, 'hyperswitch', $2)
        ON CONFLICT (provider_event_id) DO NOTHING
        RETURNING provider_event_id
        "#,
    )
    .bind(&trigger.event_id)
    .bind(&raw_json)
    .fetch_optional(pool)
    .await?;

    if inserted.is_none() {
        return Ok(WebhookHandlerResult::Duplicate);
    }

    // 3. A valid signature is only a trigger: retrieve payment from provider
    let payment_opt = provider
        .retrieve(&trigger.provider_payment_id)
        .await
        .map_err(|e| StoreError::InvalidPurchaseState(format!("Retrieve failed: {e}")))?;

    let payment = match payment_opt {
        Some(p) => p,
        None => {
            // Provider payment not found: record incident
            let _ = sqlx::query(
                r#"
                INSERT INTO incidents (id, kind, details)
                VALUES ($1, 'webhook_payment_not_found', $2)
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(serde_json::json!({
                "provider_payment_id": trigger.provider_payment_id,
                "event_id": trigger.event_id
            }))
            .execute(pool)
            .await;
            return Ok(WebhookHandlerResult::Processed(
                "Payment not found at provider".to_string(),
            ));
        }
    };

    // 4. Find corresponding purchase in database
    let purchase_row = sqlx::query(
        r#"
        SELECT id, task_id, status::text as status
        FROM purchases
        WHERE provider_payment_id = $1
           OR idempotency_key LIKE $2
        LIMIT 1
        "#,
    )
    .bind(&trigger.provider_payment_id)
    .bind(format!("%{}", trigger.provider_payment_id))
    .fetch_optional(pool)
    .await?;

    let (purchase_id, current_status) = match purchase_row {
        Some(r) => {
            let id: Uuid = r.get("id");
            let s: String = r.get("status");
            (id, s)
        }
        None => {
            return Ok(WebhookHandlerResult::Processed(
                "No local purchase found for payment ID".to_string(),
            ));
        }
    };

    // 5. Apply outcome: terminal states are absorbing
    match payment.status {
        ProviderStatus::Succeeded => {
            if current_status == "RESERVED" || current_status == "PENDING_UNKNOWN" {
                let _ = settle_captured(
                    pool,
                    purchase_id,
                    &payment.provider_payment_id,
                    "read",
                    Utc::now() + chrono::Duration::hours(1),
                )
                .await?;
            }
        }
        ProviderStatus::Failed(_) => {
            if current_status == "RESERVED" || current_status == "PENDING_UNKNOWN" {
                release(pool, purchase_id).await?;
            } else if current_status == "CAPTURED" {
                // Late failure after captured: do NOT revert history; open incident
                let _ = sqlx::query(
                    r#"
                    INSERT INTO incidents (id, kind, purchase_id, details)
                    VALUES ($1, 'late_failed_after_captured', $2, $3)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(purchase_id)
                .bind(serde_json::json!({
                    "provider_payment_id": trigger.provider_payment_id,
                    "event_id": trigger.event_id
                }))
                .execute(pool)
                .await;
            }
        }
        _ => {}
    }

    Ok(WebhookHandlerResult::Processed("Success".to_string()))
}
