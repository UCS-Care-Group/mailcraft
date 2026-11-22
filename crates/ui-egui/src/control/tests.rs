// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! While a request from the control channel is handled, the window is marked as driven by it,
//! and is as it was again afterwards, however the handling ends: an answer, a refusal, or a
//! panic in what handled it.

use std::panic::{AssertUnwindSafe, catch_unwind};

use mailcraft_engine::Session;
use serde_json::json;

use super::{ControlRequest, handle};
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The window, its host's file picker one that goes wrong: it panics.
fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.services.open_async = Some(Box::new(|_, _| panic!("a host that went wrong")));
    app
}

fn request(method: &str, params: serde_json::Value) -> ControlRequest {
    ControlRequest::new(method, params).0
}

#[test]
fn a_request_marks_the_window_driven_only_while_it_is_handled() {
    let ctx = egui::Context::default();
    // Not driven before, or driven already (input the control channel injected in this frame).
    for before in [false, true] {
        let mut app = app();
        app.driven = before;
        for (method, params) in [("engine.execute", json!({"command": "mail.markAllRead"})), ("engine.execute", json!({})), ("no.such", json!(7))] {
            let _ = handle(&mut app, &ctx, &request(method, params));
            assert_eq!(app.driven, before, "{method}");
        }
        // A request whose handling panics: as it was too.
        let unwound = catch_unwind(AssertUnwindSafe(|| handle(&mut app, &ctx, &request("engine.execute", json!({"command": "ui.open"})))));
        assert!(unwound.is_err(), "the host's picker was not asked");
        assert_eq!(app.driven, before);
    }
}

/// An occurrence's own keys are recorded within a budget of their own: a frame that records as
/// many of them as the list takes still records the window's other controls up to their own
/// limit, as before occurrences had keys (CodeRabbit on #1665, control.rs:96).
#[test]
fn keyed_entries_never_crowd_the_other_controls_out() {
    let mut hit = super::HitMap::default();
    let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0));
    for i in 0..5000u64 {
        hit.add_key(format!("eventOccurrence:{i}:2026-10-14"), rect);
    }
    for i in 0..3000u64 {
        hit.add("event", i, rect);
    }
    assert!(hit.rect("event", 2999).is_some(), "the window's controls are recorded whatever keyed entries came first");
    assert!(hit.rect("eventOccurrence:3999:2026-10-14", 0).is_some(), "keyed entries are recorded up to their own limit");
    assert!(hit.rect("eventOccurrence:4000:2026-10-14", 0).is_none(), "and no further");
    hit.clear();
    assert!(hit.rect("eventOccurrence:1:2026-10-14", 0).is_none(), "a new frame forgets what the last one recorded");
    hit.add_key("eventOccurrence:4001:2026-10-14", rect);
    assert!(hit.rect("eventOccurrence:4001:2026-10-14", 0).is_some(), "and has its budget again");
}
