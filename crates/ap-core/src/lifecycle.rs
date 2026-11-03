//! Total state machines and transition functions for all lifecycles.
//! Illegal transitions return explicit errors.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    #[error("Invalid purchase transition from {from:?} via event {event:?}")]
    InvalidPurchaseTransition {
        from: PurchaseStatus,
        event: PurchaseEvent,
    },
    #[error("Invalid reservation transition from {from:?} via event {event:?}")]
    InvalidReservationTransition {
        from: ReservationStatus,
        event: ReservationEvent,
    },
    #[error("Invalid grant transition from {from:?} via event {event:?}")]
    InvalidGrantTransition {
        from: GrantStatus,
        event: GrantEvent,
    },
    #[error("Invalid policy transition from {from:?} via event {event:?}")]
    InvalidPolicyTransition {
        from: PolicyLifecycle,
        event: PolicyEvent,
    },
}

/// Core purchase status matching SQL purchase_status ENUM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PurchaseStatus {
    AwaitingApproval,
    Reserved,
    PendingUnknown,
    Captured,
    Released,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PurchaseEvent {
    ApprovalGranted,
    ApprovalDenied,
    ReserveIntent,
    ProviderSuccess,
    ProviderDefinitiveFailure,
    ProviderTimeoutOrUnknown,
    ReconciliationConfirmedSuccess,
    ReconciliationConfirmedFailure,
}

impl PurchaseStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Captured | Self::Released | Self::Denied)
    }

    pub fn transition(&self, event: PurchaseEvent) -> Result<Self, TransitionError> {
        match (self, event) {
            (Self::AwaitingApproval, PurchaseEvent::ApprovalGranted) => Ok(Self::Reserved),
            (Self::AwaitingApproval, PurchaseEvent::ApprovalDenied) => Ok(Self::Denied),
            (Self::AwaitingApproval, PurchaseEvent::ReserveIntent) => Ok(Self::Reserved),

            (Self::Reserved, PurchaseEvent::ProviderSuccess) => Ok(Self::Captured),
            (Self::Reserved, PurchaseEvent::ProviderDefinitiveFailure) => Ok(Self::Released),
            (Self::Reserved, PurchaseEvent::ProviderTimeoutOrUnknown) => Ok(Self::PendingUnknown),

            (Self::PendingUnknown, PurchaseEvent::ProviderSuccess) => Ok(Self::Captured),
            (Self::PendingUnknown, PurchaseEvent::ProviderDefinitiveFailure) => Ok(Self::Released),
            (Self::PendingUnknown, PurchaseEvent::ReconciliationConfirmedSuccess) => {
                Ok(Self::Captured)
            }
            (Self::PendingUnknown, PurchaseEvent::ReconciliationConfirmedFailure) => {
                Ok(Self::Released)
            }

            // Terminal absorbing states: no valid transitions out
            (from, event) => Err(TransitionError::InvalidPurchaseTransition { from: *from, event }),
        }
    }
}

/// Reservation status for budget locking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReservationStatus {
    Held,
    Captured,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReservationEvent {
    SettleCapture,
    SettleRelease,
}

impl ReservationStatus {
    pub fn transition(&self, event: ReservationEvent) -> Result<Self, TransitionError> {
        match (self, event) {
            (Self::Held, ReservationEvent::SettleCapture) => Ok(Self::Captured),
            (Self::Held, ReservationEvent::SettleRelease) => Ok(Self::Released),
            (from, event) => {
                Err(TransitionError::InvalidReservationTransition { from: *from, event })
            }
        }
    }
}

/// Access grant status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GrantStatus {
    Valid,
    Revoked,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GrantEvent {
    Revoke,
    Expire,
}

impl GrantStatus {
    pub fn transition(&self, event: GrantEvent) -> Result<Self, TransitionError> {
        match (self, event) {
            (Self::Valid, GrantEvent::Revoke) => Ok(Self::Revoked),
            (Self::Valid, GrantEvent::Expire) => Ok(Self::Expired),
            (from, event) => Err(TransitionError::InvalidGrantTransition { from: *from, event }),
        }
    }
}

/// Policy deployment lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyLifecycle {
    Draft,
    Shadow,
    Active,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolicyEvent {
    MoveToShadow,
    Activate,
    Retire,
}

impl PolicyLifecycle {
    pub fn transition(&self, event: PolicyEvent) -> Result<Self, TransitionError> {
        match (self, event) {
            (Self::Draft, PolicyEvent::MoveToShadow) => Ok(Self::Shadow),
            (Self::Draft, PolicyEvent::Activate) => Ok(Self::Active),
            (Self::Shadow, PolicyEvent::Activate) => Ok(Self::Active),
            (Self::Active, PolicyEvent::Retire) => Ok(Self::Retired),
            (from, event) => Err(TransitionError::InvalidPolicyTransition { from: *from, event }),
        }
    }
}
