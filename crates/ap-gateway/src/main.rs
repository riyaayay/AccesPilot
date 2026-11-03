//! AccessPilot Gateway service executable.

use ap_gateway::api::{create_router, AppState};
use ap_gateway::reconciler::reconcile_once;
use ap_psp::simulated::SimulatedPsp;
use sqlx::postgres::PgPoolOptions;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres@127.0.0.1:9432/accesspilot".to_string());
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let addr: SocketAddr = format!("0.0.0.0:{port}").parse()?;

    tracing::info!("Connecting to database at {db_url}...");
    let pool = PgPoolOptions::new()
        .max_connections(20)
        .connect(&db_url)
        .await?;

    let psp = Arc::new(SimulatedPsp::new());

    // Background reconciler loop (every 5 seconds in demo mode, else 30 seconds)
    let rec_pool = pool.clone();
    let rec_psp = psp.clone();
    let interval_secs = if std::env::var("DEMO_MODE").is_ok() {
        5
    } else {
        30
    };

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            if let Ok(report) = reconcile_once(&rec_pool, rec_psp.clone(), "tenant_default").await {
                if report.stuck_scanned > 0 || report.drifts_detected > 0 {
                    tracing::info!(
                        "Reconciler tick: scanned={}, healed_captured={}, healed_released={}, drifts={}",
                        report.stuck_scanned,
                        report.healed_captured,
                        report.healed_released,
                        report.drifts_detected
                    );
                }
            }
        }
    });

    let state = AppState { pool, psp };
    let app = create_router(state);

    tracing::info!(
        "AccessPilot Payments Control Plane listening on http://{}",
        addr
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
