//! Runs cases and keeps their samples.
//!
//! A case is one operation measured many times. Each sample records every counter in
//! [`Cost`]; very fast operations run several times per sample (calibrated so a sample
//! takes about `target_sample`) and are reported per operation. Warm-up runs come first
//! and aren't kept. Sampling stops once there are `min_samples` and `min_time` has
//! passed, or at `max_samples` / `max_time`.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::probe::{self, Cost, METRICS};
use crate::stats::{Summary, summarize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    pub name: String,
    pub warmup: u32,
    pub min_samples: u32,
    pub max_samples: u32,
    pub min_time: Duration,
    pub max_time: Duration,
    /// Samples of slow, one-shot operations (a full scan, loading an index).
    pub slow_samples: u32,
    /// Fast operations run repeatedly until a sample takes about this long.
    pub target_sample: Duration,
}

impl Profile {
    pub fn named(name: &str) -> Option<Profile> {
        let p = |warmup, min_samples, max_samples, min_ms, max_ms, slow_samples| Profile {
            name: name.into(),
            warmup,
            min_samples,
            max_samples,
            min_time: Duration::from_millis(min_ms),
            max_time: Duration::from_millis(max_ms),
            slow_samples,
            target_sample: Duration::from_millis(2),
        };
        match name {
            "quick" => Some(p(1, 5, 30, 200, 1_500, 2)),
            "standard" => Some(p(2, 15, 150, 600, 5_000, 4)),
            "thorough" => Some(p(3, 40, 500, 2_000, 15_000, 8)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CaseResult {
    /// `index.search`, `folder.list`…
    pub suite: String,
    /// What it ran on: `synthetic-100k`, `real`, `flat-10k`…
    pub dataset: String,
    pub name: String,
    /// Operations per sample (fast operations are batched).
    pub inner: u64,
    /// Per-operation costs, one per sample.
    pub samples: Vec<Cost>,
    pub summary: BTreeMap<String, Summary>,
    /// Single numbers about the case: item counts, bytes held, rates.
    pub info: BTreeMap<String, f64>,
    pub error: Option<String>,
}

impl CaseResult {
    pub fn id(&self) -> String {
        format!("{} / {} / {}", self.suite, self.dataset, self.name)
    }

    pub fn values(&self, metric: &str) -> Vec<f64> {
        self.samples.iter().map(|s| s.get(metric)).collect()
    }

    fn finish(&mut self) {
        for m in METRICS {
            let v = self.values(m);
            if v.iter().any(|x| *x != 0.0) {
                self.summary.insert((*m).to_string(), summarize(&v));
            }
        }
    }
}

pub struct Runner {
    pub profile: Profile,
    pub cases: Vec<CaseResult>,
    filter: Option<String>,
}

impl Runner {
    pub fn new(profile: Profile, filter: Option<String>) -> Runner {
        Runner { profile, cases: Vec::new(), filter }
    }

    /// Whether a case would run (so callers can skip expensive setup).
    pub fn wants(&self, suite: &str, dataset: &str, name: &str) -> bool {
        self.filter.as_ref().is_none_or(|f| format!("{suite} / {dataset} / {name}").contains(f.as_str()))
    }

    fn start(&self, suite: &str, dataset: &str, name: &str) -> CaseResult {
        CaseResult { suite: suite.into(), dataset: dataset.into(), name: name.into(), inner: 1, ..Default::default() }
    }

    fn done(&mut self, mut case: CaseResult) {
        case.finish();
        crate::report::print_case(&case);
        self.cases.push(case);
    }

    /// A fast operation, batched and calibrated. `f` returns an item count, kept as `items`.
    pub fn bench(&mut self, suite: &str, dataset: &str, name: &str, mut f: impl FnMut() -> usize) {
        if !self.wants(suite, dataset, name) {
            return;
        }
        let mut case = self.start(suite, dataset, name);
        let mut items = 0;
        for _ in 0..self.profile.warmup.max(1) {
            items = black_box(f());
        }
        // Calibrate: how many runs make a sample of about `target_sample`.
        let t = Instant::now();
        black_box(f());
        let one = t.elapsed().max(Duration::from_nanos(50));
        let inner = (self.profile.target_sample.as_nanos() / one.as_nanos()).clamp(1, 100_000) as u64;
        case.inner = inner;
        let started = Instant::now();
        loop {
            let before = probe::usage();
            for _ in 0..inner {
                black_box(f());
            }
            case.samples.push(probe::usage().since(&before).per(inner));
            if self.enough(case.samples.len(), started) {
                break;
            }
        }
        case.info.insert("items".into(), items as f64);
        self.done(case);
    }

    /// An operation that needs fresh state each time: `setup` runs unmeasured before each
    /// sample, then `f` once, measured. Returns the last item count.
    pub fn bench_setup<S>(
        &mut self,
        suite: &str,
        dataset: &str,
        name: &str,
        mut setup: impl FnMut() -> S,
        mut f: impl FnMut(S) -> usize,
    ) {
        if !self.wants(suite, dataset, name) {
            return;
        }
        let mut case = self.start(suite, dataset, name);
        for _ in 0..self.profile.warmup {
            let s = setup();
            black_box(f(s));
        }
        let mut items;
        let started = Instant::now();
        loop {
            let s = setup();
            let before = probe::usage();
            items = black_box(f(s));
            case.samples.push(probe::usage().since(&before));
            if self.enough(case.samples.len(), started) {
                break;
            }
        }
        case.info.insert("items".into(), items as f64);
        self.done(case);
    }

    /// A slow operation, measured `slow_samples` times (no warm-up). `f` returns extra
    /// numbers to keep (the last sample's).
    pub fn bench_slow<S>(
        &mut self,
        suite: &str,
        dataset: &str,
        name: &str,
        mut setup: impl FnMut() -> S,
        mut f: impl FnMut(S) -> Result<BTreeMap<String, f64>, String>,
    ) {
        if !self.wants(suite, dataset, name) {
            return;
        }
        let mut case = self.start(suite, dataset, name);
        for _ in 0..self.profile.slow_samples.max(1) {
            let s = setup();
            let held = crate::alloc::heap().live;
            crate::alloc::reset_peak();
            let before = probe::usage();
            match f(s) {
                Ok(info) => {
                    case.samples.push(probe::usage().since(&before));
                    case.info = info;
                    // The most extra heap held at once while it ran.
                    case.info.insert("peak_heap_bytes".into(), (crate::alloc::peak() - held) as f64);
                }
                Err(e) => {
                    case.error = Some(e);
                    break;
                }
            }
        }
        self.done(case);
    }

    /// Single numbers measured once (memory held, file sizes, internal counts).
    pub fn record(&mut self, suite: &str, dataset: &str, name: &str, info: BTreeMap<String, f64>) {
        if !self.wants(suite, dataset, name) {
            return;
        }
        let mut case = self.start(suite, dataset, name);
        case.info = info;
        self.done(case);
    }

    /// Latencies measured outside the usual loop (e.g. waiting for a file change to show
    /// up), one sample each.
    pub fn record_samples(
        &mut self,
        suite: &str,
        dataset: &str,
        name: &str,
        samples: Vec<Cost>,
        info: BTreeMap<String, f64>,
    ) {
        if !self.wants(suite, dataset, name) {
            return;
        }
        let mut case = self.start(suite, dataset, name);
        case.samples = samples;
        case.info = info;
        self.done(case);
    }

    pub fn fail(&mut self, suite: &str, dataset: &str, name: &str, error: String) {
        let mut case = self.start(suite, dataset, name);
        case.error = Some(error);
        self.done(case);
    }

    fn enough(&self, n: usize, started: Instant) -> bool {
        let p = &self.profile;
        let elapsed = started.elapsed();
        (n as u32 >= p.min_samples && elapsed >= p.min_time) || n as u32 >= p.max_samples || elapsed >= p.max_time
    }
}

/// Shorthand for an info map.
pub fn info<const N: usize>(pairs: [(&str, f64); N]) -> BTreeMap<String, f64> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}
