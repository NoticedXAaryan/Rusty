#[derive(Clone, Debug, PartialEq)]
pub struct Statistics {
    pub count: usize,
    pub minimum_ns: u128,
    pub maximum_ns: u128,
    pub median_ns: f64,
    pub median_ci95_low_ns: f64,
    pub median_ci95_high_ns: f64,
    pub mean_ns: f64,
    pub variance_ns2: f64,
    pub standard_deviation_ns: f64,
    pub mean_ci95_low_ns: f64,
    pub mean_ci95_high_ns: f64,
    pub tukey_outliers: usize,
}

impl Statistics {
    pub fn from_samples(samples: &[u128]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let count = sorted.len();
        let mean_ns = sorted.iter().map(|sample| *sample as f64).sum::<f64>() / count as f64;
        let variance_ns2 = if count > 1 {
            sorted
                .iter()
                .map(|sample| {
                    let difference = *sample as f64 - mean_ns;
                    difference * difference
                })
                .sum::<f64>()
                / (count - 1) as f64
        } else {
            0.0
        };
        let standard_deviation_ns = variance_ns2.sqrt();
        let margin = 1.96 * standard_deviation_ns / (count as f64).sqrt();
        let (q1, q3) = quartiles(&sorted);
        let iqr = q3 - q1;
        let lower_fence = q1 - 1.5 * iqr;
        let upper_fence = q3 + 1.5 * iqr;
        let tukey_outliers = sorted
            .iter()
            .filter(|sample| {
                let value = **sample as f64;
                value < lower_fence || value > upper_fence
            })
            .count();

        let (median_ci95_low_ns, median_ci95_high_ns) = bootstrap_median_interval(&sorted, 2_000);
        Some(Self {
            count,
            minimum_ns: sorted[0],
            maximum_ns: sorted[count - 1],
            median_ns: median(&sorted),
            median_ci95_low_ns,
            median_ci95_high_ns,
            mean_ns,
            variance_ns2,
            standard_deviation_ns,
            mean_ci95_low_ns: mean_ns - margin,
            mean_ci95_high_ns: mean_ns + margin,
            tukey_outliers,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Report {
    pub warmup_runs: usize,
    pub samples_ns: Vec<u128>,
    pub statistics: Statistics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Classification {
    NeuralWin,
    ReferenceWin,
    StatisticalTie,
    NeuralIncorrect,
    NeuralFailed,
    VerifierRejected,
    FallbackUsed,
}

impl Classification {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NeuralWin => "NEURAL_WIN",
            Self::ReferenceWin => "REFERENCE_WIN",
            Self::StatisticalTie => "STATISTICAL_TIE",
            Self::NeuralIncorrect => "NEURAL_INCORRECT",
            Self::NeuralFailed => "NEURAL_FAILED",
            Self::VerifierRejected => "VERIFIER_REJECTED",
            Self::FallbackUsed => "FALLBACK_USED",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    pub reference_median_ns: f64,
    pub candidate_median_ns: f64,
    pub speedup: f64,
    pub speedup_ci95_low: f64,
    pub speedup_ci95_high: f64,
    pub minimum_speedup: f64,
    pub classification: Classification,
    pub accepted: bool,
}

impl Comparison {
    pub fn from_samples(
        reference: &[u128],
        candidate: &[u128],
        minimum_speedup: f64,
    ) -> Option<Self> {
        if reference.is_empty()
            || candidate.is_empty()
            || !minimum_speedup.is_finite()
            || minimum_speedup < 0.0
        {
            return None;
        }
        let reference_median_ns = median_of(reference);
        let candidate_median_ns = median_of(candidate);
        if candidate_median_ns == 0.0 {
            return None;
        }
        let speedup = reference_median_ns / candidate_median_ns;
        let (speedup_ci95_low, speedup_ci95_high) =
            bootstrap_speedup_interval(reference, candidate, 2_000);
        let classification = if speedup_ci95_low > 1.0 {
            Classification::NeuralWin
        } else if speedup_ci95_high < 1.0 {
            Classification::ReferenceWin
        } else {
            Classification::StatisticalTie
        };
        let accepted = classification == Classification::NeuralWin
            && speedup_ci95_low >= 1.0 + minimum_speedup;
        Some(Self {
            reference_median_ns,
            candidate_median_ns,
            speedup,
            speedup_ci95_low,
            speedup_ci95_high,
            minimum_speedup,
            classification,
            accepted,
        })
    }
}

pub fn geometric_mean(values: &[f64]) -> Option<f64> {
    if values.is_empty()
        || values
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return None;
    }
    Some((values.iter().map(|value| value.ln()).sum::<f64>() / values.len() as f64).exp())
}

fn median(sorted: &[u128]) -> f64 {
    let midpoint = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[midpoint - 1] as f64 + sorted[midpoint] as f64) / 2.0
    } else {
        sorted[midpoint] as f64
    }
}

fn quartiles(sorted: &[u128]) -> (f64, f64) {
    if sorted.len() == 1 {
        let value = sorted[0] as f64;
        return (value, value);
    }
    let midpoint = sorted.len() / 2;
    let lower = &sorted[..midpoint];
    let upper = if sorted.len().is_multiple_of(2) {
        &sorted[midpoint..]
    } else {
        &sorted[midpoint + 1..]
    };
    (median(lower), median(upper))
}

fn median_of(samples: &[u128]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    median(&sorted)
}

fn bootstrap_median_interval(samples: &[u128], iterations: usize) -> (f64, f64) {
    let mut state = 0x4d45_4449_414e_4349_u64;
    let mut medians = Vec::with_capacity(iterations);
    let mut sample = vec![0; samples.len()];
    for _ in 0..iterations {
        for value in &mut sample {
            *value = samples[random_index(&mut state, samples.len())];
        }
        sample.sort_unstable();
        medians.push(median(&sample));
    }
    medians.sort_by(f64::total_cmp);
    percentile_interval(&medians)
}

fn bootstrap_speedup_interval(
    reference: &[u128],
    candidate: &[u128],
    iterations: usize,
) -> (f64, f64) {
    let mut state = 0x5255_5354_5942_454e_u64;
    let mut ratios = Vec::with_capacity(iterations);
    let mut reference_sample = vec![0; reference.len()];
    let mut candidate_sample = vec![0; candidate.len()];
    for _ in 0..iterations {
        for sample in &mut reference_sample {
            *sample = reference[random_index(&mut state, reference.len())];
        }
        for sample in &mut candidate_sample {
            *sample = candidate[random_index(&mut state, candidate.len())];
        }
        reference_sample.sort_unstable();
        candidate_sample.sort_unstable();
        let denominator = median(&candidate_sample);
        ratios.push(if denominator == 0.0 {
            f64::INFINITY
        } else {
            median(&reference_sample) / denominator
        });
    }
    ratios.sort_by(f64::total_cmp);
    percentile_interval(&ratios)
}

fn percentile_interval(sorted: &[f64]) -> (f64, f64) {
    let iterations = sorted.len();
    let low = ((iterations as f64 * 0.025).floor() as usize).min(iterations - 1);
    let high = ((iterations as f64 * 0.975).ceil() as usize).min(iterations - 1);
    (sorted[low], sorted[high])
}

fn random_index(state: &mut u64, upper: usize) -> usize {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state as usize) % upper
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_sample_statistics_without_discarding_outliers() {
        let statistics = Statistics::from_samples(&[10, 11, 12, 13, 14, 15, 100]).unwrap();
        assert_eq!(statistics.count, 7);
        assert_eq!(statistics.minimum_ns, 10);
        assert_eq!(statistics.maximum_ns, 100);
        assert_eq!(statistics.median_ns, 13.0);
        assert_eq!(statistics.mean_ns, 25.0);
        assert_eq!(statistics.tukey_outliers, 1);
    }

    #[test]
    fn supports_a_single_sample() {
        let statistics = Statistics::from_samples(&[42]).unwrap();
        assert_eq!(statistics.median_ns, 42.0);
        assert_eq!(statistics.median_ci95_low_ns, 42.0);
        assert_eq!(statistics.median_ci95_high_ns, 42.0);
        assert_eq!(statistics.variance_ns2, 0.0);
        assert_eq!(statistics.mean_ci95_low_ns, 42.0);
        assert_eq!(statistics.mean_ci95_high_ns, 42.0);
    }

    #[test]
    fn rejects_an_empty_sample_set() {
        assert!(Statistics::from_samples(&[]).is_none());
    }

    #[test]
    fn classifies_clear_wins_and_applies_the_gate() {
        let comparison =
            Comparison::from_samples(&[200, 201, 202, 203, 204], &[100, 101, 102, 103, 104], 0.05)
                .unwrap();
        assert_eq!(comparison.classification, Classification::NeuralWin);
        assert!(comparison.accepted);
        assert!(comparison.speedup_ci95_low > 1.05);
    }

    #[test]
    fn classifies_overlapping_distributions_as_a_tie() {
        let comparison =
            Comparison::from_samples(&[100, 101, 102, 103, 104], &[99, 101, 102, 104, 105], 0.0)
                .unwrap();
        assert_eq!(comparison.classification, Classification::StatisticalTie);
        assert!(!comparison.accepted);
    }

    #[test]
    fn computes_geometric_mean_for_relative_results() {
        let value = geometric_mean(&[2.0, 0.5, 1.0]).unwrap();
        assert!((value - 1.0).abs() < 1e-12);
        assert!(geometric_mean(&[1.0, 0.0]).is_none());
    }
}
