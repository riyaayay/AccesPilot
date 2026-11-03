//! Policy AST, JSON serialization, and deterministic evaluation engine.
//! Order-independent evaluation with fail-closed hard invariants.

use crate::ids::RuleId;
use crate::money::{Money, MoneyError};
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Autonomy {
    AskEveryTime,
    WithinRules,
    NoPaidAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Allow,
    RequireApproval,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    AmountMinor,
    Currency,
    Provider,
    ResourceId,
    Category,
    AnomalyScore,
    Purpose,
}

impl Field {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AmountMinor => "amount_minor",
            Self::Currency => "currency",
            Self::Provider => "provider",
            Self::ResourceId => "resource_id",
            Self::Category => "category",
            Self::AnomalyScore => "anomaly_score",
            Self::Purpose => "purpose",
        }
    }
}

impl std::str::FromStr for Field {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "amount_minor" => Ok(Self::AmountMinor),
            "currency" => Ok(Self::Currency),
            "provider" => Ok(Self::Provider),
            "resource_id" => Ok(Self::ResourceId),
            "category" => Ok(Self::Category),
            "anomaly_score" => Ok(Self::AnomalyScore),
            "purpose" => Ok(Self::Purpose),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
}

impl Op {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::Neq => "neq",
            Self::Gt => "gt",
            Self::Gte => "gte",
            Self::Lt => "lt",
            Self::Lte => "lte",
        }
    }
}

impl std::str::FromStr for Op {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "eq" => Ok(Self::Eq),
            "neq" => Ok(Self::Neq),
            "gt" => Ok(Self::Gt),
            "gte" => Ok(Self::Gte),
            "lt" => Ok(Self::Lt),
            "lte" => Ok(Self::Lte),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Int(i64),
    String(String),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    All(Vec<Expr>),
    Any(Vec<Expr>),
    Not(Box<Expr>),
    Cmp(Field, Op, Value),
    In(Field, Vec<Value>),
}

