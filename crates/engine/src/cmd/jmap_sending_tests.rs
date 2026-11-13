// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A JMAP account (M6) sends its mail: a message written from it waits in its Outbox, is offered
//! whole to the next run, and leaves the Outbox when the server took it, or is held there with
//! what happened. One whose answer never came is held, not offered again. The accounts beside it
//! go on as before. (Its invitations: `calendar/invitation.rs`; its reading: `jmap_mail_tests.rs`;
//! the changes of its messages: `jmap_up_tests.rs`.)

use mailcraft_model::sync::{
    FolderListing, GraphFolder, GraphMessage, JmapChange, JmapMailResult, MessagePage, SendDone, SendOutcome, ServerMessage,
};
use mailcraft_model::{Account, AccountKind, Address, FolderKind, Id, JmapServer, Mailbox};
use serde_json::json;

use super::jmap_mail::{apply_jmap_mail, jmap_mail_request};
use crate::Session;

const JMAP: &str = "ada@jmap.example";

/// A local account (`ada@example.org`), then a JMAP account, then an IMAP account; their ids.
fn mailbox() -> (Session, Id, Id, Id) {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let local = mb.accounts[0].id;
    let server = JmapServer { session: "https://jmap.example/.well-known/jmap".into(), bearer: true };
    let jmap = mb.add_account(Account { email: JMAP.into(), kind: AccountKind::Jmap, jmap: Some(server), ..Default::default() });
    let mut s = Session::new(mb);
    let imap = s.run("account.add", &json!({"email": "ada@imap.example", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    (s, local, jmap, imap)
}

fn count(s: &Session, account: Id, kind: FolderKind) -> usize {
    s.mailbox.count(s.mailbox.folder_of_kind(account, kind).unwrap())
}

/// Write a message from the JMAP account and send it: its id, in the Outbox.
fn written(s: &mut Session, jmap: Id, subject: &str) -> Id {
    s.run("mail.new", &json!({"account": jmap, "to": "bo@example.org", "subject": subject})).unwrap();
    let sent = s.run("compose.send", &json!({})).unwrap();
    assert_eq!(sent["queued"].as_bool(), Some(true), "{sent}");
    sent["id"].as_u64().unwrap()
}

/// The messages the next run of the JMAP account would send.
fn offered(s: &Session, jmap: Id) -> Vec<Id> {
    jmap_mail_request(s, jmap, true).unwrap().sends.iter().map(|up| up.message).collect()
}

/// A piece of a run that says what became of the message sent; what taking it in answered.
fn answered(s: &mut Session, jmap: Id, message: Id, outcome: SendOutcome) -> serde_json::Value {
    let sent = vec![SendDone { message, outcome, draft: None, draft_left: String::new() }];
    apply_jmap_mail(s, JmapMailResult { account: jmap, email: JMAP.into(), sent, partial: true, ..Default::default() }).unwrap()
}

#[test]
fn a_message_written_from_a_jmap_account_waits_in_its_outbox_and_is_offered_whole() {
    let (mut s, local, jmap, _) = mailbox();
    let id = written(&mut s, jmap, "Lunch");
    assert!(s.compose.is_none());
    assert_eq!([FolderKind::Outbox, FolderKind::Sent].map(|kind| count(&s, jmap, kind)), [1, 0]);
    // (Nothing is delivered on this computer: the server sends it.)
    assert_eq!(count(&s, local, FolderKind::Inbox), 0);
    // A run of any scope offers it, whole and as no draft of the server's.
    for light in [true, false] {
        let request = jmap_mail_request(&s, jmap, light).unwrap();
        let [up] = request.sends.as_slice() else { panic!("{request:?}") };
        let whole = up.mime.as_ref().unwrap();
        assert_eq!((up.message, up.draft.is_none()), (id, true));
        assert_eq!((whole.subject.as_str(), whole.to.first().map(|to| to.email.as_str())), ("Lunch", Some("bo@example.org")));
        // Counts only in debug output: nothing of the message.
        let text = format!("{request:?}");
        assert!(text.contains("sends: 1") && !text.contains("Lunch") && !text.contains("bo@example.org"), "{text}");
    }
    // It waits to go, as the window counts it, and is no change of a message.
    assert!(jmap_mail_request(&s, jmap, true).unwrap().changes.is_empty());
    assert_eq!(super::graph_send::waiting(&s.mailbox, jmap), 1);
}

#[test]
fn a_message_the_server_took_leaves_the_outbox_and_its_original_is_marked_answered() {
    let (mut s, _, jmap, _) = mailbox();
    // The account's Inbox and Sent as the server has them, with a message to answer.
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    let folders = vec![folder("M1", "Inbox", FolderKind::Inbox), folder("M2", "Sent", FolderKind::Sent)];
    let asked = GraphMessage {
        server: ServerMessage { id: "E1".into(), size: 900, ..Default::default() },
        message_id: "e1@example.org".into(),
        subject: "Lunch?".into(),
        from: Address::new("Bo", "bo@example.org"),
        received: 100,
        ..Default::default()
    };
    let piece = || JmapMailResult { account: jmap, email: JMAP.into(), partial: true, ..Default::default() };
    apply_jmap_mail(&mut s, JmapMailResult { listing: Some(FolderListing { folders, complete: true }), ..piece() }).unwrap();
    let page = MessagePage { folder: "M1".into(), messages: vec![asked], afresh: true, ..Default::default() };
    apply_jmap_mail(&mut s, JmapMailResult { pages: vec![page], whole: true, partial: false, ..piece() }).unwrap();
    let original = s.mailbox.messages.values().find(|m| m.subject == "Lunch?").unwrap().id;
    s.run("mail.reply", &json!({"id": original})).unwrap();
    let reply = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(offered(&s, jmap), [reply]);
    assert!(!s.mailbox.message(original).unwrap().answered);
    // Sent: gone from the Outbox (the server files its copy in Sent, which comes down), counted
    // for Send/Receive Progress, and the original is marked answered, which goes up as a keyword.
    let answer = answered(&mut s, jmap, reply, SendOutcome::Sent);
    assert_eq!((answer["sent"].as_u64(), answer["errors"].as_array().map(Vec::len)), (Some(1), Some(0)));
    assert!(s.mailbox.message(reply).is_none() && offered(&s, jmap).is_empty());
    assert_eq!([FolderKind::Outbox, FolderKind::Sent].map(|kind| count(&s, jmap, kind)), [0, 0]);
    assert!(s.mailbox.message(original).unwrap().answered);
    let up = jmap_mail_request(&s, jmap, true).unwrap().changes;
    assert_eq!(up, [JmapChange::Keywords { message: original, id: "E1".into(), read: None, flagged: None, answered: Some(true), forwarded: None }]);
    let progress = s.sync_progress.json(&s.mailbox).to_string();
    assert!(progress.contains(r#""sent":1"#), "{progress}");
}

#[test]
fn a_message_the_server_refused_is_held_with_the_reason_until_send_again() {
    let (mut s, _, jmap, _) = mailbox();
    let id = written(&mut s, jmap, "Plans");
    let why = format!("the server takes no mail for bo@example.org\r\n{}", "x".repeat(5_000));
    let answer = answered(&mut s, jmap, id, SendOutcome::Held { lost: false, why });
    let said = answer["errors"][0].as_str().unwrap();
    assert!(
        said.starts_with("Couldn't send 'Plans': the server takes no mail for bo@example.org x") && said.ends_with("It is held in the Outbox."),
        "{said}"
    );
    // (A host's reason is one line, and short.)
    assert!(!said.contains('\n') && said.chars().count() < 500);
    let held = s.mailbox.message(id).unwrap();
    assert!(held.held.starts_with("It wasn't sent: the server takes no mail for bo@example.org"), "{}", held.held);
    assert_eq!(count(&s, jmap, FolderKind::Outbox), 1);
    // Held, it is not offered again, and doesn't count as waiting to go.
    assert!(offered(&s, jmap).is_empty());
    assert_eq!(super::graph_send::waiting(&s.mailbox, jmap), 0);
    // Until the owner says so.
    s.run("mail.sendAgain", &json!({"id": id})).unwrap();
    assert_eq!(offered(&s, jmap), [id]);
}

#[test]
fn a_message_that_may_have_gone_is_held_and_never_sent_again_by_itself() {
    let (mut s, _, jmap, _) = mailbox();
    // The server's answer was lost: the host says so.
    let lost = written(&mut s, jmap, "First");
    let answer = answered(&mut s, jmap, lost, SendOutcome::Held { lost: true, why: String::new() });
    assert_eq!(
        answer["errors"][0].as_str(),
        Some("'First' may have been sent, even if its copy in Sent (or Drafts) is still marked as a draft. It is held in the Outbox.")
    );
    let held = &s.mailbox.message(lost).unwrap().held;
    // Its copy is in Sent (or Drafts) marked as a draft until the submission takes the mark off,
    // and a copy still marked may have gone too: the advice never says it did not go.
    assert_eq!(
        held,
        "It may have been sent: the answer from the server was lost. Its copy is in Sent, or in Drafts where the account has no Sent folder: if the copy is no longer marked as a draft, it went; if it still is, it may have gone too. Ask a recipient before you send it again."
    );
    assert!(offered(&s, jmap).is_empty());
    // The run's answer never came at all (the program stopped): the message was marked as being
    // sent before it left, and the next run holds it instead of offering it.
    let unanswered = written(&mut s, jmap, "Second");
    assert_eq!(offered(&s, jmap), [unanswered]);
    assert!(super::graph_send::mark_sending(&mut s.mailbox, jmap, &[unanswered]));
    let mut said = Vec::new();
    assert!(super::graph_send::hold_unanswered(&mut s.mailbox, jmap, s.interface_language, &mut said));
    assert_eq!(said, ["'Second' may have been sent, even if its copy in Sent (or Drafts) is still marked as a draft. It is held in the Outbox."]);
    assert_eq!(s.mailbox.message(unanswered).unwrap().held, super::graph_send::MAY_HAVE_GONE_JMAP);
    assert!(offered(&s, jmap).is_empty());
    assert_eq!(count(&s, jmap, FolderKind::Outbox), 2);
    // A run that says nothing of a message it was offered leaves it waiting: it didn't go.
    let waits = written(&mut s, jmap, "Third");
    answered(&mut s, jmap, waits, SendOutcome::Waits);
    assert_eq!(offered(&s, jmap), [waits]);
}

/// A copy put in a JMAP account's Outbox has no server's id, so the next run would send it to
/// everybody the message names, with no Send: it isn't made. Only Send sends.
#[test]
fn a_copy_doesnt_go_into_a_jmap_accounts_outbox() {
    let (mut s, _, jmap, _) = mailbox();
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    let folders = vec![folder("M1", "Inbox", FolderKind::Inbox), folder("M2", "Sent", FolderKind::Sent)];
    let received = GraphMessage {
        server: ServerMessage { id: "E1".into(), size: 900, ..Default::default() },
        message_id: "e1@example.org".into(),
        subject: "Invoice".into(),
        from: Address::new("Bo", "bo@example.org"),
        to: vec![Address::new("Cy", "cy@example.org")],
        received: 100,
        ..Default::default()
    };
    let piece = || JmapMailResult { account: jmap, email: JMAP.into(), partial: true, ..Default::default() };
    apply_jmap_mail(&mut s, JmapMailResult { listing: Some(FolderListing { folders, complete: true }), ..piece() }).unwrap();
    let page = MessagePage { folder: "M1".into(), messages: vec![received], afresh: true, ..Default::default() };
    apply_jmap_mail(&mut s, JmapMailResult { pages: vec![page], whole: true, partial: false, ..piece() }).unwrap();
    let invoice = s.mailbox.messages.values().find(|m| m.subject == "Invoice").unwrap().id;
    let outbox = s.mailbox.folder_of_kind(jmap, FolderKind::Outbox).unwrap();
    // Mail the server has, and a message that waits to be sent: no copy of either goes there.
    let lunch = written(&mut s, jmap, "Lunch");
    for id in [invoice, lunch] {
        let refused = s.run("mail.copy", &json!({"ids": [id], "folder": outbox})).unwrap_err();
        assert!(refused.to_string().contains(mailcraft_model::COPY_INTO_OUTBOX), "{refused}");
    }
    assert_eq!(count(&s, jmap, FolderKind::Outbox), 1);
    // The next run sends what Send queued, once, and nothing else.
    assert_eq!(offered(&s, jmap), [lunch]);
    // A copy anywhere else in the account is made as before, and is nothing to send.
    let inbox = s.mailbox.folder_of_kind(jmap, FolderKind::Inbox).unwrap();
    s.run("mail.copy", &json!({"ids": [invoice], "folder": inbox})).unwrap();
    assert_eq!(offered(&s, jmap), [lunch]);
}

/// Mail only this computer has (a draft saved in the account, mail brought into it) can be moved
/// into a JMAP account's Outbox, and nothing would tell it from mail queued with Send: it is held
/// there, as in an IMAP account's Outbox, and goes only on Send Again.
#[test]
fn a_message_moved_into_a_jmap_accounts_outbox_is_held_and_not_sent() {
    let (mut s, _, jmap, _) = mailbox();
    let outbox = s.mailbox.folder_of_kind(jmap, FolderKind::Outbox).unwrap();
    let drafts = s.mailbox.folder_of_kind(jmap, FolderKind::Drafts).unwrap();
    let inbox = s.mailbox.folder_of_kind(jmap, FolderKind::Inbox).unwrap();
    let here = |folder, subject: &str, draft| mailcraft_model::Message {
        folder,
        subject: subject.into(),
        from: Address::new("Bo", "bo@example.org"),
        to: vec![Address::new("Cy", "cy@example.org")],
        date: 100,
        draft,
        ..Default::default()
    };
    let draft = s.mailbox.add_message(here(drafts, "Draft", true)).unwrap();
    let brought = s.mailbox.add_message(here(inbox, "Invoice", false)).unwrap();
    let queued = written(&mut s, jmap, "Lunch");
    s.run("mail.move", &json!({"ids": [draft, brought], "folder": outbox})).unwrap();
    for id in [draft, brought] {
        let m = s.mailbox.message(id).unwrap();
        assert_eq!((m.folder, m.held.as_str()), (outbox, mailcraft_model::MOVED_INTO_OUTBOX));
    }
    // The next run sends what Send queued, and nothing that was only moved there.
    assert_eq!(offered(&s, jmap), [queued]);
    assert_eq!(super::graph_send::waiting(&s.mailbox, jmap), 1);
    // Until the owner says so.
    s.run("mail.sendAgain", &json!({"id": draft})).unwrap();
    assert_eq!(offered(&s, jmap), [draft, queued]);
    // Moved out again, it is no longer held for having been there.
    s.run("mail.move", &json!({"ids": [brought], "folder": inbox})).unwrap();
    assert!(s.mailbox.message(brought).unwrap().held.is_empty());
    // What Send queued, taken out and put back, waits for Send Again too.
    s.run("mail.move", &json!({"ids": [queued], "folder": drafts})).unwrap();
    s.run("mail.move", &json!({"ids": [queued], "folder": outbox})).unwrap();
    assert_eq!(offered(&s, jmap), [draft]);
}

#[test]
fn sending_from_a_jmap_account_is_never_undone() {
    let (mut s, _, jmap, _) = mailbox();
    let id = written(&mut s, jmap, "Lunch");
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.message(id).is_some());
    assert_eq!(offered(&s, jmap), [id]);
}

#[test]
fn the_accounts_beside_a_jmap_account_send_as_before() {
    let (mut s, local, jmap, imap) = mailbox();
    // An IMAP account queues its mail for its own run, which a JMAP run never offers.
    s.run("mail.new", &json!({"account": imap, "to": "bo@example.org", "subject": "Plans"})).unwrap();
    let sent = s.run("compose.send", &json!({})).unwrap();
    assert_eq!((sent["queued"].as_bool(), count(&s, imap, FolderKind::Outbox), count(&s, imap, FolderKind::Sent)), (Some(true), 1, 0));
    assert!(offered(&s, jmap).is_empty());
    // A local account files its mail as sent, and delivers it to the local accounts it names.
    s.run("mail.new", &json!({"account": local, "to": "ada@example.org", "subject": "Note"})).unwrap();
    let sent = s.run("compose.send", &json!({})).unwrap();
    assert_eq!((sent["queued"].as_bool(), sent["delivered"].as_u64()), (Some(false), Some(1)));
    assert_eq!([FolderKind::Outbox, FolderKind::Sent].map(|kind| count(&s, local, kind)), [0, 1]);
}

#[test]
fn a_send_receive_of_a_jmap_account_is_its_own_request() {
    let (mut s, _, jmap, imap) = mailbox();
    // With none named, the first account with a server: the JMAP account, with the request of its
    // own (every mailbox read; nothing to change or send says nothing of either).
    let first = s.run("sync.request", &json!({})).unwrap();
    assert_eq!((first["jmap"].as_bool(), first["account"].as_u64(), first["scope"].as_str()), (Some(true), Some(jmap), Some("all")));
    assert!(first.get("sends").is_none() && first.get("changes").is_none(), "{first}");
    // With a message in its Outbox, the request offers it.
    let id = written(&mut s, jmap, "Lunch");
    let second = s.run("sync.jmapRequest", &json!({"account": jmap, "light": true})).unwrap();
    assert_eq!(second["sends"].as_array().map(|sends| sends.iter().map(|up| up["message"].as_u64()).collect::<Vec<_>>()), Some(vec![Some(id)]));
    // Named, the IMAP account's is the request it always was.
    assert_eq!(s.run("sync.request", &json!({"account": imap})).unwrap()["account"]["id"].as_u64(), Some(imap));
    // No IMAP request is made of settings a JMAP account doesn't have.
    let why = crate::cmd::account::sync_request(&s, jmap).unwrap_err().to_string();
    assert_eq!(why, "ada@jmap.example is a JMAP account, and this version doesn't send and receive with one yet");
}

#[test]
fn a_message_never_dated_is_dated_once_before_a_run_so_that_every_run_offers_the_same() {
    let (mut s, _, jmap, _) = mailbox();
    let id = written(&mut s, jmap, "Undated");
    s.mailbox.message_mut(id).unwrap().date = 0;
    let date_offered = |s: &Session| jmap_mail_request(s, jmap, true).unwrap().sends[0].mime.as_ref().unwrap().date;
    // The first run's request dates it with the clock, and keeps the date in the mailbox.
    s.clock.now = Some(1_791_993_600);
    s.run("sync.jmapRequest", &json!({"account": jmap})).unwrap();
    assert_eq!((s.mailbox.message(id).unwrap().date, date_offered(&s)), (1_791_993_600, 1_791_993_600));
    // A later run, by either way of asking, offers the same date: the same octets go up again.
    s.clock.now = Some(1_791_997_200);
    s.run("sync.jmapRequest", &json!({"account": jmap})).unwrap();
    s.run("sync.request", &json!({"account": jmap})).unwrap();
    assert_eq!((s.mailbox.message(id).unwrap().date, date_offered(&s)), (1_791_993_600, 1_791_993_600));
}
