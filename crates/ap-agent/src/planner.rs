//! Planners: Scripted, Groq, and Gemini with strict schema validation and automatic fallback.

use crate::schema::{AgentAction, AgentStep};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PlannerError {
    #[error("Network error: {0}")]
    Network(String),
    #[error("Malformed JSON: {0}")]
    MalformedJson(String),
    #[error("Schema validation failed: {0}")]
    SchemaMismatch(String),
    #[error("Planner exhausted")]
    Exhausted,
}

#[async_trait]
pub trait Planner: Send + Sync {
    async fn next_action(&self, history: &[AgentStep]) -> Result<AgentAction, PlannerError>;
}

/// Scripted deterministic planner for CI, benchmarks, and demo fallback.
#[derive(Clone, Default)]
pub struct ScriptedPlanner;

impl ScriptedPlanner {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Planner for ScriptedPlanner {
    async fn next_action(&self, history: &[AgentStep]) -> Result<AgentAction, PlannerError> {
        let step = history.len();
        match step {
            0 => Ok(AgentAction::RequestResource {
                provider: "provider_a".to_string(),
                resource_id: "free_catalog".to_string(),
                purpose: "Explore free provider metadata".to_string(),
            }),
            1 => Ok(AgentAction::RequestResource {
                provider: "provider_b".to_string(),
                resource_id: "market_overview".to_string(),
                purpose: "Purchase standard ₹10 market dataset".to_string(),
            }),
            2 => Ok(AgentAction::RequestResource {
                provider: "provider_c".to_string(),
                resource_id: "deep_analytics".to_string(),
                purpose: "Purchase ₹15 analytics insights".to_string(),
            }),
            3 => Ok(AgentAction::RequestResource {
                provider: "provider_d".to_string(),
                resource_id: "enterprise_archive".to_string(),
                purpose: "Attempt ₹40 archive (demonstrates budget denial)".to_string(),
            }),
            _ => Ok(AgentAction::Finish {
                answer: "Gathered all allowed data sources within the allocated budget cap"
                    .to_string(),
            }),
        }
    }
}

/// Planner powered by Groq (Llama 3.3 70B) with strict schema validation and fallback to ScriptedPlanner.
pub struct GroqPlanner {
    client: Client,
    api_key: String,
    fallback: ScriptedPlanner,
}

impl GroqPlanner {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
            api_key: api_key.into(),
            fallback: ScriptedPlanner::new(),
        }
    }
}

#[async_trait]
impl Planner for GroqPlanner {
    async fn next_action(&self, history: &[AgentStep]) -> Result<AgentAction, PlannerError> {
        let prompt = format!(
            "You are a research agent. You must output strictly valid JSON matching one of these shapes:\n\
            {{\"action\":\"request_resource\",\"provider\":\"provider_b\",\"resource_id\":\"res_1\",\"purpose\":\"reason\"}}\n\
            {{\"action\":\"finish\",\"answer\":\"final summary\"}}\n\
            History step count: {}. Return JSON only.",
            history.len()
        );

        let req_body = serde_json::json!({
            "model": "llama-3.3-70b-versatile",
            "messages": [
                {"role": "system", "content": "You output only valid JSON actions."},
                {"role": "user", "content": prompt}
            ],
            "response_format": {"type": "json_object"}
        });

        let resp_res = self
            .client
            .post("https://api.groq.com/openai/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&req_body)
            .send()
            .await;

        match resp_res {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(json) = resp.json::<Value>().await {
                    if let Some(content) = json
                        .pointer("/choices/0/message/content")
                        .and_then(|c| c.as_str())
                    {
                        if let Ok(action) = serde_json::from_str::<AgentAction>(content) {
                            return Ok(action);
                        }
                    }
                }
            }
            _ => {}
        }

        // Fail-closed to scripted planner on any mismatch, error, or unparseable output
        tracing::warn!(
            "Groq planner failed or returned malformed action; falling back to ScriptedPlanner"
        );
        self.fallback.next_action(history).await
    }
}

/// Planner powered by Gemini API with strict validation and fallback to ScriptedPlanner.
pub struct GeminiPlanner {
    client: Client,
    api_key: String,
    fallback: ScriptedPlanner,
}

impl GeminiPlanner {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
            api_key: api_key.into(),
            fallback: ScriptedPlanner::new(),
        }
    }
}

#[async_trait]
impl Planner for GeminiPlanner {
    async fn next_action(&self, history: &[AgentStep]) -> Result<AgentAction, PlannerError> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-flash:generateContent?key={}",
            self.api_key
        );

        let prompt = format!(
            "You are a research agent. Return strictly a JSON action with keys: action, provider, resource_id, purpose, or action='finish' and answer='...'. History count: {}",
            history.len()
        );

        let req_body = serde_json::json!({
            "contents": [{
                "parts": [{"text": prompt}]
            }],
            "generationConfig": {
                "responseMimeType": "application/json"
            }
        });

        let resp_res = self.client.post(&url).json(&req_body).send().await;

        match resp_res {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(json) = resp.json::<Value>().await {
                    if let Some(text) = json
                        .pointer("/candidates/0/content/parts/0/text")
                        .and_then(|t| t.as_str())
                    {
                        if let Ok(action) = serde_json::from_str::<AgentAction>(text) {
                            return Ok(action);
                        }
                    }
                }
            }
            _ => {}
        }

        // Fail-closed to scripted planner
        tracing::warn!(
            "Gemini planner failed or returned malformed action; falling back to ScriptedPlanner"
        );
        self.fallback.next_action(history).await
    }
}
