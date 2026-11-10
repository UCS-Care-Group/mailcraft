// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A draft begun on another device, opened here, edited and sent, goes with the next run, for an
//! Exchange server (EWS) and a Microsoft (Graph) account alike: queued here, it is the owner's to
//! send. A message the server itself put in the Outbox is still never sent from here. Every
//! message here is invented.

use mailcraft_engine::Session;
use mailcraft_engine::cmd::{ews_mail, graph_mail, graph_send};
use mailcraft_model::sync::{
    DraftDone, EwsFolderDeltas, EwsMailResult, EwsMessagePage, FolderListing, GraphFetchResult, GraphFetched, GraphFolder, GraphMailResult,
    GraphMessage, MessagePage, SendDone, SendOutcome, ServerMessage,
};
use mailcraft_model::{Address, FolderKind, Id};
use serde_json::json;

const ME: &str = "ada@example.org";
const EWS: &str = "https://exchange.example.org/EWS/Exchange.asmx";

#[derive(Clone, Copy, Debug)]
enum Kind {
    Ews,
    Graph,
}

fn folder(id: &str, name: &str, kind: FolderKind) -> GraphFolder {
    GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() }
}

fn folders() -> Vec<GraphFolder> {
    vec![
        folder("F1", "Inbox", FolderKind::Inbox),
        folder("F4", "Drafts", FolderKind::Drafts),
        folder("F2", "Sent Items", FolderKind::Sent),
        folder("F5", "Outbox", FolderKind::Outbox),
    ]
}

fn listed(id: &str, subject: &str, draft: bool) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), size: 500, change_key: "CK1".into(), ..Default::default() },
        message_id: format!("{}@example.org", id.to_lowercase()),
        subject: subject.into(),
        from: Address::new("Ada", ME),
        to: vec![Address::new("Alice", "alice@example.org"), Address::new("Bob", "bob@example.org")],
        sent: 900,
        received: 1000,
        preview: subject.into(),
        draft: Some(draft),
        ..Default::default()
    }
}

/// An account of `kind` whose server's Drafts holds D1, begun on another device, and whose
/// server's Outbox holds O1, put there by the server.
fn account(kind: Kind) -> (Session, Id) {
    let mut s = Session::empty();
    let added = match kind {
        Kind::Ews => s.run("account.add", &json!({"email": ME, "kind": "exchange", "ews": {"url": EWS}})),
        Kind::Graph => s.run("account.add", &json!({"email": ME, "kind": "microsoft"})),
    };
    let account = added.unwrap()["id"].as_u64().unwrap();
    let drafts = vec![listed("D1", "Plan", true)];
    let outbox = vec![listed("O1", "Delivered", false)];
    match kind {
        Kind::Ews => {
            let piece = || EwsMailResult { account, email: ME.into(), url: EWS.into(), partial: true, ..Default::default() };
            let deltas = EwsFolderDeltas { changed: folders(), afresh: true, ..Default::default() };
            ews_mail::apply_ews_mail(&mut s, EwsMailResult { folders: Some(deltas), hierarchy: "H1".into(), ..piece() }).unwrap();
            for (f, messages) in [("F4", drafts), ("F5", outbox)] {
                let page = EwsMessagePage { folder: f.into(), messages, afresh: true, last: true, state: "S1".into(), ..Default::default() };
                ews_mail::apply_ews_mail(&mut s, EwsMailResult { pages: vec![page], ..piece() }).unwrap();
            }
        }
        Kind::Graph => {
            let listing = Some(FolderListing { folders: folders(), complete: true });
            let pages = vec![
                MessagePage { folder: "F4".into(), messages: drafts, afresh: true, ..Default::default() },
                MessagePage { folder: "F5".into(), messages: outbox, afresh: true, ..Default::default() },
            ];
            graph_mail::apply_graph_mail(&mut s, GraphMailResult { account, email: ME.into(), listing, pages, ..Default::default() }).unwrap();
        }
    }
    (s, account)
}

