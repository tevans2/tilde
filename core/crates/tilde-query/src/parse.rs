use tilde_types::{Entry, EntryKind, with_extension};

use crate::kind::{Kind, kind_for};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SizeBound {
    Larger(u64),
    Smaller(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filter {
    /// `in:<fuzzy path>` limits the Everywhere scope; `in:.` means this folder and below.
    /// It has no effect on the Here scope.
    In(String),
    /// `ext:pdf` or `ext:png,jpg`.
    Ext(Vec<String>),
    /// `kind:image`.
    Kind(Kind),
    /// `mod:<7d`: modified within this many seconds.
    ModifiedWithin(i64),
    /// `size:>100M` or `size:<1K`.
    Size(SizeBound),
}

/// What a filter looks at. Borrowed, so the index can test items without building an
/// `Entry` for each.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub name: &'a str,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: i64,
    /// The folder containing the item, in the same form as `FilterContext::cwd`. `None`
    /// for the Here scope, where `in:` has no effect.
    pub parent: Option<&'a str>,
}

impl<'a> From<&'a Entry> for Candidate<'a> {
    fn from(e: &'a Entry) -> Self {
        Candidate { name: &e.name, kind: e.kind, size: e.size, modified: e.modified, parent: None }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FilterContext<'a> {
    /// Current Unix time, for `mod:`.
    pub now: i64,
    /// The current folder, for `in:.`.
    pub cwd: Option<&'a str>,
}

impl Filter {
    pub fn matches(&self, entry: &Entry, now: i64) -> bool {
        self.accepts(&Candidate::from(entry), &FilterContext { now, cwd: None })
    }

    pub fn accepts(&self, c: &Candidate<'_>, ctx: &FilterContext<'_>) -> bool {
        match self {
            Filter::In(v) => match c.parent {
                None => true,
                Some(parent) if v == "." => ctx.cwd.is_some_and(|cwd| {
                    parent == cwd || (parent.starts_with(cwd) && parent[cwd.len()..].starts_with('/'))
                }),
                Some(parent) => is_subsequence(v.trim_start_matches("~/"), parent),
            },
            Filter::Ext(exts) => with_extension(c.name, |ext| exts.iter().any(|e| e == ext)),
            Filter::Kind(k) => kind_for(c.kind, c.name) == *k,
            Filter::ModifiedWithin(secs) => ctx.now - c.modified <= *secs,
            Filter::Size(SizeBound::Larger(n)) => c.size > *n,
            Filter::Size(SizeBound::Smaller(n)) => c.size < *n,
        }
    }
}

/// Case-insensitive: are all chars of `needle` in `haystack`, in order?
fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut it = haystack.chars().flat_map(char::to_lowercase);
    needle.chars().flat_map(char::to_lowercase).all(|n| it.any(|h| h == n))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub filters: Vec<Filter>,
    /// Search terms, space separated, in nucleo pattern syntax.
    pub text: String,
}

impl Query {
    /// Splits input into filters and search terms. A token that looks like a filter but
    /// doesn't parse (`kind:banana`) is treated as a search term.
    pub fn parse(input: &str) -> Query {
        let mut q = Query::default();
        let mut terms = Vec::new();
        for token in input.split_whitespace() {
            match parse_filter(token) {
                Some(f) => q.filters.push(f),
                None => terms.push(token),
            }
        }
        q.text = terms.join(" ");
        q
    }

    pub fn is_empty(&self) -> bool {
        self.filters.is_empty() && self.text.is_empty()
    }
}

fn parse_filter(token: &str) -> Option<Filter> {
    let (key, value) = token.split_once(':')?;
    let value = value.to_lowercase();
    if value.is_empty() {
        return None;
    }
    match key.to_lowercase().as_str() {
        "in" => Some(Filter::In(value)),
        "ext" => Some(Filter::Ext(
            value.split(',').map(|s| s.trim_start_matches('.').to_string()).filter(|s| !s.is_empty()).collect(),
        )),
        "kind" => Kind::parse(&value).map(Filter::Kind),
        "mod" => parse_duration(value.trim_start_matches('<')).map(Filter::ModifiedWithin),
        "size" => {
            if let Some(v) = value.strip_prefix('>') {
                parse_size(v).map(|n| Filter::Size(SizeBound::Larger(n)))
            } else if let Some(v) = value.strip_prefix('<') {
                parse_size(v).map(|n| Filter::Size(SizeBound::Smaller(n)))
            } else {
                parse_size(&value).map(|n| Filter::Size(SizeBound::Larger(n)))
            }
        }
        _ => None,
    }
}

/// `30m`, `2h`, `7d`, `3w` → seconds.
fn parse_duration(s: &str) -> Option<i64> {
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
    let n: i64 = num.parse().ok()?;
    let mult = match unit {
        "m" | "min" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        _ => return None,
    };
    Some(n * mult)
}

/// `500`, `10k`, `1.5m`, `2g` (optionally with a trailing `b`) → bytes, decimal units
/// like the Finder.
fn parse_size(s: &str) -> Option<u64> {
    let s = s.to_lowercase();
    let s = s.trim_end_matches('b');
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let mult = match unit {
        "" => 1.0,
        "k" => 1e3,
        "m" => 1e6,
        "g" => 1e9,
        "t" => 1e12,
        _ => return None,
    };
    Some((n * mult) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_filters_from_terms() {
        let q = Query::parse("tilde ext:Swift,.rs swift mod:<2h size:>1.5M in:dev");
        assert_eq!(q.text, "tilde swift");
        assert_eq!(
            q.filters,
            [
                Filter::Ext(vec!["swift".into(), "rs".into()]),
                Filter::ModifiedWithin(7_200),
                Filter::Size(SizeBound::Larger(1_500_000)),
                Filter::In("dev".into()),
            ]
        );
    }

    #[test]
    fn invalid_filters_become_terms() {
        let q = Query::parse("kind:banana mod:soon size:big note:");
        assert!(q.filters.is_empty());
        assert_eq!(q.text, "kind:banana mod:soon size:big note:");
    }

    #[test]
    fn in_filter_uses_parent_and_cwd() {
        let ctx = FilterContext { now: 0, cwd: Some("Developer/projects") };
        let c = |parent| Candidate { name: "main.swift", kind: EntryKind::File, size: 1, modified: 0, parent };
        let f = Filter::In("dev".into());
        assert!(f.accepts(&c(Some("Developer/projects/tilde")), &ctx));
        assert!(!f.accepts(&c(Some("Documents")), &ctx));
        assert!(f.accepts(&c(None), &ctx), "no effect in the Here scope");

        let here = Filter::In(".".into());
        assert!(here.accepts(&c(Some("Developer/projects")), &ctx));
        assert!(here.accepts(&c(Some("Developer/projects/tilde/src")), &ctx));
        assert!(!here.accepts(&c(Some("Developer/projects-old")), &ctx));
    }

    #[test]
    fn sizes_and_durations() {
        assert_eq!(parse_size("100MB"), Some(100_000_000));
        assert_eq!(parse_size("512"), Some(512));
        assert_eq!(parse_duration("3w"), Some(1_814_400));
        assert_eq!(parse_duration("7"), None);
        assert_eq!(Query::parse("size:<1k").filters, [Filter::Size(SizeBound::Smaller(1_000))]);
    }
}
