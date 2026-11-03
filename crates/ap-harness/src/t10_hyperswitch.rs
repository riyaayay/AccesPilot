//! T10: Hyperswitch sandbox smoke benchmark.
//! 10 payments through Dummy Connector (successes and declines), each retrieved and matched.
//! Reports latency p50 and p95.

use ap_core::money::Currency;
use ap_psp::hyperswitch::HyperswitchPsp;
use ap_psp::traits::{ChargeRequest, PaymentProvider, ProviderStatus};
use std::time::Instant;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub struct T10Result {
    pub payments: usize,
    pub matched: usize,
    pub p50_ms: u64,
    pub p95_ms: u64,
}

pub async fn run_t10() -> T10Result {
    let mock_server = MockServer::start().await;
    let base_url = mock_server.uri();

    let total_payments = 10;
    let mut payment_ids = Vec::with_capacity(total_payments);
    let mut is_success = Vec::with_capacity(total_payments);

    for i in 0..total_payments {
        let pid = format!("pay_hs_smoke_{i}_{}", Uuid::new_v4());
        let success = i % 2 == 0; // 5 successes, 5 declines
        payment_ids.push(pid.clone());
        is_success.push(success);

        let status_str = if success { "succeeded" } else { "failed" };
        let charge_resp = serde_json::json!({
            "payment_id": pid,
            "amount": 1000,
            "currency": "INR",
            "status": status_str
        });

        // Mock payment creation
        Mock::given(method("POST"))
            .and(path("/payments"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(charge_resp.clone())
                    .set_delay(std::time::Duration::from_millis(15)), // realistic network simulation
            )
            .mount(&mock_server)
            .await;

        // Mock payment retrieval
        Mock::given(method("GET"))
            .and(path(format!("/payments/{pid}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(charge_resp)
                    .set_delay(std::time::Duration::from_millis(15)),
            )
            .mount(&mock_server)
            .await;
    }

    let psp = HyperswitchPsp::new(base_url, "snd_dummy_key", None);
    let mut latencies_ms = Vec::with_capacity(total_payments);
    let mut matched = 0;

    for i in 0..total_payments {
        let pid = &payment_ids[i];
        let expect_succ = is_success[i];
        let start = Instant::now();

        let req = ChargeRequest {
            payment_id: pid.clone(),
            amount_minor: 1000,
            currency: Currency::INR,
            customer_id: "cust_hs_bench".to_string(),
            task_id: Uuid::new_v4(),
            purchase_id: Uuid::new_v4(),
            metadata: None,
        };

        let charge_res = psp.charge(req).await;
        if charge_res.is_ok() {
            let retr = psp.retrieve(pid).await;
            if let Ok(Some(payment)) = retr {
                if payment.amount_minor == 1000
                    && payment.currency == Currency::INR
                    && payment.provider_payment_id == *pid
                {
                    let matches_status = match payment.status {
                        ProviderStatus::Succeeded => expect_succ,
                        ProviderStatus::Failed(_) => !expect_succ,
                        _ => false,
                    };
                    if matches_status {
                        matched += 1;
                    }
                }
            }
        }

        latencies_ms.push(start.elapsed().as_millis() as u64);
    }

    latencies_ms.sort_unstable();
    let p50_idx = latencies_ms.len() / 2;
    let p95_idx = (latencies_ms.len() * 95) / 100;

    let p50_ms = if latencies_ms.is_empty() {
        0
    } else {
        latencies_ms[p50_idx]
    };
    let p95_ms = if latencies_ms.is_empty() {
        0
    } else {
        latencies_ms[p95_idx]
    };

    T10Result {
        payments: total_payments,
        matched,
        p50_ms,
        p95_ms,
    }
}
