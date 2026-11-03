//! ap-store: Persistence layer for budgets, purchases, idempotency, ledger and grants.

use ap_core::money::Currency;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use thiserror::Error;
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("Database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("Insufficient budget for task {0}")]
    InsufficientBudget(Uuid),
    #[error("Purchase {0} not found")]
    PurchaseNotFound(Uuid),
    #[error("Invalid purchase state for operation: {0}")]
    InvalidPurchaseState(String),
    #[error("Idempotency conflict: payload mismatch for key {0}")]
    IdempotencyConflict(String),
    #[error(
        "Ledger chain verification failed at sequence {seq}: expected {expected}, got {actual}"
    )]
    LedgerChainBroken {
        seq: i64,
        expected: String,
        actual: String,
    },
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdempotencyClaim {
    New,
    InProgress,
    Completed(Value),
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReserveResult {
    pub reserved_minor: i64,
    pub captured_minor: i64,
    pub cap_minor: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantRecord {
    pub id: Uuid,
    pub purchase_id: Uuid,
    pub token: String,
    pub scope: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEventRecord {
    pub task_id: Uuid,
    pub seq: i64,
    pub event_type: String,
    pub payload: Value,
    pub prev_hash: String,
    pub hash: String,
    pub created_at: DateTime<Utc>,
}

/// Compute SHA-256 hash of (prev_hash || event_type || canonical_payload_json).
pub fn compute_event_hash(prev_hash: &str, event_type: &str, payload: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prev_hash.as_bytes());
    hasher.update(event_type.as_bytes());
    let payload_str = serde_json::to_string(payload).unwrap_or_default();
    hasher.update(payload_str.as_bytes());
    hex::encode(hasher.finalize())
}

/// Claim or check an idempotency key.
pub async fn claim_idempotency(
    pool: &PgPool,
    tenant_id: &str,
    key: &str,
    request_hash: &str,
) -> Result<IdempotencyClaim, StoreError> {
    // Attempt insert with IN_PROGRESS
    let inserted = sqlx::query(
        r#"
        INSERT INTO idempotency_keys (tenant_id, key, request_hash, status)
        VALUES ($1, $2, $3, 'IN_PROGRESS')
        ON CONFLICT (tenant_id, key) DO NOTHING
        RETURNING key
        "#,
    )
    .bind(tenant_id)
    .bind(key)
    .bind(request_hash)
    .fetch_optional(pool)
    .await?;

    if inserted.is_some() {
        return Ok(IdempotencyClaim::New);
    }

    // Key exists, fetch status
    let row = sqlx::query(
        r#"
        SELECT request_hash, status, response
        FROM idempotency_keys
        WHERE tenant_id = $1 AND key = $2
        "#,
    )
    .bind(tenant_id)
    .bind(key)
    .fetch_one(pool)
    .await?;

    let stored_hash: String = row.get("request_hash");
    let status: String = row.get("status");
    let response: Option<Value> = row.get("response");

    if stored_hash != request_hash {
        return Ok(IdempotencyClaim::Conflict);
    }

    if status == "COMPLETED" {
        return Ok(IdempotencyClaim::Completed(response.unwrap_or(Value::Null)));
    }

    Ok(IdempotencyClaim::InProgress)
}

/// Store completed response for an idempotency key.
pub async fn complete_idempotency(
    pool: &PgPool,
    tenant_id: &str,
    key: &str,
    response: &Value,
) -> Result<(), StoreError> {
    sqlx::query(
        r#"
        UPDATE idempotency_keys
        SET status = 'COMPLETED', response = $1
        WHERE tenant_id = $2 AND key = $3
        "#,
    )
    .bind(response)
    .bind(tenant_id)
    .bind(key)
    .execute(pool)
    .await?;
    Ok(())
}

/// Append a ledger event within an existing transaction.
pub async fn append_ledger_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    task_id: Uuid,
    event_type: &str,
    payload: Value,
) -> Result<LedgerEventRecord, StoreError> {
    // Find latest seq and hash for task
    let latest = sqlx::query(
        r#"
        SELECT seq, hash
        FROM ledger_events
        WHERE task_id = $1
        ORDER BY seq DESC
        LIMIT 1
        FOR UPDATE
        "#,
    )
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;

    let (seq, prev_hash) = match latest {
        Some(r) => {
            let s: i64 = r.get("seq");
            let h: String = r.get("hash");
            (s + 1, h)
        }
        None => (
            0,
            "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        ),
    };

    let hash = compute_event_hash(&prev_hash, event_type, &payload);

    let row = sqlx::query(
        r#"
        INSERT INTO ledger_events (task_id, seq, type, payload, prev_hash, hash)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING created_at
        "#,
    )
    .bind(task_id)
    .bind(seq)
    .bind(event_type)
    .bind(&payload)
    .bind(&prev_hash)
    .bind(&hash)
    .fetch_one(&mut **tx)
    .await?;

    let created_at: DateTime<Utc> = row.get("created_at");

    Ok(LedgerEventRecord {
        task_id,
        seq,
        event_type: event_type.to_string(),
        payload,
        prev_hash,
        hash,
        created_at,
    })
}

/// Append a ledger event outside a transaction.
pub async fn append_ledger_event(
    pool: &PgPool,
    task_id: Uuid,
    event_type: &str,
    payload: Value,
) -> Result<LedgerEventRecord, StoreError> {
    let mut tx = pool.begin().await?;
    let record = append_ledger_event_tx(&mut tx, task_id, event_type, payload).await?;
    tx.commit().await?;
    Ok(record)
}

/// Atomic budget reservation with write-ahead purchase intent.
#[allow(clippy::too_many_arguments)]
pub async fn reserve(
    pool: &PgPool,
    task_id: Uuid,
    purchase_id: Uuid,
    offer_id: Uuid,
    idempotency_key: &str,
    amount_minor: i64,
    currency: Currency,
    policy_version: i32,
) -> Result<ReserveResult, StoreError> {
    let mut tx = pool.begin().await?;
    let currency_str = currency.to_string();

    // Section 8.2: The atomic UPDATE statement
    let budget_row = sqlx::query(
        r#"
        UPDATE budgets
           SET reserved_minor = reserved_minor + $1, version = version + 1
         WHERE task_id = $2 AND currency = $3
           AND reserved_minor + captured_minor + $1 <= cap_minor
        RETURNING reserved_minor, captured_minor, cap_minor
        "#,
    )
    .bind(amount_minor)
    .bind(task_id)
    .bind(&currency_str)
    .fetch_optional(&mut *tx)
    .await?;

    let res = match budget_row {
        Some(b) => ReserveResult {
            reserved_minor: b.get("reserved_minor"),
            captured_minor: b.get("captured_minor"),
            cap_minor: b.get("cap_minor"),
        },
        None => {
            return Err(StoreError::InsufficientBudget(task_id));
        }
    };

    // Insert purchase record in RESERVED status
    sqlx::query(
        r#"
        INSERT INTO purchases (id, task_id, offer_id, idempotency_key, amount_minor, status, policy_version)
        VALUES ($1, $2, $3, $4, $5, 'RESERVED', $6)
        "#,
    )
    .bind(purchase_id)
    .bind(task_id)
    .bind(offer_id)
    .bind(idempotency_key)
    .bind(amount_minor)
    .bind(policy_version)
    .execute(&mut *tx)
    .await?;

    // Append ledger event: reserved
    append_ledger_event_tx(
        &mut tx,
        task_id,
        "reserved",
        serde_json::json!({
            "purchase_id": purchase_id,
            "offer_id": offer_id,
            "amount_minor": amount_minor,
            "currency": currency_str,
            "idempotency_key": idempotency_key
        }),
    )
    .await?;

    tx.commit().await?;
    Ok(res)
}

/// Settle purchase to CAPTURED, update budget, issue grant, and append ledger events in one transaction.
pub async fn settle_captured(
    pool: &PgPool,
    purchase_id: Uuid,
    provider_payment_id: &str,
    grant_scope: &str,
    grant_expiry: DateTime<Utc>,
) -> Result<GrantRecord, StoreError> {
    let mut tx = pool.begin().await?;

    // Lock and inspect purchase
    let purchase = sqlx::query(
        r#"
        SELECT task_id, amount_minor, status::text as status
        FROM purchases
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(purchase_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(StoreError::PurchaseNotFound(purchase_id))?;

    let p_status: String = purchase.get("status");
    let p_task_id: Uuid = purchase.get("task_id");
    let p_amount_minor: i64 = purchase.get("amount_minor");

    if p_status != "RESERVED" && p_status != "PENDING_UNKNOWN" {
        return Err(StoreError::InvalidPurchaseState(p_status));
    }

    // 1. Move purchase to CAPTURED
    sqlx::query(
        r#"
        UPDATE purchases
           SET status = 'CAPTURED', provider_payment_id = $2
         WHERE id = $1
        "#,
    )
    .bind(purchase_id)
    .bind(provider_payment_id)
    .execute(&mut *tx)
    .await?;

    // 2. Shift budget: reserved -> captured
    sqlx::query(
        r#"
        UPDATE budgets
           SET reserved_minor = reserved_minor - $1, captured_minor = captured_minor + $1, version = version + 1
         WHERE task_id = $2
        "#,
    )
    .bind(p_amount_minor)
    .bind(p_task_id)
    .execute(&mut *tx)
    .await?;

    // 3. Issue grant
    let grant_id = Uuid::new_v4();
    let token = format!("ap_grant_{}", Uuid::new_v4());
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let token_hash = hasher.finalize().to_vec();

    sqlx::query(
        r#"
        INSERT INTO grants (id, purchase_id, purchase_status, token_hash, scope, expires_at)
        VALUES ($1, $2, 'CAPTURED', $3, $4, $5)
        "#,
    )
    .bind(grant_id)
    .bind(purchase_id)
    .bind(&token_hash)
    .bind(grant_scope)
    .bind(grant_expiry)
    .execute(&mut *tx)
    .await?;

    // 4. Ledger events: captured and grant_issued
    append_ledger_event_tx(
        &mut tx,
        p_task_id,
        "captured",
        serde_json::json!({
            "purchase_id": purchase_id,
            "provider_payment_id": provider_payment_id,
            "amount_minor": p_amount_minor
        }),
    )
    .await?;

    append_ledger_event_tx(
        &mut tx,
        p_task_id,
        "grant_issued",
        serde_json::json!({
            "grant_id": grant_id,
            "purchase_id": purchase_id,
            "scope": grant_scope,
            "expires_at": grant_expiry
        }),
    )
    .await?;

    tx.commit().await?;

    Ok(GrantRecord {
        id: grant_id,
        purchase_id,
        token,
        scope: grant_scope.to_string(),
        expires_at: grant_expiry,
    })
}

/// Release a reserved budget upon definite failure.
pub async fn release(pool: &PgPool, purchase_id: Uuid) -> Result<(), StoreError> {
    let mut tx = pool.begin().await?;

    let purchase = sqlx::query(
        r#"
        SELECT task_id, amount_minor, status::text as status
        FROM purchases
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(purchase_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(StoreError::PurchaseNotFound(purchase_id))?;

    let p_status: String = purchase.get("status");
    let p_task_id: Uuid = purchase.get("task_id");
    let p_amount_minor: i64 = purchase.get("amount_minor");

    if p_status == "RELEASED" {
        return Ok(());
    }

    if p_status != "RESERVED" && p_status != "PENDING_UNKNOWN" {
        return Err(StoreError::InvalidPurchaseState(p_status));
    }

    // Move purchase to RELEASED
    sqlx::query(
        r#"
        UPDATE purchases
           SET status = 'RELEASED'
         WHERE id = $1
        "#,
    )
    .bind(purchase_id)
    .execute(&mut *tx)
    .await?;

    // Decrement reserved budget
    sqlx::query(
        r#"
        UPDATE budgets
           SET reserved_minor = reserved_minor - $1, version = version + 1
         WHERE task_id = $2
        "#,
    )
    .bind(p_amount_minor)
    .bind(p_task_id)
    .execute(&mut *tx)
    .await?;

    // Append ledger event: released
    append_ledger_event_tx(
        &mut tx,
        p_task_id,
        "released",
        serde_json::json!({
            "purchase_id": purchase_id,
            "amount_minor": p_amount_minor
        }),
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Mark purchase as PENDING_UNKNOWN on timeout or indeterminate outcome. Reservation stays held.
pub async fn mark_pending_unknown(pool: &PgPool, purchase_id: Uuid) -> Result<(), StoreError> {
    let mut tx = pool.begin().await?;

    let purchase = sqlx::query(
        r#"
        SELECT task_id, amount_minor, status::text as status
        FROM purchases
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(purchase_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(StoreError::PurchaseNotFound(purchase_id))?;

    let p_status: String = purchase.get("status");
    let p_task_id: Uuid = purchase.get("task_id");
    let p_amount_minor: i64 = purchase.get("amount_minor");

    if p_status == "PENDING_UNKNOWN" {
        return Ok(());
    }

    if p_status != "RESERVED" {
        return Err(StoreError::InvalidPurchaseState(p_status));
    }

    sqlx::query(
        r#"
        UPDATE purchases
           SET status = 'PENDING_UNKNOWN'
         WHERE id = $1
        "#,
    )
    .bind(purchase_id)
    .execute(&mut *tx)
    .await?;

    append_ledger_event_tx(
        &mut tx,
        p_task_id,
        "provider_timeout_or_unknown",
        serde_json::json!({
            "purchase_id": purchase_id,
            "amount_minor": p_amount_minor,
            "action": "holding_reservation"
        }),
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Verify the entire hash-chain for a task's ledger events.
pub async fn verify_ledger_chain(pool: &PgPool, task_id: Uuid) -> Result<bool, StoreError> {
    let events = sqlx::query(
        r#"
        SELECT seq, type, payload, prev_hash, hash
        FROM ledger_events
        WHERE task_id = $1
        ORDER BY seq ASC
        "#,
    )
    .bind(task_id)
    .fetch_all(pool)
    .await?;

    let mut expected_prev =
        "0000000000000000000000000000000000000000000000000000000000000000".to_string();

    for row in events {
        let seq: i64 = row.get("seq");
        let event_type: String = row.get("type");
        let payload: Value = row.get("payload");
        let prev_hash: String = row.get("prev_hash");
        let hash: String = row.get("hash");

        if prev_hash != expected_prev {
            return Err(StoreError::LedgerChainBroken {
                seq,
                expected: expected_prev,
                actual: prev_hash,
            });
        }

        let computed = compute_event_hash(&prev_hash, &event_type, &payload);
        if computed != hash {
            return Err(StoreError::LedgerChainBroken {
                seq,
                expected: computed,
                actual: hash,
            });
        }

        expected_prev = hash;
    }

    Ok(true)
}

/// Helper to seed tenant, task, policy, and budget for testing.
pub async fn seed_task_and_budget(
    pool: &PgPool,
    tenant_id: &str,
    task_id: Uuid,
    cap_minor: i64,
    currency: Currency,
) -> Result<(), StoreError> {
    let currency_str = currency.to_string();

    // Ensure tenant exists
    sqlx::query(
        r#"
        INSERT INTO tenants (id, name)
        VALUES ($1, 'Test Tenant')
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(tenant_id)
    .execute(pool)
    .await?;

    // Insert task
    sqlx::query(
        r#"
        INSERT INTO tasks (id, tenant_id, goal, status, policy_version, autonomy)
        VALUES ($1, $2, 'Bench Task', 'running', 1, 'within_rules')
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(task_id)
    .bind(tenant_id)
    .execute(pool)
    .await?;

    // Insert budget
    sqlx::query(
        r#"
        INSERT INTO budgets (task_id, currency, cap_minor, reserved_minor, captured_minor, version)
        VALUES ($1, $2, $3, 0, 0, 0)
        ON CONFLICT (task_id) DO UPDATE SET cap_minor = $3, reserved_minor = 0, captured_minor = 0
        "#,
    )
    .bind(task_id)
    .bind(&currency_str)
    .bind(cap_minor)
    .execute(pool)
    .await?;

    Ok(())
}

/// Helper to seed an offer for a task.
pub async fn seed_offer(
    pool: &PgPool,
    offer_id: Uuid,
    task_id: Uuid,
    provider_id: &str,
    resource_id: &str,
    amount_minor: i64,
    currency: Currency,
) -> Result<(), StoreError> {
    let currency_str = currency.to_string();
    let expires_at = Utc::now() + chrono::Duration::hours(1);

    sqlx::query(
        r#"
        INSERT INTO offers (id, task_id, provider_id, resource_id, amount_minor, currency, expires_at, sig)
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'test_sig')
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(offer_id)
    .bind(task_id)
    .bind(provider_id)
    .bind(resource_id)
    .bind(amount_minor)
    .bind(&currency_str)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok(())
}
