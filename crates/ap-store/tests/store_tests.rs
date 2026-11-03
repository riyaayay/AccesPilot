//! Acceptance tests T1 and T2 for ap-store.

use ap_core::money::Currency;
use ap_store::{
    claim_idempotency, complete_idempotency, reserve, seed_offer, seed_task_and_budget,
    settle_captured, verify_ledger_chain, IdempotencyClaim,
};
use chrono::Utc;
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use uuid::Uuid;

fn get_database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres@127.0.0.1:9432/accesspilot".to_string())
}

#[tokio::test]
async fn test_t1_concurrency_cap_5000_attempts() {
    let db_url = get_database_url();
    let pool = match PgPoolOptions::new()
        .max_connections(50)
        .connect(&db_url)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Skipping T1 test: database not reachable: {e}");
            return;
        }
    };

    let tenant_id = "tenant_t1";
    let task_id = Uuid::new_v4();
    let cap_minor = 5000; // ₹50.00
    let currency = Currency::INR;

    seed_task_and_budget(&pool, tenant_id, task_id, cap_minor, currency)
        .await
        .expect("Seed task and budget");

    let total_attempts = 5000;
    let accepted = Arc::new(AtomicUsize::new(0));
    let denied = Arc::new(AtomicUsize::new(0));

    let mut tasks = Vec::with_capacity(total_attempts);

    for i in 0..total_attempts {
        let pool = pool.clone();
        let accepted = accepted.clone();
        let denied = denied.clone();

        // Random-like amount between 100 (₹1) and 2000 (₹20) paise
        let amount = 100 + ((i as i64 * 37) % 1900);
        let purchase_id = Uuid::new_v4();
        let offer_id = Uuid::new_v4();
        let idempotency_key = format!("key_t1_{i}_{purchase_id}");

        tasks.push(tokio::spawn(async move {
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

            match reserve(
                &pool,
                task_id,
                purchase_id,
                offer_id,
                &idempotency_key,
                amount,
                currency,
                1,
            )
            .await
            {
                Ok(_) => {
                    accepted.fetch_add(1, Ordering::Relaxed);
                    // Settle some purchases to captured
                    if i % 2 == 0 {
                        let _ = settle_captured(
                            &pool,
                            purchase_id,
                            &format!("psp_pay_{i}"),
                            "read",
                            Utc::now() + chrono::Duration::hours(1),
                        )
                        .await;
                    }
                }
                Err(_) => {
                    denied.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    for task in tasks {
        task.await.expect("Task join");
    }

    // Verify budget invariants in database
    let budget_row = sqlx::query(
        "SELECT cap_minor, reserved_minor, captured_minor FROM budgets WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&pool)
    .await
    .expect("Fetch budget");

    let cap_minor: i64 = budget_row.get("cap_minor");
    let reserved_minor: i64 = budget_row.get("reserved_minor");
    let captured_minor: i64 = budget_row.get("captured_minor");

    let acc_count = accepted.load(Ordering::SeqCst);
    let den_count = denied.load(Ordering::SeqCst);

    println!(
        "T1 Results: attempts={total_attempts} accepted={acc_count} denied={den_count} reserved={reserved_minor} captured={captured_minor} cap={cap_minor}"
    );

    // Invariant 1: Total allocated never exceeds cap
    assert!(
        reserved_minor + captured_minor <= cap_minor,
        "Violation! reserved ({}) + captured ({}) > cap ({})",
        reserved_minor,
        captured_minor,
        cap_minor
    );

    // Invariant 2: Exactly 5,000 attempts total
    assert_eq!(acc_count + den_count, total_attempts);

    // Invariant 3: Ledger chain is 100% intact
    let chain_intact = verify_ledger_chain(&pool, task_id)
        .await
        .expect("Verify ledger chain");
    assert!(chain_intact, "Ledger chain must be intact");
}

#[tokio::test]
async fn test_t2_idempotency_storm_1000_requests() {
    let db_url = get_database_url();
    let pool = match PgPoolOptions::new()
        .max_connections(50)
        .connect(&db_url)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Skipping T2 test: database not reachable: {e}");
            return;
        }
    };

    let tenant_id = "tenant_t2";
    let single_key = format!("idem_storm_{}", Uuid::new_v4());
    let request_hash = "hash_consistent_payload_12345";

    // Ensure tenant exists for FK constraint
    sqlx::query(
        "INSERT INTO tenants (id, name) VALUES ($1, 'Test Tenant T2') ON CONFLICT (id) DO NOTHING",
    )
    .bind(tenant_id)
    .execute(&pool)
    .await
    .expect("Seed tenant");

    let total_requests = 1000;
    let new_claims = Arc::new(AtomicUsize::new(0));
    let in_progress = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));

    let mut tasks = Vec::with_capacity(total_requests);

    for _ in 0..total_requests {
        let pool = pool.clone();
        let key = single_key.clone();
        let new_claims = new_claims.clone();
        let in_progress = in_progress.clone();
        let completed = completed.clone();

        tasks.push(tokio::spawn(async move {
            match claim_idempotency(&pool, tenant_id, &key, request_hash).await {
                Ok(IdempotencyClaim::New) => {
                    new_claims.fetch_add(1, Ordering::SeqCst);
                    // Simulate doing the charge and storing the response
                    let response = serde_json::json!({
                        "status": "success",
                        "charge_id": "ch_single_charge"
                    });
                    complete_idempotency(&pool, tenant_id, &key, &response)
                        .await
                        .expect("Complete idempotency");
                }
                Ok(IdempotencyClaim::InProgress) => {
                    in_progress.fetch_add(1, Ordering::SeqCst);
                }
                Ok(IdempotencyClaim::Completed(_)) => {
                    completed.fetch_add(1, Ordering::SeqCst);
                }
                Ok(IdempotencyClaim::Conflict) => {
                    panic!("Unexpected conflict with identical payload");
                }
                Err(e) => {
                    panic!("Store error in idempotency claim: {e}");
                }
            }
        }));
    }

    for task in tasks {
        task.await.expect("Task join");
    }

    let new_count = new_claims.load(Ordering::SeqCst);
    let in_prog_count = in_progress.load(Ordering::SeqCst);
    let comp_count = completed.load(Ordering::SeqCst);

    println!(
        "T2 Results: total={total_requests} new_claims={new_count} in_progress={in_prog_count} completed_replays={comp_count}"
    );

    // Exactly one charge executed
    assert_eq!(
        new_count, 1,
        "Exactly one request must claim the idempotency key"
    );
    // All other 999 requests received in-progress or stored completed response
    assert_eq!(new_count + in_prog_count + comp_count, total_requests);
}
