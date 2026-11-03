//! T1: Cap under concurrency benchmark.
//! One task, ₹50 cap, 5,000 concurrent purchase attempts of random ₹1–₹20 amounts, simulated PSP.

use ap_core::money::Currency;
use ap_store::{
    claim_idempotency, reserve, seed_offer, seed_task_and_budget, settle_captured, StoreError,
};
use chrono::Utc;
use sqlx::{PgPool, Row};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use uuid::Uuid;

pub struct T1Result {
    pub attempts: usize,
    pub accepted: usize,
    pub denied: usize,
    pub captured_minor: i64,
    pub cap_minor: i64,
    pub violations: usize,
    pub pass: bool,
}

pub async fn run_t1(pool: &PgPool) -> Result<T1Result, StoreError> {
    let tenant_id = "tenant_t1_harness";
    let task_id = Uuid::new_v4();
    let cap_minor = 5000; // ₹50.00
    let currency = Currency::INR;

    seed_task_and_budget(pool, tenant_id, task_id, cap_minor, currency).await?;

    let total_attempts = 5000;
    let accepted = Arc::new(AtomicUsize::new(0));
    let denied = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(total_attempts);

    for i in 0..total_attempts {
        let pool = pool.clone();
        let accepted = accepted.clone();
        let denied = denied.clone();

        // Random-like amount between 100 (₹1) and 2000 (₹20) paise
        let amount = 100 + ((i as i64 * 37) % 1900);
        let purchase_id = Uuid::new_v4();
        let offer_id = Uuid::new_v4();
        let idempotency_key = format!("key_t1_harness_{i}_{purchase_id}");

        handles.push(tokio::spawn(async move {
            let _ = seed_offer(
                &pool,
                offer_id,
                task_id,
                "provider_bench",
                "res",
                amount,
                currency,
            )
            .await;

            let claim = claim_idempotency(&pool, tenant_id, &idempotency_key, "hash_t1").await;
            if claim.is_ok() {
                let res = reserve(
                    &pool,
                    task_id,
                    purchase_id,
                    offer_id,
                    &idempotency_key,
                    amount,
                    currency,
                    1,
                )
                .await;

                match res {
                    Ok(_) => {
                        accepted.fetch_add(1, Ordering::SeqCst);
                        let _ = settle_captured(
                            &pool,
                            purchase_id,
                            &format!("psp_t1_{purchase_id}"),
                            "read",
                            Utc::now() + chrono::Duration::hours(1),
                        )
                        .await;
                    }
                    Err(StoreError::InsufficientBudget(_)) => {
                        denied.fetch_add(1, Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    // Verify invariants directly from PostgreSQL
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

    let budget_cap: i64 = budget_row.get("cap_minor");
    let reserved: i64 = budget_row.get("reserved_minor");
    let captured: i64 = budget_row.get("captured_minor");

    let violations = if captured + reserved > budget_cap {
        1
    } else {
        0
    };
    let pass = violations == 0 && captured <= budget_cap;

    Ok(T1Result {
        attempts: total_attempts,
        accepted: accepted.load(Ordering::SeqCst),
        denied: denied.load(Ordering::SeqCst),
        captured_minor: captured,
        cap_minor: budget_cap,
        violations,
        pass,
    })
}
