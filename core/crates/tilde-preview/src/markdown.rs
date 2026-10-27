//! Markdown for PREVIEW, rendered two ways from one parse (pulldown-cmark, with GitHub's
//! extensions): HTML for a web view, and blocks of styled runs for a native text view.
//! Code blocks are highlighted with syntect (bat's library). Math (`$…$`, `$$…$$`,
//! ```` ```math ````) becomes MathML, which WebKit draws itself, so no scripts are needed.
//! Images next to the file are embedded; remote ones are left out, so previewing a README
//! doesn't fetch anything.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use base64::Engine;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

/// Images larger than this aren't embedded.
const IMAGE_LIMIT: u64 = 10_000_000;

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM
        | Options::ENABLE_MATH
}

/// LaTeX as MathML; the source, as code, if it can't be converted.
fn math(latex: &str, block: bool) -> String {
    let style = if block { latex2mathml::DisplayStyle::Block } else { latex2mathml::DisplayStyle::Inline };
    match latex2mathml::latex_to_mathml(&tex_aliases(latex.trim()), style) {
        // It reports some unknown commands inline rather than as an error.
        Ok(mathml) if !mathml.contains("PARSE ERROR") => unstretch(&mathml),
        _ if block => format!("<pre class=\"math-source\"><code>{}</code></pre>", escape(latex)),
        _ => format!("<code class=\"math-source\">{}</code>", escape(latex)),
    }
}

/// Rewrites common LaTeX that latex2mathml doesn't know into forms it does.
fn tex_aliases(latex: &str) -> String {
    let mut s = latex.to_string();
    for env in ["aligned", "align*", "align", "gathered", "gather*", "gather", "split"] {
        s = s
            .replace(&format!("\\begin{{{env}}}"), "\\begin{matrix}")
            .replace(&format!("\\end{{{env}}}"), "\\end{matrix}");
    }
    s = s.replace("\\begin{cases}", "\\left\\{\\begin{matrix}").replace("\\end{cases}", "\\end{matrix}\\right.");
    // Commands, matched whole (`\le` but not `\left`).
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(i) = rest.find('\\') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let len = after.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(after.len());
        let (name, tail) = after.split_at(len);
        rest = tail;
        let replacement = match name {
            "le" => "\\leq",
            "ge" => "\\geq",
            "dots" => "\\ldots",
            "neg" => "\\lnot",
            "lVert" | "rVert" => "\\|",
            "prime" => "'",
            "tfrac" | "dfrac" => "\\frac",
            "varnothing" => "\\emptyset",
            "deg" => "\\operatorname{deg}",
            "mathcal" => "\\mathscr",
            "displaystyle" | "textstyle" => "",
            "color" => {
                // `\color{red}{x}`: keep x, drop the colour.
                if let Some(end) = rest.strip_prefix('{').and_then(|r| r.find('}')) {
                    rest = &rest[end + 2..];
                }
                ""
            }
            "" => {
                // `\\`, `\{`, `\,`…: the next character belongs to the backslash.
                let c = rest.chars().next().map_or(0, char::len_utf8);
                out.push('\\');
                out.push_str(&rest[..c]);
                rest = &rest[c..];
                continue;
            }
            _ => {
                out.push('\\');
                out.push_str(name);
                continue;
            }
        };
        out.push_str(replacement);
    }
    out.push_str(rest);
    out
}

/// In LaTeX only `\left( … \right)` grows with what's inside; MathML stretches every bracket
/// unless told not to.
fn unstretch(mathml: &str) -> String {
    let mut out = mathml.to_string();
    for b in ["(", ")", "[", "]", "{", "}", "|", "∣", "‖"] {
        out = out.replace(&format!("<mo>{b}</mo>"), &format!("<mo stretchy=\"false\">{b}</mo>"));
    }
    out
}

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme(dark: bool) -> &'static Theme {
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    let set = THEMES.get_or_init(ThemeSet::load_defaults);
    &set.themes[if dark { "base16-ocean.dark" } else { "InspiredGitHub" }]
}

