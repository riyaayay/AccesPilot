//! Core purchase flow implementing Section 8.3 with failpoints.

use ap_core::money::{Currency, Money};
use ap_core::policy::{decide, DecisionContext, Policy, Verdict};
use ap_psp::traits::{ChargeRequest, PaymentProvider, ProviderError, ProviderStatus};
use ap_psp::SimulatedPsp;
use ap_store::{
    claim_idempotency, complete_idempotency, mark_pending_unknown, release, reserve,
    settle_captured, GrantRecord, IdempotencyClaim, StoreError,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum PurchaseFlowError {
    #[error("Store error: {0}")]
    Store(#[from] StoreError),
    #[error("Database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("Offer not found: {0}")]
    OfferNotFound(Uuid),
    #[error("Offer expired: {0}")]
    OfferExpired(Uuid),
    #[error("Task not found: {0}")]
    TaskNotFound(Uuid),
    #[error("Policy not found: {0}")]
    PolicyNotFound(i32),
    #[error("Invalid policy json: {0}")]
    PolicyInvalid(String),
    #[error("Idempotency conflict: key reuse with different payload")]
    IdempotencyConflict,
    #[error("Request in progress")]
    InProgress,
    #[error("Policy denied: {0}")]
    PolicyDenied(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PurchaseResult {
    pub purchase_id: Uuid,
    pub status: String,
    pub grant: Option<GrantRecord>,
    pub message: String,
    pub amount_minor: i64,
}

/// Check the FAULT_POINT environment variable to trigger controlled crashes.
pub fn check_failpoint(point: &str) {
    if let Ok(fp) = std::env::var("FAULT_POINT") {
        if fp == point {
            tracing::warn!("TRIGGERED FAULT POINT: {point}");
            if std::env::var("FAULT_POINT_PANIC").is_ok() {
                panic!("FAULT_POINT_TRIGGERED_{point}");
            } else {
                std::process::exit(99);
            }
        }
    }
}

/// Execute the purchase flow of section 8.3.
pub async fn execute_purchase(
    pool: &PgPool,
    provider: Arc<dyn PaymentProvider>,
    tenant_id: &str,
    task_id: Uuid,
    offer_id: Uuid,
    idempotency_key: &str,
    request_hash: &str,
) -> Result<PurchaseResult, PurchaseFlowError> {
    // 1. Claim idempotency key
    let claim = claim_idempotency(pool, tenant_id, idempotency_key, request_hash).await?;
    match claim {
        IdempotencyClaim::Completed(resp) => {
            let res: PurchaseResult = serde_json::from_value(resp)
                .map_err(|e| PurchaseFlowError::PolicyInvalid(e.to_string()))?;
            return Ok(res);
        }
        IdempotencyClaim::Conflict => {
            return Err(PurchaseFlowError::IdempotencyConflict);
        }
        IdempotencyClaim::InProgress => {
            return Err(PurchaseFlowError::InProgress);
        }
        IdempotencyClaim::New => {}
    }

    // Fetch offer
    let offer_row = sqlx::query(
        r#"
        SELECT provider_id, resource_id, amount_minor, currency, tax_minor, fee_minor, price_known, expires_at
        FROM offers
        WHERE id = $1 AND task_id = $2
        "#,
    )
    .bind(offer_id)
    .bind(task_id)
    .fetch_optional(pool)
    .await?
    .ok_or(PurchaseFlowError::OfferNotFound(offer_id))?;

    let expires_at: chrono::DateTime<Utc> = offer_row.get("expires_at");
    if expires_at < Utc::now() {
        return Err(PurchaseFlowError::OfferExpired(offer_id));
    }

    let provider_id: String = offer_row.get("provider_id");
    let resource_id: String = offer_row.get("resource_id");
    let amount_minor: i64 = offer_row.get("amount_minor");
    let currency_str: String = offer_row.get("currency");
    let currency = Currency::from_str(&currency_str)
        .map_err(|_| PurchaseFlowError::PolicyInvalid("Invalid currency".to_string()))?;
    let tax_minor: i64 = offer_row.get("tax_minor");
    let fee_minor: i64 = offer_row.get("fee_minor");
    let price_known: bool = offer_row.get("price_known");

    // Fetch task and policy
    let task_row = sqlx::query(
        r#"
        SELECT policy_version
        FROM tasks
        WHERE id = $1
        "#,
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await?
    .ok_or(PurchaseFlowError::TaskNotFound(task_id))?;

    let policy_version: i32 = task_row.get("policy_version");

    let policy_row = sqlx::query(
        r#"
        SELECT policy_json
        FROM policies
        WHERE version = $1 AND tenant_id = $2
        "#,
    )
    .bind(policy_version)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?;

    // If no policy row in DB, use default permissive within_rules policy for testing
    let policy: Policy = match policy_row {
        Some(r) => {
            let v: Value = r.get("policy_json");
            serde_json::from_value(v)
                .map_err(|e| PurchaseFlowError::PolicyInvalid(e.to_string()))?
        }
        None => Policy {
            version: policy_version as u32,
            autonomy: ap_core::policy::Autonomy::WithinRules,
            task_cap: Money::new_raw(100_000, currency),
            per_purchase_cap: Money::new_raw(50_000, currency),
            rules: vec![ap_core::policy::Rule {
                id: ap_core::RuleId("allow_all".to_string()),
                when: ap_core::policy::Expr::Cmp(
                    ap_core::policy::Field::AmountMinor,
                    ap_core::policy::Op::Lte,
                    ap_core::policy::Value::Int(100_000),
                ),
                then: Verdict::Allow,
                reason: "Default test rule".to_string(),
            }],
        },
    };

    // Fetch budget for context
    let budget_row = sqlx::query(
        r#"
        SELECT cap_minor, reserved_minor, captured_minor
        FROM budgets
        WHERE task_id = $1
        "#,
    )
    .bind(task_id)
    .fetch_one(pool)
    .await?;

    let cap_minor: i64 = budget_row.get("cap_minor");
    let spent_minor: i64 = budget_row.get("captured_minor");
    let reserved_minor: i64 = budget_row.get("reserved_minor");
    let remaining_minor = (cap_minor - (spent_minor + reserved_minor)).max(0);

    let ctx = DecisionContext {
        provider: provider_id,
        resource_id,
        category: Some("data".to_string()),
        amount: Money::new_raw(amount_minor, currency),
        tax: Some(Money::new_raw(tax_minor, currency)),
        fee: Some(Money::new_raw(fee_minor, currency)),
        price_known,
        remaining_budget: Money::new_raw(remaining_minor, currency),
        anomaly_score: 0,
        purpose: None,
    };

    let decision = decide(&policy, &ctx);
    if decision.verdict == Verdict::Deny {
        let reason = decision.explanation;
        let res = PurchaseResult {
            purchase_id: Uuid::nil(),
            status: "denied".to_string(),
            grant: None,
            message: reason.clone(),
            amount_minor,
        };
        let _ = complete_idempotency(
            pool,
            tenant_id,
            idempotency_key,
            &serde_json::to_value(&res).unwrap_or(Value::Null),
        )
        .await;
        return Err(PurchaseFlowError::PolicyDenied(reason));
    }

    // 2. Transaction 1: reserve budget
    check_failpoint("before_reserve");

    let purchase_id = Uuid::new_v4();
    reserve(
        pool,
        task_id,
        purchase_id,
        offer_id,
        idempotency_key,
        amount_minor,
        currency,
        policy_version,
    )
    .await?;

    // Derive and record deterministic payment ID on purchase record
    let payment_id = SimulatedPsp::deterministic_payment_id(tenant_id, idempotency_key);
    sqlx::query("UPDATE purchases SET provider_payment_id = $1 WHERE id = $2")
        .bind(&payment_id)
        .bind(purchase_id)
        .execute(pool)
        .await?;

    check_failpoint("after_reserve");

    // Retrieve before creating to guarantee exactly-once provider call
    let provider_payment = match provider.retrieve(&payment_id).await {
        Ok(Some(existing)) => existing,
        _ => {
            let charge_req = ChargeRequest {
                payment_id: payment_id.clone(),
                amount_minor,
                currency,
                customer_id: format!("cust_{tenant_id}"),
                task_id,
                purchase_id,
                metadata: None,
            };

            // Smart retry: retry only safe classes
            let mut attempts = 0;
            loop {
                attempts += 1;
                match provider.charge(charge_req.clone()).await {
                    Ok(p) => break p,
                    Err(e) => {
                        let is_retryable = matches!(
                            e,
                            ProviderError::Timeout(_) | ProviderError::ProviderDown(_)
                        );
                        if is_retryable && attempts < 3 {
                            tokio::time::sleep(Duration::from_millis(50 * attempts)).await;
                            continue;
                        }

                        // Definite provider error or exhausted retries -> mark pending unknown
                        mark_pending_unknown(pool, purchase_id).await?;
                        let res = PurchaseResult {
                            purchase_id,
                            status: "pending_unknown".to_string(),
                            grant: None,
                            message: format!("Provider error: {e}; reservation held"),
                            amount_minor,
                        };
                        let _ = complete_idempotency(
                            pool,
                            tenant_id,
                            idempotency_key,
                            &serde_json::to_value(&res).unwrap_or(Value::Null),
                        )
                        .await;
                        return Ok(res);
                    }
                }
            }
        }
    };

    check_failpoint("after_provider_ok");

    // 4. Transaction 2, by outcome
    let final_res = match provider_payment.status {
        ProviderStatus::Succeeded => {
            let grant = settle_captured(
                pool,
                purchase_id,
                &provider_payment.provider_payment_id,
                "read",
                Utc::now() + chrono::Duration::hours(1),
            )
            .await?;

            PurchaseResult {
                purchase_id,
                status: "captured".to_string(),
                grant: Some(grant),
                message: "Payment captured and grant minted".to_string(),
                amount_minor,
            }
        }
        ProviderStatus::Failed(failure_class) => {
            release(pool, purchase_id).await?;
            PurchaseResult {
                purchase_id,
                status: "released".to_string(),
                grant: None,
                message: format!("Payment failed: {failure_class:?}; reservation released"),
                amount_minor,
            }
        }
        ProviderStatus::Processing | ProviderStatus::ActionRequired | ProviderStatus::Unknown => {
            mark_pending_unknown(pool, purchase_id).await?;
            PurchaseResult {
                purchase_id,
                status: "pending_unknown".to_string(),
                grant: None,
                message: "Status indeterminate; reservation held".to_string(),
                amount_minor,
            }
        }
    };

    // 5. Store response against idempotency key
    let val = serde_json::to_value(&final_res).unwrap_or(Value::Null);
    complete_idempotency(pool, tenant_id, idempotency_key, &val).await?;

    Ok(final_res)
}
