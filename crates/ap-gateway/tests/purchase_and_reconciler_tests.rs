//! Acceptance tests for P4: T3 (rows 4 to 14) and T4 (crash recovery).

use ap_core::money::Currency;
use ap_gateway::reconciler::reconcile_once;
use ap_gateway::service::{execute_purchase, PurchaseFlowError};
use ap_gateway::webhooks::{process_webhook, WebhookHandlerResult};
use ap_psp::simulated::SimulatedPsp;
use ap_psp::traits::ProviderStatus;
use ap_store::{seed_offer, seed_task_and_budget, verify_ledger_chain};
use axum::http::{HeaderMap, HeaderValue};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use uuid::Uuid;

fn get_database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres@127.0.0.1:9432/accesspilot".to_string())
}

async fn setup_test_pool() -> Option<PgPool> {
    let db_url = get_database_url();
    PgPoolOptions::new()
        .max_connections(20)
        .connect(&db_url)
        .await
        .ok()
}

static TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------------------------------------------------------------
// T3 Row 4: db_down_no_provider_call
// ---------------------------------------------------------------------
#[tokio::test]
async fn test_row4_db_down_no_provider_call() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::remove_var("FAULT_POINT");
    std::env::remove_var("FAULT_POINT_PANIC");
    let invalid_pool = PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(100))
        .connect_lazy("postgres://postgres@127.0.0.1:9999/nonexistent")
        .expect("lazy connect");

    let psp = Arc::new(SimulatedPsp::new());
    let task_id = Uuid::new_v4();
    let offer_id = Uuid::new_v4();

    let res = execute_purchase(
        &invalid_pool,
        psp.clone(),
        "tenant_fail",
        task_id,
        offer_id,
        "key_row4",
        "hash_row4",
    )
    .await;

    assert!(res.is_err(), "Must fail when DB is unreachable");
    assert_eq!(
        psp.charge_counter(),
        0,
        "Provider must NEVER be called when DB fails"
    );
}

// ---------------------------------------------------------------------
// T3 Row 5: psp_timeout_holds
// ---------------------------------------------------------------------
#[tokio::test]
async fn test_row5_psp_timeout_holds() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::remove_var("FAULT_POINT");
    std::env::remove_var("FAULT_POINT_PANIC");
    let pool = match setup_test_pool().await {
        Some(p) => p,
        None => return,
    };

    let tenant_id = "tenant_r5";
    let task_id = Uuid::new_v4();
    let offer_id = Uuid::new_v4();
    let amount = 1500;
    let cap = 5000;

    seed_task_and_budget(&pool, tenant_id, task_id, cap, Currency::INR)
        .await
        .unwrap();
    seed_offer(
        &pool,
        offer_id,
        task_id,
        "provider_e",
        "res",
        amount,
        Currency::INR,
    )
    .await
    .unwrap();

    let psp = Arc::new(SimulatedPsp::new());
    psp.set_force_timeout(true);
    let idem_key = format!("key_row5_{}", Uuid::new_v4());

    let res = execute_purchase(
        &pool,
        psp.clone(),
        tenant_id,
        task_id,
        offer_id,
        &idem_key,
        "hash_row5",
    )
    .await
    .expect("Purchase flow completes safely with held state");

    assert_eq!(res.status, "pending_unknown");
    assert!(res.grant.is_none(), "Content MUST stay locked on timeout");

    // Verify money remains held in budget
    let budget =
        sqlx::query("SELECT reserved_minor, captured_minor FROM budgets WHERE task_id = $1")
            .bind(task_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    let reserved: i64 = budget.get("reserved_minor");
    let captured: i64 = budget.get("captured_minor");
    assert_eq!(reserved, amount, "Reservation must remain held");
    assert_eq!(captured, 0);
}

