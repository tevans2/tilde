//! Times a scan: `cargo run --release -p tilde-contents --example scan -- <folder> <text>`.

use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(root), Some(text)) = (args.next(), args.next()) else {
        eprintln!("usage: scan <folder> <text>");
        std::process::exit(2);
    };
    let needle = tilde_contents::Needle::new(&text).expect("text");
    let options = tilde_contents::ScanOptions {
        skip: ["node_modules", "target", "DerivedData", ".venv", "venv", "build", ".gradle", "Pods", ".next", "dist"]
            .map(String::from)
            .to_vec(),
        ..Default::default()
    };
    let start = Instant::now();
    let mut hits = 0;
    let mut first = None;
    let end = tilde_contents::scan(root.as_ref(), &needle, &options, &|| false, &mut |_, _| {
        hits += 1;
        first.get_or_insert(start.elapsed());
    });
    println!(
        "{} files, {hits} hits, first after {:?}, done in {:?}{}",
        end.files,
        first.unwrap_or_default(),
        start.elapsed(),
        if end.truncated { " (stopped at the limit)" } else { "" }
    );
}
