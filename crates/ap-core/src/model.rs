//! Pure reference model of the whole purchase and budget lifecycle.
//! Used for model-based property testing and invariant validation.

use crate::ids::{GrantId, IdempotencyKey, PurchaseId};
use crate::lifecycle::PurchaseStatus;
use crate::money::{Currency, Money};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    #[error("Insufficient budget: cap={cap}, reserved={reserved}, captured={captured}, requested={requested}")]
    InsufficientBudget {
        cap: i64,
        reserved: i64,
        captured: i64,
        requested: i64,
    },
    #[error("Idempotency key reuse with different purchase id: {0}")]
    IdempotencyConflict(String),
    #[error("Purchase not found: {0}")]
    PurchaseNotFound(PurchaseId),
    #[error("Invalid state transition for purchase {0}: {1:?}")]
    InvalidTransition(PurchaseId, PurchaseStatus),
    #[error("Invariant violation: {0}")]
    InvariantViolation(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPurchase {
    pub id: PurchaseId,
    pub amount_minor: i64,
    pub status: PurchaseStatus,
    pub idempotency_key: IdempotencyKey,
    pub grant_id: Option<GrantId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurePurchaseModel {
    pub currency: Currency,
    pub cap_minor: i64,
    pub reserved_minor: i64,
    pub captured_minor: i64,
    pub purchases: HashMap<PurchaseId, ModelPurchase>,
    pub idempotency_index: HashMap<IdempotencyKey, PurchaseId>,
}

impl PurePurchaseModel {
    pub fn new(cap: Money) -> Self {
        Self {
            currency: cap.currency,
            cap_minor: cap.amount_minor,
            reserved_minor: 0,
            captured_minor: 0,
            purchases: HashMap::new(),
            idempotency_index: HashMap::new(),
        }
    }

    pub fn check_invariants(&self) -> Result<(), ModelError> {
        // Invariant 1: reserved + captured <= cap
        if self.reserved_minor + self.captured_minor > self.cap_minor {
            return Err(ModelError::InvariantViolation(format!(
                "Cap breached: reserved({}) + captured({}) > cap({})",
                self.reserved_minor, self.captured_minor, self.cap_minor
            )));
        }

        // Invariant 2: Non-negative balances
        if self.reserved_minor < 0 || self.captured_minor < 0 {
            return Err(ModelError::InvariantViolation(
                "Negative reserved or captured balance".to_string(),
            ));
        }

        // Invariant 3: Grants only exist for CAPTURED purchases
        for p in self.purchases.values() {
            if p.grant_id.is_some() && p.status != PurchaseStatus::Captured {
                return Err(ModelError::InvariantViolation(format!(
                    "Grant exists for non-captured purchase {}",
                    p.id
                )));
            }
        }

        Ok(())
    }

    pub fn reserve(
        &mut self,
        id: PurchaseId,
        amount_minor: i64,
        key: IdempotencyKey,
    ) -> Result<bool, ModelError> {
        // Idempotency check
        if let Some(existing_id) = self.idempotency_index.get(&key) {
            if *existing_id != id {
                return Err(ModelError::IdempotencyConflict(key.0));
            }
            // Idempotent replay: return false indicating no new reservation made
            return Ok(false);
        }

        // Atomic check: reserved + captured + amount <= cap
        if self.reserved_minor + self.captured_minor + amount_minor > self.cap_minor {
            return Err(ModelError::InsufficientBudget {
                cap: self.cap_minor,
                reserved: self.reserved_minor,
                captured: self.captured_minor,
                requested: amount_minor,
            });
        }

        self.reserved_minor += amount_minor;
        self.purchases.insert(
            id.clone(),
            ModelPurchase {
                id: id.clone(),
                amount_minor,
                status: PurchaseStatus::Reserved,
                idempotency_key: key.clone(),
                grant_id: None,
            },
        );
        self.idempotency_index.insert(key, id);

        self.check_invariants()?;
        Ok(true)
    }

    pub fn capture(&mut self, id: &PurchaseId, grant_id: GrantId) -> Result<(), ModelError> {
        let purchase = self
            .purchases
            .get_mut(id)
            .ok_or_else(|| ModelError::PurchaseNotFound(id.clone()))?;

        match purchase.status {
            PurchaseStatus::Reserved => {
                self.reserved_minor -= purchase.amount_minor;
                self.captured_minor += purchase.amount_minor;
                purchase.status = PurchaseStatus::Captured;
                purchase.grant_id = Some(grant_id);
            }
            PurchaseStatus::PendingUnknown => {
                self.reserved_minor -= purchase.amount_minor;
                self.captured_minor += purchase.amount_minor;
                purchase.status = PurchaseStatus::Captured;
                purchase.grant_id = Some(grant_id);
            }
            other => return Err(ModelError::InvalidTransition(id.clone(), other)),
        }

        self.check_invariants()
    }

    pub fn release(&mut self, id: &PurchaseId) -> Result<(), ModelError> {
        let purchase = self
            .purchases
            .get_mut(id)
            .ok_or_else(|| ModelError::PurchaseNotFound(id.clone()))?;

        match purchase.status {
            PurchaseStatus::Reserved => {
                self.reserved_minor -= purchase.amount_minor;
                purchase.status = PurchaseStatus::Released;
            }
            PurchaseStatus::PendingUnknown => {
                self.reserved_minor -= purchase.amount_minor;
                purchase.status = PurchaseStatus::Released;
            }
            other => return Err(ModelError::InvalidTransition(id.clone(), other)),
        }

        self.check_invariants()
    }

    pub fn mark_pending_unknown(&mut self, id: &PurchaseId) -> Result<(), ModelError> {
        let purchase = self
            .purchases
            .get_mut(id)
            .ok_or_else(|| ModelError::PurchaseNotFound(id.clone()))?;

        if purchase.status != PurchaseStatus::Reserved {
            return Err(ModelError::InvalidTransition(id.clone(), purchase.status));
        }

        purchase.status = PurchaseStatus::PendingUnknown;
        self.check_invariants()
    }
}
