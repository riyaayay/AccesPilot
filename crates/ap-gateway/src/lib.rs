//! ap-gateway: Axum HTTP control plane, purchase flow, webhooks, reconciler, and provider simulators.

pub mod api;
pub mod api_agent_env;
pub mod reconciler;
pub mod resource_gateway;
pub mod service;
pub mod simulators;
pub mod webhooks;

pub use api::{create_router, AppState};
pub use reconciler::{reconcile_once, ReconcileReport};
pub use resource_gateway::{handle_get_resource, Resource402, ResourceSuccess};
pub use service::{check_failpoint, execute_purchase, PurchaseFlowError, PurchaseResult};
pub use simulators::{
    mint_provider_offer, sign_offer_data, verify_offer_signature, SignedOffer, OFFER_SIGNING_KEY,
};
pub use webhooks::{process_webhook, WebhookHandlerResult};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
