//! Ranking quality: the relevance cases from `tilde_index::relevance`, each recorded as
//! passed (1) or not (0), and the share passed. Not timed.

use super::Ctx;
use crate::runner::{Runner, info};

const SUITE: &str = "relevance";

pub fn run(r: &mut Runner, ctx: &Ctx) {
    let dir = tempfile::tempdir_in(&ctx.scratch).expect("scratch folder");
    let mut index = tilde_index::relevance::start(dir.path());
    let outcomes = tilde_index::relevance::check(&mut index);
    let passed = outcomes.iter().filter(|o| o.passed).count();
    for o in &outcomes {
        r.record(
            SUITE,
            "fixture",
            &format!("\"{}\": {}", o.case.query, o.case.why),
            info([("passed", o.passed as u8 as f64)]),
        );
        if !o.passed {
            eprintln!("  relevance: \"{}\" ({}) got {:?}", o.case.query, o.case.why, o.top);
        }
    }
    r.record(
        SUITE,
        "fixture",
        "cases passed",
        info([
            ("passed", passed as f64),
            ("cases", outcomes.len() as f64),
            ("share", passed as f64 / outcomes.len() as f64),
        ]),
    );
}
