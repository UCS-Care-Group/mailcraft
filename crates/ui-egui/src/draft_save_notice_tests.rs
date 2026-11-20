// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message being written that the five-minute background save can't put in Drafts says so in
//! the status bar, not only in the log, so that the owner knows what they wrote is not kept yet.
//! The failing save is a real one: the message's account has no Drafts folder.

use mailcraft_engine::Session;
use mailcraft_model::FolderKind;
use serde_json::json;

use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

#[test]
fn a_background_draft_save_that_fails_is_said_in_the_status_bar() {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    let account = a.session.mailbox.default_account().expect("the sample mailbox has an account").id;
    a.run("mail.new", json!({"account": account, "subject": "Kept here"})).expect("a new message");
    a.run("compose.type", json!({"text": "Words not yet in Drafts"})).expect("typed");
    let id = a.session.compose_id().expect("a message is being written");
    // Its account has no Drafts folder any more: a save to Drafts is refused.
    for f in a.session.mailbox.folders.iter_mut().filter(|f| f.account == account && f.kind == FolderKind::Drafts) {
        f.kind = FolderKind::Custom;
    }
    a.status_msg = None;
    let ctx = egui::Context::default();
    // The first look starts the five minutes; nothing is saved or said yet.
    a.draft_autosave_tick(&ctx);
    assert!(a.status_msg.is_none(), "nothing is said before the save is due: {:?}", a.status_msg);
    a.session.clock.now = Some(NOW + 5 * 60);
    a.draft_autosave_tick(&ctx);
    let c = a.session.composing_by_id(id).expect("the message is still being written");
    assert!(c.dirty && c.draft.is_none(), "fixture guard: the background save really failed");
    let (said, _) = a.status_msg.clone().expect("the failure is said in the status bar");
    assert!(said.contains("couldn't be saved to Drafts") && said.contains("no Drafts folder"), "the status says what happened and why: {said}");
}
