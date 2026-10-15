//! The search box's query language: words, `"exact phrases"`, and `keyword:value` filters.
//!
//! `budget from:ada has:attachment -draft after:2026-10-01`. Every term must match; `-` in front
//! of a term excludes what it matches. Keywords: `from`, `to`, `cc`, `subject`, `body`,
//! `category`, `attachment` (file name), `has:attachment`, `is:unread|read|flagged|important|categorized`,
//! `importance:high|normal|low`, `before:`, `after:`, `on:` (dates, `YYYY-MM-DD` or `M/D/YYYY`),
//! `larger:` and `smaller:` (a size: `100kb`, `2mb`, bytes without a unit). `from:` and `to:`
//! take several people separated by commas, any of whom matches (`from:ada,bob@example.org`).

use mailcraft_time::{Date, DateTime};

use crate::{Address, FlagStatus, Importance, Message, contains_ci};

/// Longest query read and most terms kept (the rest is ignored).
const MAX_QUERY: usize = 2_000;
const MAX_TERMS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Text(String),
    From(String),
    To(String),
    Cc(String),
    Subject(String),
    Body(String),
    Category(String),
    Attachment(String),
    HasAttachment,
    Unread,
    Read,
    Flagged,
    Categorized,
    Importance(Importance),
    Larger(u64),
    Smaller(u64),
    Before(Date),
    After(Date),
    On(Date),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Term {
    negate: bool,
    kind: Kind,
}

/// A parsed search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    terms: Vec<Term>,
}

