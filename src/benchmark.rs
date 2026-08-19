#[derive(Clone, Debug, PartialEq)]
pub struct Statistics {
    pub count: usize,
    pub minimum_ns: u128,
    pub maximum_ns: u128,
    pub median_ns: f64,
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

        Some(Self {
            count,
            minimum_ns: sorted[0],
            maximum_ns: sorted[count - 1],
            median_ns: median(&sorted),
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
        assert_eq!(statistics.variance_ns2, 0.0);
        assert_eq!(statistics.mean_ci95_low_ns, 42.0);
        assert_eq!(statistics.mean_ci95_high_ns, 42.0);
    }

    #[test]
    fn rejects_an_empty_sample_set() {
        assert!(Statistics::from_samples(&[]).is_none());
    }
}