/// Highlighted lines of code: (text, colour `0xRRGGBBAA`) pieces, newlines kept.
fn highlight(code: &str, lang: &str, dark: bool) -> Vec<(String, u32)> {
    let set = syntaxes();
    let syntax = set
        .find_syntax_by_token(lang.split([' ', ',', '{']).next().unwrap_or(""))
        .unwrap_or_else(|| set.find_syntax_plain_text());
    let mut h = HighlightLines::new(syntax, theme(dark));
    let mut out = Vec::new();
    for line in syntect::util::LinesWithEndings::from(code) {
        match h.highlight_line(line, set) {
            Ok(pieces) => out.extend(pieces.into_iter().map(|(style, text)| {
                let c = style.foreground;
                (text.to_string(), u32::from_be_bytes([c.r, c.g, c.b, c.a]))
            })),
            Err(_) => out.push((line.to_string(), 0)),
        }
    }
    out
}

/// A local image referenced from the document at `base`, if it exists and isn't too big.
fn local_image(src: &str, base: &Path) -> Option<PathBuf> {
    if src.contains("://") || src.starts_with("data:") || src.starts_with("//") {
        return None;
    }
    let src = src.split(['?', '#']).next().unwrap_or(src);
    let path = if src.starts_with('/') { PathBuf::from(src) } else { base.join(src) };
    let meta = std::fs::metadata(&path).ok()?;
    (meta.is_file() && meta.len() <= IMAGE_LIMIT).then_some(path)
}

fn data_uri(path: &Path) -> Option<String> {
    let mime = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => return None,
    };
    let bytes = std::fs::read(path).ok()?;
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// An attribute's value in a tag's text (`<img src="a" alt="b"`).
fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!(" {name}=\"");
    tag.find(&key).and_then(|i| tag[i + key.len()..].split('"').next().map(str::to_string))
}

/// `<img>` tags in raw HTML: local images embedded, remote ones replaced by their alt text.
fn rewrite_imgs(html: &str, base: &Path) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find("<img") {
        out.push_str(&rest[..i]);
        let Some(len) = rest[i..].find('>') else {
            rest = &rest[i..];
            break;
        };
        let tag = &rest[i..=i + len];
        let src = attr(tag, "src").unwrap_or_default();
        match local_image(&src, base).and_then(|p| data_uri(&p)) {
            Some(uri) => out.push_str(&tag.replacen(&format!("src=\"{src}\""), &format!("src=\"{uri}\""), 1)),
            None => out.push_str(&alt_label(&attr(tag, "alt").unwrap_or_default())),
        }
        rest = &rest[i + len + 1..];
    }
    out.push_str(rest);
    out
}

