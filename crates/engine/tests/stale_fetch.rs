// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! AU3-16: a message's whole MIME, asked for while it was only its headers here, is taken in only
//! while it still is: an answer that comes after the message was fetched another way, edited, or
//! queued to be sent is dropped, for Exchange (EWS), Microsoft Graph and JMAP accounts alike, so
//! that it never puts back what the owner took out. Every message here is invented.

use mailcraft_engine::Session;
use mailcraft_engine::cmd::{ews_mail, graph_mail, graph_send, jmap_mail};
use mailcraft_model::sync::{
    EwsFolderDeltas, EwsMailResult, EwsMessagePage, FolderListing, GraphFetchResult, GraphFetched, GraphFolder, GraphMailResult, GraphMessage,
    JmapMailResult, MessagePage, ServerMessage,
};
use mailcraft_model::{Address, FolderKind, Id};
use serde_json::json;

const ME: &str = "ada@example.org";
const EWS: &str = "https://exchange.example.org/EWS/Exchange.asmx";

/// What the server has of the draft: the earlier recipients and text.
const OLD: &[u8] = b"From: Ada <ada@example.org>\r\nTo: Alice <alice@example.org>, Bob <bob@example.org>\r\nSubject: Plan\r\n\
Message-ID: <d1@example.org>\r\n\r\nOld text for Bob too.\r\n";

#[derive(Clone, Copy, Debug)]
enum Kind {
    Ews,
    Graph,
    Jmap,
}

fn folder(id: &str, name: &str, kind: FolderKind) -> GraphFolder {
    GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() }
}

fn folders() -> Vec<GraphFolder> {
    vec![folder("F1", "Inbox", FolderKind::Inbox), folder("F4", "Drafts", FolderKind::Drafts), folder("F2", "Sent Items", FolderKind::Sent)]
}

/// The draft as a listing brings it: its headers, to be fetched whole.
fn draft_listed(change_key: &str) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: "D1".into(), size: 500, change_key: change_key.into(), ..Default::default() },
        message_id: "d1@example.org".into(),
        subject: "Plan".into(),
        from: Address::new("Ada", ME),
        to: vec![Address::new("Alice", "alice@example.org"), Address::new("Bob", "bob@example.org")],
        sent: 900,
        received: 1000,
        preview: "Draft".into(),
        draft: Some(true),
        ..Default::default()
    }
}

/// A session with one account of `kind` whose Drafts holds draft D1, headers only.
fn with_draft(kind: Kind) -> (Session, Id) {
    let mut s = Session::empty();
    let added = match kind {
        Kind::Ews => s.run("account.add", &json!({"email": ME, "kind": "exchange", "ews": {"url": EWS}})),
        Kind::Graph => s.run("account.add", &json!({"email": ME, "kind": "microsoft"})),
        Kind::Jmap => s.run("account.add", &json!({"email": ME, "kind": "jmap", "jmap": {"session": "https://jmap.example.org/jmap/session"}})),
    };
    let account = added.unwrap()["id"].as_u64().unwrap();
    let page = |change_key: &str| MessagePage { folder: "F4".into(), messages: vec![draft_listed(change_key)], afresh: true, ..Default::default() };
    match kind {
        Kind::Ews => {
            let piece = || EwsMailResult { account, email: ME.into(), url: EWS.into(), partial: true, ..Default::default() };
            let deltas = EwsFolderDeltas { changed: folders(), afresh: true, ..Default::default() };
            ews_mail::apply_ews_mail(&mut s, EwsMailResult { folders: Some(deltas), hierarchy: "H1".into(), ..piece() }).unwrap();
            let page = EwsMessagePage {
                folder: "F4".into(),
                messages: vec![draft_listed("CK1")],
                afresh: true,
                last: true,
                state: "S1".into(),
                ..Default::default()
            };
            ews_mail::apply_ews_mail(&mut s, EwsMailResult { pages: vec![page], ..piece() }).unwrap();
        }
        Kind::Graph => {
            let listing = Some(FolderListing { folders: folders(), complete: true });
            graph_mail::apply_graph_mail(
                &mut s,
                GraphMailResult { account, email: ME.into(), listing, pages: vec![page("CK1")], ..Default::default() },
            )
            .unwrap();
        }
        Kind::Jmap => {
            let piece = || JmapMailResult { account, email: ME.into(), partial: true, ..Default::default() };
            let listing = Some(FolderListing { folders: folders(), complete: true });
            jmap_mail::apply_jmap_mail(&mut s, JmapMailResult { listing, ..piece() }).unwrap();
            jmap_mail::apply_jmap_mail(&mut s, JmapMailResult { pages: vec![page("CK1")], ..piece() }).unwrap();
            jmap_mail::apply_jmap_mail(&mut s, JmapMailResult { whole: true, partial: false, ..piece() }).unwrap();
        }
    }
    (s, account)
}

fn draft(s: &Session) -> Id {
    s.mailbox.messages.values().find(|m| m.server.as_ref().is_some_and(|x| x.id == "D1")).map(|m| m.id).unwrap()
}

