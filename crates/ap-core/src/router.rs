//! Pure provider routing and offer ranking.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouterWeights {
    pub w_price: u32,
    pub w_fail: u32,
    pub w_latency: u32,
    pub w_data_age: u32,
}

impl Default for RouterWeights {
    fn default() -> Self {
        Self {
            w_price: 40,
            w_fail: 30,
            w_latency: 20,
            w_data_age: 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderOfferCandidate {
    pub provider_id: String,
    pub amount_minor: i64,
    pub success_rate_basis_points: u32, // 10000 = 100%
    pub p95_latency_ms: u32,
    pub data_age_secs: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankedOffer {
    pub provider_id: String,
    pub score: u64, // Lower score is better
    pub explanation: String,
}

/// Pure ranking function: calculates composite score and explains the choice.
pub fn rank_offers(
    candidates: &[ProviderOfferCandidate],
    weights: &RouterWeights,
) -> Vec<RankedOffer> {
    if candidates.is_empty() {
        return Vec::new();
    }

    let mut ranked: Vec<RankedOffer> = candidates
        .iter()
        .map(|c| {
            // Price term
            let price_term = (c.amount_minor.max(0) as u64) * (weights.w_price as u64);

            // Failure term: (10000 - success_rate) * w_fail
            let fail_bp = 10000u64.saturating_sub(c.success_rate_basis_points as u64);
            let fail_term = fail_bp * (weights.w_fail as u64);

            // Latency term
            let lat_term = (c.p95_latency_ms as u64) * (weights.w_latency as u64);

            // Data age term
            let age_term = (c.data_age_secs as u64) * (weights.w_data_age as u64);

            let score = price_term + fail_term + lat_term + age_term;

            let explanation = format!(
                "Score {score}: price_term={price_term}, fail_term={fail_term}, lat_term={lat_term}, age_term={age_term}"
            );

            RankedOffer {
                provider_id: c.provider_id.clone(),
                score,
                explanation,
            }
        })
        .collect();

    ranked.sort_by_key(|r| r.score);
    ranked
}