/// What shows for an image that isn't loaded: its alt text, as a small label.
fn alt_label(alt: &str) -> String {
    if alt.trim().is_empty() { String::new() } else { format!("<span class=\"img-alt\">{}</span>", escape(alt)) }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The document as HTML (the body only; the app adds the page and its styles). `base` is
/// the file's folder, for images.
pub fn to_html(source: &str, base: &Path, dark: bool) -> String {
    let mut events = Vec::new();
    let mut code: Option<(String, String)> = None;
    // A remote image's alt text, being collected.
    let mut alt: Option<String> = None;
    for event in Parser::new_ext(source, options()) {
        if let Some(text) = &mut alt {
            match event {
                Event::Text(t) | Event::Code(t) => text.push_str(&t),
                Event::End(TagEnd::Image) => events.push(Event::InlineHtml(alt_label(&alt.take().unwrap()).into())),
                _ => {}
            }
            continue;
        }
        match (&mut code, event) {
            (None, Event::Start(Tag::CodeBlock(kind))) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            (Some((_, text)), Event::Text(t)) => text.push_str(&t),
            (Some((lang, text)), Event::End(TagEnd::CodeBlock)) if lang.trim() == "math" => {
                events.push(Event::Html(math(text, true).into()));
                code = None;
            }
            (Some((lang, text)), Event::End(TagEnd::CodeBlock)) => {
                let mut html = String::from("<pre class=\"code\"><code>");
                for (piece, color) in highlight(text, lang, dark) {
                    html.push_str(&format!("<span style=\"color:#{:06x}\">{}</span>", color >> 8, escape(&piece)));
                }
                html.push_str("</code></pre>\n");
                events.push(Event::Html(html.into()));
                code = None;
            }
            (Some(_), _) => {}
            (None, Event::Start(Tag::Image { link_type, dest_url, title, id })) => {
                match local_image(&dest_url, base).and_then(|p| data_uri(&p)) {
                    Some(src) => events.push(Event::Start(Tag::Image { link_type, dest_url: src.into(), title, id })),
                    None => alt = Some(String::new()),
                }
            }
            (None, Event::InlineMath(m)) => events.push(Event::InlineHtml(math(&m, false).into())),
            (None, Event::DisplayMath(m)) => events.push(Event::InlineHtml(math(&m, true).into())),
            (None, Event::Html(h)) => events.push(Event::Html(rewrite_imgs(&h, base).into())),
            (None, Event::InlineHtml(h)) => events.push(Event::InlineHtml(rewrite_imgs(&h, base).into())),
            (None, e) => events.push(e),
        }
    }
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events.into_iter());
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading,
    ListItem,
    Code,
    /// A table, laid out as aligned monospaced text.
    Table,
    Rule,
    Image,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: Option<String>,
    /// Syntax colour in code blocks, `0xRRGGBBAA`.
    pub color: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// Heading level (1–6), or list depth (1 = top level).
    pub level: u8,
    /// How many quotes it's inside.
    pub quote: u8,
    /// A list item's bullet or number ("•", "3.").
    pub marker: String,
    pub runs: Vec<Run>,
    /// An image: the local file (absent for remote ones) and its alt text in `runs`.
    pub image: Option<PathBuf>,
    /// The width an HTML `<img width>` asks for, in points.
    pub image_width: Option<u32>,
}

impl Block {
    fn new(kind: BlockKind, level: u8, quote: u8) -> Block {
        Block { kind, level, quote, marker: String::new(), runs: Vec::new(), image: None, image_width: None }
    }
}

#[derive(Default)]
struct Inline {
    bold: u32,
    italic: u32,
    strike: u32,
    link: Option<String>,
}

struct Native<'a> {
    blocks: Vec<Block>,
    current: Option<Block>,
    inline: Inline,
    quote: u8,
    /// Each open list: the next number, or None for bullets.
    lists: Vec<Option<u64>>,
    base: &'a Path,
    dark: bool,
}

impl Native<'_> {
    fn flush(&mut self) {
        if let Some(b) = self.current.take()
            && (b.runs.iter().any(|r| !r.text.trim().is_empty()) || b.kind == BlockKind::Rule)
        {
            self.blocks.push(b);
        }
    }

    fn start(&mut self, kind: BlockKind, level: u8) {
        self.flush();
        self.current = Some(Block::new(kind, level, self.quote));
    }

    fn push(&mut self, text: &str, code: bool) {
        let block = self.current.get_or_insert_with(|| Block::new(BlockKind::Paragraph, 0, self.quote));
        block.runs.push(Run {
            text: text.to_string(),
            bold: self.inline.bold > 0,
            italic: self.inline.italic > 0,
            code,
            strike: self.inline.strike > 0,
            link: self.inline.link.clone(),
            color: None,
        });
    }

    fn image(&mut self, src: &str, alt: String, width: Option<u32>) {
        let resume = self.current.as_ref().map(|b| (b.kind, b.level));
        self.flush();
        let mut b = Block::new(BlockKind::Image, 0, self.quote);
        b.image = local_image(src, self.base);
        b.image_width = width;
        b.runs.push(Run { text: alt, ..Run::default() });
        self.blocks.push(b);
        if let Some((kind, level)) = resume {
            self.current = Some(Block::new(
                if kind == BlockKind::ListItem { BlockKind::Paragraph } else { kind },
                level,
                self.quote,
            ));
        }
    }
}

