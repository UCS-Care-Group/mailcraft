// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Paragraphs aligned left, centred or right (#580): how they are stored, how editing keeps
//! them, and that a list item is always left.

use super::Align::{Center, Left, Right};
use super::tests::check;
use super::*;

fn centred(text: &str) -> Paragraph {
    Paragraph::new(vec![Run::plain(text)], ListKind::None).aligned(Center)
}

fn aligns(t: &RichText) -> Vec<Align> {
    t.paragraphs().iter().map(|p| p.align).collect()
}

#[test]
fn an_unaligned_document_is_stored_byte_for_byte_as_before() {
    let t = RichText::from_paragraphs(vec![
        Paragraph::new(vec![Run::new("Agenda", Style { bold: true, ..Style::default() })], ListKind::None),
        Paragraph::new(vec![Run::plain("first")], ListKind::Bullet).at_level(1),
    ]);
    assert_eq!(
        serde_json::to_string(&t).unwrap(),
        r#"{"paragraphs":[{"runs":[{"text":"Agenda","bold":true}]},{"runs":[{"text":"first"}],"list":"bullet","level":1}]}"#
    );
}

#[test]
fn a_stored_paragraph_without_align_reads_as_left_and_one_with_it_keeps_it() {
    let json = r#"{"paragraphs":[{"runs":[{"text":"a"}]},{"runs":[{"text":"b"}],"align":"center"},{"runs":[{"text":"c"}],"align":"right"}]}"#;
    let t: RichText = serde_json::from_str(json).unwrap();
    assert_eq!(aligns(&t), [Left, Center, Right]);
    let stored = serde_json::to_string(&t).unwrap();
    assert_eq!(stored, json);
    assert_eq!(serde_json::from_str::<RichText>(&stored).unwrap(), t);
    // A list item stored aligned (by hand, say) is left.
    let item: RichText = serde_json::from_str(r#"{"paragraphs":[{"runs":[{"text":"x"}],"list":"bullet","align":"center"}]}"#).unwrap();
    assert_eq!(aligns(&item), [Left]);
    check(&t);
    check(&item);
}

#[test]
fn a_stored_paragraph_with_an_unknown_align_reads_as_left_and_the_rest_of_the_document_is_kept() {
    // What a later version might write: a value this one doesn't know, or not a value at all.
    let json = r#"{"paragraphs":[
        {"runs":[{"text":"Avery","bold":true}],"align":"justify"},
        {"runs":[{"text":"Lumen"}],"align":7},
        {"runs":[{"text":"Studio"}],"align":"right"}
    ]}"#;
    let t: RichText = serde_json::from_str(json).unwrap();
    assert_eq!(t.text(), "Avery\nLumen\nStudio");
    assert_eq!(aligns(&t), [Left, Left, Right]);
    assert!(t.paragraphs()[0].runs[0].style.bold);
    check(&t);
}

#[test]
fn a_list_item_is_never_aligned_and_a_centred_paragraph_made_an_item_is_left() {
    assert_eq!(Paragraph::new(vec![Run::plain("x")], ListKind::Bullet).aligned(Center).align, Left);
    let mut t = RichText::from_paragraphs(vec![centred("one"), centred("two"), Paragraph::new(vec![Run::plain("three")], ListKind::Numbered)]);
    assert_eq!(aligns(&t), [Center, Center, Left]);
    // Aligning passes list items by.
    t.set_align(0, t.len(), Right).unwrap();
    assert_eq!(aligns(&t), [Right, Right, Left]);
    t.set_list(0, 3, ListKind::Bullet).unwrap();
    assert_eq!(aligns(&t), [Left, Right, Left]);
    // An ordinary paragraph again, it stays left.
    t.set_list(0, 3, ListKind::None).unwrap();
    assert_eq!(t.align_at(0), Left);
    assert!(t.all_aligned(4, 7, Right).unwrap() && !t.all_aligned(0, 7, Right).unwrap());
    assert_eq!(t.align_at(99), Left, "past the end");
    check(&t);
}

