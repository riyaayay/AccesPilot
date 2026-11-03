//! AccessPilot benchmark harness runner.
//! Produces the exact measurement table specified in Section 13.2 of the Architecture Blueprint.

use ap_harness::system_info::SystemInfo;
use ap_harness::t10_hyperswitch::run_t10;
use ap_harness::t1_concurrency::run_t1;
use ap_harness::t2_idempotency::run_t2;
use ap_harness::t3_fail_closed::run_t3;
use ap_harness::t4_crash_recovery::run_t4;
use ap_harness::t5_model_properties::run_t5;
use ap_harness::t6_latency::run_t6;
use ap_harness::t7_reconciliation::run_t7;
use ap_harness::t8_anomaly::run_t8;
use ap_harness::t9_tamper::run_t9;
use sqlx::postgres::PgPoolOptions;

fn get_database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres@127.0.0.1:9432/accesspilot".to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let seed = 42;
    let sys_info = SystemInfo::collect(seed);

    let db_url = get_database_url();
    let pool = PgPoolOptions::new()
        .max_connections(50)
        .connect(&db_url)
        .await
        .map_err(|e| format!("Failed to connect to PostgreSQL at {db_url}: {e}"))?;

    // Header line
    println!(
        "AccessPilot harness  |  {}  |  {}  |  {}  |  {}  |  seed {}",
        sys_info.date,
        sys_info.hardware,
        sys_info.postgres_info,
        sys_info.rustc_version,
        sys_info.seed
    );

    // T1: Cap under concurrency
    let t1 = run_t1(&pool).await?;
    let t1_status = if t1.pass { "PASS" } else { "FAIL" };
    println!(
        "T1  cap_under_concurrency   attempts={:<4} accepted={:<3} denied={:<4} captured={:<5} cap={:<5} violations={:<1}   {}",
        t1.attempts, t1.accepted, t1.denied, t1.captured_minor, t1.cap_minor, t1.violations, t1_status
    );

    // T2: Idempotency storm
    let t2 = run_t2(&pool).await?;
    let t2_status = if t2.pass { "PASS" } else { "FAIL" };
    println!(
        "T2  idempotency_storm       requests={:<4} provider_charges={:<1} replays={:<3}                                     {}",
        t2.requests, t2.provider_charges, t2.replays, t2_status
    );

    // T3: Fail closed matrix
    let t3 = run_t3(&pool).await?;
    let t3_status = if t3.pass { "PASS" } else { "FAIL" };
    println!(
        "T3  fail_closed_matrix      passed={}/{:<2}                                                                     {}",
        t3.passed, t3.total, t3_status
    );

    // T4: Crash recovery
    let t4 = run_t4(&pool).await?;
    let t4_status = if t4.pass { "PASS" } else { "FAIL" };
    println!(
        "T4  crash_recovery          failpoints={:<1} runs={:<3} orphans={:<1} invariant_failures={:<1}                         {}",
        t4.failpoints, t4.runs, t4.orphans, t4.invariant_failures, t4_status
    );

    // T5: Model and properties
    let t5 = run_t5(seed);
    let t5_status = if t5.pass { "PASS" } else { "FAIL" };
    println!(
        "T5  model_and_properties    cases={:<6} disagreements={:<1}                                                       {}",
        t5.cases, t5.disagreements, t5_status
    );

    // T6: Latency
    let t6 = run_t6(&pool).await?;
    println!(
        "T6  latency                 decide_p99={}us req_p50={:.2}ms req_p99={:.2}ms rps_spread={} rps_single_task={}       INFO",
        t6.decide_p99_us, t6.req_p50_ms, t6.req_p99_ms, t6.rps_spread, t6.rps_single_task
    );

    // T7: Reconciliation
    let t7 = run_t7(&pool).await?;
    let t7_status = if t7.pass { "PASS" } else { "FAIL" };
    println!(
        "T7  reconciliation          drifts={:<2} detected={:<2} healed={:<2} incidents={:<2}                                   {}",
        t7.drifts, t7.detected, t7.healed, t7.incidents, t7_status
    );

    // T8: Anomaly detection
    let t8 = run_t8(seed);
    let t8_status = if t8.pass { "PASS" } else { "FAIL" };
    println!(
        "T8  anomaly_detection       samples={:<4} precision={:.2} recall={:.2}                                              {}",
        t8.samples, t8.precision, t8.recall, t8_status
    );

    // T9: Ledger tamper
    let t9 = run_t9(&pool).await?;
    let t9_status = if t9.pass { "PASS" } else { "FAIL" };
    println!(
        "T9  ledger_tamper           mutations={:<3} detected={:<3}                                                        {}",
        t9.mutations, t9.detected, t9_status
    );

    // T10: Hyperswitch sandbox
    let t10 = run_t10().await;
    println!(
        "T10 hyperswitch_sandbox     payments={:<2} matched={:<2} p50={}ms p95={}ms                                         INFO",
        t10.payments, t10.matched, t10.p50_ms, t10.p95_ms
    );

    Ok(())
}
