// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn hostile_files_do_not_panic(aff in any::<String>(), dic in any::<String>()) {
        let aff = truncate(&aff, 3000);
        let dic = truncate(&dic, 6000);
        let _ = Dictionary::parse(&aff, &dic);
    }

    #[test]
    fn check_text_ranges_are_bounded(text in any::<String>(), limit in 0usize..12) {
        let text = truncate(&text, 400);
        let Some(dict) = en_us() else { return Ok(()) };
        let ranges = dict.check_text(&text, limit);
        prop_assert!(ranges.len() <= limit);
        let mut last = 0usize;
        for range in ranges {
            prop_assert!(range.start <= range.end && range.end <= text.len());
            prop_assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
            prop_assert!(range.start >= last);
            last = range.end;
        }
    }

    #[test]
    fn suggest_does_not_panic(word in any::<String>()) {
        let word = truncate(&word, 80);
        if let Some(dict) = en_us() {
            let _ = dict.suggest(&word, &[&word]);
        }
    }
}

fn truncate(text: &str, max: usize) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if out.len().saturating_add(c.len_utf8()) > max {
            break;
        }
        out.push(c);
    }
    out
}
