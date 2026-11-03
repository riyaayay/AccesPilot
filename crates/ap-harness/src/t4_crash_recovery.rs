//! T4: Crash recovery benchmark.
//! Failpoint simulation across 200 random purchases, restart, reconcile.
//! Asserts 0 orphaned reservations, 0 invariant failures.

use ap_core::money::Currency;
use ap_gateway::reconciler::reconcile_once;
use ap_psp::simulated::SimulatedPsp;
use ap_store::{
    claim_idempotency, reserve, seed_offer, seed_task_and_budget, verify_ledger_chain, StoreError,
};
use sqlx::{PgPool, Row};
use std::sync::Arc;
use uuid::Uuid;

pub struct T4Result {
    pub failpoints: usize,
    pub runs: usize,
    pub orphans: usize,
    pub invariant_failures: usize,
    pub pass: bool,
}

pub async fn run_t4(pool: &PgPool) -> Result<T4Result, StoreError> {
    let tenant_id = "tenant_t4_harness";
    let task_id = Uuid::new_v4();
    let cap_minor = 1_000_000; // ₹10,000.00
    let currency = Currency::INR;

    seed_task_and_budget(pool, tenant_id, task_id, cap_minor, currency).await?;

    let psp = Arc::new(SimulatedPsp::new());
    let runs = 200;
    let failpoints = 2; // after_reserve, after_provider_ok

    for i in 0..runs {
        let purchase_id = Uuid::new_v4();
        let offer_id = Uuid::new_v4();
        let amount_minor = 500 + ((i as i64 * 13) % 1000); // ₹5 - ₹15
        let key = format!("key_t4_{i}_{purchase_id}");

        seed_offer(
            pool,
            offer_id,
            task_id,
            "provider_crash",
            "res",
            amount_minor,
            currency,
        )
        .await?;

        let _ = claim_idempotency(pool, tenant_id, &key, "hash_t4").await?;
        let _ = reserve(
            pool,
            task_id,
            purchase_id,
            offer_id,
            &key,
            amount_minor,
            currency,
            1,
        )
        .await?;

        if i % 2 == 0 {
            // Failpoint 1: Process crashed after reservation, before provider call
            // Provider was never called, has no record
        } else {
            // Failpoint 2: Process crashed after provider success, before local settlement
            let payment_id = SimulatedPsp::deterministic_payment_id(tenant_id, &key);
            psp.record_payment(ap_psp::traits::ProviderPayment {
                provider_payment_id: payment_id,
                amount_minor,
                currency,
                status: ap_psp::traits::ProviderStatus::Succeeded,
                error_message: None,
                raw_response: None,
            })
            .await;
        }
    }

    // Now restart / run the two-way reconciler
    let _report = reconcile_once(pool, psp.clone(), tenant_id).await?;

    // Check Invariants
    // Invariant 1: 0 orphaned reservations (no purchases remaining in RESERVED)
    let orphan_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM purchases
        WHERE task_id = $1 AND status = 'RESERVED'
        "#,
    )
    .bind(task_id)
    .fetch_one(pool)
    .await?;

    let orphans = orphan_count as usize;

    let mut invariant_failures = 0;

    // Invariant 2: captured + reserved <= cap
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

    let cap: i64 = budget_row.get("cap_minor");
    let reserved: i64 = budget_row.get("reserved_minor");
    let captured: i64 = budget_row.get("captured_minor");

    if captured + reserved > cap {
        invariant_failures += 1;
    }

    // Invariant 3: Sum of captured purchases equals budget's captured amount
    let sum_captured: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(amount_minor), 0)::BIGINT
        FROM purchases
        WHERE task_id = $1 AND status = 'CAPTURED'
        "#,
    )
    .bind(task_id)
    .fetch_one(pool)
    .await?;

    if sum_captured != captured {
        invariant_failures += 1;
    }

    // Invariant 4: Ledger chain verification
    if verify_ledger_chain(pool, task_id).await.is_err() {
        invariant_failures += 1;
    }

    let pass = orphans == 0 && invariant_failures == 0;

    Ok(T4Result {
        failpoints,
        runs,
        orphans,
        invariant_failures,
        pass,
    })
}
