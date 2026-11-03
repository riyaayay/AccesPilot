//! T5: Model-based and property tests benchmark.
//! Runs 100,000 policy and lifecycle test cases compared against ap-core reference model.

use ap_core::ids::{GrantId, IdempotencyKey, PurchaseId, RuleId};
use ap_core::lifecycle::{PurchaseEvent, PurchaseStatus};
use ap_core::model::{ModelError, PurePurchaseModel};
use ap_core::money::{Currency, Money};
use ap_core::policy::{
    decide, Autonomy, DecisionContext, Expr, Field, Op, Policy, Rule, Value, Verdict,
};
use uuid::Uuid;

pub struct T5Result {
    pub cases: usize,
    pub disagreements: usize,
    pub pass: bool,
}

pub fn run_t5(seed: u64) -> T5Result {
    let mut rng_state = seed.wrapping_add(0x9E3779B97F4A7C15);
    let mut next_u64 = || {
        rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
        rng_state
    };

    let total_cases = 100_000;
    let mut disagreements = 0;

    // Part 1: 50,000 Policy property test cases
    let policy_cases = 50_000;
    for _ in 0..policy_cases {
        let cap = (next_u64() % 100_000) as i64 + 1000;
        let remaining = (next_u64() % (cap as u64)) as i64;
        let requested = (next_u64() % 150_000) as i64;
        let price_known = (next_u64() % 10) != 0; // 90% known

        let rule = Rule {
            id: RuleId("prop_rule".to_string()),
            then: Verdict::Allow,
            when: Expr::Cmp(Field::AmountMinor, Op::Lte, Value::Int(requested)),
            reason: "test".to_string(),
        };
        let policy = Policy {
            version: 1,
            autonomy: Autonomy::WithinRules,
            task_cap: Money {
                amount_minor: cap,
                currency: Currency::INR,
            },
            per_purchase_cap: Money {
                amount_minor: cap,
                currency: Currency::INR,
            },
            rules: vec![rule],
        };

        let ctx = DecisionContext {
            provider: "provider_bench".to_string(),
            resource_id: "res_bench".to_string(),
            category: Some("data".to_string()),
            amount: Money {
                amount_minor: requested,
                currency: Currency::INR,
            },
            tax: None,
            fee: None,
            price_known,
            remaining_budget: Money {
                amount_minor: remaining,
                currency: Currency::INR,
            },
            anomaly_score: 0,
            purpose: Some("model_test".to_string()),
        };

        let verdict = decide(&policy, &ctx).verdict;

        // Invariant 1: Unknown price, tax or fee must NEVER return Allow
        if !price_known && verdict == Verdict::Allow {
            disagreements += 1;
        }

        // Invariant 2: Over-budget requests must NEVER return Allow
        if requested > remaining && verdict == Verdict::Allow {
            disagreements += 1;
        }
    }

    // Part 2: 50,000 Stateful lifecycle runs against PurePurchaseModel
    let model_cases = 50_000;
    let cap_minor = 100_000; // ₹1,000.00
    let mut model = PurePurchaseModel::new(Money {
        amount_minor: cap_minor,
        currency: Currency::INR,
    });

    for i in 0..model_cases {
        let op = next_u64() % 5;
        let purchase_id = PurchaseId(Uuid::new_v4().to_string());
        let key = IdempotencyKey(format!("key_{}_{i}", next_u64() % 1000));
        let amount = (next_u64() % 5000) as i64 + 100;

        match op {
            0 => {
                // Reserve
                let prev_reserved = model.reserved_minor;
                let prev_captured = model.captured_minor;
                let res = model.reserve(purchase_id.clone(), amount, key);
                if prev_reserved + prev_captured + amount > cap_minor {
                    if res
                        != Err(ModelError::InsufficientBudget {
                            cap: cap_minor,
                            reserved: prev_reserved,
                            captured: prev_captured,
                            requested: amount,
                        })
                    {
                        disagreements += 1;
                    }
                } else if res.is_ok() && model.reserved_minor != prev_reserved + amount {
                    disagreements += 1;
                }
            }
            1 => {
                // Settle / Capture random existing purchase
                let existing_keys: Vec<PurchaseId> = model.purchases.keys().cloned().collect();
                if !existing_keys.is_empty() {
                    let idx = (next_u64() as usize) % existing_keys.len();
                    let pid = &existing_keys[idx];
                    let grant_id = GrantId(Uuid::new_v4().to_string());
                    let prev_status = model.purchases.get(pid).map(|p| p.status);
                    let res = model.capture(pid, grant_id);
                    if let Some(PurchaseStatus::Reserved) | Some(PurchaseStatus::PendingUnknown) =
                        prev_status
                    {
                        if res.is_err() {
                            disagreements += 1;
                        }
                    } else if res.is_ok() {
                        disagreements += 1;
                    }
                }
            }
            2 => {
                // Release random existing purchase
                let existing_keys: Vec<PurchaseId> = model.purchases.keys().cloned().collect();
                if !existing_keys.is_empty() {
                    let idx = (next_u64() as usize) % existing_keys.len();
                    let pid = &existing_keys[idx];
                    let prev_status = model.purchases.get(pid).map(|p| p.status);
                    let res = model.release(pid);
                    if let Some(PurchaseStatus::Reserved) | Some(PurchaseStatus::PendingUnknown) =
                        prev_status
                    {
                        if res.is_err() {
                            disagreements += 1;
                        }
                    } else if res.is_ok() {
                        disagreements += 1;
                    }
                }
            }
            3 => {
                // Mark pending unknown
                let existing_keys: Vec<PurchaseId> = model.purchases.keys().cloned().collect();
                if !existing_keys.is_empty() {
                    let idx = (next_u64() as usize) % existing_keys.len();
                    let pid = &existing_keys[idx];
                    let prev_status = model.purchases.get(pid).map(|p| p.status);
                    let res = model.mark_pending_unknown(pid);
                    if let Some(PurchaseStatus::Reserved) = prev_status {
                        if res.is_err() {
                            disagreements += 1;
                        }
                    } else if res.is_ok() {
                        disagreements += 1;
                    }
                }
            }
            _ => {
                // Verify pure total state machine transition
                let initial_state = match next_u64() % 6 {
                    0 => PurchaseStatus::AwaitingApproval,
                    1 => PurchaseStatus::Reserved,
                    2 => PurchaseStatus::PendingUnknown,
                    3 => PurchaseStatus::Captured,
                    4 => PurchaseStatus::Released,
                    _ => PurchaseStatus::Denied,
                };
                let event = match next_u64() % 4 {
                    0 => PurchaseEvent::ApprovalGranted,
                    1 => PurchaseEvent::ProviderSuccess,
                    2 => PurchaseEvent::ProviderDefinitiveFailure,
                    _ => PurchaseEvent::ProviderTimeoutOrUnknown,
                };

                let transition_res = initial_state.transition(event);
                // Invariant: Once captured, terminal state never transitions to non-captured
                if initial_state == PurchaseStatus::Captured
                    && transition_res.is_ok()
                    && transition_res.unwrap() != PurchaseStatus::Captured
                {
                    disagreements += 1;
                }
            }
        }

        // Global invariant check
        if model.check_invariants().is_err() {
            disagreements += 1;
        }
    }

    let pass = disagreements == 0;

    T5Result {
        cases: total_cases,
        disagreements,
        pass,
    }
}
