// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A picture sized in the text (#1740): the width is the picture run's, so undo, redo and the
//! text's own copies (a draft, a session) carry it as they carry any other change of the text.

use crate::pictures::MAX_SHOWN_WIDTH;
use crate::rich::*;

/// `ab`, the picture 7, `cd`.
fn text() -> RichBody {
    RichBody::new(RichText::from_paragraphs(vec![Paragraph::new(
        vec![Run::plain("ab"), Run::picture(7, Style { bold: true, ..Style::default() }), Run::plain("cd")],
        ListKind::None,
    )]))
}

fn width_at(b: &RichBody, at: usize) -> Option<u16> {
    let mut seen = 0;
    for run in b.text().paragraphs().iter().flat_map(|p| &p.runs) {
        if seen == at {
            return run.width;
        }
        seen += run.text.chars().count();
    }
    None
}

#[test]
fn sizing_a_picture_is_one_undo_step_and_keeps_the_rest() {
    let mut b = text();
    b.select(2, 3).unwrap();
    assert_eq!(b.size_picture(2, Some(300)).unwrap(), 7);
    assert_eq!(width_at(&b, 2), Some(300));
    // The picture is still selected, with its look; the text around it is as it was.
    assert_eq!(b.selection(), Selection { anchor: 2, head: 3 });
    assert_eq!(b.text().style_at(3), Style { bold: true, ..Style::default() });
    assert_eq!(b.plain(), format!("ab{OBJECT}cd"));
    b.size_picture(2, Some(120)).unwrap();
    assert!(b.undo());
    assert_eq!(width_at(&b, 2), Some(300));
    assert!(b.undo());
    assert_eq!(width_at(&b, 2), None, "back to its own width");
    assert!(!b.can_undo());
    assert!(b.redo());
    assert_eq!(width_at(&b, 2), Some(300));
    // `None` gives it its own width back, as a step of its own.
    b.size_picture(2, None).unwrap();
    assert_eq!(width_at(&b, 2), None);
    assert!(b.undo());
    assert_eq!(width_at(&b, 2), Some(300));
}

#[test]
fn the_same_width_again_is_no_step() {
    let mut b = text();
    b.size_picture(2, Some(300)).unwrap();
    b.size_picture(2, Some(300)).unwrap();
    assert!(b.undo());
    assert!(!b.can_undo());
}

#[test]
fn only_a_picture_is_sized_and_the_width_is_kept_within_its_bounds() {
    let mut b = text();
    for at in [0, 1, 3, 4] {
        assert!(b.size_picture(at, Some(10)).is_err(), "{at}: no picture there");
    }
    assert!(b.size_picture(99, Some(10)).is_err());
    assert!(!b.can_undo(), "nothing changed");
    b.size_picture(2, Some(u16::MAX)).unwrap();
    assert_eq!(width_at(&b, 2), Some(MAX_SHOWN_WIDTH));
    b.size_picture(2, Some(0)).unwrap();
    assert_eq!(width_at(&b, 2), Some(1));
}

#[test]
fn a_width_is_a_pictures_alone_and_survives_a_copy_or_a_round_trip() {
    let mut b = text();
    b.size_picture(2, Some(300)).unwrap();
    // Cut and pasted elsewhere, the picture keeps its size.
    let piece = b.text().slice(2, 3).unwrap();
    assert_eq!(piece.paragraphs()[0].runs[0].width, Some(300));
    // Stored and read back (a message being written is), and cleaned on the way in.
    let json = serde_json::to_string(b.text()).unwrap();
    let back: RichText = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, b.text());
    let hostile = r#"{"paragraphs":[{"runs":[{"text":"x","width":40},{"text":"￼","picture":7,"width":65535}]}]}"#;
    let read: RichText = serde_json::from_str(hostile).unwrap();
    let runs = &read.paragraphs()[0].runs;
    assert_eq!(runs[0].width, None, "text has no width");
    assert_eq!(runs[1].width, Some(MAX_SHOWN_WIDTH));
    // None narrower than a pixel either, as `format.pictureWidth` and a picture's own width keep it.
    let zero: RichText = serde_json::from_str(r#"{"paragraphs":[{"runs":[{"text":"\ufffc","picture":7,"width":0}]}]}"#).unwrap();
    assert_eq!(zero.paragraphs()[0].runs[0].width, Some(1));
}
