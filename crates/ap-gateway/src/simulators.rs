//! Six provider simulators implementing Section 11.3 and signed offers.

use ap_core::money::Currency;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sqlx::PgPool;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

pub const OFFER_SIGNING_KEY: &str = "ap_provider_offer_hmac_secret_key_8888";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedOffer {
    pub id: Uuid,
    pub task_id: Uuid,
    pub provider_id: String,
    pub resource_id: String,
    pub amount_minor: i64,
    pub currency: Currency,
    pub tax_minor: i64,
    pub fee_minor: i64,
    pub price_known: bool,
    pub expires_at: DateTime<Utc>,
    pub sig: String,
}

/// Compute HMAC-SHA256 signature for an offer.
pub fn sign_offer_data(
    id: Uuid,
    provider_id: &str,
    resource_id: &str,
    amount_minor: i64,
    currency: &str,
    expires_at: i64,
) -> String {
    let mut mac = HmacSha256::new_from_slice(OFFER_SIGNING_KEY.as_bytes())
        .expect("HMAC can accept any key length");
    let payload =
        format!("{id}:{provider_id}:{resource_id}:{amount_minor}:{currency}:{expires_at}");
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Verify HMAC-SHA256 signature for an offer.
pub fn verify_offer_signature(offer: &SignedOffer) -> bool {
    let expected = sign_offer_data(
        offer.id,
        &offer.provider_id,
        &offer.resource_id,
        offer.amount_minor,
        &offer.currency.to_string(),
        offer.expires_at.timestamp(),
    );
    offer.sig == expected
}

/// Mint, sign, and store an offer for a given provider and task.
pub async fn mint_provider_offer(
    pool: &PgPool,
    task_id: Uuid,
    provider_id: &str,
    resource_id: &str,
) -> Result<SignedOffer, sqlx::Error> {
    let amount_minor = match provider_id {
        "provider_a" => 0,    // Free
        "provider_b" => 1000, // ₹10.00
        "provider_c" => 1500, // ₹15.00
        "provider_d" => 4000, // ₹40.00 (triggers over-budget with ₹50 cap if total exceeds)
        "provider_e" => 2000, // ₹20.00 (flaky)
        "provider_f" => 1200, // ₹12.00 (price drift demo)
        _ => 1000,
    };

    let offer_id = Uuid::new_v4();
    let currency = Currency::INR;
    let currency_str = currency.to_string();
    let expires_at = Utc::now() + chrono::Duration::hours(1);
    let sig = sign_offer_data(
        offer_id,
        provider_id,
        resource_id,
        amount_minor,
        &currency_str,
        expires_at.timestamp(),
    );

    sqlx::query(
        r#"
        INSERT INTO offers (id, task_id, provider_id, resource_id, amount_minor, currency, tax_minor, fee_minor, price_known, expires_at, sig)
        VALUES ($1, $2, $3, $4, $5, $6, 0, 0, true, $7, $8)
        "#,
    )
    .bind(offer_id)
    .bind(task_id)
    .bind(provider_id)
    .bind(resource_id)
    .bind(amount_minor)
    .bind(&currency_str)
    .bind(expires_at)
    .bind(&sig)
    .execute(pool)
    .await?;

    Ok(SignedOffer {
        id: offer_id,
        task_id,
        provider_id: provider_id.to_string(),
        resource_id: resource_id.to_string(),
        amount_minor,
        currency,
        tax_minor: 0,
        fee_minor: 0,
        price_known: true,
        expires_at,
        sig,
    })
}
