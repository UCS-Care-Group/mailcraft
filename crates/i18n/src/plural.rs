// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Plural forms: which `msgstr[n]` a count takes. A catalogue names its rule in the `Plural-Forms`
//! header as a C expression; rather than evaluate whatever a file holds, the rules languages use
//! are recognised by their shape, and anything else counts as English's.

/// A language's plural rule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rule {
    /// One form for every count (`nplurals=1; plural=0;`).
    Single,
    /// Singular for 1 only (`plural=(n != 1)`): English, German, Spanish…
    #[default]
    NotOne,
    /// Singular for 0 and 1 (`plural=(n > 1)`): French.
    AboveOne,
}

impl Rule {
    /// The form a count of `n` takes.
    pub fn index(self, n: u64) -> usize {
        match self {
            Rule::Single => 0,
            Rule::NotOne => usize::from(n != 1),
            Rule::AboveOne => usize::from(n > 1),
        }
    }

    /// How many forms the rule has.
    pub fn forms(self) -> usize {
        match self {
            Rule::Single => 1,
            Rule::NotOne | Rule::AboveOne => 2,
        }
    }

    /// The rule a `Plural-Forms` header names (`nplurals=2; plural=(n > 1);`); English's when it
    /// names one this doesn't know.
    pub fn from_header(value: &str) -> Rule {
        let mut nplurals = None;
        let mut plural = None;
        for part in value.split(';') {
            let Some((k, v)) = part.split_once('=') else { continue };
            match k.trim() {
                "nplurals" => nplurals = v.trim().parse::<u8>().ok(),
                "plural" => plural = Some(normalise(v)),
                _ => {}
            }
        }
        match (nplurals, plural.as_deref()) {
            (Some(1), Some("0")) => Rule::Single,
            (Some(2), Some("n>1" | "n>=2" | "n>1?1:0")) => Rule::AboveOne,
            _ => Rule::NotOne,
        }
    }
}

/// The expression without blanks and without the brackets around the whole of it.
fn normalise(expr: &str) -> String {
    let mut s: String = expr.chars().filter(|c| !c.is_whitespace()).take(256).collect();
    while s.starts_with('(') && s.ends_with(')') && s.len() >= 2 {
        let inner = s.get(1..s.len() - 1).unwrap_or("");
        // Only when the brackets match each other (not `(a)+(b)`).
        let mut depth = 0i32;
        let balanced = inner.chars().all(|c| {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            depth >= 0
        });
        if !balanced || depth != 0 {
            break;
        }
        s = inner.to_string();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_rules_by_their_shape() {
        assert_eq!(Rule::from_header("nplurals=2; plural=(n > 1);"), Rule::AboveOne);
        assert_eq!(Rule::from_header("nplurals=2; plural=n>1;"), Rule::AboveOne);
        assert_eq!(Rule::from_header("nplurals=2; plural=(n != 1);"), Rule::NotOne);
        assert_eq!(Rule::from_header("nplurals=1; plural=0;"), Rule::Single);
        assert_eq!(Rule::from_header("nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : 2);"), Rule::NotOne);
        assert_eq!(Rule::from_header("garbage((("), Rule::NotOne);
        assert_eq!(Rule::from_header("nplurals=2; plural=(n>1)+(n>2);"), Rule::NotOne);
        assert_eq!((Rule::AboveOne.index(0), Rule::AboveOne.index(1), Rule::AboveOne.index(2)), (0, 0, 1));
        assert_eq!((Rule::NotOne.index(0), Rule::NotOne.index(1), Rule::NotOne.index(2)), (1, 0, 1));
    }
}
