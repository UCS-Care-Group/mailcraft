// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sender warnings in the window (#508): the banner and its colour, the External tag, the mark
//! on a row, the mark on a link.
//!
//! The people, hosts and addresses are invented.

use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{Address, Body, FolderKind, Message};
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Window = egui_kittest::Harness<'static, MailApp>;

fn mail(name: &str, email: &str, subject: &str, results: Option<&str>, html: Option<&str>) -> Message {
    Message {
        from: Address::new(name, email),
        subject: subject.into(),
        message_id: format!("{}@example.org", subject.replace(' ', ".")),
        date: NOW - 3_600,
        headers: results.map(|r| vec![("Authentication-Results".to_string(), r.to_string())]).unwrap_or_default(),
        body: Body { text: "Hello.".into(), html: html.map(str::to_string) },
        ..Default::default()
    }
}

/// Avery's window on an IMAP account at Lumen, with Maya at Lumen and Lin at Harbor Works as
/// contacts, and the Inbox holding a message for each warning.
fn window() -> Window {
    let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0);
    let a = s
        .run("account.add", &json!({"email": "avery@lumen.example", "displayName": "Avery Quinn", "incoming": {"host": "imap.lumen.example"}}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    for (name, email) in [("Maya Okafor", "maya@lumen.example"), ("Lin Zhao", "lin.zhao@harborworks.example")] {
        s.run("people.new", &json!({"displayName": name, "email": email})).unwrap();
    }
    let new = vec![
        mail("Cloudline Billing", "billing@cloudline.example", "Failed", Some("mx.lumen.example; dmarc=fail header.from=cloudline.example"), None),
        mail("Maya Okafor", "maya.okafor@freemail.example", "Hidden", None, None),
        mail("Lin", "lin@harborwork.example", "Lookalike", None, None),
        mail(
            "Lin Zhao",
            "lin.zhao@harborworks.example",
            "Link",
            None,
            Some("<p>Track it at <a href=\"https://parcel-track.example/x\">harborworks.example/track</a>.</p>"),
        ),
        mail("Lin Zhao", "lin.zhao@harborworks.example", "Plain", None, None),
        mail("Maya Okafor", "maya@lumen.example", "Colleague", None, None),
    ];
    let new = new.into_iter().enumerate().map(|(i, m)| Message { uid: i as u32 + 1, ..m }).collect();
    let result = SyncResult {
        account: a,
        folders: vec![FolderDelta {
            id: None,
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 7,
            uid_seen: 50,
            new,
            ..Default::default()
        }],
        ..Default::default()
    };
    s.run("sync.apply", &json!({"result": result})).unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    let mut app = MailApp::new(s, Services::default());
    app.autosave = false;
    app.auto_sync = false;
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(3);
    harness
}

fn id(harness: &Window, subject: &str) -> u64 {
    harness.state().session.mailbox.messages.values().find(|m| m.subject == subject).map(|m| m.id).unwrap()
}

fn open(harness: &mut Window, subject: &str) -> u64 {
    let id = id(harness, subject);
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(4);
    id
}

fn says(harness: &Window, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

#[test]
fn a_warning_is_a_red_banner_with_its_reason() {
    let mut harness = window();
    let failed = open(&mut harness, "Failed");
    assert!(harness.state().hit.rect("senderWarnings", failed).is_some());
    assert!(says(&harness, "This message may not be what it seems."));
    assert!(says(&harness, "Failed check."));
    // External joins in as a line, beside the reason that brought the banner.
    assert!(says(&harness, "From outside your organisation."));
    assert!(harness.state().hit.rect("externalTag", failed).is_some());

    let hidden = open(&mut harness, "Hidden");
    assert!(harness.state().hit.rect("senderWarnings", hidden).is_some());
    assert!(says(&harness, "The name of someone you know."));
}

#[test]
fn cautions_are_amber_and_external_alone_is_only_a_tag() {
    let mut harness = window();
    open(&mut harness, "Lookalike");
    assert!(says(&harness, "Be careful with this message."));
    assert!(says(&harness, "Lookalike address."));
    assert!(!says(&harness, "This message may not be what it seems."));

    let link = open(&mut harness, "Link");
    assert!(says(&harness, "A link goes somewhere else."));
    assert!(says(&harness, "goes to parcel-track.example"));
    let m = harness.state().session.mailbox.messages[&link].clone();
    let flagged = super::warnings::mismatched(harness.state_mut(), &m);
    assert_eq!(flagged, ["https://parcel-track.example/x"]);
    assert!(super::warnings::is_mismatched(" https://Parcel-Track.example/x", &flagged));
    assert!(!super::warnings::is_mismatched("https://parcel-track.example/y", &flagged));

    let plain = open(&mut harness, "Plain");
    assert!(harness.state().hit.rect("senderWarnings", plain).is_none());
    assert!(harness.state().hit.rect("externalTag", plain).is_some());

    let colleague = open(&mut harness, "Colleague");
    assert!(harness.state().hit.rect("senderWarnings", colleague).is_none());
    assert!(harness.state().hit.rect("externalTag", colleague).is_none());
}

#[test]
fn only_a_failed_check_or_a_hidden_address_marks_the_row() {
    let mut harness = window();
    harness.run_steps(2);
    let state = harness.state();
    for (subject, marked) in [("Failed", true), ("Hidden", true), ("Lookalike", false), ("Link", false), ("Plain", false), ("Colleague", false)] {
        let id = id(&harness, subject);
        assert!(state.hit.rect("message", id).is_some(), "{subject} is not on screen");
        assert_eq!(state.hit.rect("messageWarning", id).is_some(), marked, "{subject}");
    }
}

#[test]
fn what_the_banner_shows_never_turns_round() {
    // A right-to-left override would show `lumen.example` for `lumen.elpmaxe`.
    assert_eq!(super::warnings::short("lumen.\u{202e}elpmaxe"), "lumen.elpmaxe");
    assert_eq!(super::warnings::short("\u{2067}Maya\u{2069} \u{200f}Okafor\u{061c}"), "Maya Okafor");
}