#[test]
fn enter_in_a_centred_paragraph_makes_a_centred_one_and_backspace_joins_keeping_the_first() {
    let mut b = RichBody::from_plain("Avery Quinn\nLumen");
    b.select(0, 0).unwrap();
    b.set_align(Center).unwrap();
    b.select(5, 5).unwrap();
    b.type_text("\n").unwrap();
    assert_eq!(b.plain(), "Avery\n Quinn\nLumen");
    assert_eq!(aligns(b.text()), [Center, Center, Left]);
    // At the end of the line too, and what is typed on the new one is centred with it.
    b.select(12, 12).unwrap();
    b.type_text("\nStudio").unwrap();
    assert_eq!(b.plain(), "Avery\n Quinn\nStudio\nLumen");
    assert_eq!(aligns(b.text()), [Center, Center, Center, Left]);
    // Backspace at the start of the left line joins it to the centred one: that stays centred.
    b.select(20, 20).unwrap();
    b.backspace().unwrap();
    assert_eq!(b.plain(), "Avery\n Quinn\nStudioLumen");
    assert_eq!(aligns(b.text()), [Center, Center, Center]);
    // A left line joined with the centred one after it stays left.
    b.select(0, 0).unwrap();
    b.set_align(Left).unwrap();
    b.select(5, 5).unwrap();
    b.delete_forward().unwrap();
    assert_eq!(aligns(b.text()), [Left, Center]);
    check(b.text());
}

#[test]
fn a_pasted_aligned_paragraph_aligns_the_one_it_starts_and_the_text_after_the_caret_joins_its_last() {
    let fragment = RichText::from_paragraphs(vec![centred("Lumen"), Paragraph::new(vec![Run::plain("Studio")], ListKind::None).aligned(Right)]);
    // At a paragraph's start, that paragraph takes the first one's alignment; what was after the
    // caret joins the last, and is aligned as it is.
    let mut t = RichText::from_plain("Avery\nQuinn");
    t.insert_rich(6, &fragment).unwrap();
    assert_eq!(t.text(), "Avery\nLumen\nStudioQuinn");
    assert_eq!(aligns(&t), [Left, Center, Right]);
    // Inside a paragraph, that one keeps its own.
    let mut t = RichText::from_plain("Avery Quinn");
    t.insert_rich(5, &fragment).unwrap();
    assert_eq!(t.text(), "AveryLumen\nStudio Quinn");
    assert_eq!(aligns(&t), [Left, Right]);
    // A left paragraph put in at the start of a centred one (a picture, say) leaves it centred.
    let mut t = RichText::from_paragraphs(vec![centred("Avery")]);
    t.insert_rich(0, &RichText::from_paragraphs(vec![Paragraph::new(vec![Run::picture(1, Style::default())], ListKind::None)])).unwrap();
    assert_eq!(aligns(&t), [Center]);
    // A list item pasted there makes it an item, and so left.
    let mut u = RichText::from_paragraphs(vec![centred("Avery")]);
    u.insert_rich(0, &RichText::from_paragraphs(vec![Paragraph::new(vec![Run::plain("x")], ListKind::Bullet)])).unwrap();
    assert_eq!((u.paragraphs()[0].list, u.paragraphs()[0].align), (ListKind::Bullet, Left));
    check(&t);
    check(&u);
}

#[test]
fn an_aligned_paragraph_pasted_at_the_start_of_a_list_item_leaves_it_a_left_item() {
    for kind in [ListKind::Bullet, ListKind::Numbered] {
        for text in ["item", ""] {
            for align in [Center, Right] {
                let fragment = RichText::from_paragraphs(vec![Paragraph::new(vec![Run::plain("x")], ListKind::None).aligned(align)]);
                let mut t = RichText::from_paragraphs(vec![Paragraph::new(vec![Run::plain(text)], kind)]);
                t.insert_rich(0, &fragment).unwrap();
                assert_eq!((t.text(), t.list_at(0), t.align_at(0)), (format!("x{text}"), kind, Left), "{kind:?} {text:?} {align:?}");
                check(&t);
                // Several paragraphs: the first joins the item, which stays left.
                let mut fragment = fragment.clone();
                fragment.insert_rich(1, &RichText::from_plain("\ny")).unwrap();
                let mut t = RichText::from_paragraphs(vec![Paragraph::new(vec![Run::plain(text)], kind)]);
                t.insert_rich(0, &fragment).unwrap();
                assert_eq!((t.text(), t.list_at(0), t.align_at(0)), (format!("x\ny{text}"), kind, Left), "{kind:?} {text:?} {align:?}");
                check(&t);
            }
        }
    }
}

