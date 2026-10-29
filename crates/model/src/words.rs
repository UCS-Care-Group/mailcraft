// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Words, as search reads them: the one tokeniser shared by the search box's query and the word
//! index kept beside the mailbox (R6.2), so that both find the same thing.
//!
//! A word is a run of letters and digits. Case and accents don't count: `Gvožđar` is `gvozdar`,
//! `Straße` is `strasse`, `Ærø` is `aero`; a combining mark is dropped where it stands, without
//! splitting the word. Everything else (spaces, punctuation, `@`, `.`) ends a word, so an address
//! is its parts (`ada@example.org` is `ada`, `example`, `org`). A Chinese character or a Japanese
//! kana is a word by itself, as those languages put no spaces between words. Only the first
//! [`MAX_WORD`] characters of a word count.
//!
//! A search word matches the start of a word ([`starts_in`]): `meet` finds `Meeting`. Several
//! search words must be found one after another (a phrase).

use crate::Message;

/// Characters of a word that count; the rest of a longer word is passed over.
pub const MAX_WORD: usize = 32;

/// The parts of a message search reads, one bit each: what a keyword limits a word to (`from:`)
/// and what the word index notes of each word.
pub const SUBJECT: u8 = 1;
pub const FROM: u8 = 2;
pub const TO: u8 = 4;
pub const CC: u8 = 8;
pub const BODY: u8 = 16;
pub const ATTACHMENT: u8 = 32;
/// Every part.
pub const ALL: u8 = SUBJECT | FROM | TO | CC | BODY | ATTACHMENT;

/// Call `f` with each piece of text of `m` that search reads and the part it is in, until it
/// returns `true`; returns whether it did. The subject, the name and the address of each person it
/// is from, to and copied to (each a piece of its own, so that a phrase doesn't run from one into
/// the next), `body` for its text (the message's own, or what was read of it from disk), and the
/// name of each attachment, and the words the sender hid in its HTML when it has any (#599). The one list of what search reads: the search box's and the word
/// index's.
pub fn texts<'a>(m: &'a Message, body: &'a str, f: impl FnMut(u8, &'a str) -> bool) -> bool {
    texts_with(m, body, &m.hidden_text, f)
}

/// [`texts`], with `hidden` the words the sender hid (the message's own, or what was read of them
/// from disk).
pub fn texts_with<'a>(m: &'a Message, body: &'a str, hidden: &'a str, mut f: impl FnMut(u8, &'a str) -> bool) -> bool {
    f(SUBJECT, &m.subject)
        || f(FROM, &m.from.name)
        || f(FROM, &m.from.email)
        || m.to.iter().any(|a| f(TO, &a.name) || f(TO, &a.email))
        || m.cc.iter().any(|a| f(CC, &a.name) || f(CC, &a.email))
        || f(BODY, body)
        || (!hidden.is_empty() && f(BODY, hidden))
        || m.attachments.iter().any(|a| f(ATTACHMENT, &a.name))
}

/// Whether `c` is a combining mark (an accent written after its letter): dropped from a word.
pub(crate) fn is_mark(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036F}' | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}' | '\u{20D0}'..='\u{20FF}' | '\u{FE20}'..='\u{FE2F}')
}

/// Whether `c` is written without spaces around its words: each is a word by itself.
fn stands_alone(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{30FF}'   // Hiragana, Katakana
        | '\u{3400}'..='\u{4DBF}' // CJK Extension A
        | '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{FF66}'..='\u{FF9F}' // Halfwidth Katakana
        | '\u{20000}'..='\u{2FA1F}')
}

