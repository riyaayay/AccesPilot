//! Acceptance tests for P5: Agent end-to-end loop with ScriptedPlanner and LLM fallbacks.

use ap_agent::planner::{GeminiPlanner, GroqPlanner, ScriptedPlanner};
use ap_agent::runner::{AgentEnvironment, AgentRunner, PurchaseResponse, ResourceResponse};
use ap_agent::schema::AgentAction;
use ap_agent::Planner;
use ap_core::money::Currency;
use ap_gateway::service::execute_purchase;
use ap_gateway::simulators::mint_provider_offer;
use ap_psp::simulated::SimulatedPsp;
use ap_store::seed_task_and_budget;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use uuid::Uuid;

fn get_database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres@127.0.0.1:9432/accesspilot".to_string())
}

struct TestGatewayEnv {
    pool: PgPool,
    psp: Arc<SimulatedPsp>,
    tenant_id: String,
}

#[async_trait]
impl AgentEnvironment for TestGatewayEnv {
    async fn fetch_resource(
        &self,
        task_id: Uuid,
        provider: &str,
        resource: &str,
        grant_token: Option<&str>,
    ) -> Result<ResourceResponse, String> {
        // Free provider
        if provider == "provider_a" {
            return Ok(ResourceResponse::Success {
                content: "Provider A Free Metadata".to_string(),
            });
        }

        // Grant check
        if let Some(token) = grant_token {
            let mut hasher = Sha256::new();
            hasher.update(token.as_bytes());
            let token_hash = hasher.finalize().to_vec();

            let row =
                sqlx::query("SELECT id, expires_at, revoked_at FROM grants WHERE token_hash = $1")
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

        // Not granted: mint signed offer
        let offer = mint_provider_offer(&self.pool, task_id, provider, resource)
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
        let res = execute_purchase(
            &self.pool,
            self.psp.clone(),
            &self.tenant_id,
            task_id,
            offer_id,
            idempotency_key,
            "agent_action_hash",
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

#[tokio::test]
async fn test_agent_end_to_end_scripted_demo_flow() {
    let db_url = get_database_url();
    let pool = match PgPoolOptions::new()
        .max_connections(10)
        .connect(&db_url)
        .await
    {
        Ok(p) => p,
        Err(_) => return,
    };

    let tenant_id = "tenant_agent_demo";
    let task_id = Uuid::new_v4();
    let cap_minor = 5000; // ₹50.00 cap

    seed_task_and_budget(&pool, tenant_id, task_id, cap_minor, Currency::INR)
        .await
        .expect("Seed task and budget");

    let psp = Arc::new(SimulatedPsp::new());
    let env = Arc::new(TestGatewayEnv {
        pool: pool.clone(),
        psp: psp.clone(),
        tenant_id: tenant_id.to_string(),
    });

    let planner = Arc::new(ScriptedPlanner::new());
    let runner = AgentRunner::new(planner, env);

    let history = runner.run(task_id).await;

    println!("Agent Run Steps:");
    for (i, step) in history.iter().enumerate() {
        println!("Step {i}: {:?} => {}", step.action, step.observation);
    }

    // Step 0: Free resource from provider_a (zero cost)
    assert!(
        history[0].observation.contains("Free Metadata"),
        "Step 0 must fetch free resource without payment"
    );
    assert_eq!(history[0].grant_token, None);

    // Step 1: Paid resource from provider_b (₹10.00) -> captured, grant issued
    assert!(
        history[1]
            .observation
            .contains("Unlocked premium data from provider_b"),
        "Step 1 must unlock provider_b"
    );
    assert!(history[1].grant_token.is_some());

    // Step 2: Paid resource from provider_c (₹15.00) -> captured, grant issued
    assert!(
        history[2]
            .observation
            .contains("Unlocked premium data from provider_c"),
        "Step 2 must unlock provider_c"
    );
    assert!(history[2].grant_token.is_some());

    // Step 3: Over-budget attempt: provider_d (₹40.00) -> ₹25 spent + ₹40 = ₹65 > ₹50 cap!
    // Control plane strictly denies over-budget spend!
    assert!(
        history[3].observation.contains("Purchase not approved"),
        "Step 3 must be denied by spend control plane"
    );
    assert_eq!(history[3].grant_token, None);

    // Step 4: Finished
    assert!(matches!(history[4].action, AgentAction::Finish { .. }));

    // Verify budget in database: Exactly ₹25 spent, ₹25 remaining, 0 overspend!
    let budget = sqlx::query(
        "SELECT cap_minor, reserved_minor, captured_minor FROM budgets WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let cap: i64 = budget.get("cap_minor");
    let reserved: i64 = budget.get("reserved_minor");
    let captured: i64 = budget.get("captured_minor");

    println!("Final Budget: cap={cap} reserved={reserved} captured={captured}");
    assert_eq!(
        captured, 2500,
        "Captured total must equal ₹25.00 (₹10 + ₹15)"
    );
    assert_eq!(reserved, 0, "No pending reservations");
    assert!(captured <= cap, "Zero overspend guarantee held");
}

#[tokio::test]
async fn test_groq_and_gemini_planners_strict_fallback() {
    // Both Groq and Gemini planners must fail-closed to the ScriptedPlanner on missing/invalid keys or network errors
    let groq = GroqPlanner::new("gsk_invalid_dummy_key");
    let action = groq
        .next_action(&[])
        .await
        .expect("Must fall back to ScriptedPlanner");
    assert!(
        matches!(action, AgentAction::RequestResource { ref provider, .. } if provider == "provider_a")
    );

    let gemini = GeminiPlanner::new("AIza_invalid_dummy_key");
    let action_gem = gemini
        .next_action(&[])
        .await
        .expect("Must fall back to ScriptedPlanner");
    assert!(
        matches!(action_gem, AgentAction::RequestResource { ref provider, .. } if provider == "provider_a")
    );
}
