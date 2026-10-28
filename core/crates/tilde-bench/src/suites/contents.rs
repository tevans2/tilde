//! Searching inside files (`text:`): scanning a tree of text files, and one big file.

use std::sync::atomic::{AtomicU64, Ordering};

use tilde_contents::{Needle, ScanOptions, scan, search_file};

use super::Ctx;
use crate::data;
use crate::runner::{Runner, info};

const SUITE: &str = "contents";

pub fn run(r: &mut Runner, ctx: &Ctx) {
    let ds = format!("text-{}", data::label(ctx.text_files));
    let root = data::text_tree(ctx.text_files);
    let bytes = AtomicU64::new(0);
    let options = ScanOptions::default();
    for (label, text) in
        [("rare word", "warpdrive"), ("common word", "report"), ("absent", "zzqxjv"), ("capitalised", "Report")]
    {
        let Some(needle) = Needle::new(text) else { continue };
        r.bench_slow(
            SUITE,
            &ds,
            &format!("scan: {label}"),
            || (),
            |_| {
                let mut hits = 0usize;
                let t = std::time::Instant::now();
                let end = scan(&root, &needle, &options, &|| false, &mut |_, _| hits += 1);
                Ok(info([
                    ("files", end.files as f64),
                    ("hits", hits as f64),
                    ("files_per_sec", end.files as f64 / t.elapsed().as_secs_f64()),
                ]))
            },
        );
    }
    // One 2 MB file (the size limit), the match near the end.
    let big = ctx.scratch.join("big.txt");
    let mut body = String::new();
    let mut rng = data::Rng::new(7);
    while body.len() < 2_000_000 {
        body.push_str(rng.pick(data::WORDS));
        body.push(if rng.chance(0.1) { '\n' } else { ' ' });
    }
    body.push_str("the warpdrive\n");
    bytes.store(body.len() as u64, Ordering::Relaxed);
    std::fs::write(&big, body).expect("write big file");
    let needle = Needle::new("warpdrive").expect("needle");
    r.bench(SUITE, "file-2mb", "search one file", || {
        search_file(&big, &needle, 4 << 20).map_or(0, |m| m.count as usize)
    });
    r.record(SUITE, "file-2mb", "file", info([("bytes", bytes.load(Ordering::Relaxed) as f64)]));
}
