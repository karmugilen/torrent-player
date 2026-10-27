//! Compare two RunReport JSON files (Go vs Rust).

use crate::report::{CompareReport, CompareRow, RunReport};
use std::collections::BTreeSet;
use std::path::Path;

/// Default: Rust may be up to 5% worse and still PASS (measurement noise).
pub const DEFAULT_NOISE: f64 = 0.05;

pub fn compare_files(
    go_path: &Path,
    rust_path: &Path,
    noise: f64,
) -> Result<CompareReport, String> {
    let go: RunReport =
        serde_json::from_str(&std::fs::read_to_string(go_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let rust: RunReport =
        serde_json::from_str(&std::fs::read_to_string(rust_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    Ok(compare_reports(&go, &rust, noise))
}

pub fn compare_reports(go: &RunReport, rust: &RunReport, noise: f64) -> CompareReport {
    let keys: BTreeSet<&String> = go.metrics.keys().chain(rust.metrics.keys()).collect();
    let mut rows = Vec::new();
    for key in keys {
        let (Some(g), Some(r)) = (go.metrics.get(key), rust.metrics.get(key)) else {
            continue;
        };
        let (advantage, pass) = if g.higher_is_better {
            // rust/go >= 1 - noise
            let adv = if g.median <= 0.0 {
                if r.median >= g.median {
                    1.0
                } else {
                    0.0
                }
            } else {
                r.median / g.median
            };
            (adv, adv + 1e-12 >= 1.0 - noise)
        } else {
            // lower is better: advantage = go/rust
            let adv = if r.median <= 0.0 {
                if g.median >= r.median {
                    1.0
                } else {
                    0.0
                }
            } else {
                g.median / r.median
            };
            (adv, adv + 1e-12 >= 1.0 - noise)
        };
        rows.push(CompareRow {
            metric: key.clone(),
            unit: g.unit.clone(),
            go_median: g.median,
            rust_median: r.median,
            rust_advantage: advantage,
            pass,
        });
    }
    let pass = !rows.is_empty() && rows.iter().all(|r| r.pass);
    CompareReport {
        go_label: go.label.clone(),
        rust_label: rust.label.clone(),
        scenario: format!("{}/{}", go.scenario, rust.scenario),
        noise_tolerance: noise,
        rows,
        pass,
    }
}

pub fn print_table(report: &CompareReport) {
    println!(
        "compare {} vs {} (noise tol {:.0}%)  overall={}",
        report.go_label,
        report.rust_label,
        report.noise_tolerance * 100.0,
        if report.pass { "PASS" } else { "FAIL" }
    );
    println!(
        "{:<36} {:>12} {:>12} {:>10} {:>6}",
        "metric", "go", "rust", "adv", "ok"
    );
    for row in &report.rows {
        println!(
            "{:<36} {:>10.3}{:<2} {:>10.3}{:<2} {:>9.3} {:>6}",
            row.metric,
            row.go_median,
            row.unit,
            row.rust_median,
            row.unit,
            row.rust_advantage,
            if row.pass { "PASS" } else { "FAIL" }
        );
    }
}