impl Serialize for Expr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::All(exprs) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("all", exprs)?;
                map.end()
            }
            Self::Any(exprs) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("any", exprs)?;
                map.end()
            }
            Self::Not(expr) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("not", expr)?;
                map.end()
            }
            Self::In(field, vals) => {
                let mut map = serializer.serialize_map(Some(1))?;
                let mut inner = std::collections::BTreeMap::new();
                inner.insert("in", vals);
                map.serialize_entry(field.as_str(), &inner)?;
                map.end()
            }
            Self::Cmp(field, op, val) => {
                let mut map = serializer.serialize_map(Some(1))?;
                let mut inner = std::collections::BTreeMap::new();
                inner.insert(op.as_str(), val);
                map.serialize_entry(field.as_str(), &inner)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Expr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ExprVisitor;

        impl<'de> Visitor<'de> for ExprVisitor {
            type Value = Expr;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a valid expression object")
            }

            fn visit_map<M>(self, mut access: M) -> Result<Expr, M::Error>
            where
                M: MapAccess<'de>,
            {
                let key: String = match access.next_key()? {
                    Some(k) => k,
                    None => return Err(de::Error::custom("empty expression object")),
                };

                match key.as_str() {
                    "all" => {
                        let exprs: Vec<Expr> = access.next_value()?;
                        Ok(Expr::All(exprs))
                    }
                    "any" => {
                        let exprs: Vec<Expr> = access.next_value()?;
                        Ok(Expr::Any(exprs))
                    }
                    "not" => {
                        let expr: Expr = access.next_value()?;
                        Ok(Expr::Not(Box::new(expr)))
                    }
                    field_name => {
                        let field: Field = field_name.parse().map_err(|_| {
                            de::Error::custom(format!("unknown field in expression: {field_name}"))
                        })?;
                        let inner: std::collections::BTreeMap<String, serde_json::Value> =
                            access.next_value()?;
                        let (op_name, val_json) = inner
                            .into_iter()
                            .next()
                            .ok_or_else(|| de::Error::custom("missing operator in condition"))?;

                        if op_name == "in" {
                            let vals: Vec<Value> =
                                serde_json::from_value(val_json).map_err(de::Error::custom)?;
                            Ok(Expr::In(field, vals))
                        } else {
                            let op: Op = op_name.parse().map_err(|_| {
                                de::Error::custom(format!("unknown operator: {op_name}"))
                            })?;
                            let val: Value =
                                serde_json::from_value(val_json).map_err(de::Error::custom)?;
                            Ok(Expr::Cmp(field, op, val))
                        }
                    }
                }
            }
        }

        deserializer.deserialize_map(ExprVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: RuleId,
    pub then: Verdict,
    pub when: Expr,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub version: u32,
    pub autonomy: Autonomy,
    pub task_cap: Money,
    pub per_purchase_cap: Money,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionContext {
    pub provider: String,
    pub resource_id: String,
    pub category: Option<String>,
    pub amount: Money,
    pub tax: Option<Money>,
    pub fee: Option<Money>,
    pub price_known: bool,
    pub remaining_budget: Money,
    pub anomaly_score: i64,
    pub purpose: Option<String>,
}

impl DecisionContext {
    pub fn get_field_value(&self, field: Field) -> Option<Value> {
        match field {
            Field::AmountMinor => Some(Value::Int(self.amount.amount_minor)),
            Field::Currency => Some(Value::String(self.amount.currency.to_string())),
            Field::Provider => Some(Value::String(self.provider.clone())),
            Field::ResourceId => Some(Value::String(self.resource_id.clone())),
            Field::Category => self.category.clone().map(Value::String),
            Field::AnomalyScore => Some(Value::Int(self.anomaly_score)),
            Field::Purpose => self.purpose.clone().map(Value::String),
        }
    }

    pub fn total_expected(&self) -> Result<Money, MoneyError> {
        let mut total = self.amount;
        if let Some(tax) = self.tax {
            total = total.checked_add(&tax)?;
        }
        if let Some(fee) = self.fee {
            total = total.checked_add(&fee)?;
        }
        Ok(total)
    }
}

impl Expr {
    pub fn evaluate(&self, ctx: &DecisionContext) -> bool {
        match self {
            Self::All(exprs) => exprs.iter().all(|e| e.evaluate(ctx)),
            Self::Any(exprs) => exprs.iter().any(|e| e.evaluate(ctx)),
            Self::Not(expr) => !expr.evaluate(ctx),
            Self::In(field, vals) => match ctx.get_field_value(*field) {
                Some(ctx_val) => vals.contains(&ctx_val),
                None => false,
            },
            Self::Cmp(field, op, expected) => match ctx.get_field_value(*field) {
                Some(actual) => evaluate_cmp(&actual, *op, expected),
                None => false,
            },
        }
    }
}

fn evaluate_cmp(actual: &Value, op: Op, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Int(a), Value::Int(b)) => match op {
            Op::Eq => a == b,
            Op::Neq => a != b,
            Op::Gt => a > b,
            Op::Gte => a >= b,
            Op::Lt => a < b,
            Op::Lte => a <= b,
        },
        (Value::String(a), Value::String(b)) => match op {
            Op::Eq => a == b,
            Op::Neq => a != b,
            Op::Gt => a > b,
            Op::Gte => a >= b,
            Op::Lt => a < b,
            Op::Lte => a <= b,
        },
        (Value::Bool(a), Value::Bool(b)) => match op {
            Op::Eq => a == b,
            Op::Neq => a != b,
            _ => false,
        },
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleHit {
    pub rule_id: RuleId,
    pub verdict: Verdict,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub verdict: Verdict,
    pub trace: Vec<RuleHit>,
    pub policy_version: u32,
    pub explanation: String,
}

/// Pure total evaluation function.
/// Order of evaluation (fail-closed by construction):
/// 1. Hard invariants first (non-configurable):
///    - Currency mismatch => Deny
///    - Price, tax, fee not fully known => RequireApproval
///    - Expected total above remaining budget or per-purchase cap => Deny
///    - Autonomy NoPaidAccess and amount > 0 => Deny
/// 2. User rules next (order-independent):
///    - Every matching rule contributes its verdict.
///    - Most restrictive wins (Deny > RequireApproval > Allow).
/// 3. Default is Deny if no rule matches or allows.
/// 4. AskEveryTime autonomy downgrades Allow to RequireApproval.
pub fn decide(policy: &Policy, ctx: &DecisionContext) -> Decision {
    let mut trace = Vec::new();

    // 1. Hard invariants first
    // Currency check
    if ctx.amount.currency != policy.task_cap.currency
        || ctx.amount.currency != policy.per_purchase_cap.currency
        || ctx.amount.currency != ctx.remaining_budget.currency
    {
        return Decision {
            verdict: Verdict::Deny,
            trace,
            policy_version: policy.version,
            explanation: format!(
                "Denied: currency mismatch (request {} vs policy {}) [hard rule H1]",
                ctx.amount.currency, policy.task_cap.currency
            ),
        };
    }

    // Price fully known check
    if !ctx.price_known || ctx.tax.is_none() || ctx.fee.is_none() {
        return Decision {
            verdict: Verdict::RequireApproval,
            trace,
            policy_version: policy.version,
            explanation: "Requires approval: price, tax or fee not fully known [hard rule H2]"
                .to_string(),
        };
    }

    // Expected total calculation and cap checks
    let total = match ctx.total_expected() {
        Ok(t) => t,
        Err(_) => {
            return Decision {
                verdict: Verdict::Deny,
                trace,
                policy_version: policy.version,
                explanation: "Denied: arithmetic error calculating expected total [hard rule H3]"
                    .to_string(),
            };
        }
    };

    if total.amount_minor > ctx.remaining_budget.amount_minor {
        return Decision {
            verdict: Verdict::Deny,
            trace,
            policy_version: policy.version,
            explanation: format!(
                "Denied: expected total {} exceeds remaining budget {} [hard rule H3]",
                total, ctx.remaining_budget
            ),
        };
    }

    if total.amount_minor > policy.per_purchase_cap.amount_minor {
        return Decision {
            verdict: Verdict::Deny,
            trace,
            policy_version: policy.version,
            explanation: format!(
                "Denied: expected total {} exceeds per-purchase cap {} [hard rule H4]",
                total, policy.per_purchase_cap
            ),
        };
    }

    if policy.autonomy == Autonomy::NoPaidAccess && ctx.amount.amount_minor > 0 {
        return Decision {
            verdict: Verdict::Deny,
            trace,
            policy_version: policy.version,
            explanation: "Denied: autonomy NoPaidAccess forbids paid resource [hard rule H5]"
                .to_string(),
        };
    }

    // 2. User rules next (order-independent)
    for rule in &policy.rules {
        if rule.when.evaluate(ctx) {
            trace.push(RuleHit {
                rule_id: rule.id.clone(),
                verdict: rule.then,
                reason: rule.reason.clone(),
            });
        }
    }

    // Determine verdict using most restrictive wins
    let mut has_allow = false;
    let mut has_require_approval = false;
    let mut has_deny = false;

    for hit in &trace {
        match hit.verdict {
            Verdict::Deny => has_deny = true,
            Verdict::RequireApproval => has_require_approval = true,
            Verdict::Allow => has_allow = true,
        }
    }

    let preliminary_verdict = if has_deny {
        Verdict::Deny
    } else if has_require_approval {
        Verdict::RequireApproval
    } else if has_allow {
        Verdict::Allow
    } else {
        // 3. Default is Deny
        Verdict::Deny
    };

    // 4. Autonomy AskEveryTime downgrades Allow to RequireApproval
    let (final_verdict, explanation) = match preliminary_verdict {
        Verdict::Allow => {
            if policy.autonomy == Autonomy::AskEveryTime {
                (
                    Verdict::RequireApproval,
                    "Requires approval: autonomy AskEveryTime requires human approval".to_string(),
                )
            } else {
                (Verdict::Allow, "Allowed by policy rules".to_string())
            }
        }
        Verdict::RequireApproval => (
            Verdict::RequireApproval,
            "Requires approval: matched rule requiring approval".to_string(),
        ),
        Verdict::Deny => {
            if has_deny {
                (
                    Verdict::Deny,
                    "Denied: matched rule denying access".to_string(),
                )
            } else {
                (
                    Verdict::Deny,
                    "Denied: default deny (no matching allow rule)".to_string(),
                )
            }
        }
    };

    Decision {
        verdict: final_verdict,
        trace,
        policy_version: policy.version,
        explanation,
    }
}
