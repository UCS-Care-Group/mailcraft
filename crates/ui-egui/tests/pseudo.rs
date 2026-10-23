// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! In the pseudo language every text that goes through `tl!` comes out accented ("Ínbóx"), so a
//! word of plain English on the screen is a text that doesn't: one a translation would miss. Each
//! main view is drawn once and searched for such words. (A process of its own: the window's
//! language is shared by everything in it.)

use std::collections::BTreeSet;

use egui::Shape;
use egui_kittest::Harness;
use mailcraft_engine::Session;
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::{Value, json};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// Shown as they are on purpose, or not the window's own text yet. Each later slice of the French
/// interface takes its words out of here.
const ALLOWED: &[&str] = &[
    // The program's name, and the names of products and providers.
    "MailCraft",
    "Microsoft",
    "Outlook",
    "Graph",
    "Gmail",
    "Google",
    "iCloud",
    "Yahoo",
    "Teams",
    "Skype",
    "IMAP",
    "SMTP",
    // A language is named in its own language; the pseudo one has no other name.
    "Pseudo",
];

/// Every word of the sample mailbox (names, addresses, subjects, bodies): data, not interface.
/// Not the names of its standard folders (Inbox, Sent Items…), which the window shows in its own
/// language.
fn mailbox_words(app: &MailApp) -> BTreeSet<String> {
    fn walk(v: &Value, out: &mut BTreeSet<String>) {
        match v {
            Value::String(s) => out.extend(words(s)),
            Value::Array(a) => a.iter().for_each(|v| walk(v, out)),
            Value::Object(o) => o.iter().for_each(|(k, v)| {
                out.extend(words(k));
                walk(v, out);
            }),
            _ => {}
        }
    }
    let mut mailbox = app.session.mailbox.clone();
    mailbox.folders.retain(|f| f.kind == mailcraft_model::FolderKind::Custom);
    let mut out = BTreeSet::new();
    walk(&serde_json::to_value(&mailbox).unwrap(), &mut out);
    out
}

fn words(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string)
}

/// A word of plain English: a vowel the pseudo language would have accented. A person's initials
/// (two capitals, on their picture) are the person's, not the interface's.
fn unmarked(w: &str) -> bool {
    let initials = w.len() <= 2 && w.chars().all(|c| c.is_ascii_uppercase());
    !initials && w.chars().any(|c| "aeiouAEIOU".contains(c)) && !w.chars().any(|c| c.is_ascii_digit())
}

fn texts(shapes: &[Shape], out: &mut Vec<String>) {
    for s in shapes {
        match s {
            Shape::Text(t) => out.push(t.galley.text().to_string()),
            Shape::Vec(v) => texts(v, out),
            _ => {}
        }
    }
}

fn shown(h: &Harness<'static, MailApp>) -> Vec<String> {
    let shapes: Vec<Shape> = h.output().shapes.iter().map(|c| c.shape.clone()).collect();
    let mut out = Vec::new();
    texts(&shapes, &mut out);
    out
}

#[test]
fn every_main_view_is_in_the_pseudo_language() {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.execute("ui.language", json!({"language": "pseudo"})).unwrap();
    let data = mailbox_words(&app);
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1440.0, 920.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    let views: [(&str, Value); 5] = [
        ("view.mail", json!({})),
        ("view.calendar", json!({})),
        ("view.people", json!({})),
        ("view.tasks", json!({})),
        ("ui.backstage", json!({"value": true, "page": "options"})),
    ];
    let mut missed = BTreeSet::new();
    for (command, params) in views {
        h.state_mut().execute(command, params).unwrap();
        h.run_steps(4);
        let lines = shown(&h);
        assert!(lines.iter().any(|l| l.contains(['á', 'é', 'í', 'ó', 'ú'])), "{command}: nothing in the pseudo language");
        for line in lines {
            for w in words(&line) {
                if unmarked(&w) && !data.contains(&w) && !ALLOWED.contains(&w.as_str()) {
                    missed.insert(format!("{command}: {w:?} in {line:?}"));
                }
            }
        }
        if command == "ui.backstage" {
            h.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
        }
    }
    assert!(missed.is_empty(), "text shown without tl! ({}):\n{}", missed.len(), missed.into_iter().collect::<Vec<_>>().join("\n"));
}