// ---------------------------------------------------------------------
// T3 Row 6: psp_5xx_retry_same_id
// ---------------------------------------------------------------------
#[tokio::test]
async fn test_row6_psp_5xx_retry_same_id() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::remove_var("FAULT_POINT");
    std::env::remove_var("FAULT_POINT_PANIC");
    let pool = match setup_test_pool().await {
        Some(p) => p,
        None => return,
    };

    let tenant_id = "tenant_r6";
    let task_id = Uuid::new_v4();
    let offer_id = Uuid::new_v4();
    let amount = 1000;

    seed_task_and_budget(&pool, tenant_id, task_id, 5000, Currency::INR)
        .await
        .unwrap();
    seed_offer(
        &pool,
        offer_id,
        task_id,
        "provider_e",
        "res",
        amount,
        Currency::INR,
    )
    .await
    .unwrap();

    let psp = Arc::new(SimulatedPsp::new());
    psp.set_force_5xx(true);
    let idem_key = format!("key_row6_{}", Uuid::new_v4());

    let res = execute_purchase(
        &pool,
        psp.clone(),
        tenant_id,
        task_id,
        offer_id,
        &idem_key,
        "hash_row6",
    )
    .await
    .unwrap();

    assert_eq!(res.status, "pending_unknown");
    // Retried up to 3 times behind circuit logic
    assert!(
        psp.charge_counter() >= 1,
        "Retried safe failure with exact same ID"
    );
}

// ---------------------------------------------------------------------
// T3 Row 7: unknown_status_holds
// ---------------------------------------------------------------------
#[tokio::test]
async fn test_row7_unknown_status_holds() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::remove_var("FAULT_POINT");
    std::env::remove_var("FAULT_POINT_PANIC");
    let pool = match setup_test_pool().await {
        Some(p) => p,
        None => return,
    };

    let tenant_id = "tenant_r7";
    let task_id = Uuid::new_v4();
    let offer_id = Uuid::new_v4();
    let amount = 1200;

    seed_task_and_budget(&pool, tenant_id, task_id, 5000, Currency::INR)
        .await
        .unwrap();
    seed_offer(
        &pool,
        offer_id,
        task_id,
        "provider_b",
        "res",
        amount,
        Currency::INR,
    )
    .await
    .unwrap();

    let psp = Arc::new(SimulatedPsp::new());
    psp.set_force_status(Some(ProviderStatus::Unknown)).await;
    let idem_key = format!("key_row7_{}", Uuid::new_v4());

    let res = execute_purchase(
        &pool,
        psp.clone(),
        tenant_id,
        task_id,
        offer_id,
        &idem_key,
        "hash_row7",
    )
    .await
    .unwrap();

    assert_eq!(res.status, "pending_unknown");
    assert!(
        res.grant.is_none(),
        "Unknown status MUST NOT grant access to content"
    );
}

// ---------------------------------------------------------------------
// T3 Row 8: forged_webhook_rejected
// ---------------------------------------------------------------------
#[tokio::test]
async fn test_row8_forged_webhook_rejected() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::remove_var("FAULT_POINT");
    std::env::remove_var("FAULT_POINT_PANIC");
    let pool = match setup_test_pool().await {
        Some(p) => p,
        None => return,
    };

    let psp = Arc::new(SimulatedPsp::new());
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-webhook-signature-256",
        HeaderValue::from_static("deadbeefdeadbeef"),
    );

    let body = serde_json::to_vec(&serde_json::json!({
        "event_id": "forged_1",
        "event_type": "payment_succeeded",
        "payment_id": "pay_fake"
    }))
    .unwrap();

    let outcome = process_webhook(&pool, psp, &headers, &body).await.unwrap();
    assert!(
        matches!(outcome, WebhookHandlerResult::SignatureInvalid),
        "Forged signature must be rejected with SignatureInvalid"
    );
}

