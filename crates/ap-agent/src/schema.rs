//! Agent action schema and execution step records.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AgentAction {
    RequestResource {
        provider: String,
        resource_id: String,
        purpose: String,
    },
    AskUser {
        question: String,
    },
    Finish {
        answer: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStep {
    pub step_index: usize,
    pub action: AgentAction,
    pub observation: String,
    pub grant_token: Option<String>,
}
