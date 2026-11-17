// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #603: a paragraph's direction from its first strong letter, and the side it stands at. The
//! text is made up: Hebrew and Arabic words with Latin, digits and marks round them.

use super::{AlignFrom, Dir, first_strong, placed, rtl};
use crate::Align;

fn strong(text: &str) -> Option<Dir> {
    first_strong(text.chars())
}

#[test]
fn the_first_letter_that_has_a_direction_decides() {
    assert_eq!(strong("שלום, מה נשמע?"), Some(Dir::Rtl));
    assert_eq!(strong("مرحبا بالعالم"), Some(Dir::Rtl));
    assert_eq!(strong("ܫܠܡܐ"), Some(Dir::Rtl), "Syriac");
    assert_eq!(strong("ދިވެހި"), Some(Dir::Rtl), "Thaana");
    assert_eq!(strong("ߒߞߏ"), Some(Dir::Rtl), "N'Ko");
    assert_eq!(strong("Hello שלום"), Some(Dir::Ltr));
    assert_eq!(strong("Grüße"), Some(Dir::Ltr));
    assert_eq!(strong("Привет"), Some(Dir::Ltr));
    assert_eq!(strong("こんにちは"), Some(Dir::Ltr));
}

#[test]
fn digits_punctuation_and_marks_say_nothing() {
    assert_eq!(strong("123 שלום"), Some(Dir::Rtl));
    assert_eq!(strong("12:30 – Meeting"), Some(Dir::Ltr));
    assert_eq!(strong("«2024» ١٢٣ مرحبا"), Some(Dir::Rtl), "Arabic-Indic digits are weak");
    assert_eq!(strong("١٢٣"), None);
    assert_eq!(strong("۱۲۳ hello"), Some(Dir::Ltr), "extended Arabic-Indic digits are weak");
    assert_eq!(strong("\u{05B8}\u{05D0}"), Some(Dir::Rtl), "a Hebrew point before the letter");
    assert_eq!(strong("\u{064E}abc"), Some(Dir::Ltr), "an Arabic mark before a Latin letter");
    assert_eq!(strong("  ... !? 42 "), None);
    assert_eq!(strong(""), None);
}

#[test]
fn direction_marks_count_and_isolates_are_skipped() {
    assert_eq!(strong("\u{200F}Invoice 42"), Some(Dir::Rtl), "a right-to-left mark");
    assert_eq!(strong("\u{200E}שלום"), Some(Dir::Ltr), "a left-to-right mark");
    assert_eq!(strong("\u{061C}abc"), Some(Dir::Rtl), "an Arabic letter mark");
    assert_eq!(strong("\u{2068}Example Ltd\u{2069} שלום"), Some(Dir::Rtl), "a Latin name in an isolate");
    assert_eq!(strong("\u{2066}\u{2067}abc\u{2069}def\u{2069}שלום"), Some(Dir::Rtl), "nested isolates");
    assert_eq!(strong("\u{2067}שלום"), None, "an isolate that never ends hides the rest");
    assert_eq!(strong("\u{2069}\u{2069}abc"), Some(Dir::Ltr), "a stray end of isolate");
}

#[test]
fn what_the_message_says_beats_the_first_letter() {
    assert!(rtl(Dir::Rtl, "Hello".chars()));
    assert!(!rtl(Dir::Ltr, "שלום".chars()));
    assert!(rtl(Dir::Auto, "שלום Hello".chars()));
    assert!(!rtl(Dir::Auto, "Hello שלום".chars()));
    assert!(!rtl(Dir::Auto, "2024".chars()), "nothing strong reads left to right");
}

#[test]
fn only_an_alignment_that_follows_the_direction_turns_round() {
    use Align::{Center, Left, Right};
    assert_eq!(placed(Left, AlignFrom::Nothing, true), Right);
    assert_eq!(placed(Left, AlignFrom::Nothing, false), Left);
    assert_eq!(placed(Left, AlignFrom::Flow, true), Right, "start, justify");
    assert_eq!(placed(Right, AlignFrom::Flow, true), Left, "end");
    assert_eq!(placed(Right, AlignFrom::Flow, false), Right);
    assert_eq!(placed(Center, AlignFrom::Flow, true), Center);
    assert_eq!(placed(Left, AlignFrom::Side, true), Left, "left is left");
    assert_eq!(placed(Right, AlignFrom::Side, true), Right);
}

#[test]
fn the_dir_attribute_is_read_in_any_case_and_direction_in_css() {
    assert_eq!(Dir::attr("RTL"), Some(Dir::Rtl));
    assert_eq!(Dir::attr(" ltr "), Some(Dir::Ltr));
    assert_eq!(Dir::attr("Auto"), Some(Dir::Auto));
    assert_eq!(Dir::attr("right"), None);
    assert_eq!(Dir::css("rtl"), Some(Dir::Rtl));
    assert_eq!(Dir::css("auto"), None, "CSS has no auto");
}