/// The letters a lower-case letter with an accent (or a letter written as two) reads as.
fn plain(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' | 'ǎ' | 'ǻ' | 'ȁ' | 'ȃ' | 'ạ' | 'ả' | 'ấ' | 'ầ' | 'ẩ' | 'ẫ' | 'ậ' | 'ắ' | 'ằ' | 'ẳ'
        | 'ẵ' | 'ặ' => "a",
        'æ' | 'ǽ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' | 'ȅ' | 'ȇ' | 'ẹ' | 'ẻ' | 'ẽ' | 'ế' | 'ề' | 'ể' | 'ễ' | 'ệ' => {
            "e"
        }
        'ĝ' | 'ğ' | 'ġ' | 'ģ' | 'ǧ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' | 'ǐ' | 'ȉ' | 'ȋ' | 'ỉ' | 'ị' => "i",
        'ĳ' => "ij",
        'ĵ' => "j",
        'ķ' | 'ǩ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' | 'ŉ' | 'ǹ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' | 'ơ' | 'ǒ' | 'ǫ' | 'ǿ' | 'ȍ' | 'ȏ' | 'ọ' | 'ỏ' | 'ố' | 'ồ' | 'ổ' | 'ỗ' | 'ộ' | 'ớ'
        | 'ờ' | 'ở' | 'ỡ' | 'ợ' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' | 'ȑ' | 'ȓ' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' | 'ſ' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' | 'ț' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' | 'ư' | 'ǔ' | 'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' | 'ȕ' | 'ȗ' | 'ụ' | 'ủ' | 'ứ' | 'ừ' | 'ử'
        | 'ữ' | 'ự' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' | 'ỳ' | 'ỵ' | 'ỷ' | 'ỹ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        // Greek with its accents, and a final sigma.
        'ά' => "α",
        'έ' => "ε",
        'ή' => "η",
        'ί' | 'ϊ' | 'ΐ' => "ι",
        'ό' => "ο",
        'ύ' | 'ϋ' | 'ΰ' => "υ",
        'ώ' => "ω",
        'ς' => "σ",
        // Cyrillic letters often written without their marks.
        'ё' => "е",
        'ї' => "і",
        _ => return None,
    })
}

/// `c` as words compare: lower case, accents dropped, pushed onto `out`; returns how many
/// characters that pushed.
fn push_folded(c: char, out: &mut String) -> usize {
    if c.is_ascii() {
        out.push(c.to_ascii_lowercase());
        return 1;
    }
    let mut n = 0;
    for lower in c.to_lowercase() {
        if is_mark(lower) {
            continue;
        }
        match plain(lower) {
            Some(p) => {
                out.push_str(p);
                n += p.chars().count();
            }
            None => {
                out.push(lower);
                n += 1;
            }
        }
    }
    n
}

/// Call `f` with each word of `text`, folded ([`fold`]), in order, until it returns `true`;
/// returns whether it did. Nothing is allocated but one word's room.
pub fn each(text: &str, mut f: impl FnMut(&str) -> bool) -> bool {
    let mut word = String::new();
    let mut chars = 0usize;
    for c in text.chars() {
        if is_mark(c) {
            continue;
        }
        if stands_alone(c) {
            if !word.is_empty() && f(&word) {
                return true;
            }
            word.clear();
            chars = 0;
            push_folded(c, &mut word);
            if f(&word) {
                return true;
            }
            word.clear();
        } else if c.is_alphanumeric() {
            if chars < MAX_WORD {
                let at = word.len();
                let added = push_folded(c, &mut word);
                if chars + added > MAX_WORD {
                    // Cut at the limit, at a character's boundary.
                    let keep: usize = word.get(at..).map_or(0, |tail| tail.char_indices().nth(MAX_WORD - chars).map_or(tail.len(), |(i, _)| i));
                    word.truncate(at + keep);
                    chars = MAX_WORD;
                } else {
                    chars += added;
                }
            }
        } else if !word.is_empty() {
            if f(&word) {
                return true;
            }
            word.clear();
            chars = 0;
        }
    }
    !word.is_empty() && f(&word)
}

/// The words of `text`, folded, in order.
pub fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    each(text, |w| {
        out.push(w.to_string());
        false
    });
    out
}

/// One word folded as words compare (`Straße` → `strasse`); the same as the first word of
/// [`words`] for a single word.
pub fn fold(text: &str) -> String {
    words(text).concat()
}