#[test]
fn a_selection_that_ends_at_a_lines_start_does_not_align_that_line() {
    // "Avery" and its break: Lumen is not in it.
    let mut t = RichText::from_plain("Avery\nLumen");
    t.set_align(0, 6, Center).unwrap();
    assert_eq!(aligns(&t), [Center, Left]);
    assert!(t.all_aligned(0, 6, Center).unwrap() && t.all_aligned(6, 0, Center).unwrap());
    t.set_align(6, 0, Right).unwrap();
    assert_eq!(aligns(&t), [Right, Left]);
    // A caret at a line's start is in that line.
    t.set_align(6, 6, Center).unwrap();
    assert_eq!(aligns(&t), [Right, Center]);
    assert!(t.all_aligned(6, 6, Center).unwrap() && !t.all_aligned(5, 6, Center).unwrap());
    // An empty line the selection ends at is not in it either, but an empty last line is: Select
    // All ends there.
    let mut t = RichText::from_plain("Avery\n\nLumen\n");
    t.set_align(0, 6, Center).unwrap();
    assert_eq!(aligns(&t), [Center, Left, Left, Left]);
    t.set_align(0, t.len(), Right).unwrap();
    assert_eq!(aligns(&t), [Right; 4]);
    assert!(t.all_aligned(0, t.len(), Right).unwrap());
    check(&t);
}

#[test]
fn a_selection_that_ends_at_a_lines_start_does_not_make_that_line_a_list_item_or_cost_it_its_alignment() {
    // "Avery" and its break, either way round: Lumen, centred, is not in it.
    for (from, to) in [(0, 6), (6, 0)] {
        let mut t = RichText::from_paragraphs(vec![Paragraph::new(vec![Run::plain("Avery")], ListKind::None), centred("Lumen")]);
        t.set_list(from, to, ListKind::Bullet).unwrap();
        assert_eq!((t.list_at(0), t.list_at(6), aligns(&t)), (ListKind::Bullet, ListKind::None, vec![Left, Center]), "{from} {to}");
        assert!(t.all_lists(from, to, ListKind::Bullet).unwrap(), "{from} {to}");
        t.set_list(from, to, ListKind::None).unwrap();
        assert_eq!((t.list_at(0), aligns(&t)), (ListKind::None, vec![Left, Center]), "{from} {to}");
        check(&t);
    }
    // Clear All Formatting leaves that line's bullet as it leaves its alignment.
    let mut b = RichBody::from_plain("Avery\nLumen");
    b.select(6, 6).unwrap();
    b.toggle_list(ListKind::Bullet).unwrap();
    b.select(0, 6).unwrap();
    b.clear_formatting().unwrap();
    assert_eq!(b.text().list_at(6), ListKind::Bullet);
    // An empty last line is in it: Select All ends there.
    let mut t = RichText::from_plain("Avery\n");
    t.set_list(0, t.len(), ListKind::Numbered).unwrap();
    assert!(t.all_lists(0, t.len(), ListKind::Numbered).unwrap());
    assert_eq!(t.list_at(t.len()), ListKind::Numbered);
}

#[test]
fn aligning_the_selection_is_one_undo_step_and_clear_formatting_makes_it_left() {
    let mut b = RichBody::from_plain("Avery\nLumen\nStudio");
    b.select(2, 8).unwrap();
    b.set_align(Right).unwrap();
    assert_eq!(aligns(b.text()), [Right, Right, Left]);
    assert!(b.text().has_formatting());
    // Lumen's break is the last thing a selection to 12 takes, so Studio is not in it.
    assert!(b.text().all_aligned(0, 12, Right).unwrap() && !b.text().all_aligned(0, 13, Right).unwrap());
    assert_eq!(b.text().align_at(9), Right);
    // Aligned the same way again, nothing changes, so there is no step to undo for it.
    b.set_align(Right).unwrap();
    assert!(b.undo());
    assert_eq!(aligns(b.text()), [Left, Left, Left]);
    assert!(!b.can_undo());
    assert!(b.redo());
    b.select(0, 18).unwrap();
    b.clear_formatting().unwrap();
    assert_eq!(aligns(b.text()), [Left, Left, Left]);
    assert!(!b.text().has_formatting());
    assert!(b.undo());
    assert_eq!(b.text().align_at(0), Right);
}
