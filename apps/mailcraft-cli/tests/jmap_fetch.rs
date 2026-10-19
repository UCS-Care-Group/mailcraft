//! A JMAP account's messages fetched whole, from end to end: the engine's `jmap_fetch_request` and
//! `apply_jmap_fetch` with `mailcraft_jmap::fetch::fetch_messages` in between, against the in-process
//! JMAP server, as the browser build runs them. Nothing here leaves this process; every name and id
//! is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::AtomicBool;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::jmap_fetch::{PREFETCH_NEWEST, apply_jmap_fetch, jmap_fetch_request};
use mailcraft_engine::cmd::jmap_mail::apply_jmap_mail;
use mailcraft_jmap::fetch::{MAX_MESSAGE, fetch_messages};
use mailcraft_jmap::standin::{SESSION, StandIn, TOKEN, USER};
use mailcraft_jmap::{Auth, Client, block_on};
use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMessage, JmapMailResult, MessagePage, ServerMessage};
use mailcraft_model::{Address, FolderKind, Id, Mailbox, Message};
use serde_json::{Value, json};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A JMAP server with an Inbox, and a session with a JMAP account for it that has not fetched anything.
struct Rig {
    server: StandIn,
    s: Session,
    account: Id,
    inbox: String,
}

/// An RFC 5322 message with a text and an attachment, as a person's mail program writes one.
fn mail(n: usize) -> Vec<u8> {
    format!(
        "From: Grace Hall <grace@example.org>\r\nTo: Ada <{USER}>\r\nSubject: Report {n}\r\nMessage-ID: <report-{n}@example.org>\r\n\
Date: Wed, 14 Oct 2026 09:00:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b{n}\"\r\n\r\n\
--b{n}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThe words of report {n}.\r\n\
--b{n}\r\nContent-Type: text/csv; name=\"figures.csv\"\r\nContent-Disposition: attachment; filename=\"figures.csv\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\nYSxiCjEsMgo=\r\n--b{n}--\r\n"
    )
    .into_bytes()
}

impl Rig {
    /// `n` reports in the Inbox, `report 1` (oldest) to `report n`, listed in the window as a run would.
    fn new(n: usize) -> Rig {
        let server = StandIn::new();
        let inbox = server.with(|d| {
            let inbox = d.mailbox("Inbox", None, Some("inbox"));
            for i in 1..=n {
                d.deliver(&inbox, &mail(i), &[], &format!("2026-10-14T09:{:02}:{:02}Z", i / 60, i % 60));
            }
            inbox
        });
        let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
        let account = s.run("account.add", &json!({"email": USER, "kind": "jmap", "jmap": {"session": SESSION, "bearer": true}})).unwrap()["id"]
            .as_u64()
            .unwrap();
        let mut rig = Rig { server, s, account, inbox: inbox.clone() };
        let folders = vec![GraphFolder { id: inbox, name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
        let listing = JmapMailResult { account, email: USER.into(), listing: Some(FolderListing { folders, complete: true }), ..Default::default() };
        apply_jmap_mail(&mut rig.s, listing).unwrap();
        let messages = rig.server.with(|d| d.emails.iter().map(|e| (e.id.clone(), e.received.clone(), e.raw.len())).collect::<Vec<_>>());
        let messages = messages.into_iter().enumerate().map(|(i, (id, _, size))| window_entry(&id, i + 1, size)).collect();
        rig.window(messages, true);
        rig
    }

    fn window(&mut self, messages: Vec<GraphMessage>, complete: bool) {
        let page = MessagePage { folder: self.inbox.clone(), messages, afresh: complete, ..Default::default() };
        let result = JmapMailResult { account: self.account, email: USER.into(), pages: vec![page], whole: complete, ..Default::default() };
        apply_jmap_mail(&mut self.s, result).unwrap();
    }

    /// The local message that stands for the server's `id`.
    fn here(&self, id: &str) -> &Message {
        self.s.mailbox.messages.values().map(|m| m.as_ref()).find(|m| m.server.as_ref().is_some_and(|s| s.id == id)).unwrap()
    }

    /// The id the server gave the `n`-th report it was given.
    fn id_of(&self, n: usize) -> String {
        self.server.with(|d| d.emails.get(n - 1).map(|e| e.id.clone())).unwrap()
    }

    /// One fetch, as a host runs it: what the engine asks for, done against the server by the client
    /// (connected with the address the request names and the secret the host holds), and taken in. What
    /// taking it in answered, and what the host brought.
    fn fetch(&mut self, message: Option<Id>) -> (Value, mailcraft_model::sync::GraphFetchResult) {
        let request = jmap_fetch_request(&self.s, self.account, message, 10).unwrap();
        let client = block_on(Client::connect(self.server.clone(), &request.session, &Auth::bearer(TOKEN))).unwrap();
        let result = block_on(fetch_messages(&client, &request, &AtomicBool::new(false)));
        (apply_jmap_fetch(&mut self.s, result.clone()).unwrap(), result)
    }
}

/// The server's `n`-th report as a window lists it: its headers and a preview, not the message.
fn window_entry(id: &str, n: usize, size: usize) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), size: size as u64, ..Default::default() },
        message_id: format!("<report-{n}@example.org>"),
        subject: format!("Report {n}"),
        from: Address::new("Grace Hall", "grace@example.org"),
        sent: 1_791_990_000 + n as i64,
        received: 1_791_990_000 + n as i64,
        preview: format!("The words of report {n}."),
        ..Default::default()
    }
}