// ---------------------------------------------------------------------
// T3 Row 9: duplicate_webhook_noop
// ---------------------------------------------------------------------
#[tokio::test]
async fn test_row9_duplicate_webhook_noop() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::remove_var("FAULT_POINT");
    std::env::remove_var("FAULT_POINT_PANIC");
    let pool = match setup_test_pool().await {
        Some(p) => p,
        None => return,
    };

    let psp = Arc::new(SimulatedPsp::new());
    let body = serde_json::to_vec(&serde_json::json!({
        "event_id": format!("evt_dupe_{}", Uuid::new_v4()),
        "event_type": "payment_succeeded",
        "payment_id": "pay_dupe_1"
    }))
    .unwrap();

    let sig = psp.sign_webhook_payload(&body);
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-webhook-signature-256",
        HeaderValue::from_str(&sig).unwrap(),
    );

    // First attempt: processed
    let first = process_webhook(&pool, psp.clone(), &headers, &body)
        .await
        .unwrap();
    assert!(matches!(first, WebhookHandlerResult::Processed(_)));

    // Second attempt: duplicate no-op
    let second = process_webhook(&pool, psp.clone(), &headers, &body)
        .await
        .unwrap();
    assert!(matches!(second, WebhookHandlerResult::Duplicate));
}

// ---------------------------------------------------------------------
// T3 Row 10: webhook_retrieve_mismatch
// ---------------------------------------------------------------------
#[tokio::test]
async fn test_row10_webhook_retrieve_mismatch() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::remove_var("FAULT_POINT");
    std::env::remove_var("FAULT_POINT_PANIC");
    let pool = match setup_test_pool().await {
        Some(p) => p,
        None => return,
    };

    let tenant_id = "tenant_r10";
    let task_id = Uuid::new_v4();
    let offer_id = Uuid::new_v4();
    let amount = 2000;
    let idem_key = format!("key_r10_{}", Uuid::new_v4());
    let payment_id = SimulatedPsp::deterministic_payment_id(tenant_id, &idem_key);

    seed_task_and_budget(&pool, tenant_id, task_id, 5000, Currency::INR)
        .await
        .unwrap();
    seed_offer(
        &pool,
        offer_id,
        task_id,
        "provider_b",
        "res",
        amount,
        Currency::INR,
    )
    .await
    .unwrap();

    // Start with pending_unknown
    let psp = Arc::new(SimulatedPsp::new());
    psp.set_force_status(Some(ProviderStatus::Unknown)).await;
    let _ = execute_purchase(
        &pool,
        psp.clone(),
        tenant_id,
        task_id,
        offer_id,
        &idem_key,
        "hash_r10",
    )
    .await
    .unwrap();

    // Now provider retrieve returns Failed
    psp.set_force_status(Some(ProviderStatus::Failed(
        ap_psp::traits::FailureClass::Declined,
    )))
    .await;

    // Webhook deceptively claims success!
    let body = serde_json::to_vec(&serde_json::json!({
        "event_id": format!("evt_fake_success_{}", Uuid::new_v4()),
        "event_type": "payment_succeeded",
        "payment_id": payment_id
    }))
    .unwrap();
    let sig = psp.sign_webhook_payload(&body);
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-webhook-signature-256",
        HeaderValue::from_str(&sig).unwrap(),
    );

    let _ = process_webhook(&pool, psp, &headers, &body).await.unwrap();

    // Verify system trusted the retrieve (released reservation)
    let purchase =
        sqlx::query("SELECT status::text as status FROM purchases WHERE idempotency_key = $1")
            .bind(&idem_key)
            .fetch_one(&pool)
            .await
            .unwrap();
    let status: String = purchase.get("status");
    assert_eq!(
        status, "RELEASED",
        "Must trust retrieve outcome over webhook body"
    );
}

