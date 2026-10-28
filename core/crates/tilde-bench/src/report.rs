//! Printing results as they come, saving a run, and comparing two runs.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::env::Environment;
use crate::runner::{CaseResult, Profile};
use crate::stats::ratio_ci;

pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema: u32,
    pub started: String,
    pub seconds: f64,
    pub label: String,
    pub args: Vec<String>,
    pub profile: Profile,
    pub env: Environment,
    pub warnings: Vec<String>,
    pub cases: Vec<CaseResult>,
}

pub fn time(ns: f64) -> String {
    match ns {
        n if !n.is_finite() => "-".into(),
        n if n < 1e3 => format!("{n:.0} ns"),
        n if n < 1e6 => format!("{:.1} µs", n / 1e3),
        n if n < 1e9 => format!("{:.2} ms", n / 1e6),
        n => format!("{:.2} s", n / 1e9),
    }
}

pub fn count(n: f64) -> String {
    match n.abs() {
        a if a >= 1e9 => format!("{:.2}G", n / 1e9),
        a if a >= 1e6 => format!("{:.2}M", n / 1e6),
        a if a >= 1e4 => format!("{:.1}k", n / 1e3),
        a if a >= 100.0 || n.fract() == 0.0 => format!("{n:.0}"),
        _ => format!("{n:.2}"),
    }
}

pub fn bytes(n: f64) -> String {
    match n.abs() {
        a if a >= 1e9 => format!("{:.2} GB", n / 1e9),
        a if a >= 1e6 => format!("{:.1} MB", n / 1e6),
        a if a >= 1e3 => format!("{:.1} KB", n / 1e3),
        _ => format!("{n:.0} B"),
    }
}

/// A number from `info`, formatted by what its name says it is.
fn info_value(key: &str, v: f64) -> String {
    if key.contains("bytes") {
        bytes(v)
    } else if key.ends_with("_ms") {
        format!("{v:.1} ms")
    } else if key.contains("share") {
        format!("{:.0}%", v * 100.0)
    } else {
        count(v)
    }
}

pub fn print_case(c: &CaseResult) {
    let head = format!("{:<9} {:<16} {}", c.suite, c.dataset, c.name);
    if let Some(e) = &c.error {
        println!("{head:<72}  FAILED: {e}");
        return;
    }
    let mut parts = Vec::new();
    if let Some(w) = c.summary.get("wall_ns") {
        parts.push(format!("p50 {:>9}  p90 {:>9}  ±{:>3.0}%  n={}", time(w.p50), time(w.p90), w.cv * 100.0, w.n));
    }
    if let Some(s) = c.summary.get("cpu_ns") {
        parts.push(format!("cpu {}", time(s.p50)));
    }
    if let Some(s) = c.summary.get("instructions") {
        parts.push(format!("{} instr", count(s.p50)));
    }
    if let Some(s) = c.summary.get("allocs") {
        parts.push(format!("{} allocs", count(s.p50)));
    }
    for (k, v) in &c.info {
        parts.push(format!("{k} {}", info_value(k, *v)));
    }
    println!("{head:<72}  {}", parts.join("  "));
}

pub fn save(report: &Report, path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(report)?)
}

pub fn load(path: &Path) -> Result<Report, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let report: Report = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if report.schema != SCHEMA {
        return Err(format!("{}: report format {} (this tool reads {SCHEMA})", path.display(), report.schema));
    }
    Ok(report)
}

/// Metrics compared with a bootstrap interval, and what "lower" means for them.
const COMPARED: &[(&str, &str)] = &[
    ("wall_ns", "time"),
    ("cpu_ns", "cpu"),
    ("instructions", "instructions"),
    ("allocs", "allocations"),
    ("alloc_bytes", "bytes allocated"),
    ("energy_nj", "energy"),
];

pub struct Change {
    pub case: String,
    pub metric: String,
    pub base: f64,
    pub new: f64,
    pub ratio: f64,
    pub ci: Option<(f64, f64)>,
    pub significant: bool,
}

