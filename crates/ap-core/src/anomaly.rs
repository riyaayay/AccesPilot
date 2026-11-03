//! Pure robust spend anomaly detection using Median and Median Absolute Deviation (MAD).

/// Computes robust z-score multiplied by 10 (e.g. 30 = 3.0 z-score) using median and MAD.
/// Returns 0 if there are insufficient data points or no variance.
pub fn compute_anomaly_score(history: &[i64], current: i64) -> i64 {
    if history.len() < 5 {
        return 0;
    }

    let mut sorted = history.to_vec();
    sorted.sort_unstable();

    let median = median_of_sorted(&sorted);

    // Compute absolute deviations from median
    let mut deviations: Vec<i64> = sorted.iter().map(|&x| (x - median).abs()).collect();
    deviations.sort_unstable();

    let mad = median_of_sorted(&deviations);
    if mad == 0 {
        if current > median {
            return 50; // Spiked with zero baseline variance
        }
        return 0;
    }

    // z = (current - median) / (1.4826 * MAD)
    // Scale by 10 for integer math: (current - median) * 10 * 10000 / (14826 * MAD)
    let diff = current - median;
    if diff <= 0 {
        return 0;
    }

    let numerator = diff.saturating_mul(100_000);
    let denominator = mad.saturating_mul(14_826);

    if denominator == 0 {
        0
    } else {
        numerator / denominator
    }
}

fn median_of_sorted(sorted: &[i64]) -> i64 {
    let n = sorted.len();
    if n == 0 {
        return 0;
    }
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[(n / 2) - 1] + sorted[n / 2]) / 2
    }
}
