//! The maths test files render whole: every formula is found and handed to KaTeX as a
//! span, and nothing is left half-parsed. (scripts/math-check.js then runs KaTeX on them.)

use std::path::Path;

#[test]
fn maths_files_are_fully_extracted() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/maths/should-render");
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let source = std::fs::read_to_string(&path).unwrap();
        let html = tilde_preview::markdown::to_html(&source, &dir, true);
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(!html.contains(tilde_preview::math::OPEN), "{name}: a token was left in the page");
        // Outside code, a `$$` left in the page is a display block that wasn't recognised.
        let text: String = html.split("<pre").map(|s| s.split_once("</pre>").map_or(s, |(_, after)| after)).collect();
        assert!(!text.contains("$$"), "{name}: a $$ block wasn't recognised");
        let spans = html.matches("class=\"math-").count();
        assert!(spans >= 30, "{name}: only {spans} formulas found");
    }
}
