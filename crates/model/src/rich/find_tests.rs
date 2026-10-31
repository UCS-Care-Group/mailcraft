// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use proptest::prelude::*;

use super::find::moved;
use super::*;

fn bold() -> Style {
    Style { bold: true, ..Style::default() }
}

fn doc() -> RichText {
    RichText::from_paragraphs(vec![
        Paragraph::new(vec![Run::plain("The cat sat "), Run::new("on the Cat", bold())], ListKind::None),
        Paragraph::new(vec![Run::plain("cat"), Run::picture(1, Style::default()), Run::plain("catalogue")], ListKind::Bullet),
    ])
}

#[test]
fn finds_every_place_in_order_with_or_without_letter_case() {
    let t = doc();
    // "The cat sat on the Cat\ncat\u{fffc}catalogue"
    assert_eq!(t.find("cat", false, 0, t.len()), [(4, 7), (19, 22), (23, 26), (27, 30)]);
    assert_eq!(t.find("cat", true, 0, t.len()), [(4, 7), (23, 26), (27, 30)]);
    assert_eq!(t.find("Cat", true, 0, t.len()), [(19, 22)]);
    // Within the range only, wholly.
    assert_eq!(t.find("cat", false, 5, 26), [(19, 22), (23, 26)]);
    assert_eq!(t.find("cat", false, 0, 6), []);
    // Across runs of different looks.
    assert_eq!(t.find("sat on", false, 0, t.len()), [(8, 14)]);
}

#[test]
fn finds_nothing_for_what_cant_be_in_one_line() {
    let t = doc();
    for needle in ["", "Cat\ncat", "a\u{0}", "\u{fffc}", &"x".repeat(MAX_FIND + 1)] {
        assert!(t.find(needle, false, 0, t.len()).is_empty(), "{needle:?}");
    }
    assert!(t.find("cat", false, 9, 3).is_empty());
    assert!(t.find("cat", false, 0, 10_000).len() == 4);
}

#[test]
fn case_folding_keeps_places_as_long_as_the_text() {
    // 'İ' lowers to two characters: compared as it is, and nothing shifts.
    let t = RichText::from_plain("İstanbul ÉCOLE école");
    assert_eq!(t.find("école", false, 0, t.len()), [(9, 14), (15, 20)]);
    assert_eq!(t.find("stan", false, 0, t.len()), [(1, 5)]);
}

#[test]
fn replacing_keeps_the_look_of_each_place() {
    let mut t = doc();
    let places = t.find("cat", false, 0, t.len());
    t.replace_places(&places, "dog").unwrap();
    assert_eq!(t.text(), "The dog sat on the dog\ndog\u{fffc}dogalogue");
    assert_eq!(t.paragraphs()[0].runs, [Run::plain("The dog sat "), Run::new("on the dog", bold())]);
    assert_eq!(t.pictures(), [1]);
    assert_eq!(t.paragraphs()[1].list, ListKind::Bullet);
    // Over two looks: the first one's.
    let mut t = doc();
    let places = t.find("sat on", false, 0, t.len());
    t.replace_places(&places, "").unwrap();
    assert_eq!(t.text(), "The cat  the Cat\ncat\u{fffc}catalogue");
    let mut t = doc();
    t.replace_places(&[(8, 14)], "lay\non").unwrap();
    assert_eq!(t.paragraphs()[0].text(), "The cat layon the Cat");
}

#[test]
fn bad_places_change_nothing() {
    let t = doc();
    for places in [vec![(4, 7), (5, 8)], vec![(7, 4)], vec![(5, 5)], vec![(20, 25)], vec![(26, 28)], vec![(100, 101)], vec![(22, 24)]] {
        let mut u = t.clone();
        assert!(u.replace_places(&places, "x").is_err(), "{places:?}");
        assert_eq!(u, t);
    }
    // Too long for the message.
    let mut big = RichText::from_plain(&"ab".repeat(MAX_CHARS / 2));
    let places = big.find("a", true, 0, big.len());
    assert!(big.replace_places(&places, "xyz").is_err());
}

#[test]
fn replace_all_is_one_undo_step_and_the_selection_follows() {
    let mut b = RichBody::new(doc());
    b.select(27, 30).unwrap();
    let places = b.text().find("cat", false, 0, b.text().len());
    b.replace_places(&places, "horse").unwrap();
    assert_eq!(b.plain(), "The horse sat on the horse\nhorse\u{fffc}horsealogue");
    assert_eq!(b.selection(), Selection { anchor: 33, head: 38 });
    assert!(b.undo());
    assert_eq!(b.text(), &doc());
    assert!(!b.can_undo());
}

#[test]
fn positions_move_with_what_changed_before_them() {
    let places = [(2, 4), (6, 7)];
    assert_eq!(moved(0, &places, 3), 0);
    assert_eq!(moved(2, &places, 3), 2);
    assert_eq!(moved(3, &places, 3), 5);
    assert_eq!(moved(4, &places, 3), 5);
    assert_eq!(moved(6, &places, 3), 7);
    assert_eq!(moved(9, &places, 3), 12);
    assert_eq!(moved(9, &places, 0), 6);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn finding_and_replacing_anything_keeps_the_document_whole(
        lines in proptest::collection::vec("[aAbB \\t\u{130}é]{0,12}", 1..5),
        needle in "[aAbB\u{130}é\n]{0,3}",
        with in "\\PC{0,4}",
        case in any::<bool>(),
        from in 0usize..40,
        to in 0usize..60,
    ) {
        let mut t = RichText::from_plain(&lines.join("\n"));
        let places = t.find(&needle, case, from, to);
        for w in places.windows(2) {
            prop_assert!(w[0].1 <= w[1].0);
        }
        for (a, b) in &places {
            prop_assert!(*a >= from && *b <= to && b > a);
        }
        let before = t.len();
        if t.replace_places(&places, &with).is_ok() {
            prop_assert!(t.len() <= before + places.len() * 4);
            prop_assert_eq!(t.len(), t.text().chars().count());
            prop_assert!(t.paragraphs().iter().all(|p| p.runs.iter().all(|r| !r.text.is_empty())));
        }
    }
}
