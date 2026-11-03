//! ap-harness: Test and benchmark suite for AccessPilot.

pub mod system_info;
pub mod t10_hyperswitch;
pub mod t1_concurrency;
pub mod t2_idempotency;
pub mod t3_fail_closed;
pub mod t4_crash_recovery;
pub mod t5_model_properties;
pub mod t6_latency;
pub mod t7_reconciliation;
pub mod t8_anomaly;
pub mod t9_tamper;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
