//! Two-way reconciler implementing Section 9.5 (Pass A and Pass B).

use ap_core::money::Currency;
use ap_psp::traits::{PaymentProvider, ProviderStatus};
use ap_psp::SimulatedPsp;
use ap_store::{mark_pending_unknown, release, settle_captured, StoreError};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ReconcileReport {
    pub stuck_scanned: usize,
    pub healed_captured: usize,
    pub healed_released: usize,
    pub drifts_detected: usize,
    pub incidents_opened: usize,
}

/// Run one reconciliation pass over stuck purchases (Pass A) and captured purchases (Pass B).
pub async fn reconcile_once(
    pool: &PgPool,
    provider: Arc<dyn PaymentProvider>,
    tenant_id: &str,
) -> Result<ReconcileReport, StoreError> {
    let mut report = ReconcileReport::default();

    // ----------------------------------------------------
    // PASS A: Stuck purchases (RESERVED or PENDING_UNKNOWN)
    // ----------------------------------------------------
    let stuck = sqlx::query(
        r#"
        SELECT p.id, p.task_id, p.idempotency_key, p.amount_minor, p.status::text as status
        FROM purchases p
        JOIN tasks t ON p.task_id = t.id
        WHERE t.tenant_id = $1 AND p.status IN ('RESERVED', 'PENDING_UNKNOWN')
        "#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;

    for row in stuck {
        report.stuck_scanned += 1;
        let purchase_id: Uuid = row.get("id");
        let idempotency_key: String = row.get("idempotency_key");
        let current_status: String = row.get("status");

        let payment_id = SimulatedPsp::deterministic_payment_id(tenant_id, &idempotency_key);

        match provider.retrieve(&payment_id).await {
            Ok(Some(payment)) => match payment.status {
                ProviderStatus::Succeeded => {
                    let _ = settle_captured(
                        pool,
                        purchase_id,
                        &payment.provider_payment_id,
                        "read",
                        Utc::now() + chrono::Duration::hours(1),
                    )
                    .await;
                    report.healed_captured += 1;
                }
                ProviderStatus::Failed(_) => {
                    let _ = release(pool, purchase_id).await;
                    report.healed_released += 1;
                }
                ProviderStatus::Processing
                | ProviderStatus::ActionRequired
                | ProviderStatus::Unknown => {
                    // Indeterminate: ensure it's in PENDING_UNKNOWN
                    if current_status == "RESERVED" {
                        let _ = mark_pending_unknown(pool, purchase_id).await;
                    }
                }
            },
            Ok(None) => {
                // Provider has no record of payment -> safe to release reservation
                let _ = release(pool, purchase_id).await;
                report.healed_released += 1;
            }
            Err(_) => {
                // Provider error/timeout on retrieve -> keep holding in PENDING_UNKNOWN
                if current_status == "RESERVED" {
                    let _ = mark_pending_unknown(pool, purchase_id).await;
                }
            }
        }
    }

    // ----------------------------------------------------
    // PASS B: Two-way compare for recent CAPTURED purchases
    // ----------------------------------------------------
    let captured = sqlx::query(
        r#"
        SELECT p.id, p.task_id, p.amount_minor, p.provider_payment_id, b.currency
        FROM purchases p
        JOIN tasks t ON p.task_id = t.id
        JOIN budgets b ON p.task_id = b.task_id
        WHERE t.tenant_id = $1 AND p.status = 'CAPTURED' AND p.provider_payment_id IS NOT NULL
        "#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;

    for row in captured {
        let purchase_id: Uuid = row.get("id");
        let amount_minor: i64 = row.get("amount_minor");
        let provider_payment_id: String = row.get("provider_payment_id");
        let currency_str: String = row.get("currency");
        let expected_currency = Currency::from_str(&currency_str).unwrap_or(Currency::INR);

        match provider.retrieve(&provider_payment_id).await {
            Ok(Some(payment)) => {
                if payment.status != ProviderStatus::Succeeded {
                    // Drift: Provider failed, ledger captured -> revoke grant, open incident
                    report.drifts_detected += 1;
                    report.incidents_opened += 1;

                    let _ = sqlx::query(
                        r#"
                        UPDATE grants
                        SET revoked_at = now()
                        WHERE purchase_id = $1
                        "#,
                    )
                    .bind(purchase_id)
                    .execute(pool)
                    .await;

                    let _ = sqlx::query(
                        r#"
                        INSERT INTO incidents (id, kind, purchase_id, details)
                        VALUES ($1, 'provider_failed_ledger_captured', $2, $3)
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind(purchase_id)
                    .bind(serde_json::json!({
                        "provider_status": format!("{:?}", payment.status),
                        "provider_payment_id": provider_payment_id
                    }))
                    .execute(pool)
                    .await;
                } else if payment.amount_minor != amount_minor
                    || payment.currency != expected_currency
                {
                    // Drift: Amount or currency differs -> freeze grant, open incident, no auto-heal
                    report.drifts_detected += 1;
                    report.incidents_opened += 1;

                    let _ = sqlx::query(
                        r#"
                        UPDATE grants
                        SET revoked_at = now()
                        WHERE purchase_id = $1
                        "#,
                    )
                    .bind(purchase_id)
                    .execute(pool)
                    .await;

                    let _ = sqlx::query(
                        r#"
                        INSERT INTO incidents (id, kind, purchase_id, details)
                        VALUES ($1, 'amount_currency_mismatch', $2, $3)
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind(purchase_id)
                    .bind(serde_json::json!({
                        "expected_amount": amount_minor,
                        "actual_amount": payment.amount_minor,
                        "expected_currency": expected_currency.to_string(),
                        "actual_currency": payment.currency.to_string()
                    }))
                    .execute(pool)
                    .await;
                }
            }
            Ok(None) => {
                // Drift: Provider has no record -> open incident, keep reservation held
                report.drifts_detected += 1;
                report.incidents_opened += 1;

                let _ = sqlx::query(
                    r#"
                    INSERT INTO incidents (id, kind, purchase_id, details)
                    VALUES ($1, 'provider_no_record', $2, $3)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(purchase_id)
                .bind(serde_json::json!({
                    "provider_payment_id": provider_payment_id
                }))
                .execute(pool)
                .await;
            }
            Err(_) => {
                // Provider lookup error: do not alter state
            }
        }
    }

    Ok(report)
}
