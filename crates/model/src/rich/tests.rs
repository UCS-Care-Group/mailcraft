// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use proptest::prelude::*;

use super::*;

fn bold() -> Style {
    Style { bold: true, ..Style::default() }
}

fn link(url: &str) -> Style {
    Style { link: Some(url.into()), ..Style::default() }
}

/// The invariants every document keeps.
fn check(t: &RichText) {
    assert!(!t.paragraphs().is_empty());
    assert_eq!(t.len(), t.text().chars().count());
    for p in t.paragraphs() {
        assert!(p.level <= MAX_LEVEL && (p.list != ListKind::None || p.level == 0), "{t:?}");
        for r in &p.runs {
            assert!(!r.text.is_empty(), "{t:?}");
            assert!(r.text.chars().all(|c| c == '\t' || !c.is_control()), "{t:?}");
            assert_eq!(r.style.clone().clean(), r.style);
        }
        for w in p.runs.windows(2) {
            assert!(w[0].style != w[1].style || w[0].picture.is_some() || w[1].picture.is_some(), "{t:?}");
        }
        for r in &p.runs {
            assert!(r.picture.is_none_or(|_| r.text == OBJECT.to_string()) && (r.picture.is_some() || !r.text.contains(OBJECT)), "{t:?}");
        }
    }
    assert!(t.len() <= MAX_CHARS && t.paragraphs().len() <= MAX_PARAGRAPHS);
}

#[test]
fn plain_text_is_one_paragraph_per_line() {
    let t = RichText::from_plain("Hello\r\nworld\rand\nmore\u{0}\u{7}\tend");
    assert_eq!(t.text(), "Hello\nworld\nand\nmore\tend");
    assert_eq!(t.paragraphs().len(), 4);
    assert_eq!(t.len(), 24);
    assert!(!t.has_formatting());
    check(&t);
    assert_eq!(RichText::from_plain("").paragraphs().len(), 1);
    assert!(RichText::new().is_empty());
}

#[test]
fn links_are_any_well_formed_scheme_but_the_unsafe_ones() {
    for ok in [
        "https://example.org",
        "HTTP://example.org/a?b=c#d",
        "mailto:ada@example.org",
        "tel:+15550100",
        " https://example.org/x ",
        "ms-settings:network-vpn",
        "MS-Settings:network-vpn",
        "slack://channel?id=1",
        "x-apple.systempreferences:com.apple.preference.network",
        "git+ssh://host/repo",
    ] {
        assert!(safe_link(ok).is_some(), "{ok}");
    }
    for bad in [
        "",
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        "data:text/html,x",
        "cid:logo@example.org",
        "file:///etc/passwd",
        "FILE:///etc/passwd",
        "vbscript:msgbox(1)",
        "VBScript:msgbox(1)",
        "DATA:text/html,x",
        "  javascript:alert(1)",
        "\tjavascript:alert(1)",
        "java\tscript:alert(1)",
        "java\r\nscript:alert(1)",
        "\u{a0}javascript:alert(1)",
        "javascript:",
        "ms-settings:",
        "1abc:x",
        "-abc:x",
        "ab_c:x",
        "ab c:x",
        ":x",
        "https://exa mple.org",
        "https://example.org/\n",
        "java\nscript:x",
        "https:",
        "https:/example.org",
        "https:///example.org",
        "mailto:",
        "example.org",
        "//example.org",
        "https://[::1",
        "http://:80/x",
        "https://[zz]/",
    ] {
        assert!(safe_link(bad).is_none(), "{bad:?}");
    }
    assert!(safe_link(&format!("https://example.org/{}", "a".repeat(MAX_LINK))).is_none());
    assert_eq!(clamp_size(1), MIN_SIZE);
    assert_eq!(clamp_size(255), MAX_SIZE);
}

