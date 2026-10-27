//! Finds the maths in a Markdown document before the Markdown parser sees it, the way
//! Obsidian, GitHub and LaTeX-minded writers expect, and swaps each formula for a token
//! the parser leaves alone (`\u{E000}<n>\u{E001}`, private-use characters).
//!
//! Display maths is the fragile part: a `$$` block may contain blank lines, or lines
//! starting with `-`, `+`, `>` or `#`, which Markdown would read as new paragraphs, lists,
//! quotes or headings. Taken out first, it stays whole. Recognised:
//!
//! - `$$ … $$` blocks over several lines (also in quotes and lists, and opened at the end
//!   of a line of text: `as an array:$$`)
//! - `\[` and `\]` on lines of their own (inline `\[TODO\]` is Markdown escaping, not maths)
//! - `\( … \)` on one line, when it looks like maths (a space inside the brackets, or one of
//!   `\ ^ _ {`), so `\(note\)` stays text
//! - GitHub's `` $`…`$ ``
//!
//! - `$…$` and `$$…$$` within a line, by Obsidian's rules: no space just after the opening
//!   `$`, and no digit straight after the closing one, so `$5 and $10` stays text (a space
//!   before the closing `$` is fine: `$x = $ text`); a `$` escaped as `\$` is never a
//!   delimiter. A formula can't run across a code span, so `` `$x$` `` stays code. In
//!   table rows a `|` inside the maths doesn't split the cell.
//!
//! Code blocks and code spans are never touched.

pub const OPEN: char = '\u{E000}';
pub const CLOSE: char = '\u{E001}';

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Math {
    pub tex: String,
    pub display: bool,
}

#[derive(Debug, Default)]
pub struct Extracted {
    /// The document with tokens in place of the maths taken out.
    pub text: String,
    pub math: Vec<Math>,
}

/// Text, or the maths a token stands for.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(&'a str),
    Math(usize),
}

/// Splits text the parser produced into plain text and tokens.
pub fn pieces(text: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(OPEN) {
        let after = &rest[i + OPEN.len_utf8()..];
        let Some(j) = after.find(CLOSE) else { break };
        let Ok(n) = after[..j].parse() else { break };
        if i > 0 {
            out.push(Piece::Text(&rest[..i]));
        }
        out.push(Piece::Math(n));
        rest = &after[j + CLOSE.len_utf8()..];
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest));
    }
    out
}

fn token(math: &mut Vec<Math>, tex: String, display: bool) -> String {
    math.push(Math { tex, display });
    format!("{OPEN}{}{CLOSE}", math.len() - 1)
}

/// A line's indentation and quote markers (`  > > `), and the rest.
fn split_prefix(line: &str) -> (&str, &str) {
    let end = line.find(|c: char| !matches!(c, ' ' | '\t' | '>')).unwrap_or(line.len());
    line.split_at(end)
}

/// Removes up to `depth` quote markers (and the spaces around them) from a line inside a
/// block that opened in a quote.
fn strip_quotes(line: &str, depth: usize) -> &str {
    let mut rest = line;
    for _ in 0..depth {
        let t = rest.trim_start_matches([' ', '\t']);
        match t.strip_prefix('>') {
            Some(r) => rest = r.strip_prefix(' ').unwrap_or(r),
            None => break,
        }
    }
    rest
}

/// A fence opening a code block: its character and length.
pub(crate) fn fence_open(trimmed: &str) -> Option<(char, usize)> {
    let c = trimmed.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = trimmed.chars().take_while(|x| *x == c).count();
    (n >= 3).then_some((c, n))
}

pub(crate) fn fence_close(trimmed: &str, c: char, n: usize) -> bool {
    let t = trimmed.trim_end();
    t.chars().take_while(|x| *x == c).count() >= n && t.chars().all(|x| x == c)
}

