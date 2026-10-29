//! Measures Tilde's core. See bench/README.md.
//!
//!   cargo run --release -p tilde-bench -- [run] [options]
//!   cargo run --release -p tilde-bench -- compare [base.json] [new.json]
//!   cargo run --release -p tilde-bench -- list

mod alloc;
mod data;
mod env;
mod probe;
mod report;
mod runner;
mod stats;
mod suites;

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use report::Report;
use runner::{Profile, Runner};
use suites::Ctx;

#[global_allocator]
static ALLOC: alloc::Counting = alloc::Counting;

const HELP: &str = "\
tilde-bench: measures Tilde's core (see bench/README.md)

  tilde-bench [run] [options]
    --profile P       quick | standard (default) | thorough: how many samples
    --suite LIST      comma-separated suites (default: all); `tilde-bench list` names them
    --sizes LIST      synthetic index sizes, like 100k,1m (index suite)
    --trees LIST      synthetic homes written to disk, like 20k,100k (scan, sizes, engine)
    --live N          items in the tree the live suite edits
    --real            also measure your own index (a copy; nothing of yours is changed)
    --real-heavy      also scan your home and measure folder sizes there (read-only, slow)
    --filter TEXT     only cases whose `suite / data / name` contains TEXT
    --label NAME      added to the results file's name
    --out DIR         where results go (default: bench/results)
    --baseline        also save the run as bench/baseline.json
    --no-save         don't write a results file

  tilde-bench compare [BASE.json] [NEW.json] [--all] [--threshold PCT] [--fail-above PCT]
    with no files: bench/baseline.json against the newest file in bench/results

  tilde-bench list";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().expect("repository folder")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("compare") => compare(&args[1..]),
        Some("list") => {
            for (name, what) in suites::ALL {
                println!("{name:<10} {what}");
            }
            ExitCode::SUCCESS
        }
        Some("-h" | "--help" | "help") => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("run") => run(&args[1..]),
        _ => run(&args),
    }
}

fn counts(value: &str) -> Result<Vec<usize>, String> {
    value.split(',').map(|s| data::parse_count(s).ok_or(format!("not a count: {s}"))).collect()
}

