//! JSON result schema for `run` / `compare`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunReport {
    pub label: String,
    pub engine: String,
    pub scenario: String,
    pub started_unix_ms: u64,
    pub finished_unix_ms: u64,
    pub repeats: u32,
    pub metrics: BTreeMap<String, MetricSeries>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricSeries {
    /// Unit label, e.g. "MiB/s", "ms", "KiB/s", "bytes", "seconds".
    pub unit: String,
    /// One sample per repeat (already the per-run summary, e.g. one throughput).
    pub samples: Vec<f64>,
    pub median: f64,
    pub p90: f64,
    pub max: f64,
    /// Higher is better (throughput) vs lower is better (latency).
    pub higher_is_better: bool,
}

impl MetricSeries {
    pub fn from_samples(unit: &str, mut samples: Vec<f64>, higher_is_better: bool) -> Self {
        samples.retain(|v| v.is_finite());
        let mut sorted = samples.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = quantile(&sorted, 0.5);
        let p90 = quantile(&sorted, 0.9);
        let max = sorted.last().copied().unwrap_or(0.0);
        Self {
            unit: unit.to_string(),
            samples,
            median,
            p90,
            max,
            higher_is_better,
        }
    }
}

pub fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((q * (sorted.len() as f64 - 1.0)).round() as usize).min(sorted.len() - 1);
    sorted[idx]
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompareRow {
    pub metric: String,
    pub unit: String,
    pub go_median: f64,
    pub rust_median: f64,
    /// rust / go for higher-is-better; go / rust for lower-is-better. >= 1 means rust wins.
    pub rust_advantage: f64,
    pub pass: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompareReport {
    pub go_label: String,
    pub rust_label: String,
    pub scenario: String,
    pub noise_tolerance: f64,
    pub rows: Vec<CompareRow>,
    pub pass: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn samples_preserve_repeat_order() {
        let s = MetricSeries::from_samples("ms", vec![10.0, 2.0, 4.0], false);
        assert_eq!(s.samples, vec![10.0, 2.0, 4.0]);
        assert_eq!(s.median, 4.0);
    }
}
