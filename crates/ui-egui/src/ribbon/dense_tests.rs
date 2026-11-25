// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The ribbon in a dense look (Classic Light, #1203): a shorter row of narrower buttons, fitted
//! to the window by the same plan as MailCraft's own sizes, so that no less of it is in view.

use egui::vec2;
use mailcraft_engine::Session;
use serde_json::json;

use super::{BAND_H, Mode, Sizes, behind_more, describe, hit_id, row};
use crate::theme::Tokens;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Window = egui_kittest::Harness<'static, MailApp>;

fn window(theme: &str, width: f32) -> Window {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.execute("ui.theme", json!({"value": theme})).unwrap();
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(width, 700.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(4);
    let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    harness.run_steps(3);
    harness
}

#[test]
fn the_dense_sizes_are_smaller_and_only_in_a_dense_look() {
    let (own, dense) = (Sizes::of(&Tokens::light()), Sizes::of(&Tokens::classic_light()));
    assert_eq!(own, Sizes::of(&Tokens::dark()));
    assert_eq!(own.band, BAND_H);
    assert!(dense.band < own.band && dense.button < own.button && dense.pad < own.pad && dense.label < own.label);
    // A button still sits inside its band with room round it.
    assert!(dense.button + 8.0 <= dense.band);
}

/// At every width the Classic Light row is shorter, inside its band, and has no more behind `…`
/// than the light theme's has.
#[test]
fn the_classic_light_row_is_shorter_and_shows_as_much() {
    for width in [1200.0, 1000.0, 800.0, 640.0] {
        let mut light = window("light", width);
        let mut classic = window("classicLight", width);
        assert_eq!(classic.state().ui.ribbon_mode, Mode::Simplified);
        for tab in ["Home", "View", "Folder"] {
            for h in [&mut light, &mut classic] {
                h.state_mut().run("ui.tab", json!({"tab": tab})).unwrap();
                h.run_steps(3);
            }
            let name = format!("{tab} at {width}");
            let band = |h: &Window| h.state().hit.rect("ribbonBand", 0).unwrap_or_else(|| panic!("{name}: no band"));
            let (lb, cb) = (band(&light), band(&classic));
            assert!(cb.height() < lb.height(), "{name}: {cb:?} {lb:?}");
            assert!((cb.height() - Sizes::of(&Tokens::classic_light()).band).abs() < 0.5, "{name}: {cb:?}");
            // The panes start higher up.
            assert!(cb.max.y < lb.max.y, "{name}");
            let (lm, cm) = (behind_more(&light.ctx), behind_more(&classic.ctx));
            assert!(cm.len() <= lm.len(), "{name}: {cm:?} behind … where light has {lm:?}");
            for b in row(&describe(classic.state())) {
                let key = b.item.key;
                match classic.state().hit.rect("ribbon", hit_id(key)) {
                    Some(r) => assert!(r.min.y >= cb.min.y - 0.5 && r.max.y <= cb.max.y + 0.5 && r.max.x <= cb.max.x + 0.5, "{name}: {key} at {r:?}"),
                    None => assert!(cm.contains(&key), "{name}: {key} is neither in the row nor behind …"),
                }
            }
        }
    }
}
