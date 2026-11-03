//! ap-core: Pure types, policy AST, evaluation engine, and state machines.
//! No I/O, no async, total functions only.

#![deny(clippy::unwrap_used, clippy::panic)]

pub mod anomaly;
pub mod ids;
pub mod lifecycle;
pub mod model;
pub mod money;
pub mod policy;
pub mod router;

pub use anomaly::compute_anomaly_score;
pub use ids::{GrantId, IdempotencyKey, OfferId, PurchaseId, RuleId, TaskId, TenantId};
pub use lifecycle::{
    GrantEvent, GrantStatus, PolicyEvent, PolicyLifecycle, PurchaseEvent, PurchaseStatus,
    ReservationEvent, ReservationStatus, TransitionError,
};
pub use model::{ModelPurchase, PurePurchaseModel};
pub use money::{Currency, Money, MoneyError};
pub use policy::{
    decide, Autonomy, Decision, DecisionContext, Expr, Field, Op, Policy, Rule, RuleHit, Value,
    Verdict,
};
pub use router::{rank_offers, ProviderOfferCandidate, RankedOffer, RouterWeights};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample_policy() -> Policy {
        Policy {
            version: 3,
            autonomy: Autonomy::WithinRules,
            task_cap: Money::new_raw(5000, Currency::INR),
            per_purchase_cap: Money::new_raw(2000, Currency::INR),
            rules: vec![
                Rule {
                    id: RuleId::new("allow-trusted"),
                    then: Verdict::Allow,
                    when: Expr::All(vec![
                        Expr::In(
                            Field::Provider,
                            vec![
                                Value::String("news-a".to_string()),
                                Value::String("data-b".to_string()),
                            ],
                        ),
                        Expr::Cmp(Field::AmountMinor, Op::Lte, Value::Int(2000)),
                    ]),
                    reason: "trusted provider under cap".to_string(),
                },
                Rule {
                    id: RuleId::new("approve-unknown"),
                    then: Verdict::RequireApproval,
                    when: Expr::Not(Box::new(Expr::In(
                        Field::Provider,
                        vec![
                            Value::String("news-a".to_string()),
                            Value::String("data-b".to_string()),
                            Value::String("api-c".to_string()),
                        ],
                    ))),
                    reason: "provider not on allowlist".to_string(),
                },
                Rule {
                    id: RuleId::new("no-subscriptions"),
                    then: Verdict::Deny,
                    when: Expr::Cmp(
                        Field::Category,
                        Op::Eq,
                        Value::String("subscription".to_string()),
                    ),
                    reason: "subscriptions are never autonomous".to_string(),
                },
            ],
        }
    }

    #[test]
    fn test_sample_policy_deserialization() {
        let json_data = r#"{
            "version": 3,
            "autonomy": "within_rules",
            "task_cap": {"amount_minor": 5000, "currency": "INR"},
            "per_purchase_cap": {"amount_minor": 2000, "currency": "INR"},
            "rules": [
                {
                    "id": "allow-trusted",
                    "then": "allow",
                    "when": {
                        "all": [
                            {"provider": {"in": ["news-a", "data-b"]}},
                            {"amount_minor": {"lte": 2000}}
                        ]
                    },
                    "reason": "trusted provider under the per-purchase cap"
                },
                {
                    "id": "approve-unknown",
                    "then": "require_approval",
                    "when": {
                        "not": {
                            "provider": {"in": ["news-a", "data-b", "api-c"]}
                        }
                    },
                    "reason": "provider not on the allowlist"
                },
                {
                    "id": "no-subscriptions",
                    "then": "deny",
                    "when": {
                        "category": {"eq": "subscription"}
                    },
                    "reason": "subscriptions are never autonomous"
                }
            ]
        }"#;

        let policy: Policy = serde_json::from_str(json_data).unwrap();
        assert_eq!(policy.version, 3);
        assert_eq!(policy.rules.len(), 3);

        // Test decision with allowed provider
        let ctx = DecisionContext {
            provider: "news-a".to_string(),
            resource_id: "res-1".to_string(),
            category: Some("article".to_string()),
            amount: Money::new_raw(1000, Currency::INR),
            tax: Some(Money::new_raw(0, Currency::INR)),
            fee: Some(Money::new_raw(0, Currency::INR)),
            price_known: true,
            remaining_budget: Money::new_raw(4000, Currency::INR),
            anomaly_score: 0,
            purpose: None,
        };

        let decision = decide(&policy, &ctx);
        assert_eq!(decision.verdict, Verdict::Allow);
    }

    #[test]
    fn test_hard_invariants() {
        let policy = sample_policy();

        // 1. Currency mismatch -> Deny
        let ctx_wrong_currency = DecisionContext {
            provider: "news-a".to_string(),
            resource_id: "res-1".to_string(),
            category: Some("article".to_string()),
            amount: Money::new_raw(1000, Currency::USD),
            tax: Some(Money::new_raw(0, Currency::USD)),
            fee: Some(Money::new_raw(0, Currency::USD)),
            price_known: true,
            remaining_budget: Money::new_raw(4000, Currency::USD),
            anomaly_score: 0,
            purpose: None,
        };
        assert_eq!(decide(&policy, &ctx_wrong_currency).verdict, Verdict::Deny);

        // 2. Price not fully known -> RequireApproval
        let ctx_unknown_price = DecisionContext {
            provider: "news-a".to_string(),
            resource_id: "res-1".to_string(),
            category: Some("article".to_string()),
            amount: Money::new_raw(1000, Currency::INR),
            tax: None,
            fee: None,
            price_known: false,
            remaining_budget: Money::new_raw(4000, Currency::INR),
            anomaly_score: 0,
            purpose: None,
        };
        assert_eq!(
            decide(&policy, &ctx_unknown_price).verdict,
            Verdict::RequireApproval
        );

        // 3. Exceeds remaining budget -> Deny
        let ctx_exceeds_budget = DecisionContext {
            provider: "news-a".to_string(),
            resource_id: "res-1".to_string(),
            category: Some("article".to_string()),
            amount: Money::new_raw(1500, Currency::INR),
            tax: Some(Money::new_raw(0, Currency::INR)),
            fee: Some(Money::new_raw(0, Currency::INR)),
            price_known: true,
            remaining_budget: Money::new_raw(1000, Currency::INR),
            anomaly_score: 0,
            purpose: None,
        };
        assert_eq!(decide(&policy, &ctx_exceeds_budget).verdict, Verdict::Deny);

        // 4. Exceeds per-purchase cap -> Deny
        let ctx_exceeds_cap = DecisionContext {
            provider: "news-a".to_string(),
            resource_id: "res-1".to_string(),
            category: Some("article".to_string()),
            amount: Money::new_raw(2500, Currency::INR),
            tax: Some(Money::new_raw(0, Currency::INR)),
            fee: Some(Money::new_raw(0, Currency::INR)),
            price_known: true,
            remaining_budget: Money::new_raw(4000, Currency::INR),
            anomaly_score: 0,
            purpose: None,
        };
        assert_eq!(decide(&policy, &ctx_exceeds_cap).verdict, Verdict::Deny);
    }

    // Proptests
    proptest! {
        // Property 1: Permutation invariance
        // Shuffling rules never changes the verdict
        #[test]
        fn prop_permutation_invariance(seed in 0..1000u64) {
            let mut policy = sample_policy();
            let ctx = DecisionContext {
                provider: "news-a".to_string(),
                resource_id: "res-1".to_string(),
                category: Some("article".to_string()),
                amount: Money::new_raw(1500, Currency::INR),
                tax: Some(Money::new_raw(0, Currency::INR)),
                fee: Some(Money::new_raw(0, Currency::INR)),
                price_known: true,
                remaining_budget: Money::new_raw(4000, Currency::INR),
                anomaly_score: 0,
                purpose: None,
            };

            let v1 = decide(&policy, &ctx).verdict;

            // Pseudo-random shuffle of rules
            if seed % 2 == 1 {
                policy.rules.reverse();
            } else if policy.rules.len() > 1 {
                policy.rules.swap(0, 1);
            }
            let v2 = decide(&policy, &ctx).verdict;

            prop_assert_eq!(v1, v2);
        }

        // Property 2: Price monotonicity
        // Raising amount never turns Deny into Allow
        #[test]
        fn prop_price_monotonicity(amt1 in 1..2000i64, amt2 in 2001..5000i64) {
            let policy = sample_policy();
            let ctx1 = DecisionContext {
                provider: "news-a".to_string(),
                resource_id: "res-1".to_string(),
                category: Some("article".to_string()),
                amount: Money::new_raw(amt1, Currency::INR),
                tax: Some(Money::new_raw(0, Currency::INR)),
                fee: Some(Money::new_raw(0, Currency::INR)),
                price_known: true,
                remaining_budget: Money::new_raw(5000, Currency::INR),
                anomaly_score: 0,
                purpose: None,
            };
            let ctx2 = DecisionContext {
                provider: "news-a".to_string(),
                resource_id: "res-1".to_string(),
                category: Some("article".to_string()),
                amount: Money::new_raw(amt2, Currency::INR),
                tax: Some(Money::new_raw(0, Currency::INR)),
                fee: Some(Money::new_raw(0, Currency::INR)),
                price_known: true,
                remaining_budget: Money::new_raw(5000, Currency::INR),
                anomaly_score: 0,
                purpose: None,
            };

            let v1 = decide(&policy, &ctx1).verdict;
            let v2 = decide(&policy, &ctx2).verdict;

            // If low price was Deny, higher price cannot be Allow
            if v1 == Verdict::Deny {
                prop_assert_ne!(v2, Verdict::Allow);
            }
        }

        // Property 3: Budget safety
        // If expected total > remaining budget, verdict is never Allow
        #[test]
        fn prop_budget_safety(amount in 100..5000i64, budget in 0..100i64) {
            let policy = sample_policy();
            let ctx = DecisionContext {
                provider: "news-a".to_string(),
                resource_id: "res-1".to_string(),
                category: Some("article".to_string()),
                amount: Money::new_raw(amount, Currency::INR),
                tax: Some(Money::new_raw(0, Currency::INR)),
                fee: Some(Money::new_raw(0, Currency::INR)),
                price_known: true,
                remaining_budget: Money::new_raw(budget, Currency::INR),
                anomaly_score: 0,
                purpose: None,
            };

            let decision = decide(&policy, &ctx);
            prop_assert_ne!(decision.verdict, Verdict::Allow);
        }

        // Property 4: Unknown is never Allow
        #[test]
        fn prop_unknown_never_allow(price_known: bool, has_tax: bool, has_fee: bool) {
            let policy = sample_policy();
            let ctx = DecisionContext {
                provider: "news-a".to_string(),
                resource_id: "res-1".to_string(),
                category: Some("article".to_string()),
                amount: Money::new_raw(500, Currency::INR),
                tax: if has_tax { Some(Money::new_raw(0, Currency::INR)) } else { None },
                fee: if has_fee { Some(Money::new_raw(0, Currency::INR)) } else { None },
                price_known,
                remaining_budget: Money::new_raw(5000, Currency::INR),
                anomaly_score: 0,
                purpose: None,
            };

            let decision = decide(&policy, &ctx);
            if !price_known || !has_tax || !has_fee {
                prop_assert_ne!(decision.verdict, Verdict::Allow);
            }
        }

        // Property 5: JSON round-trip
        #[test]
        fn prop_json_round_trip(version in 1..100u32, cap in 100..100000i64) {
            let policy = Policy {
                version,
                autonomy: Autonomy::WithinRules,
                task_cap: Money::new_raw(cap, Currency::INR),
                per_purchase_cap: Money::new_raw(cap / 2, Currency::INR),
                rules: vec![
                    Rule {
                        id: RuleId::new("rule-1"),
                        then: Verdict::Allow,
                        when: Expr::Cmp(Field::AmountMinor, Op::Lte, Value::Int(cap / 2)),
                        reason: "under cap".to_string(),
                    }
                ],
            };

            let json_str = serde_json::to_string(&policy).unwrap();
            let deserialized: Policy = serde_json::from_str(&json_str).unwrap();
            prop_assert_eq!(policy, deserialized);
        }
    }
}
