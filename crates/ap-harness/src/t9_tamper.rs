//! T9: Ledger tamper benchmark.
//! Injects 100 random single-field mutations into ledger events.
//! Verifies that 100 out of 100 are detected at the exact sequence number.

use ap_core::money::Currency;
use ap_store::{append_ledger_event, seed_task_and_budget, verify_ledger_chain, StoreError};
use sqlx::PgPool;
use uuid::Uuid;

pub struct T9Result {
    pub mutations: usize,
    pub detected: usize,
    pub pass: bool,
}

pub async fn run_t9(pool: &PgPool) -> Result<T9Result, StoreError> {
    let total_mutations = 100;
    let mut detected = 0;

    // Temporarily allow mutation by disabling the immutability trigger for benchmark injection
    sqlx::query("ALTER TABLE ledger_events DISABLE TRIGGER trg_no_ledger_mutation")
        .execute(pool)
        .await?;

    for i in 0..total_mutations {
        let tenant_id = "tenant_t9_tamper";
        let task_id = Uuid::new_v4();
        seed_task_and_budget(pool, tenant_id, task_id, 100_000, Currency::INR).await?;

        // Append 4 events to build a hash chain:
        // seq 1: task_created
        // seq 2: offer_seen
        // seq 3: reserved
        // seq 4: provider_called
        let _e1 = append_ledger_event(
            pool,
            task_id,
            "task_created",
            serde_json::json!({"tenant": tenant_id}),
        )
        .await?;

        let _e2 = append_ledger_event(
            pool,
            task_id,
            "offer_seen",
            serde_json::json!({"provider": "prov_a", "amount_minor": 1000}),
        )
        .await?;

        let _e3 = append_ledger_event(
            pool,
            task_id,
            "reserved",
            serde_json::json!({"amount_minor": 1000}),
        )
        .await?;

        let _e4 = append_ledger_event(
            pool,
            task_id,
            "provider_called",
            serde_json::json!({"payment_id": "pay_test"}),
        )
        .await?;

        // Before tamper: chain must verify cleanly
        assert!(
            verify_ledger_chain(pool, task_id).await.is_ok(),
            "Pre-tamper chain must verify"
        );

        // Pick a target sequence to tamper (between 1 and 3)
        let target_seq: i64 = 1 + ((i as i64) % 3);

        // Mutate a single field in the payload of target_seq
        sqlx::query(
            r#"
            UPDATE ledger_events
            SET payload = jsonb_set(payload, '{tampered_field}', '"corrupted_value"')
            WHERE task_id = $1 AND seq = $2
            "#,
        )
        .bind(task_id)
        .bind(target_seq)
        .execute(pool)
        .await?;

        // Re-verify the ledger chain
        match verify_ledger_chain(pool, task_id).await {
            Err(StoreError::LedgerChainBroken { seq, .. }) if seq == target_seq => {
                detected += 1;
            }
            _ => {
                // Not detected or wrong error
            }
        }
    }

    // Re-enable the immutability trigger
    sqlx::query("ALTER TABLE ledger_events ENABLE TRIGGER trg_no_ledger_mutation")
        .execute(pool)
        .await?;

    let pass = detected == total_mutations;

    Ok(T9Result {
        mutations: total_mutations,
        detected,
        pass,
    })
}
