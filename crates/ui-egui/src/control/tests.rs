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
    app.services.open_async = Some(Box::new(|_| panic!("a host that went wrong")));
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
