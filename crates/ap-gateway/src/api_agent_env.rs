//! Adapter connecting ap-agent runner with gateway purchase and resource functions.

use ap_agent::runner::{AgentEnvironment, PurchaseResponse, ResourceResponse};
use ap_psp::simulated::SimulatedPsp;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

pub struct ApiAgentEnv {
    pub pool: PgPool,
    pub psp: Arc<SimulatedPsp>,
    pub tenant_id: String,
}

#[async_trait]
impl AgentEnvironment for ApiAgentEnv {
    async fn fetch_resource(
        &self,
        task_id: Uuid,
        provider: &str,
        resource: &str,
        grant_token: Option<&str>,
    ) -> Result<ResourceResponse, String> {
        if provider == "provider_a" {
            return Ok(ResourceResponse::Success {
                content: "Provider A Free Metadata".to_string(),
            });
        }

        if let Some(token) = grant_token {
            let mut hasher = Sha256::new();
            hasher.update(token.as_bytes());
            let token_hash = hasher.finalize().to_vec();

            let row =
                sqlx::query("SELECT id FROM grants WHERE token_hash = $1 AND revoked_at IS NULL")
                    .bind(&token_hash)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|e| e.to_string())?;

            if row.is_some() {
                return Ok(ResourceResponse::Success {
                    content: format!(
                        "Unlocked premium data from {provider} for resource {resource}"
                    ),
                });
            }
        }

        let offer = crate::simulators::mint_provider_offer(&self.pool, task_id, provider, resource)
            .await
            .map_err(|e| e.to_string())?;

        Ok(ResourceResponse::PaymentRequired {
            offer_id: offer.id,
            amount_minor: offer.amount_minor,
        })
    }

    async fn buy_offer(
        &self,
        task_id: Uuid,
        offer_id: Uuid,
        idempotency_key: &str,
    ) -> Result<PurchaseResponse, String> {
        let res = crate::service::execute_purchase(
            &self.pool,
            self.psp.clone(),
            &self.tenant_id,
            task_id,
            offer_id,
            idempotency_key,
            "demo_hash",
        )
        .await;

        match res {
            Ok(p_res) => Ok(PurchaseResponse {
                status: p_res.status,
                grant_token: p_res.grant.map(|g| g.token),
                message: p_res.message,
            }),
            Err(e) => Ok(PurchaseResponse {
                status: "denied".to_string(),
                grant_token: None,
                message: e.to_string(),
            }),
        }
    }
}
