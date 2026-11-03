//! Agent loop runner orchestrating planner actions and resource purchases.

use crate::planner::Planner;
use crate::schema::{AgentAction, AgentStep};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResourceResponse {
    Success { content: String },
    PaymentRequired { offer_id: Uuid, amount_minor: i64 },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PurchaseResponse {
    pub status: String,
    pub grant_token: Option<String>,
    pub message: String,
}

#[async_trait]
pub trait AgentEnvironment: Send + Sync {
    async fn fetch_resource(
        &self,
        task_id: Uuid,
        provider: &str,
        resource: &str,
        grant_token: Option<&str>,
    ) -> Result<ResourceResponse, String>;

    async fn buy_offer(
        &self,
        task_id: Uuid,
        offer_id: Uuid,
        idempotency_key: &str,
    ) -> Result<PurchaseResponse, String>;
}

pub struct AgentRunner {
    planner: Arc<dyn Planner>,
    env: Arc<dyn AgentEnvironment>,
    max_steps: usize,
}

impl AgentRunner {
    pub fn new(planner: Arc<dyn Planner>, env: Arc<dyn AgentEnvironment>) -> Self {
        Self {
            planner,
            env,
            max_steps: 8,
        }
    }

    pub fn with_max_steps(mut self, max: usize) -> Self {
        self.max_steps = max;
        self
    }

    pub async fn run(&self, task_id: Uuid) -> Vec<AgentStep> {
        let mut history: Vec<AgentStep> = Vec::new();

        for step_idx in 0..self.max_steps {
            let action = match self.planner.next_action(&history).await {
                Ok(a) => a,
                Err(e) => {
                    history.push(AgentStep {
                        step_index: step_idx,
                        action: AgentAction::Finish {
                            answer: format!("Planner stopped on error: {e}"),
                        },
                        observation: "Aborted".to_string(),
                        grant_token: None,
                    });
                    break;
                }
            };

            match &action {
                AgentAction::Finish { answer } => {
                    history.push(AgentStep {
                        step_index: step_idx,
                        action: action.clone(),
                        observation: format!("Finished: {answer}"),
                        grant_token: None,
                    });
                    break;
                }
                AgentAction::AskUser { question } => {
                    history.push(AgentStep {
                        step_index: step_idx,
                        action: action.clone(),
                        observation: format!("User asked: {question}"),
                        grant_token: None,
                    });
                }
                AgentAction::RequestResource {
                    provider,
                    resource_id,
                    purpose: _,
                } => {
                    // 1. Initial fetch attempt (without grant)
                    let initial_resp = self
                        .env
                        .fetch_resource(task_id, provider, resource_id, None)
                        .await;

                    match initial_resp {
                        Ok(ResourceResponse::Success { content }) => {
                            // Free resource path
                            history.push(AgentStep {
                                step_index: step_idx,
                                action: action.clone(),
                                observation: format!("Access granted (free): {content}"),
                                grant_token: None,
                            });
                        }
                        Ok(ResourceResponse::PaymentRequired {
                            offer_id,
                            amount_minor: _,
                        }) => {
                            // 2. Paid path: execute purchase flow
                            let idem_key = format!("agent_step_{step_idx}_{}", Uuid::new_v4());
                            let buy_res = self.env.buy_offer(task_id, offer_id, &idem_key).await;

                            match buy_res {
                                Ok(p_resp) if p_resp.status == "captured" => {
                                    if let Some(token) = p_resp.grant_token {
                                        // 3. Re-fetch with valid grant
                                        let final_fetch = self
                                            .env
                                            .fetch_resource(
                                                task_id,
                                                provider,
                                                resource_id,
                                                Some(&token),
                                            )
                                            .await;

                                        let obs = match final_fetch {
                                            Ok(ResourceResponse::Success { content }) => {
                                                format!("Purchased & unlocked: {content}")
                                            }
                                            _ => "Grant issued but retrieval failed".to_string(),
                                        };

                                        history.push(AgentStep {
                                            step_index: step_idx,
                                            action: action.clone(),
                                            observation: obs,
                                            grant_token: Some(token),
                                        });
                                    }
                                }
                                Ok(p_resp) => {
                                    // Denied, pending_unknown, or failed
                                    history.push(AgentStep {
                                        step_index: step_idx,
                                        action: action.clone(),
                                        observation: format!(
                                            "Purchase not approved: {} ({})",
                                            p_resp.message, p_resp.status
                                        ),
                                        grant_token: None,
                                    });
                                }
                                Err(err) => {
                                    history.push(AgentStep {
                                        step_index: step_idx,
                                        action: action.clone(),
                                        observation: format!("Purchase error: {err}"),
                                        grant_token: None,
                                    });
                                }
                            }
                        }
                        Ok(ResourceResponse::Error { message }) => {
                            history.push(AgentStep {
                                step_index: step_idx,
                                action: action.clone(),
                                observation: format!("Gateway error: {message}"),
                                grant_token: None,
                            });
                        }
                        Err(e) => {
                            history.push(AgentStep {
                                step_index: step_idx,
                                action: action.clone(),
                                observation: format!("Network fetch error: {e}"),
                                grant_token: None,
                            });
                        }
                    }
                }
            }
        }

        history
    }
}
