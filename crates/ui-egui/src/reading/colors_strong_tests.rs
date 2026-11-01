// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's colours with high contrast on: 7:1 against the paper, and kept rather than stripped.

use egui::Color32;
use mailcraft_html::Rgb;

use super::colors::{HIGH_CONTRAST, Paper, c32, contrast, readable_strong};
use crate::theme::{Accent, Tokens};

#[test]
fn every_text_colour_reaches_seven_to_one_on_every_paper() {
    for dark in [false, true] {
        let t = Tokens::build(dark, Accent::Blue, true);
        let mut papers = vec![Paper::window(&t)];
        for r in [0u8, 0x33, 0x80, 0xcc, 0xff] {
            for g in [0u8, 0x80, 0xff] {
                for b in [0u8, 0x66, 0xff] {
                    papers.push(Paper::of(Rgb(r, g, b), &t));
                }
            }
        }
        for paper in papers {
            for quoted in [false, true] {
                let ink = paper.text(&t, quoted);
                // A mid-tone paper can't give 7:1 to anything but black or white.
                assert!(contrast(ink, paper.fill) >= HIGH_CONTRAST || ink == Color32::BLACK || ink == Color32::WHITE, "text on {paper:?}");
            }
            for asked in [Rgb(255, 255, 255), Rgb(0, 0, 0), Rgb(0x1f, 0x49, 0x7d), Rgb(0x80, 0x80, 0x80), Rgb(0xc0, 0, 0)] {
                for link in [false, true] {
                    let ink = paper.ink(&t, Some(asked), link, false);
                    assert!(
                        contrast(ink, paper.fill) >= HIGH_CONTRAST || ink == Color32::BLACK || ink == Color32::WHITE,
                        "{asked:?} on {paper:?} is {ink:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn colours_already_strong_are_kept_and_weak_ones_keep_their_hue() {
    let white = Color32::WHITE;
    let navy = c32(Rgb(0x1f, 0x49, 0x7d));
    assert_eq!(readable_strong(navy, white), navy);
    // A mid blue on white is moved, not turned grey: still a blue.
    let weak = c32(Rgb(0x30, 0x80, 0xe0));
    let shown = readable_strong(weak, white);
    assert!(contrast(shown, white) >= HIGH_CONTRAST && shown.b() > shown.r(), "{shown:?}");
}

#[test]
fn normal_contrast_is_not_raised() {
    let t = Tokens::light();
    let asked = Rgb(0x70, 0x70, 0x70);
    assert_eq!(Paper::window(&t).ink(&t, Some(asked), false, false), c32(asked));
}