/// `<img src="…" alt="…" width="…">` in raw HTML.
fn html_images(html: &str) -> Vec<(String, String, Option<u32>)> {
    html.split("<img")
        .skip(1)
        .filter_map(|tag| {
            let tag = format!(" {}", tag.split('>').next()?);
            let width = attr(&tag, "width").and_then(|w| w.trim_end_matches("px").parse().ok());
            Some((attr(&tag, "src")?, attr(&tag, "alt").unwrap_or_default(), width))
        })
        .collect()
}

/// Lays a table out as text: columns padded to their widest cell, the header ruled off.
fn table_text(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let width = |c: usize| rows.iter().filter_map(|r| r.get(c)).map(|s| s.chars().count()).max().unwrap_or(0);
    let widths: Vec<usize> = (0..cols).map(width).collect();
    let mut out = String::new();
    for (i, row) in rows.iter().enumerate() {
        let cells: Vec<String> = (0..cols)
            .map(|c| {
                let s = row.get(c).map(String::as_str).unwrap_or("");
                format!("{s}{}", " ".repeat(widths[c] - s.chars().count()))
            })
            .collect();
        out.push_str(cells.join("  │  ").trim_end());
        out.push('\n');
        if i == 0 {
            let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
            out.push_str(&rule.join("──┼──"));
            out.push('\n');
        }
    }
    out.pop();
    out
}

