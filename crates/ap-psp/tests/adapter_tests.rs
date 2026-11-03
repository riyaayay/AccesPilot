//! Wiremock and unit tests for PaymentProvider adapters.

use ap_core::money::Currency;
use ap_psp::hyperswitch::HyperswitchPsp;
use ap_psp::simulated::SimulatedPsp;
use ap_psp::traits::{
    ChargeRequest, FailureClass, PaymentProvider, ProviderError, ProviderStatus, RefundRequest,
    WebhookError,
};
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderValue};
use sha2::Sha512;
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

type HmacSha512 = Hmac<Sha512>;

#[tokio::test]
async fn test_hyperswitch_charge_success() {
    let mock_server = MockServer::start().await;

    let response_body = serde_json::json!({
        "payment_id": "pay_test_123",
        "amount": 2500,
        "currency": "INR",
        "status": "succeeded"
    });

    Mock::given(method("POST"))
        .and(path("/payments"))
        .and(header("api-key", "snd_test_key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&mock_server)
        .await;

    let psp = HyperswitchPsp::new(mock_server.uri(), "snd_test_key", None);

    let req = ChargeRequest {
        payment_id: "pay_test_123".to_string(),
        amount_minor: 2500,
        currency: Currency::INR,
        customer_id: "cust_1".to_string(),
        task_id: Uuid::new_v4(),
        purchase_id: Uuid::new_v4(),
        metadata: None,
    };

    let payment = psp.charge(req).await.expect("Charge should succeed");
    assert_eq!(payment.provider_payment_id, "pay_test_123");
    assert_eq!(payment.amount_minor, 2500);
    assert_eq!(payment.currency, Currency::INR);
    assert_eq!(payment.status, ProviderStatus::Succeeded);
}

#[tokio::test]
async fn test_hyperswitch_charge_declined() {
    let mock_server = MockServer::start().await;

    let response_body = serde_json::json!({
        "payment_id": "pay_declined_123",
        "amount": 1000,
        "currency": "INR",
        "status": "failed",
        "error_code": "card_declined",
        "error_message": "The card was declined"
    });

    Mock::given(method("POST"))
        .and(path("/payments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&mock_server)
        .await;

    let psp = HyperswitchPsp::new(mock_server.uri(), "snd_test_key", None);

    let req = ChargeRequest {
        payment_id: "pay_declined_123".to_string(),
        amount_minor: 1000,
        currency: Currency::INR,
        customer_id: "cust_1".to_string(),
        task_id: Uuid::new_v4(),
        purchase_id: Uuid::new_v4(),
        metadata: None,
    };

    let payment = psp
        .charge(req)
        .await
        .expect("Charge completed with failure status");
    assert_eq!(
        payment.status,
        ProviderStatus::Failed(FailureClass::Declined)
    );
    assert_eq!(
        payment.error_message,
        Some("The card was declined".to_string())
    );
}

#[tokio::test]
async fn test_hyperswitch_charge_insufficient_funds() {
    let mock_server = MockServer::start().await;

    let response_body = serde_json::json!({
        "payment_id": "pay_funds_123",
        "amount": 1000,
        "currency": "INR",
        "status": "failed",
        "error_code": "insufficient_funds",
        "error_message": "Balance too low"
    });

    Mock::given(method("POST"))
        .and(path("/payments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&mock_server)
        .await;

    let psp = HyperswitchPsp::new(mock_server.uri(), "snd_test_key", None);

    let req = ChargeRequest {
        payment_id: "pay_funds_123".to_string(),
        amount_minor: 1000,
        currency: Currency::INR,
        customer_id: "cust_1".to_string(),
        task_id: Uuid::new_v4(),
        purchase_id: Uuid::new_v4(),
        metadata: None,
    };

    let payment = psp.charge(req).await.expect("Charge response parsed");
    assert_eq!(
        payment.status,
        ProviderStatus::Failed(FailureClass::InsufficientFunds)
    );
}

#[tokio::test]
async fn test_hyperswitch_charge_500_provider_down() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/payments"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Switch Error"))
        .mount(&mock_server)
        .await;

    let psp = HyperswitchPsp::new(mock_server.uri(), "snd_test_key", None);

    let req = ChargeRequest {
        payment_id: "pay_500_123".to_string(),
        amount_minor: 1000,
        currency: Currency::INR,
        customer_id: "cust_1".to_string(),
        task_id: Uuid::new_v4(),
        purchase_id: Uuid::new_v4(),
        metadata: None,
    };

    let err = psp.charge(req).await.expect_err("Must fail on 500");
    match err {
        ProviderError::ProviderDown(_) => {}
        other => panic!("Expected ProviderDown, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_hyperswitch_retrieve_success() {
    let mock_server = MockServer::start().await;

    let response_body = serde_json::json!({
        "payment_id": "pay_ret_123",
        "amount": 3000,
        "currency": "INR",
        "status": "succeeded"
    });

    Mock::given(method("GET"))
        .and(path("/payments/pay_ret_123"))
        .and(header("api-key", "snd_test_key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&mock_server)
        .await;

    let psp = HyperswitchPsp::new(mock_server.uri(), "snd_test_key", None);
    let payment = psp
        .retrieve("pay_ret_123")
        .await
        .expect("Retrieve succeeds")
        .expect("Payment found");

    assert_eq!(payment.provider_payment_id, "pay_ret_123");
    assert_eq!(payment.status, ProviderStatus::Succeeded);
    assert_eq!(payment.amount_minor, 3000);
}

#[tokio::test]
async fn test_hyperswitch_retrieve_404_not_found() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/payments/pay_nonexistent"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock_server)
        .await;

    let psp = HyperswitchPsp::new(mock_server.uri(), "snd_test_key", None);
    let payment = psp
        .retrieve("pay_nonexistent")
        .await
        .expect("Retrieve executes without network error");

    assert!(payment.is_none());
}

#[tokio::test]
async fn test_hyperswitch_refund_success() {
    let mock_server = MockServer::start().await;

    let response_body = serde_json::json!({
        "refund_id": "ref_123",
        "payment_id": "pay_ref_123",
        "amount": 1500,
        "status": "succeeded"
    });

    Mock::given(method("POST"))
        .and(path("/refunds"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .mount(&mock_server)
        .await;

    let psp = HyperswitchPsp::new(mock_server.uri(), "snd_test_key", None);
    let refund = psp
        .refund(RefundRequest {
            refund_id: "ref_123".to_string(),
            provider_payment_id: "pay_ref_123".to_string(),
            amount_minor: 1500,
            currency: Currency::INR,
            reason: Some("Customer requested refund".to_string()),
        })
        .await
        .expect("Refund succeeds");

    assert_eq!(refund.refund_id, "ref_123");
    assert_eq!(refund.status, "succeeded");
}

#[tokio::test]
async fn test_hyperswitch_webhook_signature_verification() {
    let secret = "test_webhook_secret_key_98765";
    let psp = HyperswitchPsp::new(
        "https://sandbox.hyperswitch.io",
        "snd_key",
        Some(secret.to_string()),
    );

    let body = serde_json::json!({
        "event_id": "evt_101",
        "event_type": "payment_succeeded",
        "content": {
            "payment_id": "pay_hs_webhook_1"
        }
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();

    // 1. Valid HMAC-SHA512 signature
    let mut mac = HmacSha512::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(&body_bytes);
    let valid_sig = hex::encode(mac.finalize().into_bytes());

    let mut headers = HeaderMap::new();
    headers.insert(
        "x-webhook-signature-512",
        HeaderValue::from_str(&valid_sig).unwrap(),
    );

    let trigger = psp
        .verify_webhook(&headers, &body_bytes)
        .expect("Valid signature should pass");
    assert_eq!(trigger.provider_payment_id, "pay_hs_webhook_1");
    assert_eq!(trigger.event_id, "evt_101");
    assert_eq!(trigger.event_type, "payment_succeeded");

    // 2. Forged signature
    let mut bad_headers = HeaderMap::new();
    bad_headers.insert(
        "x-webhook-signature-512",
        HeaderValue::from_static("deadbeef0123456789abcdef"),
    );

    let err = psp
        .verify_webhook(&bad_headers, &body_bytes)
        .expect_err("Forged sig must fail");
    match err {
        WebhookError::InvalidSignature => {}
        other => panic!("Expected InvalidSignature, got {other:?}"),
    }

    // 3. Missing signature
    let empty_headers = HeaderMap::new();
    let err = psp
        .verify_webhook(&empty_headers, &body_bytes)
        .expect_err("Missing sig must fail");
    match err {
        WebhookError::MissingSignature => {}
        other => panic!("Expected MissingSignature, got {other:?}"),
    }
}

#[test]
fn test_status_mapping_table_covers_all_docs_strings() {
    // Exact status strings listed in Hyperswitch API documentation:
    // https://api-reference.hyperswitch.io/introduction
    let test_cases = vec![
        ("succeeded", None, ProviderStatus::Succeeded),
        (
            "failed",
            None,
            ProviderStatus::Failed(FailureClass::Declined),
        ),
        (
            "failed",
            Some("insufficient_funds"),
            ProviderStatus::Failed(FailureClass::InsufficientFunds),
        ),
        (
            "failed",
            Some("timeout"),
            ProviderStatus::Failed(FailureClass::Timeout),
        ),
        (
            "failed",
            Some("internal_server_error"),
            ProviderStatus::Failed(FailureClass::ProviderDown),
        ),
        (
            "failed",
            Some("invalid_request"),
            ProviderStatus::Failed(FailureClass::InvalidRequest),
        ),
        (
            "cancelled",
            None,
            ProviderStatus::Failed(FailureClass::Declined),
        ),
        ("processing", None, ProviderStatus::Processing),
        (
            "requires_payment_method",
            None,
            ProviderStatus::ActionRequired,
        ),
        (
            "requires_confirmation",
            None,
            ProviderStatus::ActionRequired,
        ),
        (
            "requires_customer_action",
            None,
            ProviderStatus::ActionRequired,
        ),
        (
            "requires_merchant_action",
            None,
            ProviderStatus::ActionRequired,
        ),
        ("requires_capture", None, ProviderStatus::Processing),
        ("partially_captured", None, ProviderStatus::Processing),
        // Unknown strings fail closed to Unknown
        ("unknown_future_status", None, ProviderStatus::Unknown),
        ("pending_switch_verification", None, ProviderStatus::Unknown),
        ("", None, ProviderStatus::Unknown),
        ("random_garbage_string", None, ProviderStatus::Unknown),
    ];

    for (status_str, error_code, expected) in test_cases {
        let result = HyperswitchPsp::map_status(status_str, error_code);
        assert_eq!(
            result, expected,
            "Mapping failed for status: {status_str}, error_code: {error_code:?}"
        );
    }
}

#[test]
fn test_failure_class_safe_to_retry() {
    // Pure function contract:
    // Timeout and ProviderDown are transient => safe to retry
    assert!(FailureClass::Timeout.is_safe_to_retry());
    assert!(FailureClass::ProviderDown.is_safe_to_retry());

    // Declined, InsufficientFunds, InvalidRequest are permanent => NEVER retry
    assert!(!FailureClass::Declined.is_safe_to_retry());
    assert!(!FailureClass::InsufficientFunds.is_safe_to_retry());
    assert!(!FailureClass::InvalidRequest.is_safe_to_retry());
}

#[tokio::test]
async fn test_simulated_psp_deterministic_ids_and_counters() {
    let sim = SimulatedPsp::new();

    // Deterministic ID is repeatable
    let id1 = SimulatedPsp::deterministic_payment_id("tenant_a", "key_1");
    let id2 = SimulatedPsp::deterministic_payment_id("tenant_a", "key_1");
    let id3 = SimulatedPsp::deterministic_payment_id("tenant_b", "key_1");
    assert_eq!(id1, id2);
    assert_ne!(id1, id3);

    // Charge counter increments
    assert_eq!(sim.charge_counter(), 0);
    let req = ChargeRequest {
        payment_id: id1.clone(),
        amount_minor: 1000,
        currency: Currency::INR,
        customer_id: "cust_test".to_string(),
        task_id: Uuid::new_v4(),
        purchase_id: Uuid::new_v4(),
        metadata: None,
    };

    let payment = sim.charge(req).await.expect("Simulated charge succeeds");
    assert_eq!(sim.charge_counter(), 1);
    assert_eq!(payment.status, ProviderStatus::Succeeded);

    // Retrieve finds the payment
    let retrieved = sim
        .retrieve(&id1)
        .await
        .expect("Retrieve succeeds")
        .expect("Found");
    assert_eq!(retrieved.provider_payment_id, id1);
    assert_eq!(sim.retrieve_counter(), 1);

    // Fault injection: force timeout
    sim.set_force_timeout(true);
    let timeout_err = sim
        .retrieve(&id1)
        .await
        .expect_err("Should error on forced timeout");
    match timeout_err {
        ProviderError::Timeout(_) => {}
        other => panic!("Expected Timeout, got {other:?}"),
    }
    sim.set_force_timeout(false);

    // Fault injection: force 5xx
    sim.set_force_5xx(true);
    let down_err = sim
        .retrieve(&id1)
        .await
        .expect_err("Should error on forced 5xx");
    match down_err {
        ProviderError::ProviderDown(_) => {}
        other => panic!("Expected ProviderDown, got {other:?}"),
    }
}