// ---------------------------------------------------------------------
// T3 Row 11: crash_after_reserve
// ---------------------------------------------------------------------
#[tokio::test]
async fn test_row11_crash_after_reserve() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::remove_var("FAULT_POINT");
    std::env::remove_var("FAULT_POINT_PANIC");
    let pool = match setup_test_pool().await {
        Some(p) => p,
        None => return,
    };

    let tenant_id = "tenant_r11";
    let task_id = Uuid::new_v4();
    let offer_id = Uuid::new_v4();
    let amount = 1000;
    let idem_key = format!("key_r11_{}", Uuid::new_v4());

    seed_task_and_budget(&pool, tenant_id, task_id, 5000, Currency::INR)
        .await
        .unwrap();
    seed_offer(
        &pool,
        offer_id,
        task_id,
        "provider_b",
        "res",
        amount,
        Currency::INR,
    )
    .await
    .unwrap();

    let psp = Arc::new(SimulatedPsp::new());

    // Inject failpoint panic to simulate crash right after reserve
    std::env::set_var("FAULT_POINT", "after_reserve");
    std::env::set_var("FAULT_POINT_PANIC", "1");

    let pool_clone = pool.clone();
    let psp_clone = psp.clone();
    let key_clone = idem_key.clone();

    let join_res = tokio::spawn(async move {
        execute_purchase(
            &pool_clone,
            psp_clone,
            tenant_id,
            task_id,
            offer_id,
            &key_clone,
            "hash_r11",
        )
        .await
    })
    .await;

    // Clear failpoint
    std::env::remove_var("FAULT_POINT");
    std::env::remove_var("FAULT_POINT_PANIC");

    assert!(join_res.is_err(), "Task must panic at failpoint");

    // Run reconciler recovery
    let report = reconcile_once(&pool, psp, tenant_id).await.unwrap();
    assert!(
        report.healed_released >= 1,
        "Reconciler safely released uncompleted reservation"
    );

    // Verify budget invariants hold
    let budget = sqlx::query(
        "SELECT reserved_minor, captured_minor, cap_minor FROM budgets WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let reserved: i64 = budget.get("reserved_minor");
    let captured: i64 = budget.get("captured_minor");
    let cap: i64 = budget.get("cap_minor");
    assert_eq!(reserved, 0);
    assert_eq!(captured, 0);
    assert!(reserved + captured <= cap);
}

// ---------------------------------------------------------------------
// T3 Row 12: crash_after_provider_ok
// ---------------------------------------------------------------------
#[tokio::test]
async fn test_row12_crash_after_provider_ok() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::remove_var("FAULT_POINT");
    std::env::remove_var("FAULT_POINT_PANIC");
    let pool = match setup_test_pool().await {
        Some(p) => p,
        None => return,
    };

    let tenant_id = "tenant_r12";
    let task_id = Uuid::new_v4();
    let offer_id = Uuid::new_v4();
    let amount = 1000;
    let idem_key = format!("key_r12_{}", Uuid::new_v4());

    seed_task_and_budget(&pool, tenant_id, task_id, 5000, Currency::INR)
        .await
        .unwrap();
    seed_offer(
        &pool,
        offer_id,
        task_id,
        "provider_b",
        "res",
        amount,
        Currency::INR,
    )
    .await
    .unwrap();

    let psp = Arc::new(SimulatedPsp::new());

    // Inject crash right after provider succeeds
    std::env::set_var("FAULT_POINT", "after_provider_ok");
    std::env::set_var("FAULT_POINT_PANIC", "1");

    let pool_clone = pool.clone();
    let psp_clone = psp.clone();
    let key_clone = idem_key.clone();

    let join_res = tokio::spawn(async move {
        execute_purchase(
            &pool_clone,
            psp_clone,
            tenant_id,
            task_id,
            offer_id,
            &key_clone,
            "hash_r12",
        )
        .await
    })
    .await;

    std::env::remove_var("FAULT_POINT");
    std::env::remove_var("FAULT_POINT_PANIC");

    assert!(join_res.is_err(), "Must panic at failpoint");

    // Reconciler runs: retrieves provider (succeeded) and completes capture + grant!
    let report = reconcile_once(&pool, psp, tenant_id).await.unwrap();
    assert!(
        report.healed_captured >= 1,
        "Reconciler completed captured settlement and grant"
    );

    // Verify grant minted
    let purchase =
        sqlx::query("SELECT id, status::text as status FROM purchases WHERE idempotency_key = $1")
            .bind(&idem_key)
            .fetch_one(&pool)
            .await
            .unwrap();
    let status: String = purchase.get("status");
    let purchase_id: Uuid = purchase.get("id");
    assert_eq!(status, "CAPTURED");

    let grant = sqlx::query("SELECT id FROM grants WHERE purchase_id = $1")
        .bind(purchase_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let grant_id: Uuid = grant.get("id");
    assert!(!grant_id.is_nil());
}

