// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The list of format characters against Unicode 17.0, and the two steps a name takes: what they
//! remove, what they keep, how they cut, and that a name cleaned once stays as it is.

use super::*;
use proptest::prelude::*;

/// General category Cf in Unicode 17.0 (`DerivedGeneralCategory.txt`): 21 ranges, 170 characters.
const CF: [(u32, u32); 21] = [
    (0xad, 0xad),
    (0x600, 0x605),
    (0x61c, 0x61c),
    (0x6dd, 0x6dd),
    (0x70f, 0x70f),
    (0x890, 0x891),
    (0x8e2, 0x8e2),
    (0x180e, 0x180e),
    (0x200b, 0x200f),
    (0x202a, 0x202e),
    (0x2060, 0x2064),
    (0x2066, 0x206f),
    (0xfeff, 0xfeff),
    (0xfff9, 0xfffb),
    (0x110bd, 0x110bd),
    (0x110cd, 0x110cd),
    (0x13430, 0x1343f),
    (0x1bca0, 0x1bca3),
    (0x1d173, 0x1d17a),
    (0xe0001, 0xe0001),
    (0xe0020, 0xe007f),
];

fn all_chars() -> impl Iterator<Item = char> {
    (0..=0x10ffff).filter_map(char::from_u32)
}

#[test]
fn the_list_is_unicode_format_characters() {
    assert_eq!(all_chars().filter(|c| is_format(*c)).count(), 170);
    for (first, last) in CF {
        for c in (first..=last).filter_map(char::from_u32) {
            assert!(is_format(c), "U+{:04X} is a format character", u32::from(c));
        }
        for outside in [first - 1, last + 1] {
            let c = char::from_u32(outside).unwrap();
            assert!(!is_format(c), "U+{outside:04X} is not a format character");
        }
    }
    // Characters that show nothing but are not format characters: spaces, controls, a combining
    // mark, a variation selector, fillers, a code point not assigned between two ranges.
    for c in ['\u{0}', '\u{9}', '\u{a0}', '\u{2028}', '\u{34f}', '\u{fe0f}', '\u{3164}', '\u{2800}', '\u{fffc}', '\u{2065}'] {
        assert!(!is_format(c), "U+{:04X}", u32::from(c));
    }
}

#[test]
fn the_joiners_are_two_format_characters() {
    let joiners: Vec<char> = all_chars().filter(|c| is_joiner(*c)).collect();
    assert_eq!(joiners, ['\u{200c}', '\u{200d}']);
    assert!(joiners.iter().all(|c| is_format(*c)));
}

/// Both steps, when nothing is cut.
fn both(text: &str) -> String {
    let kept = incoming(text, 200).unwrap_or_default();
    assert_eq!(shown(text, 200), kept, "{text:?}");
    kept
}

#[test]
fn control_characters_go_and_white_space_parts_words() {
    assert_eq!(both("Ada\u{0}\u{7}\u{1b}\u{7f}\u{9b} Lovelace\u{1c}"), "Ada Lovelace");
    assert_eq!(both("A\u{1}da"), "Ada");
    // The information separators are controls, not white space, as Rust reads them.
    assert_eq!(both("Ada\u{1c}Love\u{1f}lace"), "AdaLovelace");
    assert_eq!(both("Ada\r\nLovelace\tand\u{2028}Byron\u{85}x\u{b}y\u{c}z"), "Ada Lovelace and Byron x y z");
    assert_eq!(both("  Ada \u{a0}\u{3000}  Lovelace  \n"), "Ada Lovelace");
}

#[test]
fn format_characters_go() {
    assert_eq!(both("Invoice\u{202e}fdp.exe"), "Invoicefdp.exe");
    assert_eq!(both("\u{feff}Re\u{200b}ports\u{2066}x\u{2069}\u{ad}\u{e0041}\u{e007f}"), "Reportsx");
    assert_eq!(both("\u{200e}Ada\u{200f} \u{2060}Lovelace\u{61c}"), "Ada Lovelace");
    // A format character parts no words: it shows nothing, so the words around it touch.
    assert_eq!(both("Ada\u{200b}Lovelace"), "AdaLovelace");
}

