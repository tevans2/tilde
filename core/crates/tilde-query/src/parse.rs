use tilde_types::Entry;

use crate::kind::{Kind, kind_of};

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

impl Filter {
    pub fn matches(&self, entry: &Entry, now: i64) -> bool {
        match self {
            Filter::In(_) => true,
            Filter::Ext(exts) => exts.contains(&entry.extension()),
            Filter::Kind(k) => kind_of(entry) == *k,
            Filter::ModifiedWithin(secs) => now - entry.modified <= *secs,
            Filter::Size(SizeBound::Larger(n)) => entry.size > *n,
            Filter::Size(SizeBound::Smaller(n)) => entry.size < *n,
        }
    }
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
    fn sizes_and_durations() {
        assert_eq!(parse_size("100MB"), Some(100_000_000));
        assert_eq!(parse_size("512"), Some(512));
        assert_eq!(parse_duration("3w"), Some(1_814_400));
        assert_eq!(parse_duration("7"), None);
        assert_eq!(Query::parse("size:<1k").filters, [Filter::Size(SizeBound::Smaller(1_000))]);
    }
}