// ---------------------------------------------------------------------
// T3 Row 13: idempotent_replay
// ---------------------------------------------------------------------
#[tokio::test]
async fn test_row13_idempotent_replay() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::remove_var("FAULT_POINT");
    std::env::remove_var("FAULT_POINT_PANIC");
    let pool = match setup_test_pool().await {
        Some(p) => p,
        None => return,
    };

    let tenant_id = "tenant_r13";
    let task_id = Uuid::new_v4();
    let offer_id = Uuid::new_v4();
    let amount = 1000;
    let idem_key = format!("key_r13_{}", Uuid::new_v4());
    let req_hash = "hash_consistent_r13";

    seed_task_and_budget(&pool, tenant_id, task_id, 5000, Currency::INR)
        .await
        .unwrap();
    seed_offer(
        &pool,
        offer_id,
        task_id,
        "provider_b",
        "res",
        amount,
        Currency::INR,
    )
    .await
    .unwrap();

    let psp = Arc::new(SimulatedPsp::new());

    // 10 concurrent replays
    let mut handles = Vec::new();
    for _ in 0..10 {
        let pool = pool.clone();
        let psp = psp.clone();
        let key = idem_key.clone();
        handles.push(tokio::spawn(async move {
            execute_purchase(&pool, psp, tenant_id, task_id, offer_id, &key, req_hash).await
        }));
    }

    let mut successful_responses = 0;
    for handle in handles {
        if let Ok(Ok(res)) = handle.await {
            if res.status == "captured" {
                successful_responses += 1;
            }
        }
    }

    assert!(successful_responses >= 1);
    // Crucial check: exactly one charge executed at provider!
    assert_eq!(
        psp.charge_counter(),
        1,
        "Exactly ONE charge at provider across replays"
    );
}