/// Ask for the background's batch, or one message at once.
fn ask(s: &Session, kind: Kind, account: Id, message: Option<Id>) -> Vec<String> {
    match kind {
        Kind::Ews => ews_mail::ews_fetch_request(s, account, message, 10).unwrap().ids,
        Kind::Graph => graph_mail::graph_fetch_request(s, account, message, 10).unwrap().ids,
        Kind::Jmap => jmap_mail::jmap_fetch_request(s, account, message, 10).unwrap().ids,
    }
}

/// The server's answer to a fetch of D1; how many messages were taken in.
fn answer(s: &mut Session, kind: Kind, account: Id, mime: &[u8]) -> u64 {
    let result =
        GraphFetchResult { account, email: ME.into(), fetched: vec![GraphFetched { id: "D1".into(), mime: mime.to_vec() }], ..Default::default() };
    let said = match kind {
        Kind::Ews => ews_mail::apply_ews_fetch(s, result),
        Kind::Graph => graph_mail::apply_graph_fetch(s, result),
        Kind::Jmap => jmap_mail::apply_jmap_fetch(s, result),
    };
    said.unwrap()["fetched"].as_u64().unwrap()
}

fn recipients(s: &Session, id: Id) -> Vec<String> {
    s.mailbox.message(id).unwrap().to.iter().map(|a| a.email.clone()).collect()
}

/// v01: the background asks for the draft, the owner opens it (fetched at once), takes Bob off,
/// writes new text and sends it; then the background's answer comes. (A JMAP account doesn't
/// send yet: its drafts are held to the same rule by the tests below.)
#[test]
fn a_late_fetch_never_overwrites_a_draft_edited_and_queued() {
    for kind in [Kind::Ews, Kind::Graph] {
        let (mut s, account) = with_draft(kind);
        let d = draft(&s);
        assert!(s.mailbox.message(d).unwrap().unfetched);
        // 1. The background's batch, in flight.
        assert!(ask(&s, kind, account, None).contains(&"D1".to_owned()), "{kind:?}");
        // 2. Opened: fetched at once, and answered first.
        assert_eq!(ask(&s, kind, account, Some(d)), ["D1"]);
        assert_eq!(answer(&mut s, kind, account, OLD), 1);
        // 3. Edited and sent.
        s.run("mail.editDraft", &json!({"id": d})).unwrap();
        s.run("compose.set", &json!({"to": "alice@example.org", "body": "Only for Alice."})).unwrap();
        s.run("compose.send", &json!({})).unwrap();
        let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
        let queued: Vec<Id> = s.mailbox.messages_in(outbox).map(|m| m.id).collect();
        assert_eq!(queued.len(), 1, "{kind:?}");
        let queued = queued[0];
        let before = s.mailbox.message(queued).cloned().unwrap();
        assert_eq!(recipients(&s, queued), ["alice@example.org"]);
        // 4. The background's answer: dropped, and the queued message is as the owner left it.
        assert_eq!(answer(&mut s, kind, account, OLD), 0, "{kind:?}");
        let after = s.mailbox.message(queued).unwrap();
        assert!(*after == before, "{kind:?}");
        assert!(after.body.text.contains("Only for Alice.") && !after.body.text.contains("Bob"));
        // 5. What the next run sends is what the owner wrote, to whom the owner wrote it: as MIME
        // for an Exchange server, as the draft's fields for Microsoft Graph.
        let sends = graph_send::sends(&s, account);
        assert_eq!(sends.len(), 1, "{kind:?}");
        match (sends[0].mime.as_ref(), sends[0].draft.as_ref()) {
            (Some(mime), _) => {
                assert_eq!(mime.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["alice@example.org"], "{kind:?}");
                assert!(mime.body.text.contains("Only for Alice."), "{kind:?}");
            }
            (None, Some(draft)) => {
                let to: Vec<&str> = draft.to.iter().flatten().map(|a| a.email.as_str()).collect();
                assert_eq!(to, ["alice@example.org"], "{kind:?}");
                let body = draft.body.as_ref().map(|b| b.content.as_str()).unwrap_or_default();
                assert!(body.contains("Only for Alice.") && !body.contains("Bob"), "{kind:?}: {body}");
            }
            (None, None) => panic!("{kind:?}: nothing to send"),
        }
    }
}

