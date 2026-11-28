// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn the_whole_list_is_walked_to_the_first_family_known() {
    assert_eq!(stand_in("aptos,calibri,sans-serif"), (Generic::Own, 400));
    assert_eq!(stand_in("my brand font,georgia,serif"), (Generic::Serif, 400));
    assert_eq!(stand_in("unknown one,unknown two,monospace"), (Generic::Mono, 400));
    assert_eq!(stand_in("unknown one"), (Generic::Own, 400));
    assert_eq!(stand_in(""), (Generic::Own, 400));
    // The first known wins, as the first installed does on the desktop.
    assert_eq!(stand_in("courier new,times new roman"), (Generic::Mono, 400));
}

#[test]
fn office_families_come_to_their_kind() {
    for sans in ["calibri", "aptos", "segoe ui", "arial", "verdana", "tahoma", "helvetica neue"] {
        assert_eq!(known(sans), Some((Generic::Own, 400)), "{sans}");
    }
    for serif in ["georgia", "times new roman", "cambria", "garamond", "palatino linotype"] {
        assert_eq!(known(serif), Some((Generic::Serif, 400)), "{serif}");
    }
    for mono in ["courier new", "consolas", "lucida console", "cascadia code"] {
        assert_eq!(known(mono), Some((Generic::Mono, 400)), "{mono}");
    }
}

#[test]
fn a_name_may_give_a_weight_and_a_width() {
    assert_eq!(known("segoe ui light"), Some((Generic::Own, 300)));
    assert_eq!(known("segoe ui semilight"), Some((Generic::Own, 350)));
    assert_eq!(known("segoe ui semibold"), Some((Generic::Own, 600)));
    assert_eq!(known("calibri light"), Some((Generic::Own, 300)));
    assert_eq!(known("arial black"), Some((Generic::Own, 900)));
    assert_eq!(known("segoe ui bold"), Some((Generic::Own, 700)));
    assert_eq!(known("segoe ui regular"), Some((Generic::Own, 400)));
    assert_eq!(known("arial narrow bold"), Some((Generic::Condensed, 700)));
    assert_eq!(known("arial rounded mt bold"), Some((Generic::Own, 700)));
    assert_eq!(known("arial narrow"), Some((Generic::Condensed, 400)));
    assert_eq!(known("aptos narrow"), Some((Generic::Condensed, 400)));
    assert_eq!(known("franklin gothic medium cond"), Some((Generic::Condensed, 500)));
    assert_eq!(known("roboto condensed light"), Some((Generic::Condensed, 300)));
    // Only a sans-serif family has a narrow face here.
    assert_eq!(known("times new roman condensed"), Some((Generic::Serif, 400)));
    // A weight on a name that is not known says nothing.
    assert_eq!(known("brand light"), None);
    assert_eq!(known("light"), None);
    assert_eq!(known("cursive"), Some((Generic::Own, 400)));
}

/// The (bold, weight) of each span with text, in order.
fn weights(html: &str) -> Vec<(String, bool, Option<u16>)> {
    crate::parse(html)
        .iter()
        .flat_map(|b| b.spans.iter())
        .filter(|s| !s.text.trim().is_empty())
        .map(|s| (s.text.trim().to_string(), s.bold, s.weight))
        .collect()
}

#[test]
fn weights_are_kept_and_stepped_from_the_weight_around() {
    let got = weights(
        "<p><span style=\"font-weight:300\">light</span> <span style=\"font-weight:400\">regular</span> \
         <span style=\"font-weight:700\">bold</span> <span style=\"font-weight:900\">black</span> \
         <span style=\"font-weight:600\">semi</span> <span style=\"font:italic 200 12px Arial\">thin</span></p>",
    );
    assert_eq!(
        got,
        [
            ("light".into(), false, Some(300)),
            ("regular".into(), false, None),
            ("bold".into(), true, None),
            ("black".into(), true, Some(900)),
            ("semi".into(), true, Some(600)),
            ("thin".into(), false, Some(200)),
        ]
    );
    // `lighter` and `bolder` go from what the text is in (CSS Fonts 4).
    let got = weights(
        "<p><b><span style=\"font-weight:lighter\">a</span></b> <span style=\"font-weight:lighter\">b</span> \
         <span style=\"font-weight:300\"><span style=\"font-weight:bolder\">c</span></span> \
         <b><span style=\"font-weight:bolder\">d</span></b> \
         <span style=\"font-weight:900\"><span style=\"font-weight:lighter\">e</span></span></p>",
    );
    assert_eq!(
        got,
        [
            ("a".into(), false, None),
            ("b".into(), false, Some(100)),
            ("c".into(), false, None),
            ("d".into(), true, Some(900)),
            ("e".into(), true, None),
        ]
    );
    // A heading or a header cell that asks to be light is light; `<b>` inside light text is bold.
    let got = weights("<h1 style=\"font-weight:300\">head</h1><p style=\"font-weight:300\"><b>strong</b></p>");
    assert_eq!(got, [("head".into(), false, Some(300)), ("strong".into(), true, None)]);
}

/// CSS Fonts 4: `lighter` below 100 and `bolder` from 900 leave the weight as it is.
#[test]
fn a_step_past_the_ends_changes_nothing() {
    use crate::style::Weight;
    assert_eq!(Weight::Lighter.within(50), 50);
    assert_eq!(Weight::Lighter.within(100), 100);
    assert_eq!(Weight::Bolder.within(950), 950);
    assert_eq!(Weight::Bolder.within(900), 900);
    assert_eq!(Weight::Bolder.within(800), 900);
    assert_eq!(Weight::Lighter.within(600), 400);
}