#[test]
fn a_message_the_owner_opens_comes_whole_through_the_server() {
    let mut rig = Rig::new(3);
    let id = rig.id_of(2);
    let message = rig.here(&id).id;
    assert!(rig.here(&id).unfetched && rig.here(&id).attachments.is_empty());
    let (said, brought) = rig.fetch(Some(message));
    assert_eq!((said["fetched"].as_u64(), said["missing"].as_u64(), said["failed"].as_u64()), (Some(1), Some(0), Some(0)), "{said}");
    assert_eq!(brought.fetched.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), [id.as_str()]);
    // The octets are the server's, and the message is read from them as any message is.
    assert_eq!(brought.fetched[0].mime, mail(2));
    let m = rig.here(&id);
    assert!(!m.unfetched && m.body.text.contains("The words of report 2."), "{:?}", m.body.text);
    assert_eq!((m.subject.as_str(), m.from.email.as_str()), ("Report 2", "grace@example.org"));
    assert_eq!(m.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["figures.csv"]);
    assert_eq!(m.server.as_ref().map(|s| s.id.as_str()), Some(id.as_str()), "it is still the server's message");
    // The server was asked for the blob's name, and for the blob: nothing else of the message.
    assert_eq!(rig.server.calls(), ["Email/get"]);
    let downloads: Vec<_> = rig.server.seen().into_iter().filter(|s| s.path.starts_with("/jmap/download/")).collect();
    assert_eq!(downloads.len(), 1);
    // The others were not touched.
    assert!(rig.here(&rig.id_of(1)).unfetched && rig.here(&rig.id_of(3)).unfetched);
}

#[test]
fn the_background_fetches_the_inboxs_newest_forty_and_no_more() {
    let mut rig = Rig::new(55);
    let mut fetched = 0;
    for _ in 0..20 {
        let (said, brought) = rig.fetch(None);
        if brought.fetched.is_empty() && brought.missing.is_empty() && brought.failed.is_empty() {
            break;
        }
        fetched += brought.fetched.len();
        assert_eq!(said["errors"], json!([]), "{said}");
    }
    assert_eq!(fetched, PREFETCH_NEWEST);
    // The newest forty are whole, the fifteen older wait until they are opened.
    for n in 1..=55 {
        assert_eq!(rig.here(&rig.id_of(n)).unfetched, n <= 55 - PREFETCH_NEWEST, "report {n}");
    }
    // Nothing is left for the background to ask for.
    assert!(jmap_fetch_request(&rig.s, rig.account, None, 10).unwrap().ids.is_empty());
    // Opening one of the older fetches it.
    let oldest = rig.here(&rig.id_of(1)).id;
    let (said, _) = rig.fetch(Some(oldest));
    assert_eq!(said["fetched"], 1);
    assert!(!rig.here(&rig.id_of(1)).unfetched);
}

#[test]
fn a_message_the_server_lost_stays_as_it_is_and_the_background_stops_asking_for_it() {
    let mut rig = Rig::new(4);
    let (lost, kept) = (rig.id_of(4), rig.id_of(3));
    assert!(rig.server.with(|d| d.remove(&lost)));
    let (said, brought) = rig.fetch(None);
    assert_eq!(brought.missing, std::slice::from_ref(&lost));
    assert_eq!(brought.fetched.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), [kept.as_str(), rig.id_of(2).as_str(), rig.id_of(1).as_str()]);
    assert!(brought.errors.is_empty() && !brought.offline, "a message that is gone is no error: {:?}", brought.errors);
    assert_eq!(said["missing"], 1);
    assert!(rig.here(&lost).unfetched, "its headers and preview are what it has");
    // Not asked for again by itself, but asked for when the owner opens it.
    assert!(jmap_fetch_request(&rig.s, rig.account, None, 10).unwrap().ids.is_empty());
    let lost_here = rig.here(&lost).id;
    assert_eq!(jmap_fetch_request(&rig.s, rig.account, Some(lost_here), 1).unwrap().ids, std::slice::from_ref(&lost));
    let (_, again) = rig.fetch(Some(lost_here));
    assert_eq!(again.missing, [lost]);
    // The next run lists the Inbox without it, and it is gone from here too.
    let rest = (1..=3).map(|n| window_entry(&rig.id_of(n), n, mail(n).len())).collect();
    rig.window(rest, true);
    assert_eq!(rig.s.mailbox.messages.len(), 3);
}