/// Split on spaces outside double quotes; the quotes are dropped.
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in s.chars().take(MAX_QUERY) {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_date(s: &str) -> Option<Date> {
    if let Some(d) = Date::parse_iso(s) {
        return Some(d);
    }
    let mut parts = s.split('/');
    let (m, d, y) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    Date::new(y, m, d)
}

/// `100kb`, `2 MB`, `1.5mb`, `5000`: bytes.
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let (number, unit) = match s.find(|c: char| !(c.is_ascii_digit() || c == '.')) {
        Some(at) => (s.get(..at)?, s.get(at..)?.trim()),
        None => (s.as_str(), ""),
    };
    let n: f64 = number.parse().ok()?;
    let scale = match unit {
        "" | "b" => 1.0,
        "k" | "kb" => 1024.0,
        "m" | "mb" => 1024.0 * 1024.0,
        "g" | "gb" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = n * scale;
    (bytes.is_finite() && bytes >= 0.0).then(|| bytes.min(u64::MAX as f64) as u64)
}

/// Whether anyone in `list` matches `needle`, or one of the comma-separated people in it.
fn any_address(list: &[Address], needle: &str) -> bool {
    let one = |needle: &str| list.iter().any(|a| contains_ci(&a.name, needle) || contains_ci(&a.email, needle));
    if needle.contains(',') { needle.split(',').map(str::trim).filter(|n| !n.is_empty()).any(one) } else { one(needle) }
}

impl Query {
    pub fn parse(s: &str) -> Query {
        let mut terms = Vec::new();
        for token in tokens(s).into_iter().take(MAX_TERMS) {
            let (negate, body) = match token.strip_prefix('-') {
                Some(rest) if !rest.is_empty() => (true, rest),
                _ => (false, token.as_str()),
            };
            let text = || Kind::Text(body.to_string());
            let kind = match body.split_once(':') {
                Some((key, value)) if !value.is_empty() => {
                    let v = value.to_string();
                    match key.to_ascii_lowercase().as_str() {
                        "from" => Kind::From(v),
                        "to" => Kind::To(v),
                        "cc" => Kind::Cc(v),
                        "subject" => Kind::Subject(v),
                        "body" => Kind::Body(v),
                        "category" | "categories" => Kind::Category(v),
                        "attachment" | "attachments" => Kind::Attachment(v),
                        "has" | "hasattachment" | "hasattachments" => match v.to_ascii_lowercase().as_str() {
                            "attachment" | "attachments" | "yes" | "true" => Kind::HasAttachment,
                            _ => text(),
                        },
                        "is" => match v.to_ascii_lowercase().as_str() {
                            "unread" => Kind::Unread,
                            "read" => Kind::Read,
                            "flagged" => Kind::Flagged,
                            "important" => Kind::Importance(Importance::High),
                            "categorized" | "categorised" => Kind::Categorized,
                            _ => text(),
                        },
                        "importance" => Importance::parse(&v).map_or_else(text, Kind::Importance),
                        "before" => parse_date(&v).map_or_else(text, Kind::Before),
                        "after" => parse_date(&v).map_or_else(text, Kind::After),
                        "on" | "received" | "sent" => parse_date(&v).map_or_else(text, Kind::On),
                        "larger" | "bigger" => parse_size(&v).map_or_else(text, Kind::Larger),
                        "smaller" => parse_size(&v).map_or_else(text, Kind::Smaller),
                        // `10:30`, `re: budget`, a URL: plain text.
                        _ => text(),
                    }
                }
                _ => text(),
            };
            if !matches!(&kind, Kind::Text(t) if t.is_empty()) {
                terms.push(Term { negate, kind });
            }
        }
        Query { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Whether any term reads a message's text (a bare word, or `body:`): such a search wants the
    /// bodies of the messages it looks at in memory.
    pub fn reads_bodies(&self) -> bool {
        self.terms.iter().any(|t| matches!(t.kind, Kind::Text(_) | Kind::Body(_)))
    }

    /// Whether `m` matches every term. Dates compare in a zone `offset_minutes` east of UTC.
    pub fn matches(&self, m: &Message, offset_minutes: i32) -> bool {
        let day = || DateTime::from_timestamp(m.date, offset_minutes).date;
        self.terms.iter().all(|t| {
            let hit = match &t.kind {
                Kind::Text(x) => {
                    contains_ci(&m.subject, x)
                        || any_address(std::slice::from_ref(&m.from), x)
                        || any_address(&m.to, x)
                        || any_address(&m.cc, x)
                        || contains_ci(&m.body.text, x)
                        || m.attachments.iter().any(|a| contains_ci(&a.name, x))
                }
                Kind::From(x) => any_address(std::slice::from_ref(&m.from), x),
                Kind::To(x) => any_address(&m.to, x),
                Kind::Cc(x) => any_address(&m.cc, x),
                Kind::Subject(x) => contains_ci(&m.subject, x),
                Kind::Body(x) => contains_ci(&m.body.text, x),
                Kind::Category(x) => m.categories.iter().any(|c| contains_ci(c, x)),
                Kind::Attachment(x) => m.attachments.iter().any(|a| contains_ci(&a.name, x)),
                Kind::HasAttachment => m.has_attachments(),
                Kind::Unread => !m.read,
                Kind::Read => m.read,
                Kind::Flagged => m.flag == FlagStatus::Flagged,
                Kind::Categorized => !m.categories.is_empty(),
                Kind::Larger(n) => m.size() > *n,
                Kind::Smaller(n) => m.size() < *n,
                Kind::Importance(i) => m.importance == *i,
                Kind::Before(d) => day() < *d,
                Kind::After(d) => day() > *d,
                Kind::On(d) => day() == *d,
            };
            hit != t.negate
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Attachment;

    fn msg() -> Message {
        Message {
            from: Address::new("Ada Lovelace", "ada@example.org"),
            to: vec![Address::new("Charles", "charles@example.org")],
            cc: vec![Address::new("", "team@example.org")],
            subject: "Budget review: Q4".into(),
            body: crate::Body { text: "Numbers attached. See the engine estimate.".into(), html: None },
            attachments: vec![Attachment::new("estimate.xlsx", "application/octet-stream", vec![1, 2, 3])],
            categories: vec!["Blue category".into()],
            // 2026-10-11T23:30:00Z
            date: 1_791_761_400,
            ..Default::default()
        }
    }

    fn hit(q: &str) -> bool {
        Query::parse(q).matches(&msg(), 0)
    }

    #[test]
    fn words_and_phrases_must_all_match() {
        assert!(hit("budget"));
        assert!(hit("BUDGET engine"));
        assert!(hit("\"budget review\""));
        assert!(!hit("\"review budget\""));
        assert!(!hit("budget invoice"));
        assert!(hit("estimate.xlsx") && hit("charles") && hit("team@"));
        assert!(hit(""));
        assert!(Query::parse("  \"\"  ").is_empty());
    }

    #[test]
    fn keywords_filter_fields() {
        assert!(hit("from:ada") && !hit("from:charles"));
        assert!(hit("to:charles") && hit("cc:team") && !hit("to:team"));
        assert!(hit("subject:q4") && !hit("subject:engine") && hit("body:engine"));
        assert!(hit("from:\"Ada Lovelace\""));
        assert!(hit("has:attachment") && hit("hasattachment:yes") && hit("attachment:estimate"));
        assert!(hit("is:unread") && !hit("is:read") && !hit("is:flagged"));
        assert!(hit("category:blue") && !hit("category:red"));
        assert!(hit("importance:normal") && !hit("is:important"));
    }

    #[test]
    fn negation_and_dates() {
        assert!(hit("budget -invoice") && !hit("budget -engine") && !hit("-from:ada") && hit("-is:read"));
        assert!(hit("on:2026-10-11") && hit("after:2026-10-10") && hit("before:10/12/2026"));
        assert!(!hit("after:2026-10-11") && !hit("before:2026-10-11"));
        // In a zone an hour east it is already the 12th.
        assert!(Query::parse("on:2026-10-12").matches(&msg(), 60));
    }

    #[test]
    fn sizes_categories_and_several_people() {
        // The message is about 40 bytes of text and 3 of attachment.
        assert!(hit("larger:10") && !hit("larger:1kb") && hit("smaller:1kb") && !hit("smaller:10"));
        assert!(hit("larger:0.01kb") && !hit("larger:1MB"));
        assert!(!hit("larger:lots") && !hit("larger:5parsecs"));
        assert!(hit("is:categorized") && !Query::parse("is:categorized").matches(&Message::default(), 0));
        assert!(hit("from:bob,ada") && hit("from:,ada@example.org,") && !hit("from:bob,carol"));
        assert!(hit("to:x,charles") && !hit("-from:bob,ada"));
        assert_eq!(parse_size("2mb"), Some(2 * 1024 * 1024));
        assert_eq!(parse_size("1e400"), None);
        assert_eq!(parse_size("99999999999999999999999gb"), Some(u64::MAX));
    }

    #[test]
    fn unknown_keywords_and_junk_are_text() {
        assert!(hit("review: q4"));
        assert!(!hit("nosuch:thing"));
        assert!(!hit("before:someday"));
        assert!(Query::parse("-").matches(&Message { subject: "a - b".into(), ..Default::default() }, 0));
        let long = "x ".repeat(10_000);
        assert!(Query::parse(&long).terms.len() <= MAX_TERMS);
        let _ = Query::parse("\"unterminated from:").matches(&msg(), i32::MAX);
    }
}