#[test]
fn typing_inside_a_run_takes_the_style_given() {
    let mut t = RichText::from_plain("Hello world");
    t.format(0, 5, &Format::Bold(true)).unwrap();
    assert_eq!(t.paragraphs()[0].runs, [Run::new("Hello", bold()), Run::plain(" world")]);
    assert_eq!(t.insert(2, "XX", &bold()).unwrap(), 4);
    assert_eq!(t.paragraphs()[0].runs, [Run::new("HeXXllo", bold()), Run::plain(" world")]);
    assert_eq!(t.style_at(0), bold());
    assert_eq!(t.style_at(7), bold());
    assert_eq!(t.style_at(8), Style::default());
    check(&t);
    assert!(t.insert(99, "x", &Style::default()).is_err());
}

#[test]
fn a_line_break_splits_a_paragraph_and_a_list_goes_on() {
    let mut t = RichText::from_plain("one two");
    t.set_list(0, 0, ListKind::Bullet).unwrap();
    t.insert(3, "\nthree\n", &Style::default()).unwrap();
    assert_eq!(t.text(), "one\nthree\n two");
    assert!(t.paragraphs().iter().all(|p| p.list == ListKind::Bullet));
    check(&t);
}

#[test]
fn deleting_across_paragraphs_joins_them() {
    let mut t = RichText::from_plain("alpha\nbeta\ngamma");
    t.set_list(0, 0, ListKind::Numbered).unwrap();
    t.format(8, 13, &Format::Italic(true)).unwrap();
    t.delete(3, 8).unwrap();
    assert_eq!(t.text(), "alpta\ngamma");
    let p = &t.paragraphs()[0];
    assert_eq!(p.list, ListKind::Numbered);
    assert_eq!(p.runs, [Run::plain("alp"), Run::new("ta", Style { italic: true, ..Style::default() })]);
    // Order doesn't matter, past the end does.
    t.delete(5, 0).unwrap();
    assert_eq!(t.text(), "\ngamma");
    assert!(t.delete(0, 100).is_err());
    assert_eq!(t.text(), "\ngamma");
    t.delete(0, 6).unwrap();
    assert!(t.is_empty());
    check(&t);
}

#[test]
fn formatting_spans_paragraphs_and_neighbours_merge() {
    let mut t = RichText::from_plain("ab\ncd");
    t.format(1, 4, &Format::Underline(true)).unwrap();
    assert_eq!(t.paragraphs()[0].runs.len(), 2);
    assert_eq!(t.paragraphs()[1].runs.len(), 2);
    t.format(0, 5, &Format::Underline(true)).unwrap();
    assert_eq!(t.paragraphs()[0].runs.len(), 1);
    assert!(t.all(0, 5, |s| s.underline).unwrap());
    t.format(0, 5, &Format::Size(Some(200))).unwrap();
    assert_eq!(t.style_at(1).size, Some(MAX_SIZE));
    assert!(t.format(0, 1, &Format::Link(Some("javascript:alert(1)".into()))).is_err());
    t.format(0, 2, &Format::Link(Some("https://example.org".into()))).unwrap();
    assert_eq!(t.link_span(1), Some((0, 2)));
    assert_eq!(t.link_span(0), Some((0, 2)));
    assert_eq!(t.link_span(2), Some((0, 2)));
    assert_eq!(t.link_span(4), None);
    t.format(0, 5, &Format::Clear).unwrap();
    assert!(!t.has_formatting());
    check(&t);
}

#[test]
fn a_link_in_runs_of_different_looks_is_one_link() {
    let mut t = RichText::from_plain("see the page now");
    t.format(4, 12, &Format::Link(Some("https://example.org".into()))).unwrap();
    t.format(8, 12, &Format::Bold(true)).unwrap();
    assert_eq!(t.link_span(5), Some((4, 12)));
    assert_eq!(t.link_span(11), Some((4, 12)));
}

