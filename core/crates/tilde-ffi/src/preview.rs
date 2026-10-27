//! Syntax-highlighted text previews via bat.

use std::path::Path;

/// A styled run in a text preview. Offsets are UTF-16 code units; colours `0xRRGGBBAA`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct TextSpan {
    pub start: u32,
    pub length: u32,
    pub fg: Option<u32>,
    pub bg: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TextPreview {
    pub text: String,
    pub spans: Vec<TextSpan>,
}

/// Renders a text file with bat. `None` if bat isn't installed, the file isn't text, or
/// bat fails; the app then falls back to Quick Look. Blocking: call off the main thread.
#[uniffi::export]
pub fn bat_preview(path: String, theme: String, max_lines: u32) -> Option<TextPreview> {
    let styled = tilde_preview::bat_preview(Path::new(&path), &tilde_preview::BatOptions { theme, max_lines })?;
    Some(TextPreview {
        text: styled.text,
        spans: styled
            .spans
            .into_iter()
            .map(|s| TextSpan {
                start: s.start,
                length: s.length,
                fg: s.fg,
                bg: s.bg,
                bold: s.bold,
                italic: s.italic,
                underline: s.underline,
                dim: s.dim,
            })
            .collect(),
    })
}

#[uniffi::export]
pub fn bat_installed() -> bool {
    tilde_preview::find_bat().is_some()
}

/// Markdown files larger than this aren't rendered.
const MARKDOWN_LIMIT: u64 = 5_000_000;

fn read_markdown(path: &str) -> Option<(String, std::path::PathBuf)> {
    let path = Path::new(path);
    if std::fs::metadata(path).ok()?.len() > MARKDOWN_LIMIT {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    Some((text, path.parent().unwrap_or(Path::new("/")).to_path_buf()))
}

/// A Markdown file as HTML (the body; the app wraps it in a page with Tilde's styles).
/// `None` if it can't be read or is over 5 MB. Blocking: call off the main thread.
#[uniffi::export]
pub fn markdown_html(path: String, dark: bool) -> Option<String> {
    let (text, base) = read_markdown(&path)?;
    Some(tilde_preview::markdown::to_html(&text, &base, dark))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MarkdownBlockKind {
    Paragraph,
    Heading,
    ListItem,
    Code,
    Table,
    Rule,
    Image,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MarkdownRun {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: Option<String>,
    /// Syntax colour in code blocks, `0xRRGGBBAA`.
    pub color: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MarkdownBlock {
    pub kind: MarkdownBlockKind,
    /// Heading level (1–6), or list depth.
    pub level: u8,
    pub quote: u8,
    pub marker: String,
    pub runs: Vec<MarkdownRun>,
    /// A local image's path.
    pub image: Option<String>,
    /// The width an HTML `<img width>` asks for, in points.
    pub image_width: Option<u32>,
}

/// A Markdown file as blocks of styled runs, for a native text view. Blocking.
#[uniffi::export]
pub fn markdown_blocks(path: String, dark: bool) -> Option<Vec<MarkdownBlock>> {
    use tilde_preview::markdown::BlockKind as K;
    let (text, base) = read_markdown(&path)?;
    let blocks = tilde_preview::markdown::to_blocks(&text, &base, dark);
    Some(
        blocks
            .into_iter()
            .map(|b| MarkdownBlock {
                kind: match b.kind {
                    K::Paragraph => MarkdownBlockKind::Paragraph,
                    K::Heading => MarkdownBlockKind::Heading,
                    K::ListItem => MarkdownBlockKind::ListItem,
                    K::Code => MarkdownBlockKind::Code,
                    K::Table => MarkdownBlockKind::Table,
                    K::Rule => MarkdownBlockKind::Rule,
                    K::Image => MarkdownBlockKind::Image,
                },
                level: b.level,
                quote: b.quote,
                marker: b.marker,
                runs: b
                    .runs
                    .into_iter()
                    .map(|r| MarkdownRun {
                        text: r.text,
                        bold: r.bold,
                        italic: r.italic,
                        code: r.code,
                        strike: r.strike,
                        link: r.link,
                        color: r.color,
                    })
                    .collect(),
                image: b.image.map(|p| p.to_string_lossy().into_owned()),
                image_width: b.image_width,
            })
            .collect(),
    )
}
