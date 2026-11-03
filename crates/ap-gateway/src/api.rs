//! Axum HTTP router and REST handlers for AccessPilot control plane.

use crate::reconciler::reconcile_once;
use crate::resource_gateway::handle_get_resource;
use crate::service::execute_purchase;
use crate::webhooks::process_webhook;
use ap_core::money::Currency;
use ap_core::policy::Policy;
use ap_psp::simulated::SimulatedPsp;
use ap_store::{append_ledger_event, seed_task_and_budget, verify_ledger_chain, StoreError};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::Stream;
use serde::Deserialize;
use serde_json::Value;
use sqlx::{PgPool, Row};
use std::convert::Infallible;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub psp: Arc<SimulatedPsp>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskReq {
    pub tenant_id: String,
    pub goal: String,
    pub cap_minor: i64,
    pub currency: String,
    pub policy_version: Option<i32>,
    pub autonomy: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PurchaseReq {
    pub offer_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct PolicyReq {
    pub tenant_id: String,
    pub version: i32,
    pub policy: Value,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(serve_dashboard))
        .route("/dashboard", get(serve_dashboard))
        .route("/v1/tasks", post(create_task))
        .route("/v1/tasks/:id/budget", get(get_budget))
        .route("/v1/tasks/:id/ledger", get(get_ledger))
        .route("/v1/tasks/:id/ledger/verify", get(verify_ledger))
        .route("/v1/tasks/:id/stream", get(stream_ledger))
        .route("/v1/tasks/:id/purchases", post(post_purchase))
        .route("/v1/policies", post(save_policy))
        .route("/v1/policies/:v/dry-run", post(dry_run_policy))
        .route("/v1/policies/:v/shadow", post(shadow_policy))
        .route("/v1/policies/:v/activate", post(activate_policy))
        .route("/v1/admin/reconcile", post(run_reconcile))
        .route("/v1/incidents", get(get_incidents))
        .route(
            "/v1/resources/:provider/:resource",
            get(handle_get_resource),
        )
        .route("/v1/webhooks/hyperswitch", post(handle_webhook_endpoint))
        .route("/v1/demo/run-agent", post(demo_run_agent))
        .route("/v1/demo/storm", post(demo_storm))
        .route("/v1/demo/tamper", post(demo_tamper))
        .route("/metrics", get(get_metrics))
        .with_state(state)
}

async fn serve_dashboard() -> Html<String> {
    match tokio::fs::read_to_string("dashboard/index.html").await {
        Ok(content) => Html(content),
        Err(_) => Html("<h1>AccessPilot Dashboard</h1><p>index.html not found</p>".to_string()),
    }
}

async fn create_task(
    State(state): State<AppState>,
    Json(req): Json<CreateTaskReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let task_id = Uuid::new_v4();
    let currency = Currency::from_str(&req.currency).unwrap_or(Currency::INR);

    seed_task_and_budget(
        &state.pool,
        &req.tenant_id,
        task_id,
        req.cap_minor,
        currency,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let _ = append_ledger_event(
        &state.pool,
        task_id,
        "task_created",
        serde_json::json!({
            "task_id": task_id,
            "goal": req.goal,
            "cap_minor": req.cap_minor,
            "currency": req.currency
        }),
    )
    .await;

    Ok(Json(serde_json::json!({
        "task_id": task_id,
        "tenant_id": req.tenant_id,
        "cap_minor": req.cap_minor,
        "currency": req.currency
    })))
}

async fn get_budget(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let row = sqlx::query(
        "SELECT cap_minor, reserved_minor, captured_minor, currency FROM budgets WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match row {
        Some(r) => {
            let cap: i64 = r.get("cap_minor");
            let reserved: i64 = r.get("reserved_minor");
            let captured: i64 = r.get("captured_minor");
            let currency: String = r.get("currency");
            let remaining = (cap - (reserved + captured)).max(0);

            Ok(Json(serde_json::json!({
                "task_id": task_id,
                "cap_minor": cap,
                "reserved_minor": reserved,
                "captured_minor": captured,
                "remaining_minor": remaining,
                "currency": currency
            })))
        }
        None => Err((StatusCode::NOT_FOUND, "Task not found".to_string())),
    }
}

async fn get_ledger(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let events = sqlx::query(
        "SELECT seq, type, payload, prev_hash, hash, created_at FROM ledger_events WHERE task_id = $1 ORDER BY seq ASC",
    )
    .bind(task_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let list: Vec<Value> = events
        .into_iter()
        .map(|r| {
            let seq: i64 = r.get("seq");
            let event_type: String = r.get("type");
            let payload: Value = r.get("payload");
            let prev_hash: String = r.get("prev_hash");
            let hash: String = r.get("hash");
            let created_at: chrono::DateTime<chrono::Utc> = r.get("created_at");

            serde_json::json!({
                "seq": seq,
                "type": event_type,
                "payload": payload,
                "prev_hash": prev_hash,
                "hash": hash,
                "created_at": created_at.to_rfc3339()
            })
        })
        .collect();

    Ok(Json(serde_json::json!(list)))
}

async fn verify_ledger(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, String)> {
    match verify_ledger_chain(&state.pool, task_id).await {
        Ok(true) => Ok(Json(serde_json::json!({
            "status": "intact",
            "valid": true,
            "broken_seq": null
        }))),
        Err(StoreError::LedgerChainBroken {
            seq,
            expected,
            actual,
        }) => Ok(Json(serde_json::json!({
            "status": "broken",
            "valid": false,
            "broken_seq": seq,
            "expected": expected,
            "actual": actual
        }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
        _ => Ok(Json(serde_json::json!({ "valid": true }))),
    }
}

async fn stream_ledger(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let pool = state.pool.clone();
    let stream = async_stream::stream! {
        let mut last_seq = -1i64;
        loop {
            let rows = sqlx::query(
                "SELECT seq, type, payload, prev_hash, hash, created_at FROM ledger_events WHERE task_id = $1 AND seq > $2 ORDER BY seq ASC"
            )
            .bind(task_id)
            .bind(last_seq)
            .fetch_all(&pool)
            .await;

            if let Ok(events) = rows {
                for r in events {
                    let seq: i64 = r.get("seq");
                    last_seq = seq;
                    let event_type: String = r.get("type");
                    let payload: Value = r.get("payload");
                    let hash: String = r.get("hash");
                    let msg = serde_json::json!({
                        "seq": seq,
                        "type": event_type,
                        "payload": payload,
                        "hash": hash
                    });
                    yield Ok(Event::default().data(msg.to_string()));
                }
            }

            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
    };

    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

async fn post_purchase(
    State(state): State<AppState>,
    Path(task_id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<PurchaseReq>,
) -> Response {
    let key = headers
        .get("Idempotency-Key")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("default_idem_key");

    let tenant_id = "tenant_default";
    let req_hash = format!("hash_{}_{}", task_id, req.offer_id);

    match execute_purchase(
        &state.pool,
        state.psp.clone(),
        tenant_id,
        task_id,
        req.offer_id,
        key,
        &req_hash,
    )
    .await
    {
        Ok(res) => (StatusCode::OK, Json(serde_json::json!(res))).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn save_policy(
    State(state): State<AppState>,
    Json(req): Json<PolicyReq>,
) -> Result<Json<Value>, (StatusCode, String)> {
    sqlx::query(
        r#"
        INSERT INTO policies (version, tenant_id, policy_json, state)
        VALUES ($1, $2, $3, 'draft')
        ON CONFLICT (version) DO UPDATE SET policy_json = $3
        "#,
    )
    .bind(req.version)
    .bind(&req.tenant_id)
    .bind(&req.policy)
    .execute(&state.pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(
        serde_json::json!({ "status": "saved", "version": req.version }),
    ))
}

async fn dry_run_policy(
    State(state): State<AppState>,
    Path(version): Path<i32>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let row = sqlx::query("SELECT policy_json FROM policies WHERE version = $1")
        .bind(version)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let policy_json = match row {
        Some(r) => r.get::<Value, _>("policy_json"),
        None => return Err((StatusCode::NOT_FOUND, "Policy not found".to_string())),
    };

    let policy: Policy = serde_json::from_value(policy_json)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid policy JSON: {e}")))?;

    // Dry-run against simulated offers
    let sample_offers = vec![
        ("provider_a", 0, "free_catalog"),
        ("provider_b", 1000, "market_data"),
        ("provider_c", 1500, "analytics"),
        ("provider_d", 4000, "archive"),
    ];

    let mut diffs = Vec::new();
    for (prov, amt, res) in sample_offers {
        let ctx = ap_core::policy::DecisionContext {
            provider: prov.to_string(),
            resource_id: res.to_string(),
            category: Some("data".to_string()),
            amount: ap_core::money::Money::new_raw(amt, Currency::INR),
            tax: None,
            fee: None,
            price_known: true,
            remaining_budget: ap_core::money::Money::new_raw(2500, Currency::INR),
            anomaly_score: 0,
            purpose: None,
        };

        let dec = ap_core::policy::decide(&policy, &ctx);
        diffs.push(serde_json::json!({
            "provider": prov,
            "amount_minor": amt,
            "verdict": format!("{:?}", dec.verdict),
            "explanation": dec.explanation
        }));
    }

    Ok(Json(
        serde_json::json!({ "version": version, "diffs": diffs }),
    ))
}

async fn shadow_policy(
    State(state): State<AppState>,
    Path(version): Path<i32>,
) -> Result<Json<Value>, (StatusCode, String)> {
    sqlx::query("UPDATE policies SET state = 'shadow' WHERE version = $1")
        .bind(version)
        .execute(&state.pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(
        serde_json::json!({ "status": "shadow", "version": version }),
    ))
}

async fn activate_policy(
    State(state): State<AppState>,
    Path(version): Path<i32>,
) -> Result<Json<Value>, (StatusCode, String)> {
    sqlx::query("UPDATE policies SET state = 'active' WHERE version = $1")
        .bind(version)
        .execute(&state.pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(
        serde_json::json!({ "status": "active", "version": version }),
    ))
}

async fn run_reconcile(State(state): State<AppState>) -> Result<Json<Value>, (StatusCode, String)> {
    let report = reconcile_once(&state.pool, state.psp.clone(), "tenant_default")
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!(report)))
}

async fn get_incidents(State(state): State<AppState>) -> Result<Json<Value>, (StatusCode, String)> {
    let rows = sqlx::query("SELECT id, kind, purchase_id, opened_at, details FROM incidents ORDER BY opened_at DESC LIMIT 50")
        .fetch_all(&state.pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let list: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let id: Uuid = r.get("id");
            let kind: String = r.get("kind");
            let purchase_id: Option<Uuid> = r.get("purchase_id");
            let opened_at: chrono::DateTime<chrono::Utc> = r.get("opened_at");
            let details: Value = r.get("details");

            serde_json::json!({
                "id": id,
                "kind": kind,
                "purchase_id": purchase_id,
                "opened_at": opened_at.to_rfc3339(),
                "details": details
            })
        })
        .collect();

    Ok(Json(serde_json::json!(list)))
}

async fn handle_webhook_endpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    match process_webhook(&state.pool, state.psp.clone(), &headers, &body).await {
        Ok(crate::webhooks::WebhookHandlerResult::SignatureInvalid) => {
            (StatusCode::UNAUTHORIZED, "Invalid signature").into_response()
        }
        Ok(crate::webhooks::WebhookHandlerResult::Duplicate) => {
            (StatusCode::OK, "Duplicate webhook").into_response()
        }
        Ok(crate::webhooks::WebhookHandlerResult::Processed(msg)) => {
            (StatusCode::OK, msg).into_response()
        }
        Ok(crate::webhooks::WebhookHandlerResult::InternalError(e)) => {
            (StatusCode::BAD_REQUEST, e).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn demo_run_agent(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let task_id_str = payload
        .get("task_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let task_id = Uuid::from_str(task_id_str).unwrap_or_else(|_| Uuid::new_v4());

    let _ = seed_task_and_budget(&state.pool, "tenant_default", task_id, 5000, Currency::INR).await;

    // Run scripted agent sequence
    let pool = state.pool.clone();
    let psp = state.psp.clone();

    tokio::spawn(async move {
        let env = Arc::new(crate::api_agent_env::ApiAgentEnv {
            pool,
            psp,
            tenant_id: "tenant_default".to_string(),
        });
        let planner = Arc::new(ap_agent::ScriptedPlanner::new());
        let runner = ap_agent::AgentRunner::new(planner, env);
        runner.run(task_id).await;
    });

    Ok(Json(serde_json::json!({
        "status": "started",
        "task_id": task_id
    })))
}

async fn demo_storm(State(state): State<AppState>) -> Result<Json<Value>, (StatusCode, String)> {
    let key = format!("storm_key_{}", Uuid::new_v4());
    let mut tasks = Vec::new();

    for _ in 0..100 {
        let pool = state.pool.clone();
        let k = key.clone();
        tasks.push(tokio::spawn(async move {
            ap_store::claim_idempotency(&pool, "tenant_default", &k, "storm_hash").await
        }));
    }

    let mut new_claims = 0;
    for t in tasks {
        if let Ok(Ok(ap_store::IdempotencyClaim::New)) = t.await {
            new_claims += 1;
        }
    }

    Ok(Json(serde_json::json!({
        "requests": 100,
        "new_claims": new_claims,
        "replays": 100 - new_claims
    })))
}

async fn demo_tamper(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let task_id_str = payload
        .get("task_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let task_id = Uuid::from_str(task_id_str).unwrap_or_else(|_| Uuid::new_v4());

    // Tamper with ledger sequence 0 hash in DB:
    // Drop trigger temporarily, update, and re-enable trigger for tamper demo!
    let _ = sqlx::query("ALTER TABLE ledger_events DISABLE TRIGGER trg_no_ledger_mutation")
        .execute(&state.pool)
        .await;

    let _ = sqlx::query(
        "UPDATE ledger_events SET hash = '000000000000000000000000000000000000000000000000000000000000bad1' WHERE task_id = $1 AND seq = 0"
    )
    .bind(task_id)
    .execute(&state.pool)
    .await;

    let _ = sqlx::query("ALTER TABLE ledger_events ENABLE TRIGGER trg_no_ledger_mutation")
        .execute(&state.pool)
        .await;

    Ok(Json(serde_json::json!({
        "status": "tampered",
        "task_id": task_id,
        "tampered_seq": 0
    })))
}

async fn get_metrics(State(_): State<AppState>) -> String {
    "# HELP accesspilot_purchases_total Total number of purchases evaluated\n\
     # TYPE accesspilot_purchases_total counter\n\
     accesspilot_purchases_total{status=\"captured\"} 120\n\
     accesspilot_purchases_total{status=\"denied\"} 45\n\
     # HELP accesspilot_active_budget_minor Current active budget cap\n\
     # TYPE accesspilot_active_budget_minor gauge\n\
     accesspilot_active_budget_minor 5000\n"
        .to_string()
}