fn by_server(s: &Session, id: &str) -> Option<Id> {
    s.mailbox.messages.values().find(|m| m.server.as_ref().is_some_and(|x| x.id == id)).map(|m| m.id)
}

fn fetched(s: &mut Session, kind: Kind, account: Id, id: &str, mime: &[u8]) {
    let result =
        GraphFetchResult { account, email: ME.into(), fetched: vec![GraphFetched { id: id.into(), mime: mime.to_vec() }], ..Default::default() };
    let said = match kind {
        Kind::Ews => ews_mail::apply_ews_fetch(s, result),
        Kind::Graph => graph_mail::apply_graph_fetch(s, result),
    };
    assert_eq!(said.unwrap()["fetched"], 1);
}

const PLAN: &[u8] = b"From: Ada <ada@example.org>\r\nTo: Alice <alice@example.org>, Bob <bob@example.org>\r\nSubject: Plan\r\n\
Message-ID: <d1@example.org>\r\n\r\nBegun on the phone.\r\n";

#[test]
fn a_draft_begun_elsewhere_edited_and_sent_here_goes() {
    for kind in [Kind::Ews, Kind::Graph] {
        let (mut s, account) = account(kind);
        let d = by_server(&s, "D1").unwrap();
        fetched(&mut s, kind, account, "D1", PLAN);
        s.run("mail.editDraft", &json!({"id": d})).unwrap();
        s.run("compose.set", &json!({"to": "alice@example.org", "body": "Only for Alice."})).unwrap();
        s.run("compose.send", &json!({})).unwrap();
        let sends = graph_send::sends(&s, account);
        assert_eq!(sends.len(), 1, "{kind:?}: the draft sent here waits to go");
        let queued = s.mailbox.message(sends[0].message).unwrap();
        assert_eq!(queued.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["alice@example.org"], "{kind:?}");
        assert_eq!(queued.server.as_ref().map(|x| x.id.as_str()), Some("D1"), "{kind:?}: sent as the server's draft");
        // What the server put in its Outbox itself is still not sent from here (AU2-V2).
        let delivered = by_server(&s, "O1").unwrap();
        assert!(!sends.iter().any(|up| up.message == delivered), "{kind:?}");
        assert!(graph_send::from_server(s.mailbox.message(delivered).unwrap()), "{kind:?}");
    }
}

/// Edited and saved first, then sent from the draft as it is: the same.
#[test]
fn a_draft_begun_elsewhere_saved_then_sent_here_goes() {
    for kind in [Kind::Ews, Kind::Graph] {
        let (mut s, account) = account(kind);
        let d = by_server(&s, "D1").unwrap();
        fetched(&mut s, kind, account, "D1", PLAN);
        s.run("mail.editDraft", &json!({"id": d})).unwrap();
        s.run("compose.set", &json!({"body": "Changed here."})).unwrap();
        s.run("compose.saveDraft", &json!({})).unwrap();
        s.run("compose.send", &json!({})).unwrap();
        assert_eq!(graph_send::sends(&s, account).len(), 1, "{kind:?}");
    }
}

/// What 0.8.1 to 0.11.0 left behind: a draft begun elsewhere, sent here without a save, in the
/// Outbox with no marker (sent, so with the owner's change waiting to go up). It goes with the
/// next run now; what the server put in its Outbox still doesn't.
#[test]
fn a_draft_left_in_the_outbox_by_an_earlier_version_goes() {
    for kind in [Kind::Ews, Kind::Graph] {
        let (mut s, account) = account(kind);
        let d = by_server(&s, "D1").unwrap();
        fetched(&mut s, kind, account, "D1", PLAN);
        s.run("mail.editDraft", &json!({"id": d})).unwrap();
        s.run("compose.set", &json!({"to": "alice@example.org"})).unwrap();
        s.run("compose.send", &json!({})).unwrap();
        let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
        let stuck = s.mailbox.messages_in(outbox).find(|m| m.server.as_ref().is_some_and(|x| x.id == "D1")).map(|m| m.id).unwrap();
        // As the earlier version left it: no marker.
        let sd = s.mailbox.message_mut(stuck).unwrap().server_draft.as_mut().unwrap();
        sd.marker.clear();
        assert!(sd.pending, "{kind:?}: sent here, its change waits to go up");
        let sends = graph_send::sends(&s, account);
        assert_eq!(sends.iter().map(|up| up.message).collect::<Vec<_>>(), [stuck], "{kind:?}");
        let delivered = by_server(&s, "O1").unwrap();
        assert!(graph_send::from_server(s.mailbox.message(delivered).unwrap()), "{kind:?}");
        assert!(!graph_send::from_server(s.mailbox.message(stuck).unwrap()), "{kind:?}");
    }
}