fn run(args: &[String]) -> ExitCode {
    let mut profile = Profile::named("standard").unwrap();
    let mut chosen: Option<Vec<String>> = None;
    let (mut sizes, mut trees, mut live) = (None, None, None);
    let (mut real, mut real_heavy, mut save, mut baseline) = (false, false, true, false);
    let (mut filter, mut label, mut out) = (None, String::new(), repo().join("bench/results"));
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{a} needs a value"));
        let r: Result<(), String> = (|| {
            match a.as_str() {
                "--profile" => {
                    let v = value()?;
                    profile = Profile::named(&v).ok_or(format!("no profile {v}"))?;
                }
                "--suite" => chosen = Some(value()?.split(',').map(String::from).collect()),
                "--sizes" => sizes = Some(counts(&value()?)?),
                "--trees" => trees = Some(counts(&value()?)?),
                "--live" => live = Some(data::parse_count(&value()?).ok_or("--live needs a count")?),
                "--real" => real = true,
                "--real-heavy" => {
                    real = true;
                    real_heavy = true;
                }
                "--filter" => filter = Some(value()?),
                "--label" => label = value()?,
                "--out" => out = PathBuf::from(value()?),
                "--baseline" => baseline = true,
                "--no-save" => save = false,
                "-h" | "--help" => return Err(HELP.into()),
                other => return Err(format!("unknown option {other}\n\n{HELP}")),
            }
            Ok(())
        })();
        if let Err(e) = r {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    }
    let known: Vec<&str> = suites::ALL.iter().map(|(n, _)| *n).collect();
    let chosen = chosen.unwrap_or_else(|| known.iter().map(|s| s.to_string()).collect());
    if let Some(bad) = chosen.iter().find(|s| !known.contains(&s.as_str())) {
        eprintln!("no suite {bad} (try `tilde-bench list`)");
        return ExitCode::FAILURE;
    }

    let quick = profile.name == "quick";
    let thorough = profile.name == "thorough";
    let scratch = tempfile::tempdir().expect("scratch folder");
    let ctx = Ctx {
        sizes: sizes.unwrap_or(if quick {
            vec![50_000]
        } else if thorough {
            vec![100_000, 500_000, 1_000_000]
        } else {
            vec![100_000, 500_000]
        }),
        trees: trees.unwrap_or(if quick { vec![20_000] } else { vec![100_000] }),
        live: live.unwrap_or(if quick { 10_000 } else { 30_000 }),
        text_files: if quick { 1_000 } else { 5_000 },
        archive_members: if quick { 500 } else { 2_000 },
        real,
        real_heavy,
        scratch: scratch.path().canonicalize().expect("scratch folder"),
    };

    // Measure on the performance cores, as an app in front would run.
    // SAFETY: only changes this thread's scheduling class.
    unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0) };

    let mut env = env::capture(&repo());
    let warnings = env.warnings();
    println!(
        "tilde-bench · {} profile · {} · {} · commit {}{}",
        profile.name,
        env.cpu,
        env.macos,
        env.commit,
        if env.dirty { " (uncommitted changes)" } else { "" }
    );
    for w in &warnings {
        println!("  warning: {w}");
    }
    println!();

    let started = local_time();
    let t = Instant::now();
    let mut runner = Runner::new(profile.clone(), filter);
    for name in &chosen {
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| match name.as_str() {
            "index" => suites::index::run(&mut runner, &ctx),
            "live" => suites::live::run(&mut runner, &ctx),
            "scan" => suites::scan::run(&mut runner, &ctx),
            "folder" => suites::folder::run(&mut runner, &ctx),
            "sizes" => suites::sizes::run(&mut runner, &ctx),
            "relevance" => suites::relevance::run(&mut runner, &ctx),
            "contents" => suites::contents::run(&mut runner, &ctx),
            "archive" => suites::archive::run(&mut runner, &ctx),
            "engine" => suites::engine::run(&mut runner, &ctx),
            _ => unreachable!(),
        }));
        if let Err(panic) = result {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "panicked".into());
            runner.fail(name, "-", "suite", message);
        }
    }
    env.load_after = env::load_average();

    let report = Report {
        schema: report::SCHEMA,
        started: started.clone(),
        seconds: t.elapsed().as_secs_f64(),
        label: label.clone(),
        args: args.to_vec(),
        profile,
        warnings,
        env,
        cases: runner.cases,
    };
    let failed = report.cases.iter().filter(|c| c.error.is_some()).count();
    println!(
        "\n{} cases in {:.0} s{}",
        report.cases.len(),
        report.seconds,
        if failed > 0 { format!(", {failed} failed") } else { String::new() }
    );
    if save {
        let stamp = started.replace([':', ' '], "").replace('-', "");
        let name = format!(
            "{stamp}-{}{}.json",
            report.env.commit,
            if label.is_empty() { String::new() } else { format!("-{label}") }
        );
        let path = out.join(name);
        match report::save(&report, &path) {
            Ok(()) => println!("saved {}", path.display()),
            Err(e) => eprintln!("couldn't save {}: {e}", path.display()),
        }
        if baseline {
            let path = repo().join("bench/baseline.json");
            match report::save(&report, &path) {
                Ok(()) => println!("saved {}", path.display()),
                Err(e) => eprintln!("couldn't save {}: {e}", path.display()),
            }
        }
    }
    if failed > 0 { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

fn compare(args: &[String]) -> ExitCode {
    let (mut files, mut all, mut threshold, mut fail_above) = (Vec::new(), false, 3.0, None::<f64>);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--all" => all = true,
            "--threshold" => threshold = it.next().and_then(|v| v.parse().ok()).unwrap_or(threshold),
            "--fail-above" => fail_above = it.next().and_then(|v| v.parse().ok()),
            f => files.push(PathBuf::from(f)),
        }
    }
    let results = repo().join("bench/results");
    let newest = || {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&results)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        entries.sort();
        entries.pop()
    };
    let (base, new) = match files.len() {
        0 => (repo().join("bench/baseline.json"), newest()),
        1 => (repo().join("bench/baseline.json"), Some(files[0].clone())),
        _ => (files[0].clone(), Some(files[1].clone())),
    };
    let Some(new) = new else {
        eprintln!("no results in {}", results.display());
        return ExitCode::FAILURE;
    };
    let (base, new) = match (report::load(&base), report::load(&new)) {
        (Ok(b), Ok(n)) => (b, n),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let changes = report::compare(&base, &new, threshold / 100.0);
    report::print_comparison(&base, &new, &changes, all);
    if let Some(limit) = fail_above {
        let worst = changes
            .iter()
            .filter(|c| c.significant && c.metric == "wall_ns")
            .map(|c| (c.ratio - 1.0) * 100.0)
            .fold(f64::MIN, f64::max);
        if worst > limit {
            eprintln!("slower by {worst:.1}% somewhere (limit {limit}%)");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// Local time as `2026-10-28 17:45:03`.
fn local_time() -> String {
    // SAFETY: plain C time functions on stack values.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}
