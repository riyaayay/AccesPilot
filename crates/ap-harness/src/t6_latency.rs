//! T6: Latency and throughput benchmark.
//! Decision p99 with a 20-rule policy; load run of purchases spread over 500 tasks, then single task.

use ap_core::ids::RuleId;
use ap_core::money::{Currency, Money};
use ap_core::policy::{
    decide, Autonomy, DecisionContext, Expr, Field, Op, Policy, Rule, Value, Verdict,
};
use ap_gateway::service::execute_purchase;
use ap_psp::simulated::SimulatedPsp;
use ap_store::{seed_offer, seed_task_and_budget, StoreError};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

pub struct T6Result {
    pub decide_p99_us: u64,
    pub req_p50_ms: f64,
    pub req_p99_ms: f64,
    pub rps_spread: u64,
    pub rps_single_task: u64,
}

pub async fn run_t6(pool: &PgPool) -> Result<T6Result, StoreError> {
    // ----------------------------------------------------
    // 1. Pure decide() microbenchmark with 20-rule policy
    // ----------------------------------------------------
    let mut rules = Vec::with_capacity(20);
    for i in 1..=20 {
        rules.push(Rule {
            id: RuleId(format!("rule_{i}")),
            then: if i % 2 == 0 {
                Verdict::Allow
            } else {
                Verdict::RequireApproval
            },
            when: Expr::All(vec![
                Expr::Cmp(Field::AmountMinor, Op::Lt, Value::Int(i * 500)),
                Expr::Cmp(
                    Field::Provider,
                    Op::Eq,
                    Value::String(format!("provider_{i}")),
                ),
            ]),
            reason: format!("rule_{i}_condition"),
        });
    }
    let policy = Policy {
        version: 1,
        autonomy: Autonomy::WithinRules,
        task_cap: Money {
            amount_minor: 100_000,
            currency: Currency::INR,
        },
        per_purchase_cap: Money {
            amount_minor: 50_000,
            currency: Currency::INR,
        },
        rules,
    };

    let ctx = DecisionContext {
        provider: "provider_10".to_string(),
        resource_id: "res_bench".to_string(),
        category: Some("benchmark".to_string()),
        amount: Money {
            amount_minor: 2500,
            currency: Currency::INR,
        },
        tax: None,
        fee: None,
        price_known: true,
        remaining_budget: Money {
            amount_minor: 50000,
            currency: Currency::INR,
        },
        anomaly_score: 5,
        purpose: Some("bench".to_string()),
    };

    // Warmup
    for _ in 0..1000 {
        let _ = decide(&policy, &ctx);
    }

    let iterations = 50_000;
    let mut decide_durations_ns = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let _ = decide(&policy, &ctx);
        decide_durations_ns.push(start.elapsed().as_nanos() as u64);
    }
    decide_durations_ns.sort_unstable();
    let p99_idx = (iterations * 99) / 100;
    let decide_p99_us = (decide_durations_ns[p99_idx] / 1000).max(1);

    // ----------------------------------------------------
    // 2. Spread load benchmark: 5,000 attempts over 500 tasks
    // ----------------------------------------------------
    let tenant_id = "tenant_t6_bench";
    let num_tasks = 500;
    let attempts_per_task = 10;
    let total_spread_attempts = num_tasks * attempts_per_task;

    let mut task_ids = Vec::with_capacity(num_tasks);
    let mut offer_ids = Vec::with_capacity(num_tasks);

    for _ in 0..num_tasks {
        let tid = Uuid::new_v4();
        let oid = Uuid::new_v4();
        seed_task_and_budget(pool, tenant_id, tid, 1_000_000, Currency::INR).await?;
        seed_offer(
            pool,
            oid,
            tid,
            "provider_fast",
            "res_fast",
            100,
            Currency::INR,
        )
        .await?;
        task_ids.push(tid);
        offer_ids.push(oid);
    }

    let psp = Arc::new(SimulatedPsp::new()); // 0ms delay

    let spread_start = Instant::now();
    let sem = Arc::new(tokio::sync::Semaphore::new(30));
    let mut handles = Vec::with_capacity(total_spread_attempts);

    for i in 0..total_spread_attempts {
        let pool = pool.clone();
        let psp = psp.clone();
        let sem = sem.clone();
        let task_idx = i % num_tasks;
        let tid = task_ids[task_idx];
        let oid = offer_ids[task_idx];
        let key = format!("key_t6_spread_{i}");

        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let req_start = Instant::now();
            let _ =
                execute_purchase(&pool, psp, tenant_id, tid, oid, &key, "req_hash_spread").await;
            req_start.elapsed().as_secs_f64() * 1000.0 // ms
        }));
    }

    let mut latencies_ms = Vec::with_capacity(total_spread_attempts);
    for h in handles {
        if let Ok(dur_ms) = h.await {
            latencies_ms.push(dur_ms);
        }
    }
    let spread_total_duration = spread_start.elapsed();

    latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50_idx = latencies_ms.len() / 2;
    let p99_idx = (latencies_ms.len() * 99) / 100;

    let req_p50_ms = if latencies_ms.is_empty() {
        0.0
    } else {
        latencies_ms[p50_idx]
    };
    let req_p99_ms = if latencies_ms.is_empty() {
        0.0
    } else {
        latencies_ms[p99_idx]
    };
    let rps_spread = ((latencies_ms.len() as f64) / spread_total_duration.as_secs_f64()) as u64;

    // ----------------------------------------------------
    // 3. Single-task load benchmark (lock contention demo)
    // ----------------------------------------------------
    let single_task_id = Uuid::new_v4();
    let single_offer_id = Uuid::new_v4();
    seed_task_and_budget(pool, tenant_id, single_task_id, 1_000_000, Currency::INR).await?;
    seed_offer(
        pool,
        single_offer_id,
        single_task_id,
        "provider_fast",
        "res_fast",
        100,
        Currency::INR,
    )
    .await?;

    let single_attempts = 500;
    let single_start = Instant::now();
    let mut single_handles = Vec::with_capacity(single_attempts);

    for i in 0..single_attempts {
        let pool = pool.clone();
        let psp = psp.clone();
        let key = format!("key_t6_single_{i}");

        single_handles.push(tokio::spawn(async move {
            let _ = execute_purchase(
                &pool,
                psp,
                tenant_id,
                single_task_id,
                single_offer_id,
                &key,
                "req_hash_single",
            )
            .await;
        }));
    }

    for h in single_handles {
        let _ = h.await;
    }
    let single_duration = single_start.elapsed();
    let rps_single_task = ((single_attempts as f64) / single_duration.as_secs_f64()) as u64;

    Ok(T6Result {
        decide_p99_us,
        req_p50_ms,
        req_p99_ms,
        rps_spread,
        rps_single_task,
    })
}