#[test]
fn plain_text_part_has_markers_and_addresses() {
    let t = RichText::from_paragraphs(vec![
        Paragraph::new(vec![Run::plain("Agenda")], ListKind::None),
        Paragraph::new(vec![Run::plain("first")], ListKind::Numbered),
        Paragraph::new(vec![Run::plain("second")], ListKind::Numbered),
        Paragraph::new(vec![Run::plain("point")], ListKind::Bullet),
        Paragraph::new(vec![Run::plain("again")], ListKind::Numbered),
        Paragraph::new(
            vec![Run::new("site", link("https://example.org")), Run::plain(" or "), Run::new("example.org", link("https://example.org"))],
            ListKind::None,
        ),
        Paragraph::new(vec![Run::new("ada@example.org", link("mailto:ada@example.org"))], ListKind::None),
    ]);
    assert_eq!(t.to_plain(), "Agenda\n1. first\n2. second\n- point\n1. again\nsite <https://example.org> or example.org\nada@example.org");
}

#[test]
fn slices_keep_formatting() {
    let mut t = RichText::from_plain("one\ntwo\nthree");
    t.set_list(4, 13, ListKind::Bullet).unwrap();
    t.format(5, 10, &Format::Bold(true)).unwrap();
    let s = t.slice(5, 10).unwrap();
    assert_eq!(s.text(), "wo\nth");
    assert!(s.paragraphs().iter().all(|p| p.list == ListKind::Bullet));
    assert!(s.all(0, s.len(), |st| st.bold).unwrap());
    let mut u = RichText::from_plain("X");
    u.insert_rich(1, &s).unwrap();
    assert_eq!(u.text(), "Xwo\nth");
    // The first pasted paragraph joins where it went in, keeping that paragraph's kind; the
    // last brings its own.
    assert_eq!(u.paragraphs()[0].list, ListKind::None);
    assert_eq!(u.paragraphs()[1].list, ListKind::Bullet);
    // Pasted at the start of a paragraph, a list item stays one.
    let mut v = RichText::from_plain("Z");
    v.insert_rich(0, &s).unwrap();
    assert_eq!(v.paragraphs()[0].list, ListKind::Bullet);
    check(&u);
    check(&v);
}

#[test]
fn a_plain_change_keeps_the_formatting_around_it() {
    let mut t = RichText::from_plain("Hello world");
    t.format(0, 5, &Format::Bold(true)).unwrap();
    t.format(6, 11, &Format::Italic(true)).unwrap();
    t.set_text("Hello there world").unwrap();
    assert_eq!(t.text(), "Hello there world");
    assert!(t.all(0, 5, |s| s.bold).unwrap());
    assert!(t.all(12, 17, |s| s.italic).unwrap());
    t.set_text("Hello there world").unwrap();
    t.set_text("").unwrap();
    assert!(t.is_empty());
    check(&t);
}

