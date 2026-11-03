//! T7: Reconciliation benchmark.
//! Injects 50 drifts across 5 kinds (10 each) and verifies exact detection and auto-healing.

use ap_core::money::Currency;
use ap_gateway::reconciler::reconcile_once;
use ap_psp::simulated::SimulatedPsp;
use ap_psp::traits::{FailureClass, ProviderPayment, ProviderStatus};
use ap_store::{
    claim_idempotency, reserve, seed_offer, seed_task_and_budget, settle_captured, StoreError,
};
use chrono::Utc;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

pub struct T7Result {
    pub drifts: usize,
    pub detected: usize,
    pub healed: usize,
    pub incidents: usize,
    pub pass: bool,
}

pub async fn run_t7(pool: &PgPool) -> Result<T7Result, StoreError> {
    let tenant_id_str = format!("tenant_t7_{}", Uuid::new_v4());
    let tenant_id = &tenant_id_str;
    let task_id = Uuid::new_v4();
    let cap_minor = 1_000_000;
    let currency = Currency::INR;

    seed_task_and_budget(pool, tenant_id, task_id, cap_minor, currency).await?;

    let psp = Arc::new(SimulatedPsp::new());

    // ----------------------------------------------------
    // Kind 1 (10): Provider succeeded, ledger pending -> auto-heals to CAPTURED
    // ----------------------------------------------------
    for i in 0..10 {
        let pid = Uuid::new_v4();
        let oid = Uuid::new_v4();
        let key = format!("key_t7_k1_{i}_{pid}");
        seed_offer(pool, oid, task_id, "prov_recon", "res", 1000, currency).await?;
        let _ = claim_idempotency(pool, tenant_id, &key, "hash_k1").await?;
        let _ = reserve(pool, task_id, pid, oid, &key, 1000, currency, 1).await?;

        let payment_id = SimulatedPsp::deterministic_payment_id(tenant_id, &key);
        psp.record_payment(ProviderPayment {
            provider_payment_id: payment_id,
            amount_minor: 1000,
            currency,
            status: ProviderStatus::Succeeded,
            error_message: None,
            raw_response: None,
        })
        .await;
    }

    // ----------------------------------------------------
    // Kind 2 (10): Provider failed, ledger captured -> revoke grant, open incident
    // ----------------------------------------------------
    for i in 0..10 {
        let pid = Uuid::new_v4();
        let oid = Uuid::new_v4();
        let key = format!("key_t7_k2_{i}_{pid}");
        seed_offer(pool, oid, task_id, "prov_recon", "res", 1000, currency).await?;
        let _ = claim_idempotency(pool, tenant_id, &key, "hash_k2").await?;
        let _ = reserve(pool, task_id, pid, oid, &key, 1000, currency, 1).await?;
        let payment_id = format!("pay_k2_{pid}");
        let _ = settle_captured(
            pool,
            pid,
            &payment_id,
            "read",
            Utc::now() + chrono::Duration::hours(1),
        )
        .await?;
        psp.record_payment(ProviderPayment {
            provider_payment_id: payment_id,
            amount_minor: 1000,
            currency,
            status: ProviderStatus::Failed(FailureClass::Declined),
            error_message: Some("Card declined".to_string()),
            raw_response: None,
        })
        .await;
    }

    // ----------------------------------------------------
    // Kind 3 (10): Amount differs -> freeze grant, open incident, no auto-heal
    // ----------------------------------------------------
    for i in 0..10 {
        let pid = Uuid::new_v4();
        let oid = Uuid::new_v4();
        let key = format!("key_t7_k3_{i}_{pid}");
        seed_offer(pool, oid, task_id, "prov_recon", "res", 1000, currency).await?;
        let _ = claim_idempotency(pool, tenant_id, &key, "hash_k3").await?;
        let _ = reserve(pool, task_id, pid, oid, &key, 1000, currency, 1).await?;
        let payment_id = format!("pay_k3_{pid}");
        let _ = settle_captured(
            pool,
            pid,
            &payment_id,
            "read",
            Utc::now() + chrono::Duration::hours(1),
        )
        .await?;
        psp.record_payment(ProviderPayment {
            provider_payment_id: payment_id,
            amount_minor: 500, // Different amount (500 != 1000)
            currency,
            status: ProviderStatus::Succeeded,
            error_message: None,
            raw_response: None,
        })
        .await;
    }

    // ----------------------------------------------------
    // Kind 4 (10): Currency differs -> freeze grant, open incident, no auto-heal
    // ----------------------------------------------------
    for i in 0..10 {
        let pid = Uuid::new_v4();
        let oid = Uuid::new_v4();
        let key = format!("key_t7_k4_{i}_{pid}");
        seed_offer(pool, oid, task_id, "prov_recon", "res", 1000, currency).await?;
        let _ = claim_idempotency(pool, tenant_id, &key, "hash_k4").await?;
        let _ = reserve(pool, task_id, pid, oid, &key, 1000, currency, 1).await?;
        let payment_id = format!("pay_k4_{pid}");
        let _ = settle_captured(
            pool,
            pid,
            &payment_id,
            "read",
            Utc::now() + chrono::Duration::hours(1),
        )
        .await?;
        psp.record_payment(ProviderPayment {
            provider_payment_id: payment_id,
            amount_minor: 1000,
            currency: Currency::USD, // Different currency (USD != INR)
            status: ProviderStatus::Succeeded,
            error_message: None,
            raw_response: None,
        })
        .await;
    }

    // ----------------------------------------------------
    // Kind 5 (10): Provider has no record -> open incident, keep reservation held
    // ----------------------------------------------------
    for i in 0..10 {
        let pid = Uuid::new_v4();
        let oid = Uuid::new_v4();
        let key = format!("key_t7_k5_{i}_{pid}");
        seed_offer(pool, oid, task_id, "prov_recon", "res", 1000, currency).await?;
        let _ = claim_idempotency(pool, tenant_id, &key, "hash_k5").await?;
        let _ = reserve(pool, task_id, pid, oid, &key, 1000, currency, 1).await?;
        let payment_id = format!("pay_k5_nonexistent_{pid}");
        let _ = settle_captured(
            pool,
            pid,
            &payment_id,
            "read",
            Utc::now() + chrono::Duration::hours(1),
        )
        .await?;
        // Provider has NO record in psp
    }

    // Run reconciliation
    let report = reconcile_once(pool, psp, tenant_id).await?;

    let drifts_injected = 50;
    let total_detected = report.healed_captured + report.drifts_detected;
    let healed = report.healed_captured;
    let incidents = report.incidents_opened;

    let pass = total_detected == drifts_injected && healed == 10 && incidents == 40;

    Ok(T7Result {
        drifts: drifts_injected,
        detected: total_detected,
        healed,
        incidents,
        pass,
    })
}
