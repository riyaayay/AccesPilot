//! ap-agent: Planner clients (Scripted, Groq, Gemini), Action schema, and Agent loop runner.

pub mod planner;
pub mod runner;
pub mod schema;

pub use planner::{GeminiPlanner, GroqPlanner, Planner, PlannerError, ScriptedPlanner};
pub use runner::{AgentEnvironment, AgentRunner, PurchaseResponse, ResourceResponse};
pub use schema::{AgentAction, AgentStep};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