/// Every metric of every case in both reports. A change is significant when its 95%
/// interval excludes no change and it's at least `threshold` (0.03 = 3%).
pub fn compare(base: &Report, new: &Report, threshold: f64) -> Vec<Change> {
    let by_id: BTreeMap<String, &CaseResult> = base.cases.iter().map(|c| (c.id(), c)).collect();
    let mut changes = Vec::new();
    for c in &new.cases {
        let Some(b) = by_id.get(&c.id()) else { continue };
        if c.error.is_some() || b.error.is_some() {
            continue;
        }
        for (metric, _) in COMPARED {
            let (bv, nv) = (b.values(metric), c.values(metric));
            if bv.iter().all(|x| *x == 0.0) || nv.iter().all(|x| *x == 0.0) {
                continue;
            }
            let Some((lo, ratio, hi)) = ratio_ci(&bv, &nv) else { continue };
            let significant = (lo > 1.0 || hi < 1.0) && (ratio - 1.0).abs() >= threshold;
            changes.push(Change {
                case: c.id(),
                metric: metric.to_string(),
                base: crate::stats::median(&bv),
                new: crate::stats::median(&nv),
                ratio,
                ci: Some((lo, hi)),
                significant,
            });
        }
        for (key, &nv) in &c.info {
            let Some(&bv) = b.info.get(key) else { continue };
            if bv == 0.0 || !(key.contains("bytes") || key == "dead" || key == "injected") {
                continue;
            }
            let ratio = nv / bv;
            changes.push(Change {
                case: c.id(),
                metric: key.clone(),
                base: bv,
                new: nv,
                ratio,
                ci: None,
                significant: (ratio - 1.0).abs() >= threshold.max(0.05),
            });
        }
    }
    changes
}

pub fn print_comparison(base: &Report, new: &Report, changes: &[Change], all: bool) {
    println!(
        "base: {} ({}, {}{})",
        base.started,
        base.env.commit,
        base.profile.name,
        if base.env.dirty { ", uncommitted changes" } else { "" }
    );
    println!(
        "new:  {} ({}, {}{})",
        new.started,
        new.env.commit,
        new.profile.name,
        if new.env.dirty { ", uncommitted changes" } else { "" }
    );
    let (b, n) = (&base.env, &new.env);
    for (what, x, y) in [
        ("machine", &b.machine, &n.machine),
        ("cpu", &b.cpu, &n.cpu),
        ("macOS", &b.macos, &n.macos),
        ("power", &b.power, &n.power),
        ("rustc", &b.rustc, &n.rustc),
    ] {
        if x != y {
            println!("  note: {what} differs ({x} → {y}): changes may not be the code's");
        }
    }
    if !n.warnings().is_empty() || !b.warnings().is_empty() {
        println!("  note: one of the runs had warnings: {:?} / {:?}", b.warnings(), n.warnings());
    }
    println!();
    let mut shown: Vec<&Change> = changes.iter().filter(|c| all || c.significant).collect();
    shown.sort_by(|a, b| (b.ratio - 1.0).abs().total_cmp(&(a.ratio - 1.0).abs()));
    let fmt = |metric: &str, v: f64| {
        if metric.ends_with("_ns") {
            time(v)
        } else if metric.contains("bytes") {
            bytes(v)
        } else {
            count(v)
        }
    };
    for c in &shown {
        let pct = (c.ratio - 1.0) * 100.0;
        let verdict = if !c.significant {
            "  same"
        } else if pct < 0.0 {
            "better"
        } else {
            " WORSE"
        };
        let ci =
            c.ci.map(|(lo, hi)| format!("[{:+.1}%, {:+.1}%]", (lo - 1.0) * 100.0, (hi - 1.0) * 100.0))
                .unwrap_or_default();
        println!(
            "{verdict}  {pct:>+7.1}%  {ci:<20} {:<16} {:>10} → {:<10}  {}",
            c.metric,
            fmt(&c.metric, c.base),
            fmt(&c.metric, c.new),
            c.case
        );
    }
    let significant = changes.iter().filter(|c| c.significant).count();
    println!(
        "\n{significant} significant changes of {} compared{}",
        changes.len(),
        if all { "" } else { " (--all shows every one)" }
    );
}
