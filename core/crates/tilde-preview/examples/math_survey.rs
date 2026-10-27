//! Parses every Markdown file under a folder the way the preview does (maths taken out
//! first, then the Markdown parser) and writes the
//! maths it finds as JSON lines (`{"file","display","tex"}`), for checking with KaTeX
//! (scripts/demo/math_survey.js). Also reports `$` left in plain text: maths the parser
//! didn't recognise.
//!
//!     cargo run -q -p tilde-preview --example math_survey -- <folder> > math.jsonl

use std::path::Path;

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
            continue;
        }
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

fn json(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn main() {
    let root = std::env::args().nth(1).expect("folder");
    let mut files = Vec::new();
    walk(Path::new(&root), &mut files);
    let (mut inline, mut display, mut stray) = (0, 0, 0);
    let mut stray_examples = Vec::new();
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let name = f.strip_prefix(&root).unwrap_or(f).to_string_lossy().into_owned();
        let extracted = tilde_preview::math::extract(&text);
        for m in &extracted.math {
            if m.display {
                display += 1
            } else {
                inline += 1
            }
            println!("{{\"file\":{},\"display\":{},\"tex\":{}}}", json(&name), m.display, json(&m.tex));
        }
        for (event, range) in tilde_preview::markdown::parse(&extracted.text).into_offset_iter() {
            use pulldown_cmark::Event;
            let d = matches!(event, Event::DisplayMath(_));
            match event {
                Event::InlineMath(m) | Event::DisplayMath(m) => {
                    if d {
                        display += 1
                    } else {
                        inline += 1
                    }
                    println!("{{\"file\":{},\"display\":{d},\"tex\":{}}}", json(&name), json(&m));
                }
                Event::Text(t) if t.contains('$') => {
                    stray += 1;
                    if stray_examples.len() < 40 {
                        let line = extracted.text[..range.start].matches('\n').count() + 1;
                        stray_examples.push(format!(
                            "{name}:~{line}: {}",
                            t.chars().take(100).collect::<String>().replace('\n', " ")
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    eprintln!("{} files: {inline} inline, {display} display, {stray} text pieces with a stray $", files.len());
    for e in stray_examples {
        eprintln!("  {e}");
    }
}