#[test]
fn a_joiner_stays_inside_a_word() {
    for word in ["👩\u{200d}💻", "👨\u{200d}👩\u{200d}👧", "می\u{200c}خواهم", "क्\u{200d}ष", "Ada \u{1f3f3}\u{fe0f}\u{200d}\u{1f308} Lovelace"]
    {
        assert_eq!(both(word), word);
    }
    // What is removed between them doesn't part them.
    assert_eq!(both("👩\u{202e}\u{200d}\u{200b}💻"), "👩\u{200d}💻");
    assert_eq!(both("👩\u{200d}\u{1}💻"), "👩\u{200d}💻");
    // A run of them is one, the last.
    assert_eq!(both("a\u{200c}\u{200d}\u{200d}b"), "a\u{200d}b");
}

#[test]
fn a_joiner_goes_anywhere_else() {
    for (text, expected) in [
        ("\u{200d}Ada", "Ada"),
        ("Ada\u{200d}", "Ada"),
        ("Ada \u{200d}Lovelace", "Ada Lovelace"),
        ("Ada\u{200c} Lovelace", "Ada Lovelace"),
        ("Ada\u{200d}\nLovelace", "Ada Lovelace"),
        ("\u{200c}\u{200d}", ""),
        (" \u{200d} ", ""),
    ] {
        assert_eq!(both(text), expected, "{text:?}");
    }
}

#[test]
fn nothing_left_is_no_name() {
    for text in ["", " ", "\n\t\r", "\u{0}\u{1b}", "\u{200b}\u{202e}\u{feff}", "\u{200d}", " \u{a0}\u{2028} \u{e0001}"] {
        assert_eq!(incoming(text, 120), None, "{text:?}");
        assert_eq!(shown(text, 120), "", "{text:?}");
    }
}

#[test]
fn incoming_is_cut_at_the_length() {
    assert_eq!(incoming("Lovelace", 3).as_deref(), Some("Lov"));
    assert_eq!(incoming("Lovelace", 8).as_deref(), Some("Lovelace"));
    assert_eq!(incoming("é😀ß漢", 3).as_deref(), Some("é😀ß"));
    // Never with a space or a joiner at the end.
    assert_eq!(incoming("Ada Lovelace", 4).as_deref(), Some("Ada"));
    assert_eq!(incoming("ab\u{200d}c", 3).as_deref(), Some("ab"));
    // Spaces and format characters cost nothing until something shows after them.
    assert_eq!(incoming("Ada \u{202e}\u{200b} \n", 3).as_deref(), Some("Ada"));
    assert_eq!(incoming("Ada", 0), None);
}

#[test]
fn shown_is_cut_with_an_ellipsis_inside_the_length() {
    for (text, max, expected) in [
        ("Lovelace", 8, "Lovelace"),
        ("Lovelace", 7, "Lovela…"),
        ("Lovelace", 4, "Lov…"),
        ("Ada Lovelace", 5, "Ada…"),
        ("Ada Lovelace", 4, "Ada…"),
        ("ab\u{200d}cd", 4, "ab…"),
        ("Ada  \u{200b}\n", 3, "Ada"),
        ("Ada", 1, "…"),
        ("Ada", 0, ""),
        ("", 0, ""),
    ] {
        assert_eq!(shown(text, max), expected, "{text:?} at {max}");
    }
    let long = "é\u{202e}\u{0}x ".repeat(50_000);
    assert_eq!(shown(&long, 80).chars().count(), 80);
    assert!(shown(&long, 80).ends_with('…'));
}

#[test]
fn a_hostile_name_comes_out_short() {
    let name = format!("{}a{}b{}", "\u{202e}".repeat(1_000_000), "\u{200d}".repeat(1_000_000), " ".repeat(1_000_000));
    assert_eq!(incoming(&name, 120).as_deref(), Some("a\u{200d}b"));
    let words = "word ".repeat(1_000_000);
    assert_eq!(incoming(&words, 120).map(|n| n.chars().count()), Some(119));
    assert_eq!(shown(&words, 120).chars().count(), 120);
}

