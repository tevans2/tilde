//! Terminal colour codes (SGR) to plain text plus style spans.

/// A run of text with one style. Offsets are UTF-16 code units (what `NSString` uses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub length: u32,
    /// `0xRRGGBBAA`
    pub fg: Option<u32>,
    pub bg: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyledText {
    pub text: String,
    pub spans: Vec<Span>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Style {
    fg: Option<u32>,
    bg: Option<u32>,
    bold: bool,
    italic: bool,
    underline: bool,
    dim: bool,
}

/// Parses text containing SGR escape codes (`ESC [ … m`). Other escape sequences are
/// dropped. Adjacent runs with the same style are merged; unstyled text has no span.
pub fn parse(input: &str) -> StyledText {
    let mut out = StyledText::default();
    let mut style = Style::default();
    let mut run_start = 0u32;
    let mut pos = 0u32;
    let mut chars = input.chars().peekable();

    let flush = |out: &mut StyledText, style: &Style, start: u32, end: u32| {
        if end > start && *style != Style::default() {
            if let Some(last) = out.spans.last_mut()
                && last.start + last.length == start
                && same_style(last, style)
            {
                last.length += end - start;
                return;
            }
            out.spans.push(Span {
                start,
                length: end - start,
                fg: style.fg,
                bg: style.bg,
                bold: style.bold,
                italic: style.italic,
                underline: style.underline,
                dim: style.dim,
            });
        }
    };

    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.text.push(c);
            pos += c.len_utf16() as u32;
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                let mut params = String::new();
                let mut final_byte = None;
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        final_byte = Some(c);
                        break;
                    }
                    params.push(c);
                }
                if final_byte == Some('m') {
                    flush(&mut out, &style, run_start, pos);
                    run_start = pos;
                    apply(&mut style, &params);
                }
            }
            Some(']') => {
                // OSC (e.g. hyperlinks): skip to BEL or ESC \.
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    flush(&mut out, &style, run_start, pos);
    out
}

fn same_style(span: &Span, style: &Style) -> bool {
    span.fg == style.fg
        && span.bg == style.bg
        && span.bold == style.bold
        && span.italic == style.italic
        && span.underline == style.underline
        && span.dim == style.dim
}

fn apply(style: &mut Style, params: &str) {
    let codes: Vec<u32> =
        if params.is_empty() { vec![0] } else { params.split(';').map(|p| p.parse().unwrap_or(0)).collect() };
    let mut i = 0;
    while i < codes.len() {
        match codes[i] {
            0 => *style = Style::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => style.italic = false,
            24 => style.underline = false,
            n @ 30..=37 => style.fg = Some(basic(n - 30, false)),
            n @ 90..=97 => style.fg = Some(basic(n - 90, true)),
            39 => style.fg = None,
            n @ 40..=47 => style.bg = Some(basic(n - 40, false)),
            n @ 100..=107 => style.bg = Some(basic(n - 100, true)),
            49 => style.bg = None,
            n @ (38 | 48) => {
                let color = match codes.get(i + 1) {
                    Some(5) => {
                        i += 2;
                        codes.get(i).map(|&c| xterm256(c))
                    }
                    Some(2) => {
                        let rgb = codes.get(i + 2..i + 5).map(|c| rgba(c[0], c[1], c[2]));
                        i += 4;
                        rgb
                    }
                    _ => None,
                };
                if n == 38 { style.fg = color } else { style.bg = color }
            }
            _ => {}
        }
        i += 1;
    }
}

fn rgba(r: u32, g: u32, b: u32) -> u32 {
    (r.min(255) << 24) | (g.min(255) << 16) | (b.min(255) << 8) | 0xff
}

/// The 16 standard terminal colours (xterm defaults).
fn basic(n: u32, bright: bool) -> u32 {
    const NORMAL: [(u32, u32, u32); 8] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
    ];
    const BRIGHT: [(u32, u32, u32); 8] = [
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    let (r, g, b) = if bright { BRIGHT[n as usize % 8] } else { NORMAL[n as usize % 8] };
    rgba(r, g, b)
}

/// xterm's 256-colour palette.
fn xterm256(n: u32) -> u32 {
    match n {
        0..=7 => basic(n, false),
        8..=15 => basic(n - 8, true),
        16..=231 => {
            let n = n - 16;
            let level = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
            rgba(level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        _ => {
            let v = 8 + (n.min(255) - 232) * 10;
            rgba(v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_has_no_spans() {
        assert_eq!(parse("hello\nworld"), StyledText { text: "hello\nworld".into(), spans: vec![] });
    }

    #[test]
    fn truecolor_runs_and_reset() {
        let t = parse("\x1b[38;2;102;217;239mfn\x1b[0m main");
        assert_eq!(t.text, "fn main");
        assert_eq!(t.spans.len(), 1);
        assert_eq!((t.spans[0].start, t.spans[0].length), (0, 2));
        assert_eq!(t.spans[0].fg, Some(0x66d9efff));
    }

    #[test]
    fn adjacent_same_style_runs_merge() {
        let t = parse("\x1b[38;2;1;2;3mab\x1b[0m\x1b[38;2;1;2;3mcd\x1b[0m");
        assert_eq!(t.text, "abcd");
        assert_eq!(t.spans.len(), 1);
        assert_eq!(t.spans[0].length, 4);
    }

    #[test]
    fn attributes_256_colours_and_utf16_offsets() {
        let t = parse("é\x1b[1;3;38;5;196m😀x\x1b[22;23mz\x1b[39m!");
        assert_eq!(t.text, "é😀xz!");
        // "é" is one UTF-16 unit; "😀" is two.
        assert_eq!(t.spans[0].start, 1);
        assert_eq!(t.spans[0].length, 3);
        assert!(t.spans[0].bold && t.spans[0].italic);
        assert_eq!(t.spans[0].fg, Some(0xff0000ff));
        assert_eq!((t.spans[1].start, t.spans[1].length, t.spans[1].bold), (4, 1, false));
        assert_eq!(t.spans.len(), 2, "unstyled '!' has no span");
    }

    #[test]
    fn other_sequences_are_dropped() {
        let t = parse("\x1b[2Ka\x1b]8;;https://x.y\x07link\x1b]8;;\x07b");
        assert_eq!(t.text, "alinkb");
    }
}