/// The document as blocks for a native text view. `base` is the file's folder.
pub fn to_blocks(source: &str, base: &Path, dark: bool) -> Vec<Block> {
    let mut n = Native {
        blocks: Vec::new(),
        current: None,
        inline: Inline::default(),
        quote: 0,
        lists: Vec::new(),
        base,
        dark,
    };
    let mut code: Option<(String, String)> = None;
    let mut table: Option<Vec<Vec<String>>> = None;
    let mut image: Option<(String, String)> = None;
    for event in Parser::new_ext(source, options()) {
        if let Some((_, text)) = &mut code {
            match event {
                Event::Text(t) => text.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, text) = code.take().unwrap();
                    n.start(BlockKind::Code, 0);
                    let block = n.current.as_mut().unwrap();
                    let text = text.strip_suffix('\n').unwrap_or(&text);
                    for (piece, color) in highlight(text, &lang, n.dark) {
                        block.runs.push(Run { text: piece, code: true, color: Some(color), ..Run::default() });
                    }
                    n.flush();
                }
                _ => {}
            }
            continue;
        }
        if let Some((_, alt)) = &mut image {
            match event {
                Event::Text(t) | Event::Code(t) => alt.push_str(&t),
                Event::End(TagEnd::Image) => {
                    let (src, alt) = image.take().unwrap();
                    n.image(&src, alt, None);
                }
                _ => {}
            }
            continue;
        }
        if let Some(rows) = &mut table {
            match event {
                Event::Start(Tag::TableHead | Tag::TableRow) => rows.push(Vec::new()),
                Event::Start(Tag::TableCell) => rows.last_mut().unwrap().push(String::new()),
                Event::Text(t) | Event::Code(t) => rows.last_mut().and_then(|r| r.last_mut()).unwrap().push_str(&t),
                Event::End(TagEnd::Table) => {
                    let rows = table.take().unwrap();
                    n.start(BlockKind::Table, 0);
                    n.push(&table_text(&rows), true);
                    n.flush();
                }
                _ => {}
            }
            continue;
        }
        match event {
            Event::Start(Tag::Paragraph) => {
                let continues_item =
                    n.current.as_ref().is_some_and(|b| b.kind == BlockKind::ListItem && b.runs.is_empty());
                if !continues_item {
                    let level = n.lists.len() as u8;
                    n.start(BlockKind::Paragraph, level);
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let level = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                n.start(BlockKind::Heading, level);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                n.flush();
                n.quote += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                n.flush();
                n.quote = n.quote.saturating_sub(1);
            }
            Event::Start(Tag::List(first)) => {
                n.flush();
                n.lists.push(first);
            }
            Event::End(TagEnd::List(_)) => {
                n.flush();
                n.lists.pop();
            }
            Event::Start(Tag::Item) => {
                let level = n.lists.len() as u8;
                let marker = match n.lists.last_mut() {
                    Some(Some(next)) => {
                        *next += 1;
                        format!("{}.", *next - 1)
                    }
                    _ => ["•", "◦", "▪"][(level as usize).saturating_sub(1) % 3].to_string(),
                };
                n.start(BlockKind::ListItem, level);
                n.current.as_mut().unwrap().marker = marker;
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::Start(Tag::Table(_)) => {
                n.flush();
                table = Some(Vec::new());
            }
            Event::Start(Tag::Image { dest_url, .. }) => image = Some((dest_url.to_string(), String::new())),
            Event::Start(Tag::Strong) => n.inline.bold += 1,
            Event::End(TagEnd::Strong) => n.inline.bold -= 1,
            Event::Start(Tag::Emphasis) => n.inline.italic += 1,
            Event::End(TagEnd::Emphasis) => n.inline.italic -= 1,
            Event::Start(Tag::Strikethrough) => n.inline.strike += 1,
            Event::End(TagEnd::Strikethrough) => n.inline.strike -= 1,
            Event::Start(Tag::Link { dest_url, .. }) => n.inline.link = Some(dest_url.to_string()),
            Event::End(TagEnd::Link) => n.inline.link = None,
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item) => n.flush(),
            Event::Text(t) => n.push(&t, false),
            Event::Code(t) | Event::InlineMath(t) => n.push(&t, true),
            Event::DisplayMath(t) => {
                // Shown as its LaTeX: a native text view can't lay out math.
                n.start(BlockKind::Code, 0);
                n.push(t.trim(), true);
                n.flush();
            }
            Event::SoftBreak => n.push(" ", false),
            Event::HardBreak => n.push("\n", false),
            Event::TaskListMarker(done) => {
                let mark = if done { "☑" } else { "☐" };
                match n.current.as_mut() {
                    Some(b) if b.kind == BlockKind::ListItem && b.runs.is_empty() => b.marker = mark.into(),
                    _ => n.push(&format!("{mark} "), false),
                }
            }
            Event::FootnoteReference(name) => n.push(&format!("[{name}]"), false),
            Event::Rule => {
                n.start(BlockKind::Rule, 0);
                n.flush();
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                for (src, alt, width) in html_images(&h) {
                    n.image(&src, alt, width);
                }
                if h.trim_start().starts_with("<br") {
                    n.push("\n", false);
                }
            }
            _ => {}
        }
    }
    n.flush();
    n.blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# Title\n\nSome **bold** and `code` and a [link](https://example.org).\n\n\
        - one\n- two\n  1. nested\n\n> quoted\n\n```rust\nfn main() {}\n```\n\n\
        | a | bb |\n|---|---|\n| 1 | 2 |\n\n---\n\n![logo](logo.png) ![remote](https://x.org/a.png)\n";

    #[test]
    fn native_blocks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("logo.png"), b"png").unwrap();
        let blocks = to_blocks(DOC, dir.path(), true);
        let kinds: Vec<BlockKind> = blocks.iter().map(|b| b.kind).collect();
        use BlockKind::*;
        assert_eq!(
            kinds,
            [Heading, Paragraph, ListItem, ListItem, ListItem, Paragraph, Code, Table, Rule, Image, Image]
        );
        assert_eq!(blocks[0].level, 1);
        let p = &blocks[1].runs;
        assert!(p.iter().any(|r| r.bold && r.text == "bold"));
        assert!(p.iter().any(|r| r.code && r.text == "code"));
        assert!(p.iter().any(|r| r.link.as_deref() == Some("https://example.org")));
        assert_eq!((blocks[2].marker.as_str(), blocks[4].marker.as_str(), blocks[4].level), ("•", "1.", 2));
        assert_eq!(blocks[5].quote, 1);
        assert!(blocks[6].runs.iter().all(|r| r.color.is_some()));
        assert_eq!(blocks[7].runs[0].text, "a  │  bb\n───┼────\n1  │  2");
        assert_eq!(blocks[9].image.as_deref(), Some(dir.path().join("logo.png").as_path()));
        assert_eq!((blocks[10].image.as_ref(), blocks[10].runs[0].text.as_str()), (None, "remote"));
        let html_img = to_blocks("<img src=\"logo.png\" width=\"128\" alt=\"Logo\">", dir.path(), true);
        assert_eq!(html_img[0].image_width, Some(128));
        let tasks = to_blocks("- [x] done\n- [ ] todo\n", dir.path(), true);
        assert_eq!((tasks[0].marker.as_str(), tasks[1].marker.as_str()), ("☑", "☐"));
        assert_eq!(tasks[0].runs[0].text, "done");
    }

    #[test]
    fn math_becomes_mathml() {
        let dir = tempfile::tempdir().unwrap();
        let html =
            to_html("Euler: $e^{i\\pi} + 1 = 0$\n\n$$\\frac{a}{b}$$\n\n```math\n\\sqrt{x}\n```\n", dir.path(), true);
        assert!(html.contains("<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\">"), "{html}");
        assert!(html.contains("<mfrac>") && html.contains("<msqrt>"), "{html}");
        assert_eq!(html.matches("display=\"block\"").count(), 2, "{html}");
        // Aliases for what latex2mathml lacks, brackets that don't grow, and the source for
        // anything still unknown.
        let m = math(r"P(A \mid B) \le \dfrac{1}{2}, \begin{cases} a & x \ge 0 \\ b \end{cases}", false);
        assert!(!m.contains("PARSE ERROR") && m.contains("<mfrac>") && m.contains("≤") && m.contains("≥"), "{m}");
        assert!(m.contains("<mo stretchy=\"false\">(</mo>"), "{m}");
        assert_eq!(tex_aliases(r"\left(\color{red}{x}\right) \\ \{a\}"), r"\left({x}\right) \\ \{a\}");
        assert_eq!(math(r"\unknowncommand{x}", false), r#"<code class="math-source">\unknowncommand{x}</code>"#);
        let blocks = to_blocks("Inline $x^2$ here.\n", dir.path(), true);
        assert!(blocks[0].runs.iter().any(|r| r.code && r.text == "x^2"));
    }

    #[test]
    fn html_embeds_local_images_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("logo.png"), b"png").unwrap();
        let html = to_html(DOC, dir.path(), false);
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("src=\"data:image/png;base64,"));
        assert!(!html.contains("x.org"), "remote images are dropped");
        assert!(html.contains("<table>"));
        assert!(html.contains("<pre class=\"code\"><code><span style=\"color:#"));
        let raw = rewrite_imgs("<p align=\"center\"><img src=\"logo.png\" width=\"9\"></p>", dir.path());
        assert!(raw.contains("data:image/png;base64,") && raw.contains("width=\"9\""));
        let badge = rewrite_imgs("<a href=\"x\"><img src=\"https://img.shields.io/b\" alt=\"CI\"></a>", dir.path());
        assert_eq!(badge, "<a href=\"x\"><span class=\"img-alt\">CI</span></a>");
        assert!(html.contains("<span class=\"img-alt\">remote</span>"));
    }
}