pub fn extract(source: &str) -> Extracted {
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut out = String::with_capacity(source.len());
    let mut math = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let (prefix, body) = split_prefix(line);
        let trimmed = body.trim_end();
        if let Some((c, n)) = fence {
            if fence_close(body, c, n) {
                fence = None;
            }
            out.push_str(line);
            i += 1;
            continue;
        }
        if let Some(f) = fence_open(body) {
            fence = Some(f);
            out.push_str(line);
            i += 1;
            continue;
        }
        let depth = prefix.matches('>').count();
        let newline = if line.ends_with('\n') { "\n" } else { "" };

        // `$$` opening a block over several lines (one line is left to the parser).
        if let Some(rest) = body.strip_prefix("$$")
            && !rest.contains("$$")
            && let Some((tex, close, after)) =
                block_until(&lines, i + 1, depth, rest, |b| b.find("$$").map(|p| (p, p + 2)))
        {
            out.push_str(prefix);
            out.push_str(&token(&mut math, tex, true));
            out.push_str(after.trim_end_matches('\n'));
            out.push('\n');
            i = close + 1;
            continue;
        }
        // A line of text ending in `$$` that opens a block (`as an array:$$`).
        if let Some(before) = trimmed.strip_suffix("$$")
            && !before.is_empty()
            && !before.contains("$$")
            && let Some((tex, close, after)) =
                block_until(&lines, i + 1, depth, "", |b| b.find("$$").map(|p| (p, p + 2)))
        {
            out.push_str(prefix);
            out.push_str(&inline(before, &mut math));
            out.push_str(&token(&mut math, tex, true));
            out.push_str(after.trim_end_matches('\n'));
            out.push('\n');
            i = close + 1;
            continue;
        }
        // `\[` alone on its line, closed by `\]` alone on a later one.
        if trimmed == "\\[" {
            let closing = |b: &str| (b.trim() == "\\]").then_some((0, b.len()));
            if let Some((tex, close, _)) = block_until(&lines, i + 1, depth, "", closing) {
                out.push_str(prefix);
                out.push_str(&token(&mut math, tex, true));
                out.push('\n');
                i = close + 1;
                continue;
            }
        }
        out.push_str(&inline(line.strip_suffix('\n').unwrap_or(line), &mut math));
        out.push_str(newline);
        i += 1;
    }
    Extracted { text: out, math }
}

/// Collects a display block's lines from `start` until `close` finds its end in a line.
/// Returns the TeX, the closing line's index, and what follows the closing delimiter.
fn block_until<'a>(
    lines: &[&'a str],
    start: usize,
    depth: usize,
    first: &str,
    close: impl Fn(&str) -> Option<(usize, usize)>,
) -> Option<(String, usize, &'a str)> {
    let mut tex = String::from(first.trim_end_matches('\n'));
    for (j, line) in lines.iter().enumerate().skip(start) {
        let body = strip_quotes(line, depth);
        // A code fence means the block was never closed.
        if fence_open(body.trim_start()).is_some() {
            return None;
        }
        if let Some((end, after)) = close(body) {
            tex.push('\n');
            tex.push_str(&body[..end]);
            return Some((tex.trim().to_string(), j, &body[after..]));
        }
        tex.push('\n');
        tex.push_str(body.trim_end_matches('\n'));
    }
    None
}

