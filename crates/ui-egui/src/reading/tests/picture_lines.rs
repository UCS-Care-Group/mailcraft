// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1653: a picture the message puts under another stays under it at every width, with the gap
//! the message gives; a column the message gives a width in pixels keeps it when a cell that
//! spans it wants more room; pictures that stand beside each other still share a line.
//!
//! The people, addresses and pictures are invented.

use mailcraft_model::{Attachment, Message};

use super::{find, in_frame, lay_out, message, png, shown_pictures};

/// A photo with a logo under it in a column 120 pixels wide, beside the contact lines, over a
/// note that spans both columns.
const SIGNATURE: &str = r##"<table cellpadding="0" cellspacing="0" border="0" width="500" role="presentation"><tr>
  <td width="120" valign="top" style="padding:2px 18px 0 0">
    <img src="cid:photo@example.org" width="96" height="96" alt="Photo" style="display:block;border:0">
    <table cellpadding="0" cellspacing="0" border="0" role="presentation" style="margin-top:12px"><tr><td>
      <a href="https://www.example.org"><img src="cid:logo@example.org" width="102" height="28" alt="Logo" style="display:block;border:0"></a>
    </td></tr></table>
  </td>
  <td valign="top" style="border-left:3px solid #0f5e63;padding:0 0 0 18px">Elena Marsh<br>Senior Textile Designer</td>
</tr><tr>
  <td colspan="2" style="padding-top:16px;font-size:10px">This message and anything attached to it are meant for the person it is addressed to.</td>
</tr></table>"##;

fn with_pictures(html: &str, pictures: &[(&str, u32, u32)]) -> Message {
    let mut m = message(0, ("Elena Marsh", "elena@example.org"), "Samples", Some(html.to_string()), mailcraft_html::to_text(html));
    m.attachments = pictures
        .iter()
        .map(|(id, w, h)| Attachment { content_id: (*id).into(), inline: true, ..Attachment::new(format!("{w}x{h}.png"), "image/png", png(*w, *h)) })
        .collect();
    m
}

#[test]
fn the_logo_stays_under_the_photo_and_the_column_keeps_its_width() {
    let m = with_pictures(SIGNATURE, &[("photo@example.org", 96, 96), ("logo@example.org", 102, 28)]);
    let mut second = Vec::new();
    for width in [360.0, 1400.0] {
        let edge = in_frame(false, |ctx| {
            let laid = lay_out(ctx, &m, width);
            let shown = shown_pictures(&laid);
            assert_eq!(shown.len(), 2, "at {width}");
            let (photo, logo) = (shown[0].rect, shown[1].rect);
            assert_eq!((photo.width(), photo.height(), logo.width(), logo.height()), (96.0, 96.0, 102.0, 28.0), "at {width}");
            // Under it, at the same left edge, 12 pixels below.
            assert_eq!(logo.min.x, photo.min.x, "at {width}: {photo:?} {logo:?}");
            assert_eq!(logo.min.y - photo.max.y, 12.0, "at {width}: {photo:?} {logo:?}");
            find(&laid, "Elena Marsh").rect.min.x
        });
        second.push(edge);
    }
    assert_eq!(second[0], second[1], "the second column starts where it did: {second:?}");
}

#[test]
fn pictures_beside_each_other_still_share_a_line() {
    let html = r#"<p><a href="https://a.example"><img src="cid:a@example.org" width="24" height="24" alt="A"></a> <a href="https://b.example"><img src="cid:b@example.org" width="24" height="24" alt="B"></a><img src="cid:c@example.org" width="24" height="24" alt="C"></p>"#;
    let m = with_pictures(html, &[("a@example.org", 24, 24), ("b@example.org", 24, 24), ("c@example.org", 24, 24)]);
    in_frame(false, |ctx| {
        let laid = lay_out(ctx, &m, 600.0);
        let shown = shown_pictures(&laid);
        assert_eq!(shown.len(), 3);
        assert!(shown.iter().all(|p| p.rect.min.y == shown[0].rect.min.y), "{:?}", shown.iter().map(|p| p.rect).collect::<Vec<_>>());
        assert!(shown[0].rect.max.x <= shown[1].rect.min.x && shown[1].rect.max.x <= shown[2].rect.min.x);
    });
}

/// Text "Aa" in a first column set as `width`, "Bb" in a second, and a 400 pixel picture across both.
fn spanning(width: &str) -> f32 {
    let html = format!(
        r#"<table cellpadding="0" cellspacing="0" border="0"><tr><td width="{width}">Aa</td><td>Bb</td></tr><tr><td colspan="2"><img src="cid:wide@example.org" width="400" height="20" alt="W"></td></tr></table>"#
    );
    let m = with_pictures(&html, &[("wide@example.org", 400, 20)]);
    in_frame(false, |ctx| {
        let laid = lay_out(ctx, &m, 1400.0);
        let (a, b) = (find(&laid, "Aa").rect.min.x, find(&laid, "Bb").rect.min.x);
        let picture = shown_pictures(&laid)[0].rect.width();
        assert_eq!(picture, 400.0, "{width}");
        b - a
    })
}

#[test]
fn a_spanning_cell_leaves_a_pixel_column_alone() {
    assert_eq!(spanning("120"), 120.0);
}

#[test]
fn a_spanning_cell_leaves_a_percent_column_to_its_share() {
    // 20% of the 400 the picture needs; the room the spanning cell wants lands on the other column
    // through the spare-room step, so a percent column mixed into the span loses none of it.
    assert_eq!(spanning("20%"), 80.0);
}
