//! Resource gateway serving content only for valid grants or returning 402 with signed offer.

use crate::simulators::mint_provider_offer;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct ResourceQuery {
    pub task_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct ResourceSuccess {
    pub status: String,
    pub provider: String,
    pub resource: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct Resource402 {
    pub error: String,
    pub offer: crate::simulators::SignedOffer,
}

/// Handler for GET /v1/resources/:provider/:resource
pub async fn handle_get_resource(
    State(state): State<crate::api::AppState>,
    Path((provider, resource)): Path<(String, String)>,
    Query(query): Query<ResourceQuery>,
    headers: HeaderMap,
) -> Response {
    let pool = &state.pool;
    // 1. Free provider A: always 200 without grant
    if provider == "provider_a" {
        return (
            StatusCode::OK,
            Json(ResourceSuccess {
                status: "ok".to_string(),
                provider,
                resource,
                content: "Provider A Free Metadata: verified free access with 0 charge".to_string(),
            }),
        )
            .into_response();
    }

    // 2. Paid provider: inspect grant token if provided
    let grant_token = headers
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-grant-token").and_then(|h| h.to_str().ok()));

    if let Some(token) = grant_token {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let token_hash = hasher.finalize().to_vec();

        let grant_row = sqlx::query(
            r#"
            SELECT id, scope, expires_at, revoked_at
            FROM grants
            WHERE token_hash = $1
            "#,
        )
        .bind(&token_hash)
        .fetch_optional(pool)
        .await;

        if let Ok(Some(row)) = grant_row {
            let expires_at: chrono::DateTime<Utc> = row.get("expires_at");
            let revoked_at: Option<chrono::DateTime<Utc>> = row.get("revoked_at");

            if revoked_at.is_none() && expires_at > Utc::now() {
                return (
                    StatusCode::OK,
                    Json(ResourceSuccess {
                        status: "ok".to_string(),
                        provider: provider.clone(),
                        resource: resource.clone(),
                        content: format!(
                            "Unlocked premium data from {provider} for resource {resource}"
                        ),
                    }),
                )
                    .into_response();
            }
        }
    }

    // 3. Not granted: mint signed offer and return 402 Payment Required
    let task_id = query.task_id.unwrap_or_else(Uuid::new_v4);
    match mint_provider_offer(pool, task_id, &provider, &resource).await {
        Ok(offer) => (
            StatusCode::PAYMENT_REQUIRED,
            Json(Resource402 {
                error: "Payment Required".to_string(),
                offer,
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to mint offer: {e}") })),
        )
            .into_response(),
    }
}