#[test]
fn a_large_message_waits_for_the_owner_and_one_beyond_the_most_is_refused() {
    let mut rig = Rig::new(2);
    // Over the 10 MB an account fetches in the background, and over the 50 MB nothing is fetched at all.
    let (large, huge) = rig.server.with(|d| {
        let mut big = mail(3);
        big.extend(std::iter::repeat_n(b' ', 11 << 20));
        let large = d.deliver(&rig.inbox, &big, &[], "2026-10-14T10:00:00Z");
        let huge = d.deliver(&rig.inbox, &vec![b' '; MAX_MESSAGE + 1], &[], "2026-10-14T10:01:00Z");
        (large, huge)
    });
    let sizes = rig.server.with(|d| (d.email(&large).map(|e| e.raw.len()), d.email(&huge).map(|e| e.raw.len())));
    let entries = vec![
        window_entry(&rig.id_of(1), 1, mail(1).len()),
        window_entry(&rig.id_of(2), 2, mail(2).len()),
        window_entry(&large, 3, sizes.0.unwrap_or(0)),
        window_entry(&huge, 4, sizes.1.unwrap_or(0)),
    ];
    rig.window(entries, true);
    // The background leaves the large ones; the answer counts them as waiting for the owner.
    let (said, brought) = rig.fetch(None);
    assert_eq!(brought.fetched.len(), 2, "only the two small ones: {:?}", brought.failed);
    assert!(brought.failed.is_empty() && brought.missing.is_empty());
    assert_eq!((said["large"].as_u64(), said["waiting"].as_u64(), said["all"].as_u64()), (Some(2), Some(2), Some(4)), "{said}");
    // Opened, the large one comes whole; the huge one is refused before a byte of it is asked for.
    let (large_here, huge_here) = (rig.here(&large).id, rig.here(&huge).id);
    let (_, brought) = rig.fetch(Some(large_here));
    assert_eq!(brought.fetched.len(), 1);
    assert!(!rig.here(&large).unfetched);
    let downloads = |rig: &Rig| rig.server.seen().iter().filter(|s| s.path.starts_with("/jmap/download/")).count();
    let before = downloads(&rig);
    let (said, brought) = rig.fetch(Some(huge_here));
    assert_eq!((brought.fetched.len(), brought.failed.clone()), (0, vec![huge.clone()]), "{said}");
    assert_eq!(downloads(&rig), before, "it was not downloaded");
    assert!(rig.here(&huge).unfetched);
}

#[test]
fn a_server_that_turns_the_secret_down_fetches_nothing_and_says_so() {
    let mut rig = Rig::new(2);
    let request = jmap_fetch_request(&rig.s, rig.account, None, 10).unwrap();
    // The host's token is not the one the server holds.
    let refused = block_on(Client::connect(rig.server.clone(), &request.session, &Auth::bearer("not-the-token")));
    assert!(matches!(refused, Err(mailcraft_jmap::Error::Auth(_))));
    // A token that is turned down after the connection was made.
    let client = block_on(Client::connect(rig.server.clone(), &request.session, &Auth::bearer(TOKEN))).unwrap();
    rig.server.with(|d| d.token = Some("rotated".into()));
    let result = block_on(fetch_messages(&client, &request, &AtomicBool::new(false)));
    assert!(result.fetched.is_empty() && result.errors.len() == 1 && !result.offline, "{:?}", result.errors);
    let said = apply_jmap_fetch(&mut rig.s, result).unwrap();
    assert_eq!((said["fetched"].as_u64(), said["errors"].as_array().map(Vec::len)), (Some(0), Some(1)));
    // Nothing was passed over for it: the next try asks for the same messages.
    assert_eq!(jmap_fetch_request(&rig.s, rig.account, None, 10).unwrap().ids, request.ids);
}