/// Such a leftover tried by a run and held (#738): the draft went up whole, so its change no longer
/// waits, and then the send was refused or its answer lost. Send Again still sends it, and it isn't
/// taken for mail the server put in its Outbox when the folder is let go of: the owner queued it
/// here. Until Send Again it isn't sent again by itself (AU3-26).
#[test]
fn a_leftover_held_after_its_draft_went_up_goes_with_send_again() {
    let lang = mailcraft_i18n::Lang::default();
    let outcomes = [SendOutcome::Held { lost: true, why: String::new() }, SendOutcome::Held { lost: false, why: "it can't take it".into() }];
    for kind in [Kind::Ews, Kind::Graph] {
        for outcome in outcomes.clone() {
            let (mut s, account) = account(kind);
            let d = by_server(&s, "D1").unwrap();
            fetched(&mut s, kind, account, "D1", PLAN);
            s.run("mail.editDraft", &json!({"id": d})).unwrap();
            s.run("compose.set", &json!({"to": "alice@example.org"})).unwrap();
            s.run("compose.send", &json!({})).unwrap();
            let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
            let stuck = s.mailbox.messages_in(outbox).find(|m| m.server.as_ref().is_some_and(|x| x.id == "D1")).map(|m| m.id).unwrap();
            s.mailbox.message_mut(stuck).unwrap().server_draft.as_mut().unwrap().marker.clear();
            // The run: the draft goes up whole, and the send is held.
            let up = graph_send::sends(&s, account).into_iter().find(|up| up.message == stuck).and_then(|up| up.draft).unwrap();
            let draft = DraftDone {
                id: "D1".into(),
                fields: true,
                attachments: up.attachments.as_ref().map(|_| Vec::new()),
                change: up.without_data(),
                ..Default::default()
            };
            let done = SendDone { message: stuck, outcome: outcome.clone(), draft: Some(draft), draft_left: String::new() };
            let mut said = Vec::new();
            assert!(graph_send::send_done(&mut s.mailbox, account, &done, lang, &mut said), "{kind:?} {outcome:?}");
            let m = s.mailbox.message(stuck).unwrap();
            assert!(!m.held.is_empty() && !said.is_empty(), "{kind:?} {outcome:?}: held, and said");
            assert!(!m.server_draft.as_ref().unwrap().pending, "{kind:?} {outcome:?}: its change went up");
            assert!(graph_send::sends(&s, account).is_empty(), "{kind:?} {outcome:?}: not sent again by itself");
            // Still the owner's, never the server's.
            assert!(!graph_send::from_server(m), "{kind:?} {outcome:?}");
            let delivered = by_server(&s, "O1").unwrap();
            assert!(graph_send::from_server(s.mailbox.message(delivered).unwrap()), "{kind:?} {outcome:?}");
            s.run("mail.sendAgain", &json!({"id": stuck})).unwrap();
            let sends = graph_send::sends(&s, account);
            assert_eq!(sends.iter().map(|up| up.message).collect::<Vec<_>>(), [stuck], "{kind:?} {outcome:?}: Send Again sends it");
            assert_eq!(graph_send::waiting(&s.mailbox, account), 1, "{kind:?} {outcome:?}");
        }
    }
}