/// Inline maths in one line (`$…$`, `$$…$$`, `\( … \)`, `` $`…`$ ``), outside code spans.
fn inline(line: &str, math: &mut Vec<Math>) -> String {
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        if rest.starts_with("$`")
            && let Some(end) = rest[2..].find("`$")
        {
            out.push_str(&token(math, rest[2..2 + end].to_string(), false));
            i += end + 4;
            continue;
        }
        if bytes[i] == b'`' {
            // A code span: copied as it is, up to the matching run of backticks.
            let run = rest.bytes().take_while(|b| *b == b'`').count();
            let ticks = &rest[..run];
            match rest[run..].find(ticks) {
                Some(end) => {
                    out.push_str(&rest[..run + end + run]);
                    i += run + end + run;
                }
                None => {
                    out.push_str(ticks);
                    i += run;
                }
            }
            continue;
        }
        // `$$…$$` within the line: display maths.
        if rest.starts_with("$$")
            && let Some(end) = rest[2..].find("$$")
            && !rest[2..2 + end].trim().is_empty()
            && !rest[2..2 + end].contains('`')
        {
            out.push_str(&token(math, rest[2..2 + end].to_string(), true));
            i += end + 4;
            continue;
        }
        if bytes[i] == b'$'
            && !rest.starts_with("$$")
            && let Some(end) = closing_dollar(&rest[1..])
        {
            out.push_str(&token(math, rest[1..1 + end].to_string(), false));
            i += end + 2;
            continue;
        }
        // An escaped `\$` (or any escaped character) is text.
        if bytes[i] == b'\\' && i + 1 < line.len() && line.is_char_boundary(i + 2) && bytes[i + 1] != b'(' {
            out.push_str(&rest[..2]);
            i += 2;
            continue;
        }
        if rest.starts_with("\\(")
            && let Some(end) = rest[2..].find("\\)")
        {
            let inner = &rest[2..2 + end];
            let mathy = inner.starts_with(' ') || inner.ends_with(' ') || inner.contains(['\\', '^', '_', '{']);
            if !inner.trim().is_empty() && mathy {
                out.push_str(&token(math, inner.trim().to_string(), false));
                i += end + 4;
                continue;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Where an inline formula opened by a `$` (just before `after`) closes: the next `$` not
/// followed by a digit, before any code span.
fn closing_dollar(after: &str) -> Option<usize> {
    if after.starts_with(char::is_whitespace) {
        return None;
    }
    let bytes = after.as_bytes();
    let mut j = 0;
    while j < after.len() {
        match bytes[j] {
            b'`' => return None,
            b'\\' => j += 2,
            b'$' => {
                let inner = &after[..j];
                let digit_after = after[j + 1..].starts_with(|c: char| c.is_ascii_digit());
                if !inner.trim().is_empty() && !digit_after {
                    return Some(j);
                }
                j += 1;
            }
            _ => j += 1,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tex(e: &Extracted) -> Vec<(&str, bool)> {
        e.math.iter().map(|m| (m.tex.as_str(), m.display)).collect()
    }

    #[test]
    fn display_blocks_survive_blank_lines_and_list_like_lines() {
        let e = extract("Text\n\n$$\n\\begin{align}\na &= b\n\n- c &= d\n+ e\n\\end{align}\n$$\n\nAfter $x$.\n");
        assert_eq!(tex(&e), [("\\begin{align}\na &= b\n\n- c &= d\n+ e\n\\end{align}", true), ("x", false)]);
        assert_eq!(e.text, format!("Text\n\n{OPEN}0{CLOSE}\n\nAfter {OPEN}1{CLOSE}.\n"));
    }

    #[test]
    fn blocks_in_quotes_and_lists_keep_their_place() {
        let e = extract("> Callout\n> $$\n> x^2\n>\n> y\n> $$\n\n- item\n  $$\n  \\frac{1}{2}\n  $$\n");
        assert_eq!(tex(&e), [("x^2\n\ny", true), ("\\frac{1}{2}", true)]);
        assert_eq!(e.text, format!("> Callout\n> {OPEN}0{CLOSE}\n\n- item\n  {OPEN}1{CLOSE}\n"));
    }

    #[test]
    fn content_on_the_delimiter_lines() {
        let e = extract("$$\\begin{aligned}\na\\\\\nb\n\\end{aligned}$$\n");
        assert_eq!(tex(&e), [("\\begin{aligned}\na\\\\\nb\n\\end{aligned}", true)]);
    }

    #[test]
    fn code_is_left_alone() {
        let src = "```\n$$\nnot maths\n$$\n```\n\nUse `\\( x \\)` or `$$`.\n";
        let e = extract(src);
        assert!(e.math.is_empty());
        assert_eq!(e.text, src);
    }

    #[test]
    fn brackets_and_github_inline() {
        let e = extract(
            "Let \\( X \\) be, and \\(note\\) stays; GitHub $`\\sqrt{2}`$.\n\\[\n\\int f\n\\]\nMark \\[TODO\\] stays.\n",
        );
        assert_eq!(tex(&e), [("X", false), ("\\sqrt{2}", false), ("\\int f", true)]);
        assert!(e.text.contains("\\(note\\)") && e.text.contains("\\[TODO\\]"));
    }

    #[test]
    fn unclosed_blocks_and_money_stay_text() {
        let src = "It costs $$5 now.\n\nAnd $3 or $4.\n";
        let e = extract(src);
        assert!(e.math.is_empty());
        assert_eq!(e.text, src);
    }

    #[test]
    fn tables_keep_pipes_in_maths() {
        let e = extract(
            "| Rule | Formula |\n|---|---|\n| RB | $E[\\hat\\theta|T]$ costs $5 |\n| R | $$R^2 = 1$$ | $x \\to $ y |\n",
        );
        assert_eq!(tex(&e), [("E[\\hat\\theta|T]", false), ("R^2 = 1", true), ("x \\to ", false)]);
        assert!(e.text.contains(&format!("| RB | {OPEN}0{CLOSE} costs $5 |\n")));
    }

    #[test]
    fn block_opened_at_the_end_of_a_line() {
        let e = extract("- A circular array:$$\n\\begin{array}{|c|c|}\n1 & 2\n\\end{array}\n$$\n");
        assert_eq!(tex(&e), [("\\begin{array}{|c|c|}\n1 & 2\n\\end{array}", true)]);
        assert_eq!(e.text, format!("- A circular array:{OPEN}0{CLOSE}\n"));
    }

    #[test]
    fn dollars_by_pandoc_rules() {
        let e =
            extract("Prices $5 and $10, \\$20, `$code$` and $x$, $\\$ 5$, $$y$$ inline, $a = $ b, $ c$ no, $d$5 no.\n");
        assert_eq!(tex(&e), [("x", false), ("\\$ 5", false), ("y", true), ("a = ", false)]);
        assert!(e.text.contains("Prices $5 and $10, \\$20, `$code$` and "), "{}", e.text);
        assert!(e.text.contains("$ c$ no, $d$5 no."), "{}", e.text);
    }

    #[test]
    fn splits_tokens() {
        let s = format!("a {OPEN}0{CLOSE} b{OPEN}12{CLOSE}");
        assert_eq!(pieces(&s), [Piece::Text("a "), Piece::Math(0), Piece::Text(" b"), Piece::Math(12)]);
    }
}
