//! T2: Idempotency storm benchmark.
//! 1,000 concurrent requests, exactly one idempotency key.

use ap_core::money::Currency;
use ap_psp::simulated::SimulatedPsp;
use ap_psp::traits::{ChargeRequest, PaymentProvider};
use ap_store::{
    claim_idempotency, complete_idempotency, reserve, seed_offer, seed_task_and_budget,
    settle_captured, IdempotencyClaim, StoreError,
};
use chrono::Utc;
use sqlx::PgPool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use uuid::Uuid;

pub struct T2Result {
    pub requests: usize,
    pub provider_charges: usize,
    pub replays: usize,
    pub pass: bool,
}

pub async fn run_t2(pool: &PgPool) -> Result<T2Result, StoreError> {
    let tenant_id = "tenant_t2_harness";
    let task_id = Uuid::new_v4();
    let cap_minor = 10000; // ₹100.00
    let currency = Currency::INR;

    seed_task_and_budget(pool, tenant_id, task_id, cap_minor, currency).await?;

    let offer_id = Uuid::new_v4();
    let amount_minor = 1000; // ₹10.00
    seed_offer(
        pool,
        offer_id,
        task_id,
        "provider_storm",
        "res_storm",
        amount_minor,
        currency,
    )
    .await?;

    let single_key = format!("storm_key_harness_{}", Uuid::new_v4());
    let req_hash = "storm_hash";

    let psp = Arc::new(SimulatedPsp::new());
    let total_requests = 1000;
    let replays = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(total_requests);

    for _ in 0..total_requests {
        let pool = pool.clone();
        let psp = psp.clone();
        let key = single_key.clone();
        let replays = replays.clone();

        handles.push(tokio::spawn(async move {
            let claim = claim_idempotency(&pool, tenant_id, &key, req_hash).await;
            match claim {
                Ok(IdempotencyClaim::New) => {
                    let purchase_id = Uuid::new_v4();
                    if reserve(
                        &pool,
                        task_id,
                        purchase_id,
                        offer_id,
                        &key,
                        amount_minor,
                        currency,
                        1,
                    )
                    .await
                    .is_ok()
                    {
                        let charge_req = ChargeRequest {
                            payment_id: format!("pay_{purchase_id}"),
                            amount_minor,
                            currency,
                            customer_id: "cust_t2".to_string(),
                            task_id,
                            purchase_id,
                            metadata: None,
                        };

                        if let Ok(payment) = psp.charge(charge_req).await {
                            let _ = settle_captured(
                                &pool,
                                purchase_id,
                                &payment.provider_payment_id,
                                "read",
                                Utc::now() + chrono::Duration::hours(1),
                            )
                            .await;

                            let resp_body = serde_json::json!({
                                "status": "CAPTURED",
                                "purchase_id": purchase_id,
                                "amount_minor": amount_minor
                            });

                            let _ = complete_idempotency(&pool, tenant_id, &key, &resp_body).await;
                        }
                    }
                }
                Ok(IdempotencyClaim::Completed(_))
                | Ok(IdempotencyClaim::Conflict)
                | Err(StoreError::IdempotencyConflict(_)) => {
                    replays.fetch_add(1, Ordering::SeqCst);
                }
                _ => {
                    // In-flight or retried
                    replays.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    let provider_charges = psp.charge_counter();
    let replay_count = replays.load(Ordering::SeqCst);
    let pass = provider_charges == 1 && (replay_count == total_requests - 1);

    Ok(T2Result {
        requests: total_requests,
        provider_charges,
        replays: replay_count,
        pass,
    })
}