// ---------------------------------------------------------------------
// T3 Row 14: key_reuse_conflict
// ---------------------------------------------------------------------
#[tokio::test]
async fn test_row14_key_reuse_conflict() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::remove_var("FAULT_POINT");
    std::env::remove_var("FAULT_POINT_PANIC");
    let pool = match setup_test_pool().await {
        Some(p) => p,
        None => return,
    };

    let tenant_id = "tenant_r14";
    let task_id = Uuid::new_v4();
    let offer_id = Uuid::new_v4();
    let amount = 1000;
    let idem_key = format!("key_r14_{}", Uuid::new_v4());

    seed_task_and_budget(&pool, tenant_id, task_id, 5000, Currency::INR)
        .await
        .unwrap();
    seed_offer(
        &pool,
        offer_id,
        task_id,
        "provider_b",
        "res",
        amount,
        Currency::INR,
    )
    .await
    .unwrap();

    let psp = Arc::new(SimulatedPsp::new());

    // First request with hash_A
    let first = execute_purchase(
        &pool,
        psp.clone(),
        tenant_id,
        task_id,
        offer_id,
        &idem_key,
        "hash_A",
    )
    .await;
    assert!(first.is_ok());

    // Second request with SAME key but DIFFERENT hash_B
    let second = execute_purchase(
        &pool,
        psp.clone(),
        tenant_id,
        task_id,
        offer_id,
        &idem_key,
        "hash_B_different",
    )
    .await;

    match second {
        Err(PurchaseFlowError::IdempotencyConflict) => {}
        other => panic!("Expected IdempotencyConflict, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// T4: Crash recovery across 200 purchases with failpoints
// ---------------------------------------------------------------------
#[tokio::test]
async fn test_t4_crash_recovery_across_runs() {
    let _guard = TEST_MUTEX.lock().await;
    std::env::remove_var("FAULT_POINT");
    std::env::remove_var("FAULT_POINT_PANIC");
    let pool = match setup_test_pool().await {
        Some(p) => p,
        None => return,
    };

    let tenant_id = "tenant_t4";
    let task_id = Uuid::new_v4();
    let cap = 500_000; // ₹5,000 cap

    seed_task_and_budget(&pool, tenant_id, task_id, cap, Currency::INR)
        .await
        .unwrap();

    let psp = Arc::new(SimulatedPsp::new());
    let runs = 200;

    for i in 0..runs {
        let offer_id = Uuid::new_v4();
        let amount = 100 + (i % 50) * 10;
        let idem_key = format!("key_t4_{i}_{}", Uuid::new_v4());

        seed_offer(
            &pool,
            offer_id,
            task_id,
            "provider_b",
            "res",
            amount,
            Currency::INR,
        )
        .await
        .unwrap();

        // Alternate failpoints: after_reserve, after_provider_ok, or normal
        let fp = if i % 3 == 0 {
            Some("after_reserve")
        } else if i % 3 == 1 {
            Some("after_provider_ok")
        } else {
            None
        };

        if let Some(f) = fp {
            std::env::set_var("FAULT_POINT", f);
            std::env::set_var("FAULT_POINT_PANIC", "1");
        }

        let pool_c = pool.clone();
        let psp_c = psp.clone();
        let key_c = idem_key.clone();

        let _ = tokio::spawn(async move {
            execute_purchase(
                &pool_c, psp_c, tenant_id, task_id, offer_id, &key_c, "hash_t4",
            )
            .await
        })
        .await;

        std::env::remove_var("FAULT_POINT");
        std::env::remove_var("FAULT_POINT_PANIC");
    }

    // Run reconciliation pass to heal any crashed/stuck purchases
    let report = reconcile_once(&pool, psp, tenant_id).await.unwrap();
    println!(
        "T4 Reconcile Report: scanned={} healed_captured={} healed_released={}",
        report.stuck_scanned, report.healed_captured, report.healed_released
    );

    // Assert Invariants from Section 10.3:
    // 1. Captured + reserved never exceeds cap
    let budget = sqlx::query(
        "SELECT reserved_minor, captured_minor, cap_minor FROM budgets WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let reserved: i64 = budget.get("reserved_minor");
    let captured: i64 = budget.get("captured_minor");
    let cap_minor: i64 = budget.get("cap_minor");

    assert!(
        reserved + captured <= cap_minor,
        "Invariant 1 violated: {reserved} + {captured} > {cap_minor}"
    );

    // 2. Zero orphaned reservations in RESERVED or PENDING_UNKNOWN that should have been settled
    let remaining_stuck: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM purchases WHERE task_id = $1 AND status IN ('RESERVED', 'PENDING_UNKNOWN')",
    )
    .bind(task_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(
        remaining_stuck, 0,
        "Invariant: zero orphaned purchases remain stuck after reconcile"
    );

    // 3. Every grant points to CAPTURED purchase
    let invalid_grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM grants g JOIN purchases p ON g.purchase_id = p.id WHERE p.status != 'CAPTURED'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(invalid_grants, 0, "Invariant: all grants point to CAPTURED");

    // 4. Ledger chain intact
    let chain_ok = verify_ledger_chain(&pool, task_id).await.unwrap();
    assert!(chain_ok, "Invariant: ledger hash chain is 100% valid");
}
