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
