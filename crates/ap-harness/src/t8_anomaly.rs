//! T8: Spend anomaly detector benchmark.
//! Synthetic series with injected spikes; reports precision and recall.

use ap_core::anomaly::compute_anomaly_score;

pub struct T8Result {
    pub samples: usize,
    pub precision: f64,
    pub recall: f64,
    pub pass: bool,
}

pub fn run_t8(seed: u64) -> T8Result {
    let mut rng_state = seed.wrapping_add(0x85EBCA6B);
    let mut next_u64 = || {
        rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
        rng_state
    };

    // 1. Build a stable baseline history of normal transactions (median ~ 1000 paise, MAD ~ 100 paise)
    let mut history = Vec::with_capacity(60);
    for _ in 0..60 {
        let jitter = (next_u64() % 300) as i64 - 150; // -150 to +150
        history.push(1000 + jitter);
    }

    let total_samples = 1000;
    let num_spikes = 50;

    let mut tp = 0;
    let mut fp = 0;
    let mut fn_count = 0;

    for i in 0..total_samples {
        let is_spike = i < num_spikes; // First 50 are injected spikes
        let amount = if is_spike {
            // Anomalous spike: 5,000 to 12,000 paise (5x to 12x normal)
            5000 + ((next_u64() % 7000) as i64)
        } else {
            // Normal transaction: 850 to 1150 paise
            850 + ((next_u64() % 300) as i64)
        };

        let score = compute_anomaly_score(&history, amount);
        let flagged = score >= 30; // 3.0 z-score threshold

        if is_spike && flagged {
            tp += 1;
        } else if !is_spike && flagged {
            fp += 1;
        } else if is_spike && !flagged {
            fn_count += 1;
        }
    }

    let precision = if tp + fp > 0 {
        tp as f64 / (tp + fp) as f64
    } else {
        1.0
    };
    let recall = if tp + fn_count > 0 {
        tp as f64 / (tp + fn_count) as f64
    } else {
        1.0
    };

    let pass = precision >= 0.95 && recall >= 0.95;

    T8Result {
        samples: total_samples,
        precision,
        recall,
        pass,
    }
}
