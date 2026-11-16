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
    /// Three forms (`plural=(n==1 ? 0 : (n==0 || (n%100 > 0 && n%100 < 20)) ? 1 : 2)`): Romanian.
    /// One for 1; another for 0 and for 2 to 19 past the hundreds (101, 119); the third, written
    /// with "de", for the rest (20, 100, 120).
    Romanian,
    /// Three forms (`plural=(n==1 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2)`): Polish.
    /// One for 1; another for counts ending 2 to 4 except 12 to 14 (2, 22, 104); the third for the rest.
    Polish,
    /// Four forms (`plural=(n%100==1 ? 0 : n%100==2 ? 1 : n%100==3 || n%100==4 ? 2 : 3)`): Slovenian.
    /// Singular when the last two digits are 1 (1, 101); dual when they are 2 (2, 102); the nominative
    /// plural for 3 and 4 (3, 4, 103); the genitive plural for the rest (0, 5, 11, 12, 22, 100).
    Slovenian,
}

impl Rule {
    /// The form a count of `n` takes.
    pub fn index(self, n: u64) -> usize {
        match self {
            Rule::Single => 0,
            Rule::NotOne => usize::from(n != 1),
            Rule::AboveOne => usize::from(n > 1),
            Rule::Polish => match n {
                1 => 0,
                n if (2..=4).contains(&(n % 10)) && !(10..20).contains(&(n % 100)) => 1,
                _ => 2,
            },
            Rule::Slovenian => match n % 100 {
                1 => 0,
                2 => 1,
                3 | 4 => 2,
                _ => 3,
            },
            Rule::Romanian => match n {
                1 => 0,
                0 => 1,
                n if (1..20).contains(&(n % 100)) => 1,
                _ => 2,
            },
        }
    }

    /// How many forms the rule has.
    pub fn forms(self) -> usize {
        match self {
            Rule::Single => 1,
            Rule::NotOne | Rule::AboveOne => 2,
            Rule::Romanian | Rule::Polish => 3,
            Rule::Slovenian => 4,
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
            (Some(3), Some("n==1?0:(n==0||(n%100>0&&n%100<20))?1:2")) => Rule::Romanian,
            (Some(3), Some("n==1?0:n%10>=2&&n%10<=4&&(n%100<10||n%100>=20)?1:2")) => Rule::Polish,
            (Some(4), Some("n%100==1?0:n%100==2?1:n%100==3||n%100==4?2:3")) => Rule::Slovenian,
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
        let ro = Rule::from_header("nplurals=3; plural=(n==1 ? 0 : (n==0 || (n%100 > 0 && n%100 < 20)) ? 1 : 2);");
        assert_eq!(ro, Rule::Romanian);
        let forms: Vec<usize> = [1, 0, 2, 19, 20, 100, 101, 119, 120, 1001].iter().map(|n| ro.index(*n)).collect();
        assert_eq!(forms, [0, 1, 1, 1, 2, 2, 1, 1, 2, 1]);
        assert_eq!(ro.forms(), 3);
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