/// Whether `text` holds the search words `wanted` (folded, as [`words`] makes them) one after
/// another, each at the start of a word: `["meet"]` is in "The Meeting", `["budget", "rev"]` in
/// "Budget review". No words are in every text.
pub fn starts_in(text: &str, wanted: &[String]) -> bool {
    match wanted {
        [] => true,
        [one] => each(text, |w| w.starts_with(one.as_str())),
        _ => {
            // The last few words seen, to check a run of them against the search words.
            let mut last: std::collections::VecDeque<String> = std::collections::VecDeque::with_capacity(wanted.len());
            each(text, |w| {
                if last.len() == wanted.len() {
                    last.pop_front();
                }
                last.push_back(w.to_string());
                last.len() == wanted.len() && last.iter().zip(wanted).all(|(w, x)| w.starts_with(x.as_str()))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_split_on_what_isnt_a_letter_or_digit() {
        assert_eq!(words("Budget review: Q4"), ["budget", "review", "q4"]);
        assert_eq!(words("ada.lovelace@example.org"), ["ada", "lovelace", "example", "org"]);
        assert_eq!(words("estimate.xlsx — 2026-10-11"), ["estimate", "xlsx", "2026", "10", "11"]);
        assert!(words(" \t--@@..!? ").is_empty());
        assert_eq!(words("don't"), ["don", "t"]);
    }

    #[test]
    fn case_and_accents_dont_count() {
        assert_eq!(words("Gvožđar"), ["gvozdar"]);
        assert_eq!(words("Straße ÆRØ Œuvre Łódź"), ["strasse", "aero", "oeuvre", "lodz"]);
        assert_eq!(words("CAFÉ café cafe\u{301}"), ["cafe", "cafe", "cafe"]);
        assert_eq!(words("İstanbul"), ["istanbul"]);
        assert_eq!(words("Ἀθῆναι ΆΘΗΝΑ"), words("ἀθῆναι άθηνα"));
        assert_eq!(fold("Ёлка"), "елка");
    }

    #[test]
    fn chinese_and_japanese_characters_are_words_by_themselves() {
        assert_eq!(words("会議の資料"), ["会", "議", "の", "資", "料"]);
        assert_eq!(words("Q4会議"), ["q4", "会", "議"]);
        assert!(starts_in("明日の会議について", &words("会議")));
        assert!(!starts_in("明日の会議について", &words("議会")));
    }

    #[test]
    fn a_search_word_matches_the_start_of_a_word() {
        assert!(starts_in("The Meeting is at noon", &words("meet")));
        assert!(!starts_in("The Meeting is at noon", &words("eeting")));
        assert!(starts_in("Đorđe Gvožđar", &words("gvozdar")));
        assert!(starts_in("Budget review: Q4", &words("budget rev")));
        assert!(!starts_in("Budget review: Q4", &words("review budget")));
        assert!(!starts_in("Budget and review", &words("budget review")));
        assert!(starts_in("anything", &[]));
        assert!(starts_in("team@example.org", &words("team@")));
    }

    #[test]
    fn long_words_count_as_far_as_the_limit() {
        let long = "a".repeat(100);
        assert_eq!(words(&long), ["a".repeat(MAX_WORD)]);
        assert!(starts_in(&format!("x {long} y"), &words(&"a".repeat(60))));
        // A letter that folds to two at the limit is cut at the limit.
        let edge = format!("{}ß", "a".repeat(MAX_WORD - 1));
        assert_eq!(words(&edge)[0].chars().count(), MAX_WORD);
    }

    #[test]
    fn texts_names_each_part_of_a_message() {
        let m = Message {
            subject: "Plan".into(),
            from: crate::Address::new("Ada", "ada@example.org"),
            to: vec![crate::Address::new("Kit", "kit@example.org")],
            cc: vec![crate::Address::new("", "team@example.org")],
            attachments: vec![crate::Attachment::new("plan.pdf", "application/pdf", vec![1])],
            ..Default::default()
        };
        let mut seen = Vec::new();
        texts(&m, "the text", |field, text| {
            seen.push((field, text.to_string()));
            false
        });
        let want = [(SUBJECT, "Plan"), (FROM, "Ada"), (FROM, "ada@example.org"), (TO, "Kit"), (TO, "kit@example.org")];
        assert_eq!(seen[..5], want.map(|(f, t)| (f, t.to_string())));
        assert_eq!(seen[5..], [(CC, String::new()), (CC, "team@example.org".into()), (BODY, "the text".into()), (ATTACHMENT, "plan.pdf".into())]);
        assert!(texts(&m, "", |field, _| field == TO));
    }

    proptest::proptest! {
        #[test]
        fn any_text_splits_without_panicking(text in "\\PC{0,200}", bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..300)) {
            let lossy = String::from_utf8_lossy(&bytes);
            for t in [text.as_str(), &lossy] {
                let ws = words(t);
                for w in &ws {
                    proptest::prop_assert!(!w.is_empty() && w.chars().count() <= MAX_WORD);
                    // A word is its own folding: folding twice changes nothing.
                    proptest::prop_assert_eq!(&fold(w), w);
                }
                proptest::prop_assert!(starts_in(t, &ws.iter().take(3).cloned().collect::<Vec<_>>()));
            }
        }
    }
}