#[test]
fn stored_documents_are_cleaned_when_read() {
    let json = r#"{"paragraphs":[
        {"runs":[{"text":"a\nb","bold":true},{"text":"","italic":true},{"text":"c","bold":true,"size":200,"link":"javascript:x"}],"list":"bullet"},
        {"runs":[{"text":"\u0000x\u0007"}]}
    ]}"#;
    let t: RichText = serde_json::from_str(json).unwrap();
    check(&t);
    assert_eq!(t.text(), "a\nbc\nx");
    assert_eq!(t.paragraphs()[1].list, ListKind::Bullet);
    assert_eq!(t.paragraphs()[1].runs.len(), 2);
    assert_eq!(t.paragraphs()[1].runs[1].style, Style { bold: true, size: Some(MAX_SIZE), ..Style::default() });
    let empty: RichText = serde_json::from_str(r#"{"paragraphs":[]}"#).unwrap();
    assert_eq!(empty, RichText::new());
    let back: RichText = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
    assert_eq!(back, t);
    assert!(serde_json::from_str::<RichText>(r#"{"paragraphs":[{"runs":[{"text":5}]}]}"#).is_err());
}

#[test]
fn huge_documents_are_capped() {
    let t = RichText::from_plain(&"x".repeat(MAX_CHARS + 10));
    assert_eq!(t.len(), MAX_CHARS);
    let t = RichText::from_plain(&"\n".repeat(MAX_PARAGRAPHS + 10));
    assert_eq!(t.paragraphs().len(), MAX_PARAGRAPHS);
    let mut t = RichText::from_plain(&"x".repeat(MAX_CHARS - 1));
    assert!(t.insert(0, "yy", &Style::default()).is_err());
    assert_eq!(t.len(), MAX_CHARS - 1);
    t.insert(0, "y", &Style::default()).unwrap();
    check(&t);
}

// ---- writing: RichBody ----

#[test]
fn a_body_saved_as_plain_text_still_opens() {
    let b: RichBody = serde_json::from_str(r#""Dear Ada,\n\nThanks.""#).unwrap();
    assert_eq!(b.plain(), "Dear Ada,\n\nThanks.");
    let b2: RichBody = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
    assert_eq!(b2.text(), b.text());
    // A selection past the end is brought back.
    let b3: RichBody = serde_json::from_str(r#"{"text":{"paragraphs":[{"runs":[{"text":"ab"}]}]},"selection":{"anchor":9,"head":1}}"#).unwrap();
    assert_eq!(b3.selection(), Selection { anchor: 2, head: 1 });
    let b4: RichBody = serde_json::from_str("{}").unwrap();
    assert!(b4.text().is_empty());
}

#[test]
fn typing_is_one_undo_step_until_the_caret_moves() {
    let mut b = RichBody::default();
    for c in ["H", "e", "y"] {
        b.type_text(c).unwrap();
    }
    assert_eq!(b.plain(), "Hey");
    b.type_text("\n").unwrap();
    b.type_text("A").unwrap();
    b.select(0, 0).unwrap();
    b.type_text(">").unwrap();
    assert_eq!(b.plain(), ">Hey\nA");
    assert!(b.undo());
    assert_eq!(b.plain(), "Hey\nA");
    assert!(b.undo());
    assert_eq!(b.plain(), "Hey\n");
    assert!(b.undo());
    assert_eq!(b.plain(), "Hey");
    assert!(b.undo());
    assert_eq!(b.plain(), "");
    assert!(!b.undo());
    assert!(b.redo() && b.redo());
    assert_eq!(b.plain(), "Hey\n");
    assert_eq!(b.selection(), Selection::caret(4));
    // A new edit forgets what could be redone.
    b.type_text("x").unwrap();
    assert!(!b.can_redo());
}

#[test]
fn bold_with_nothing_selected_is_for_what_is_typed_next() {
    let mut b = RichBody::from_plain("Hi ");
    b.select(3, 3).unwrap();
    assert!(b.toggle_bold().unwrap());
    assert_eq!(b.pending(), Some(&bold()));
    b.type_text("there").unwrap();
    assert_eq!(b.text().paragraphs()[0].runs, [Run::plain("Hi "), Run::new("there", bold())]);
    // Typing at the end of bold text goes on in bold.
    b.type_text("!").unwrap();
    assert_eq!(b.text().paragraphs()[0].runs[1].text, "there!");
    // Moving the caret forgets the choice.
    assert!(!b.toggle_bold().unwrap());
    b.select(1, 1).unwrap();
    assert!(b.pending().is_none());
    // Nothing changed in the text, so nothing to undo for it.
    assert!(b.undo());
    assert_eq!(b.plain(), "Hi ");
}

#[test]
fn toggling_a_selection_turns_it_on_unless_all_of_it_is() {
    let mut b = RichBody::from_plain("one two");
    b.select(0, 3).unwrap();
    assert!(b.toggle_bold().unwrap());
    b.select(7, 0).unwrap();
    assert!(b.toggle_bold().unwrap());
    assert!(b.text().all(0, 7, |s| s.bold).unwrap());
    assert!(!b.toggle_bold().unwrap());
    assert!(!b.text().has_formatting());
    assert!(b.toggle_italic().unwrap() && b.toggle_underline().unwrap());
    b.format(Format::Size(Some(14))).unwrap();
    assert!(b.text().all(0, 7, |s| s.italic && s.underline && s.size == Some(14)).unwrap());
    b.clear_formatting().unwrap();
    assert!(!b.text().has_formatting());
    assert!(b.undo());
    assert!(b.text().has_formatting());
}

#[test]
fn list_items_have_levels_numbered_level_by_level() {
    let item = |text: &str, list, level| Paragraph::new(vec![Run::plain(text)], list).at_level(level);
    let doc = RichText::from_paragraphs(vec![
        item("a", ListKind::Numbered, 0),
        item("a1", ListKind::Numbered, 1),
        item("a2", ListKind::Numbered, 1),
        item("dot", ListKind::Bullet, 2),
        item("a3", ListKind::Numbered, 1),
        item("b", ListKind::Numbered, 0),
        item("b1", ListKind::Numbered, 1),
        item("x", ListKind::Bullet, 1),
        item("b2", ListKind::Numbered, 1),
        item("plain", ListKind::None, 5),
        item("c", ListKind::Numbered, 99),
    ]);
    assert_eq!(numbers(doc.paragraphs()), [1, 1, 2, 0, 3, 2, 1, 0, 1, 0, 1]);
    assert_eq!(doc.paragraphs()[9].level, 0, "an ordinary paragraph has no level");
    assert_eq!(doc.paragraphs()[10].level, MAX_LEVEL);
    assert!(doc.to_plain().starts_with("1. a\n   1. a1\n   2. a2\n      - dot\n   3. a3\n2. b"));
    // Stored before levels: no level, 0; stored with a level out of bounds: the deepest.
    let old: RichText = serde_json::from_str(r#"{"paragraphs":[{"runs":[{"text":"x"}],"list":"bullet"}]}"#).unwrap();
    assert_eq!(old.paragraphs()[0].level, 0);
    let deep: RichText = serde_json::from_str(r#"{"paragraphs":[{"runs":[{"text":"x"}],"list":"bullet","level":200}]}"#).unwrap();
    assert_eq!(deep.paragraphs()[0].level, MAX_LEVEL);
    assert!(!serde_json::to_string(&old).unwrap().contains("level"));
}

#[test]
fn tab_and_shift_tab_move_items_between_levels() {
    let mut b = RichBody::from_plain("a\nb\nc");
    b.select(0, 5).unwrap();
    b.toggle_list(ListKind::Bullet).unwrap();
    b.select(2, 2).unwrap();
    for _ in 0..20 {
        b.indent(1).unwrap();
    }
    assert_eq!(b.text().paragraphs().iter().map(|p| p.level).collect::<Vec<_>>(), [0, MAX_LEVEL, 0]);
    assert!(!b.indent(1).unwrap(), "no deeper than the deepest");
    // Enter on an empty item a level deep goes a level out, then out of the list.
    b.select(5, 5).unwrap();
    b.type_text("\n").unwrap();
    b.indent(1).unwrap();
    assert_eq!(b.text().paragraphs()[3].level, 1);
    b.type_text("\n").unwrap();
    assert_eq!((b.text().paragraphs()[3].level, b.text().paragraphs()[3].list), (0, ListKind::Bullet));
    b.type_text("\n").unwrap();
    assert_eq!(b.text().paragraphs()[3].list, ListKind::None);
    // Shift+Tab: a level out; at the outermost level, out of the list.
    b.select(0, 5).unwrap();
    b.indent(-1).unwrap();
    assert_eq!(
        b.text().paragraphs().iter().map(|p| (p.list, p.level)).take(3).collect::<Vec<_>>(),
        [(ListKind::None, 0), (ListKind::Bullet, MAX_LEVEL - 1), (ListKind::None, 0)]
    );
    // Ordinary paragraphs don't indent; the step is one undo.
    let mut plain = RichBody::from_plain("x");
    assert!(!plain.indent(1).unwrap() && !plain.can_undo());
    b.undo();
    assert_eq!(b.text().paragraphs()[1].level, MAX_LEVEL);
}

#[test]
fn enter_goes_on_with_a_list_and_ends_it_on_an_empty_item() {
    let mut b = RichBody::from_plain("one");
    b.select(0, 0).unwrap();
    b.toggle_list(ListKind::Numbered).unwrap();
    b.select(3, 3).unwrap();
    b.type_text("\n").unwrap();
    b.type_text("two").unwrap();
    b.type_text("\n").unwrap();
    let kinds = |b: &RichBody| b.text().paragraphs().iter().map(|p| p.list).collect::<Vec<_>>();
    assert_eq!(kinds(&b), [ListKind::Numbered; 3]);
    // Enter on the empty item: no new paragraph, the item is an ordinary paragraph now.
    b.type_text("\n").unwrap();
    assert_eq!(kinds(&b), [ListKind::Numbered, ListKind::Numbered, ListKind::None]);
    assert_eq!((b.plain().as_str(), b.selection()), ("one\ntwo\n", Selection::caret(8)));
    // Once more, an ordinary new paragraph; and one undo step brings the item back.
    b.type_text("\n").unwrap();
    assert_eq!(b.plain(), "one\ntwo\n\n");
    b.undo();
    b.undo();
    assert_eq!(kinds(&b), [ListKind::Numbered; 3]);
    assert_eq!(b.text().paragraph_span(5), Some((4, 7)));
    assert_eq!(b.text().paragraph_span(99), None);
    assert!(b.text().paragraph_is_empty(8) && !b.text().paragraph_is_empty(0) && !b.text().paragraph_is_empty(99));
}

#[test]
fn lists_toggle_and_backspace_ends_an_item() {
    let mut b = RichBody::from_plain("a\nb\nc");
    b.select(0, 3).unwrap();
    assert_eq!(b.toggle_list(ListKind::Bullet).unwrap(), ListKind::Bullet);
    assert_eq!(b.text().paragraphs().iter().map(|p| p.list).collect::<Vec<_>>(), [ListKind::Bullet, ListKind::Bullet, ListKind::None]);
    assert_eq!(b.toggle_list(ListKind::Numbered).unwrap(), ListKind::Numbered);
    assert_eq!(b.toggle_list(ListKind::Numbered).unwrap(), ListKind::None);
    b.select(0, 0).unwrap();
    b.toggle_list(ListKind::Bullet).unwrap();
    b.select(2, 2).unwrap();
    b.toggle_list(ListKind::Bullet).unwrap();
    // At the start of an item, Backspace first ends the list there, then joins.
    b.backspace().unwrap();
    assert_eq!(b.text().paragraphs()[1].list, ListKind::None);
    assert_eq!(b.plain(), "a\nb\nc");
    b.backspace().unwrap();
    assert_eq!(b.plain(), "ab\nc");
    assert_eq!(b.selection(), Selection::caret(1));
    b.select(0, 0).unwrap();
    b.backspace().unwrap();
    b.backspace().unwrap();
    assert_eq!(b.plain(), "ab\nc");
    b.select(4, 4).unwrap();
    b.delete_forward().unwrap();
    b.select(2, 2).unwrap();
    b.delete_forward().unwrap();
    assert_eq!(b.plain(), "abc");
}

#[test]
fn links_are_made_put_in_and_removed() {
    let mut b = RichBody::from_plain("Read this.");
    b.select(5, 9).unwrap();
    assert!(b.set_link(Some("javascript:alert(1)")).is_err());
    assert!(!b.text().has_formatting());
    b.set_link(Some("https://example.org/a")).unwrap();
    assert_eq!(b.text().link_span(6), Some((5, 9)));
    b.select(7, 7).unwrap();
    b.set_link(None).unwrap();
    assert!(!b.text().has_formatting());
    // With nothing selected the address goes in as its own link, and typing after it is plain.
    b.select(10, 10).unwrap();
    b.type_text(" ").unwrap();
    b.set_link(Some("mailto:ada@example.org")).unwrap();
    assert_eq!(b.plain(), "Read this. ada@example.org");
    assert_eq!(b.text().style_at(b.text().len()).link.as_deref(), Some("mailto:ada@example.org"));
    b.type_text(" ok").unwrap();
    assert_eq!(b.text().style_at(b.text().len()).link, None);
    assert!(b.format(Format::Link(Some("data:x".into()))).is_err());
}

#[test]
fn failed_edits_change_nothing() {
    let mut b = RichBody::from_plain("abc");
    let before = b.clone();
    assert!(b.select(0, 9).is_err());
    assert!(b.delete(1, 9).is_err());
    assert!(b.paste(&RichText::from_plain(&"x".repeat(MAX_CHARS))).is_err());
    assert_eq!(b, before);
}

#[test]
fn a_plain_editor_change_keeps_formatting_and_is_one_step() {
    let mut b = RichBody::from_plain("Hello world");
    b.select(0, 5).unwrap();
    b.toggle_bold().unwrap();
    b.set_plain("Hello, world").unwrap();
    assert!(b.text().all(0, 5, |s| s.bold).unwrap());
    assert_eq!(b.selection(), Selection::caret(6));
    b.set_plain("Hello, world").unwrap();
    assert!(b.undo());
    assert_eq!(b.plain(), "Hello world");
}

#[test]
fn undo_is_bounded() {
    let mut b = RichBody::default();
    for i in 0..(MAX_UNDO + 20) {
        b.select(0, 0).unwrap();
        b.type_text(&i.to_string()).unwrap();
    }
    let mut steps = 0;
    while b.undo() {
        steps += 1;
    }
    assert_eq!(steps, MAX_UNDO);
}

#[test]
fn inserting_at_keeps_the_selection_where_it_was() {
    let mut b = RichBody::from_plain("abc\nquote");
    b.select(1, 2).unwrap();
    b.insert_at(3, "\n-- \nAda", &Style::default()).unwrap();
    assert_eq!(b.plain(), "abc\n-- \nAda\nquote");
    assert_eq!(b.selection(), Selection { anchor: 1, head: 2 });
    b.select(9, 9).unwrap();
    b.insert_at(0, "X", &Style::default()).unwrap();
    assert_eq!(b.selection(), Selection::caret(10));
}

#[derive(Clone, Debug)]
enum Op {
    Select(usize, usize),
    Type(String),
    Paste(String),
    Backspace,
    Delete,
    Bold,
    Italic,
    Underline,
    Size(u8),
    Link(String),
    Unlink,
    List(u8),
    Indent(bool),
    Clear,
    Plain(String),
    Undo,
    Redo,
}

fn op() -> impl Strategy<Value = Op> {
    let text = || "[a-c \\n\\r\\t\\x00é]{0,6}";
    prop_oneof![
        (0usize..40, 0usize..40).prop_map(|(a, b)| Op::Select(a, b)),
        text().prop_map(Op::Type),
        text().prop_map(Op::Paste),
        Just(Op::Backspace),
        Just(Op::Delete),
        Just(Op::Bold),
        Just(Op::Italic),
        Just(Op::Underline),
        any::<u8>().prop_map(Op::Size),
        prop_oneof![Just("https://example.org".to_string()), Just("javascript:x".to_string()), "\\PC{0,8}"].prop_map(Op::Link),
        Just(Op::Unlink),
        (0u8..3).prop_map(Op::List),
        any::<bool>().prop_map(Op::Indent),
        Just(Op::Clear),
        text().prop_map(Op::Plain),
        Just(Op::Undo),
        Just(Op::Redo),
    ]
}

#[test]
fn a_signature_is_put_in_replaced_and_taken_out_as_paragraphs_of_its_own() {
    let mut t = RichText::from_plain("Hi there");
    t.put_signature(&RichText::from_plain("Avery\nLumen"), 2).unwrap();
    assert_eq!(t.to_plain(), "Hi\nAvery\nLumen\n there");
    assert_eq!((t.signature_start(), t.signature().map(|s| s.to_plain())), (Some(3), Some("Avery\nLumen".into())));
    let (before, sig, after) = t.around_signature().unwrap();
    assert_eq!(
        (before.map(|b| b.to_plain()), sig.to_plain(), after.map(|a| a.to_plain())),
        (Some("Hi".into()), "Avery\nLumen".into(), Some(" there".into()))
    );
    // Another replaces it, wherever asked to go.
    t.put_signature(&RichText::from_plain("Bo"), 0).unwrap();
    assert_eq!(t.to_plain(), "Hi\nBo\n there");
    t.remove_signature();
    assert_eq!((t.to_plain().as_str(), t.signature()), ("Hi\n there", None));
    // In a body, the caret after it moves with it; one before it stays.
    let mut b = RichBody::from_plain("Hi");
    b.put_signature(Some(&RichText::from_plain("Avery")), 2).unwrap();
    assert_eq!((b.plain().as_str(), b.selection().head), ("Hi\nAvery", 0));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// Any edits in any order keep the document whole, and undoing all of them gives back the
    /// text it started with.
    #[test]
    fn any_edits_keep_the_document_whole(ops in proptest::collection::vec(op(), 0..40)) {
        let mut b = RichBody::from_plain("start\nhere");
        let start = b.text().clone();
        for op in ops {
            let _ = match op {
                Op::Select(a, h) => b.select(a, h),
                Op::Type(s) => b.type_text(&s),
                Op::Paste(s) => b.paste(&RichText::from_plain(&s)),
                Op::Backspace => b.backspace(),
                Op::Delete => b.delete_forward(),
                Op::Bold => b.toggle_bold().map(|_| ()),
                Op::Italic => b.toggle_italic().map(|_| ()),
                Op::Underline => b.toggle_underline().map(|_| ()),
                Op::Size(n) => b.format(Format::Size(Some(n))),
                Op::Link(u) => b.set_link(Some(&u)),
                Op::Unlink => b.set_link(None),
                Op::List(k) => b.toggle_list([ListKind::None, ListKind::Bullet, ListKind::Numbered][k as usize]).map(|_| ()),
                Op::Indent(deeper) => b.indent(if deeper { 1 } else { -1 }).map(|_| ()),
                Op::Clear => b.clear_formatting(),
                Op::Plain(s) => b.set_plain(&s),
                Op::Undo => { b.undo(); Ok(()) }
                Op::Redo => { b.redo(); Ok(()) }
            };
            check(b.text());
            prop_assert!(b.selection().end() <= b.text().len());
        }
        while b.undo() {}
        prop_assert_eq!(b.text(), &start);
    }

    /// Any documents, cleaned, survive being stored.
    #[test]
    fn documents_survive_storing(texts in proptest::collection::vec(("\\PC{0,8}", any::<bool>(), any::<u8>(), 0u8..3, any::<u8>()), 1..8)) {
        let paragraphs = texts.into_iter().map(|(text, b, size, list, level)| Paragraph {
            runs: vec![Run::new(text, Style { bold: b, size: Some(size), ..Style::default() })],
            list: [ListKind::None, ListKind::Bullet, ListKind::Numbered][list as usize],
            level,
            signature: b,
        }).collect();
        let t = RichText::from_paragraphs(paragraphs);
        check(&t);
        let json = serde_json::to_string(&t).unwrap();
        prop_assert_eq!(serde_json::from_str::<RichText>(&json).unwrap(), t);
    }
}
