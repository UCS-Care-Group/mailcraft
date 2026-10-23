// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Drafts up through Microsoft Graph (R3.4.1), from end to end: the engine's `graph_mail_request`
//! and `apply_graph_mail` with `mailcraft_net::graph::sync_mail` in between, against the strict
//! Graph stand-in. A draft written here is in the server's Drafts and back, once, after a lost
//! answer; saved again it is changed in place, field by field; an answer stays in its
//! conversation; attachments come and go, a large one by upload session; a draft edited on both
//! sides is kept twice. Nothing here leaves this machine; every message is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::AtomicBool;

use mailcraft_charset::base64::STANDARD;
use mailcraft_engine::Session;
use mailcraft_engine::cmd::graph_mail::{apply_graph_fetch, apply_graph_mail, graph_fetch_request, graph_mail_request, unpushed};
use mailcraft_model::{Id, Mailbox};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use serde_json::{Value, json};

/// Friday 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000;

struct Rig {
    server: StandIn,
    s: Session,
    account: Id,
}

impl Rig {
    fn new(address: &str) -> Rig {
        let server = StandIn::start().unwrap();
        let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
        let account = s.run("account.add", &json!({"email": address, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        let mut rig = Rig { server, s, account };
        // The folders first, so that a draft has a Drafts that is the server's.
        rig.run();
        rig
    }

    fn run(&mut self) -> Vec<Value> {
        let request = graph_mail_request(&self.s, self.account).unwrap();
        let _routed = self.server.route();
        let cancel = AtomicBool::new(false);
        let s = &mut self.s;
        let mut answers = Vec::new();
        mailcraft_net::graph::sync_mail(TOKEN, &request, &cancel, &mut |piece| answers.push(apply_graph_mail(s, piece).unwrap()));
        answers
    }

    fn fetch(&mut self) -> Value {
        let request = graph_fetch_request(&self.s, self.account, None, 100).unwrap();
        let _routed = self.server.route();
        let result = mailcraft_net::graph::fetch_messages(TOKEN, &request, &AtomicBool::new(false));
        apply_graph_fetch(&mut self.s, result).unwrap()
    }

    /// The drafts here: subject and server id.
    fn drafts_here(&self) -> Vec<(String, String)> {
        let mut all: Vec<(String, String)> = self
            .s
            .mailbox
            .messages
            .values()
            .filter(|m| m.account == self.account && m.draft)
            .map(|m| (m.subject.clone(), m.server.as_ref().map(|s| s.id.clone()).unwrap_or_default()))
            .collect();
        all.sort();
        all
    }

    /// The drafts on the server: subject and id.
    fn drafts_there(&self) -> Vec<mailcraft_net::graph::standin::Message> {
        let state = self.server.state();
        let drafts = state.well_known("drafts");
        let mut all: Vec<_> = state.mailbox.messages.values().filter(|m| m.folder == drafts).cloned().collect();
        all.sort_by(|a, b| a.subject.cmp(&b.subject));
        all
    }

    fn write(&mut self, to: &str, subject: &str, body: &str) -> Id {
        self.s.run("mail.new", &json!({"account": self.account, "to": to, "subject": subject, "body": body})).unwrap();
        self.s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap()
    }

    fn errors(answers: &[Value]) -> Vec<Value> {
        answers.iter().flat_map(|a| a["errors"].as_array().cloned().unwrap_or_default()).collect()
    }
}

#[test]
fn a_draft_is_made_once_on_the_server_after_a_lost_answer_and_changed_in_place() {
    let mut rig = Rig::new("drafts-once@example.org");
    let draft = rig.write("kit@example.org", "Plans for Friday", "Shall we meet at ten?");
    assert_eq!(unpushed(&rig.s, rig.account), 1, "a saved draft waits to go up");
    rig.server.state().lose_next.push("POST /v1.0/me/messages".into());

    let answers = rig.run();
    assert_eq!(Rig::errors(&answers), Vec::<Value>::new(), "{answers:?}");
    let there = rig.drafts_there();
    assert_eq!(there.len(), 1, "made once, its lost answer found by the marker");
    assert_eq!((there[0].subject.as_str(), there[0].to.as_slice()), ("Plans for Friday", ["kit@example.org".to_string()].as_slice()));
    assert!(there[0].body.contains("Shall we meet at ten?"));
    let server_id = rig.s.mailbox.message(draft).unwrap().server.as_ref().unwrap().id.clone();
    assert_eq!(server_id, there[0].id);
    assert_eq!(unpushed(&rig.s, rig.account), 0);
    // Back down, it is the same draft: nothing twice, here or there.
    rig.run();
    rig.fetch();
    assert_eq!(rig.drafts_here(), [("Plans for Friday".to_string(), server_id.clone())]);
    assert_eq!(rig.drafts_there().len(), 1);

    // Saved again: changed in place, the subject alone.
    rig.s.run("mail.editDraft", &json!({"id": draft})).unwrap();
    rig.s.run("compose.set", &json!({"subject": "Plans for Saturday"})).unwrap();
    rig.s.run("compose.saveDraft", &json!({})).unwrap();
    let from = rig.server.state().requests.len();
    let answers = rig.run();
    assert_eq!(Rig::errors(&answers), Vec::<Value>::new(), "{answers:?}");
    let there = rig.drafts_there();
    assert_eq!((there.len(), there[0].id.as_str(), there[0].subject.as_str()), (1, server_id.as_str(), "Plans for Saturday"));
    let state = rig.server.state();
    let patches: Vec<Value> = state.requests[from..].iter().filter(|r| r.method == "PATCH").map(|r| r.json()).collect();
    assert_eq!(patches.len(), 1, "{patches:?}");
    assert_eq!(patches[0], json!({"subject": "Plans for Saturday"}), "field by field");
    assert!(!state.requests[from..].iter().any(|r| r.method == "POST" && r.target.starts_with("/v1.0/me/messages")), "nothing made a second time");
    drop(state);
    rig.run();
    assert_eq!(rig.drafts_here(), [("Plans for Saturday".to_string(), server_id)]);
}

#[test]
fn an_answer_stays_in_its_conversation() {
    let mut rig = Rig::new("drafts-reply@example.org");
    let source = {
        let mut state = rig.server.state();
        let inbox = state.well_known("inbox");
        state.mailbox.add(&inbox, "Invoice 42", NOW - 3_600)
    };
    rig.run();
    rig.fetch();
    let here = rig.s.mailbox.messages.values().find(|m| m.subject == "Invoice 42").unwrap().id;
    rig.s.run("mail.reply", &json!({"id": here})).unwrap();
    rig.s.run("compose.set", &json!({"body": "Paid today."})).unwrap();
    rig.s.run("compose.saveDraft", &json!({})).unwrap();
    let answers = rig.run();
    assert_eq!(Rig::errors(&answers), Vec::<Value>::new(), "{answers:?}");
    let there = rig.drafts_there();
    assert_eq!(there.len(), 1);
    let state = rig.server.state();
    let orig = state.mailbox.messages[&source].clone();
    assert_eq!(there[0].conversation_id(), orig.conversation_id(), "in its conversation");
    assert_eq!(there[0].in_reply_to, orig.internet_id);
    assert!(there[0].subject.eq_ignore_ascii_case("RE: Invoice 42"), "{}", there[0].subject);
    assert!(there[0].body.contains("Paid today."), "the words written here, not the server's own");
    assert!(state.requests.iter().any(|r| r.target.ends_with("/createReply")));
}

#[test]
fn attachments_come_and_go_and_a_large_one_goes_by_upload_session() {
    let mut rig = Rig::new("drafts-files@example.org");
    rig.s.run("mail.new", &json!({"account": rig.account, "to": "kit@example.org", "subject": "Files"})).unwrap();
    let small = b"The notes.".to_vec();
    let large: Vec<u8> = (0..(3 * 1024 * 1024 + 4_321)).map(|n| (n % 251) as u8).collect();
    rig.s.run("compose.attach", &json!({"name": "notes.txt", "data": STANDARD.encode(&small), "mime": "text/plain"})).unwrap();
    rig.s.run("compose.attach", &json!({"name": "scan.bin", "data": STANDARD.encode(&large), "mime": "application/octet-stream"})).unwrap();
    let draft = rig.s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    let answers = rig.run();
    assert_eq!(Rig::errors(&answers), Vec::<Value>::new(), "{answers:?}");
    let there = rig.drafts_there();
    assert_eq!(there.len(), 1);
    let mut names: Vec<(&str, usize)> = there[0].attachments.iter().map(|a| (a.name.as_str(), a.bytes.len())).collect();
    names.sort();
    assert_eq!(names, [("notes.txt", small.len()), ("scan.bin", large.len())]);
    assert!(there[0].attachments.iter().any(|a| a.bytes == large), "every byte");
    {
        let state = rig.server.state();
        assert!(state.requests.iter().any(|r| r.target.ends_with("/createUploadSession")));
        assert!(state.requests.iter().filter(|r| r.method == "PUT").all(|r| r.header("authorization").is_empty()), "no sign-in goes with the pieces");
    }
    assert_eq!(unpushed(&rig.s, rig.account), 0);

    // One removed here is removed there; the other stays, and isn't sent again.
    rig.s.run("mail.editDraft", &json!({"id": draft})).unwrap();
    let at = rig.s.compose.as_ref().unwrap().attachments.iter().position(|a| a.name == "notes.txt").unwrap();
    rig.s.run("compose.removeAttachment", &json!({"index": at})).unwrap();
    rig.s.run("compose.saveDraft", &json!({})).unwrap();
    let from = rig.server.state().requests.len();
    rig.run();
    let there = rig.drafts_there();
    assert_eq!(there[0].attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["scan.bin"]);
    let state = rig.server.state();
    assert!(!state.requests[from..].iter().any(|r| r.method == "PUT" || r.target.ends_with("/attachments") && r.method == "POST"));
}

#[test]
fn a_draft_edited_here_and_there_is_kept_twice() {
    let mut rig = Rig::new("drafts-twice@example.org");
    let draft = rig.write("kit@example.org", "Agenda", "First words.");
    rig.run();
    let id = rig.drafts_there()[0].id.clone();
    // Changed in Outlook, and here, before the next run.
    rig.server.state().mailbox.update(&id, |m| m.subject = "Agenda (from Outlook)".into());
    rig.s.run("mail.editDraft", &json!({"id": draft})).unwrap();
    rig.s.run("compose.set", &json!({"subject": "Agenda (from here)"})).unwrap();
    rig.s.run("compose.saveDraft", &json!({})).unwrap();
    let answers = rig.run();
    assert_eq!(Rig::errors(&answers), Vec::<Value>::new(), "{answers:?}");
    // The server's version is here as a draft of its own; this one goes up anew.
    rig.run();
    let here: Vec<String> = rig.drafts_here().into_iter().map(|(subject, _)| subject).collect();
    assert_eq!(here, ["Agenda (from Outlook)", "Agenda (from here)"]);
    let there: Vec<String> = rig.drafts_there().into_iter().map(|m| m.subject).collect();
    assert_eq!(there, ["Agenda (from Outlook)", "Agenda (from here)"], "nothing lost on either side");
    rig.run();
    assert_eq!(rig.drafts_there().len(), 2, "and nothing more");
    assert!(rig.drafts_here().iter().all(|(_, id)| !id.is_empty()));
}

#[test]
fn a_draft_changed_elsewhere_only_comes_down_afresh_and_nothing_goes_up() {
    let mut rig = Rig::new("drafts-there@example.org");
    let draft = rig.write("kit@example.org", "Minutes", "Draft one.");
    rig.run();
    let id = rig.drafts_there()[0].id.clone();
    rig.server.state().mailbox.update(&id, |m| {
        m.subject = "Minutes, final".into();
        m.body = "Draft two, from Outlook.".into();
    });
    let from = rig.server.state().requests.len();
    rig.run();
    rig.fetch();
    let m = rig.s.mailbox.message(draft).unwrap();
    assert_eq!(m.subject, "Minutes, final");
    assert!(m.body.text.contains("Draft two, from Outlook."), "{:?}", m.body);
    assert_eq!(unpushed(&rig.s, rig.account), 0);
    rig.run();
    let state = rig.server.state();
    assert!(!state.requests[from..].iter().any(|r| r.method == "PATCH" || r.method == "POST" && r.target.starts_with("/v1.0/me/messages")));
    assert_eq!(rig.drafts_here().len(), 1);
}