/// Text with every kind of character the steps treat apart, and some that are kept.
fn name_text() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just('a'),
        Just('Ž'),
        Just('漢'),
        Just('😀'),
        Just('\u{301}'),
        Just('…'),
        Just(' '),
        Just('\u{a0}'),
        Just('\n'),
        Just('\t'),
        Just('\u{2028}'),
        Just('\u{0}'),
        Just('\u{1b}'),
        Just('\u{85}'),
        Just('\u{200b}'),
        Just('\u{202e}'),
        Just('\u{2066}'),
        Just('\u{feff}'),
        Just('\u{e0041}'),
        Just('\u{200c}'),
        Just('\u{200d}'),
        any::<char>(),
    ];
    prop_oneof![prop::collection::vec(piece, 0..48).prop_map(|v| v.into_iter().collect()), any::<String>()]
}

/// The characters of `text` that show, in order: what a cleaned name holds besides its spaces
/// and joiners.
fn showing(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace() && !c.is_control() && !is_format(*c)).collect()
}

/// Whether `name`, cut from `full` for `max` characters, is as long as it could be: the next
/// character that shows, with the space or joiner before it, would not have fitted.
fn cut_late(name: &str, full: &str, max: usize) -> bool {
    let rest = full.get(name.len()..).unwrap_or_default();
    let next = rest.chars().take_while(|c| *c == ' ' || is_joiner(*c)).count() + 1;
    rest.is_empty() || name.chars().count() + next > max
}

/// What either step makes: one line, a space only between two characters that show, a joiner
/// only inside a word, and nothing else that shows nothing.
fn is_clean(name: &str) -> bool {
    let chars: Vec<char> = name.chars().collect();
    let shows = |i: Option<usize>| i.and_then(|i| chars.get(i)).is_some_and(|c| *c != ' ' && !is_joiner(*c));
    chars.iter().enumerate().all(|(i, c)| {
        if *c == ' ' || is_joiner(*c) {
            shows(i.checked_sub(1)) && shows(i.checked_add(1))
        } else {
            !c.is_control() && !c.is_whitespace() && !is_format(*c)
        }
    })
}

proptest! {
    #[test]
    fn incoming_names_are_clean_short_and_stay_as_they_are(text in name_text(), max in 0usize..40) {
        let full = incoming(&text, usize::MAX).unwrap_or_default();
        prop_assert_eq!(showing(&full), showing(&text));
        prop_assert!(is_clean(&full), "{:?} from {:?}", full, text);
        match incoming(&text, max) {
            Some(name) => {
                prop_assert!(!name.is_empty());
                prop_assert!(name.chars().count() <= max);
                prop_assert!(is_clean(&name), "{:?} from {:?}", name, text);
                prop_assert!(full.starts_with(&name), "{:?} is not the start of {:?}", name, full);
                prop_assert!(cut_late(&name, &full, max), "{:?} from {:?} at {}", name, full, max);
                prop_assert_eq!(incoming(&name, max), Some(name.clone()));
            }
            None => prop_assert!(full.is_empty() || max == 0),
        }
    }

    #[test]
    fn shown_names_are_clean_fit_and_stay_as_they_are(text in name_text(), max in 0usize..40) {
        let full = incoming(&text, usize::MAX).unwrap_or_default();
        let name = shown(&text, max);
        prop_assert!(name.chars().count() <= max);
        prop_assert!(is_clean(&name), "{:?} from {:?}", name, text);
        prop_assert_eq!(shown(&name, max), name.clone());
        if full.chars().count() <= max {
            prop_assert_eq!(&name, &full);
        } else if max > 0 {
            let kept = name.strip_suffix('…').unwrap_or_default();
            prop_assert!(name.ends_with('…') && full.starts_with(kept), "{:?} from {:?}", name, full);
            prop_assert!(cut_late(kept, &full, max - 1), "{:?} from {:?} at {}", name, full, max);
        }
    }
}
