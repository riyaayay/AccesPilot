//! ap-psp: Payment Provider abstractions and adapters (Hyperswitch and Simulated PSP).

pub mod hyperswitch;
pub mod simulated;
pub mod traits;

pub use hyperswitch::HyperswitchPsp;
pub use simulated::SimulatedPsp;
pub use traits::{
    ChargeRequest, FailureClass, PaymentProvider, ProviderError, ProviderPayment, ProviderRefund,
    ProviderStatus, RefundRequest, WebhookError, WebhookTrigger,
};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