/// Fetched whole once, a message takes no second answer; edited and saved here, its change
/// still to go up, a draft takes none either, even when it is back to its headers meanwhile.
#[test]
fn a_late_fetch_leaves_a_message_fetched_or_changed_here_as_it_is() {
    for kind in [Kind::Ews, Kind::Graph, Kind::Jmap] {
        let (mut s, account) = with_draft(kind);
        let d = draft(&s);
        assert_eq!(answer(&mut s, kind, account, OLD), 1, "{kind:?}");
        // Whole now: the same answer again is dropped.
        assert_eq!(answer(&mut s, kind, account, OLD), 0, "{kind:?}");
        s.run("mail.editDraft", &json!({"id": d})).unwrap();
        s.run("compose.set", &json!({"to": "alice@example.org", "body": "Only for Alice."})).unwrap();
        s.run("compose.saveDraft", &json!({})).unwrap();
        s.run("compose.close", &json!({})).ok();
        let saved = s.mailbox.messages.values().find(|m| m.server.as_ref().is_some_and(|x| x.id == "D1")).cloned().unwrap();
        assert_eq!(answer(&mut s, kind, account, OLD), 0, "{kind:?}");
        assert_eq!(recipients(&s, saved.id), ["alice@example.org"]);
        // A JMAP account sends no draft up yet: nothing of it waits to go up.
        if let Kind::Jmap = kind {
            continue;
        }
        assert!(saved.server_draft.as_ref().is_some_and(|sd| sd.pending), "{kind:?}: the change waits to go up");
        s.mailbox.message_mut(saved.id).unwrap().unfetched = true;
        assert_eq!(answer(&mut s, kind, account, OLD), 0, "{kind:?}");
        assert_eq!(recipients(&s, saved.id), ["alice@example.org"]);
        assert!(s.mailbox.message(saved.id).unwrap().body.text.contains("Only for Alice."));
    }
}

/// A message in the Outbox takes no fetched MIME, even one still only its headers; one that is
/// still to fetch, and nothing else, takes it.
#[test]
fn a_message_in_the_outbox_takes_no_fetch_and_one_still_to_fetch_does() {
    for kind in [Kind::Ews, Kind::Graph, Kind::Jmap] {
        let (mut s, account) = with_draft(kind);
        let d = draft(&s);
        let drafts = s.mailbox.message(d).unwrap().folder;
        let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
        s.mailbox.message_mut(d).unwrap().folder = outbox;
        assert_eq!(answer(&mut s, kind, account, OLD), 0, "{kind:?}");
        assert!(s.mailbox.message(d).unwrap().unfetched);
        s.mailbox.message_mut(d).unwrap().folder = drafts;
        assert_eq!(answer(&mut s, kind, account, OLD), 1, "{kind:?}");
        let m = s.mailbox.message(d).unwrap();
        assert!(!m.unfetched && m.body.text.contains("Old text"));
    }
}

/// A message only its headers that the owner moves into the Outbox is never asked for: its
/// answer would be dropped, and a background that asked again at once would ask forever.
#[test]
fn a_message_in_the_outbox_is_never_asked_for() {
    for kind in [Kind::Ews, Kind::Graph, Kind::Jmap] {
        let (mut s, account) = with_draft(kind);
        let d = draft(&s);
        let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
        if s.run("mail.move", &json!({"ids": [d], "folder": outbox})).is_err() {
            s.mailbox.message_mut(d).unwrap().folder = outbox;
        }
        let m = s.mailbox.message(d).unwrap();
        assert!(m.folder == outbox && m.unfetched, "{kind:?}");
        assert!(ask(&s, kind, account, None).is_empty(), "{kind:?}");
        assert!(ask(&s, kind, account, Some(d)).is_empty(), "{kind:?}");
    }
}

/// A draft sent here and still in the Outbox, changed on the server meanwhile: not asked for
/// either, whatever the server says of its copy.
#[test]
fn a_queued_draft_the_server_changed_is_never_asked_for() {
    for kind in [Kind::Ews, Kind::Graph] {
        let (mut s, account) = with_draft(kind);
        let d = draft(&s);
        assert_eq!(answer(&mut s, kind, account, OLD), 1);
        s.run("mail.editDraft", &json!({"id": d})).unwrap();
        s.run("compose.set", &json!({"to": "alice@example.org", "body": "Only for Alice."})).unwrap();
        s.run("compose.send", &json!({})).unwrap();
        let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
        let queued = s.mailbox.messages_in(outbox).map(|m| m.id).next().unwrap();
        // Its copy on the server changed on another device: a delta says so.
        match kind {
            Kind::Ews => {
                let piece = EwsMailResult { account, email: ME.into(), url: EWS.into(), partial: true, ..Default::default() };
                let page =
                    EwsMessagePage { folder: "F4".into(), messages: vec![draft_listed("CK9")], last: true, state: "S2".into(), ..Default::default() };
                ews_mail::apply_ews_mail(&mut s, EwsMailResult { pages: vec![page], ..piece }).unwrap();
            }
            _ => {
                let page = MessagePage { folder: "F4".into(), messages: vec![draft_listed("CK9")], ..Default::default() };
                graph_mail::apply_graph_mail(&mut s, GraphMailResult { account, email: ME.into(), pages: vec![page], ..Default::default() }).unwrap();
            }
        }
        assert!(!ask(&s, kind, account, None).contains(&"D1".to_owned()), "{kind:?}");
        if s.mailbox.message(queued).is_some() {
            assert!(ask(&s, kind, account, Some(queued)).is_empty(), "{kind:?}");
            assert_eq!(recipients(&s, queued), ["alice@example.org"], "{kind:?}");
        }
    }
}
