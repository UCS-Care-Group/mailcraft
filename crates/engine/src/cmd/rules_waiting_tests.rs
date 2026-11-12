// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rules on this computer that read what a server's list of new mail doesn't give (the text,
//! headers such as Sensitivity, the size) have their say once the message is fetched whole (#566):
//! for a Microsoft account and an Exchange server account, and through a JMAP account's fetch
//! too, though its runs don't yet put new mail through the rules as it arrives.

use mailcraft_model::sync::{
    EwsFolderDeltas, EwsMailResult, EwsMessagePage, FolderListing, GraphFetchResult, GraphFetched, GraphFolder, GraphMailResult, GraphMessage,
    JmapMailResult, MessagePage, ServerMessage,
};
use mailcraft_model::{Address, FlagStatus, FolderKind, Id, Message, ServerFolder};
use serde_json::json;

use crate::Session;
use crate::cmd::{ews_mail, graph_mail, jmap_mail};

const ME: &str = "ada@example.org";

/// What a server's list gives of a new message: its headers and a preview, no text. (An Exchange
/// server's gives its size too; a Microsoft account's doesn't, which
/// `a_size_rule_on_microsoft_new_mail_listed_without_a_size_waits_for_the_download` takes.)
fn listed(id: &str, folder: &str, subject: &str) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), folder: folder.into(), size: 1_200, change_key: "CK1".into(), ..Default::default() },
        message_id: format!("{id}@example.org"),
        subject: subject.into(),
        from: Address::new("Zoe Park", "zoe@example.org"),
        to: vec![Address::new("Ada", ME)],
        sent: 1_000,
        received: 1_060,
        preview: format!("About {subject}"),
        ..Default::default()
    }
}

/// The whole message, as a fetch brings it.
fn whole(id: &str, subject: &str, headers: &str, text: &str) -> GraphFetched {
    let mime = format!(
        "From: Zoe Park <zoe@example.org>\r\nTo: Ada <{ME}>\r\nSubject: {subject}\r\nMessage-ID: <{id}@example.org>\r\n{headers}Content-Type: text/plain\r\n\r\n{text}\r\n"
    );
    GraphFetched { id: id.into(), mime: mime.into_bytes() }
}

/// Two rules of this computer: one with an exception that reads the text, one whose condition
/// reads the Sensitivity header.
fn rules(s: &mut Session) {
    let unless = json!({
        "name": "Zoe", "server": false,
        "conditions": [{"type": "fromContains", "value": "zoe"}],
        "exceptions": [{"type": "bodyContains", "value": "unsubscribe"}],
        "actions": [{"type": "flag"}],
    });
    s.run("rules.add", &unless).unwrap();
    let private =
        json!({"name": "Private", "server": false, "conditions": [{"type": "sensitivity", "value": "private"}], "actions": [{"type": "markRead"}]});
    s.run("rules.add", &private).unwrap();
}

fn here<'s>(s: &'s Session, account: Id, id: &str) -> &'s Message {
    s.mailbox.messages.values().map(|m| &**m).find(|m| m.account == account && m.server.as_ref().is_some_and(|server| server.id == id)).unwrap()
}

/// After the arrival: neither rule could tell, so neither acted; once the messages are here whole,
/// each has its say: the one whose text holds the exception's word is left alone, the private one
/// is marked read.
fn check(s: &mut Session, account: Id, fetch: impl Fn(&mut Session, Vec<GraphFetched>)) {
    for id in ["N1", "N2", "N3"] {
        let m = here(s, account, id);
        assert!(m.unfetched && m.flag == FlagStatus::None && !m.read, "{id}: {m:?}");
        assert!(m.rules_waiting.is_some(), "{id}");
    }
    fetch(
        s,
        vec![
            whole("N1", "Plan", "", "The plan."),
            whole("N2", "Plan", "", "To unsubscribe, write back."),
            whole("N3", "Plan", "Sensitivity: Private\r\n", "Between us."),
        ],
    );
    let got = |s: &Session, id: &str| {
        let m = here(s, account, id);
        assert!(!m.unfetched && m.rules_waiting.is_none(), "{id}");
        (m.flag, m.read)
    };
    assert_eq!(got(s, "N1"), (FlagStatus::Flagged, false));
    assert_eq!(got(s, "N2"), (FlagStatus::None, false));
    assert_eq!(got(s, "N3"), (FlagStatus::Flagged, true));
}

#[test]
fn a_microsoft_accounts_new_mail_has_the_rules_that_read_its_text_once_it_is_fetched() {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": ME, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    // Its first round has ended: what comes now arrives.
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), caught_up: true, ..Default::default() });
    rules(&mut s);
    let page = MessagePage {
        folder: "F-in".into(),
        messages: ["N1", "N2", "N3"].iter().map(|id| listed(id, "F-in", "Plan")).collect(),
        ..Default::default()
    };
    graph_mail::apply_graph_mail(&mut s, GraphMailResult { account, email: ME.into(), pages: vec![page], partial: true, ..Default::default() })
        .unwrap();
    check(&mut s, account, |s, fetched| {
        graph_mail::apply_graph_fetch(s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    });
}

#[test]
fn a_jmap_fetch_gives_the_rules_waiting_on_a_listed_message_their_say() {
    let mut s = Session::empty();
    let jmap = json!({"email": ME, "kind": "jmap", "jmap": {"session": "https://jmap.example.org/jmap/session", "bearer": true}});
    let account = s.run("account.add", &jmap).unwrap()["id"].as_u64().unwrap();
    rules(&mut s);
    let folders = vec![GraphFolder { id: "M1".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let piece = || JmapMailResult { account, email: ME.into(), partial: true, ..Default::default() };
    let run = |s: &mut Session, messages: Vec<GraphMessage>, afresh: bool| {
        let listing = Some(FolderListing { folders: folders.clone(), complete: true });
        jmap_mail::apply_jmap_mail(s, JmapMailResult { listing, ..piece() }).unwrap();
        let window = MessagePage { folder: "M1".into(), messages, afresh, ..Default::default() };
        jmap_mail::apply_jmap_mail(s, JmapMailResult { pages: vec![window], ..piece() }).unwrap();
        jmap_mail::apply_jmap_mail(s, JmapMailResult { whole: true, partial: false, ..piece() }).unwrap();
    };
    run(&mut s, vec![], true);
    run(&mut s, ["N1", "N2", "N3"].iter().map(|id| listed(id, "M1", "Plan")).collect(), false);
    // A JMAP account's runs don't put what they bring through the rules yet (nor did they before);
    // the rules run here on what was listed, as they do on a Microsoft account's new mail, and a
    // JMAP fetch then gives those that couldn't tell their say.
    let clock = s.rule_clock();
    for id in ["N1", "N2", "N3"] {
        let m = here(&s, account, id).id;
        s.mailbox.apply_rules(m, &clock);
    }
    check(&mut s, account, |s, fetched| {
        jmap_mail::apply_jmap_fetch(s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    });
}

#[test]
fn an_exchange_server_accounts_new_mail_has_the_rules_that_read_its_text_once_it_is_fetched() {
    let mut s = Session::empty();
    let url = "https://exchange.example.org/EWS/Exchange.asmx";
    let account = s.run("account.add", &json!({"email": ME, "kind": "exchange", "ews": {"url": url}})).unwrap()["id"].as_u64().unwrap();
    rules(&mut s);
    let piece = || EwsMailResult { account, email: ME.into(), url: url.into(), partial: true, ..Default::default() };
    let inbox = vec![GraphFolder { id: "F1".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let deltas = EwsFolderDeltas { changed: inbox, afresh: true, ..Default::default() };
    ews_mail::apply_ews_mail(&mut s, EwsMailResult { folders: Some(deltas), hierarchy: "HIER-1".into(), ..piece() }).unwrap();
    // The first read of the Inbox, and the run's end: mail from now on arrives.
    let first = EwsMessagePage { folder: "F1".into(), afresh: true, last: true, state: "S-1".into(), ..Default::default() };
    ews_mail::apply_ews_mail(&mut s, EwsMailResult { pages: vec![first], ..piece() }).unwrap();
    ews_mail::apply_ews_mail(&mut s, EwsMailResult { whole: true, partial: false, ..piece() }).unwrap();
    let delta = EwsMessagePage {
        folder: "F1".into(),
        messages: ["N1", "N2", "N3"].iter().map(|id| listed(id, "F1", "Plan")).collect(),
        last: true,
        from: "S-1".into(),
        state: "S-2".into(),
        ..Default::default()
    };
    ews_mail::apply_ews_mail(&mut s, EwsMailResult { pages: vec![delta], ..piece() }).unwrap();
    check(&mut s, account, |s, fetched| {
        ews_mail::apply_ews_fetch(s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    });
}

/// New mail older than the account's mail to keep offline (moved back into the Inbox from an
/// archive on another device, say): the background doesn't fetch it, so a rule that reads the
/// text acts on it once the owner opens it, while it is open (#966 gave Exchange server accounts
/// the window a Microsoft account has).
#[test]
fn an_exchange_rule_reading_text_waits_on_new_mail_older_than_mail_to_keep_offline_until_it_is_opened() {
    // 2026-10-15.
    const NOW: i64 = 1_792_000_000;
    const DAY: i64 = 86_400;
    let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0);
    let url = "https://exchange.example.org/EWS/Exchange.asmx";
    let account = s.run("account.add", &json!({"email": ME, "kind": "exchange", "ews": {"url": url}})).unwrap()["id"].as_u64().unwrap();
    let rule =
        json!({"name": "Invoices", "server": false, "conditions": [{"type": "bodyContains", "value": "invoice"}], "actions": [{"type": "markRead"}]});
    s.run("rules.add", &rule).unwrap();
    let piece = || EwsMailResult { account, email: ME.into(), url: url.into(), partial: true, ..Default::default() };
    let inbox = vec![GraphFolder { id: "F1".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let deltas = EwsFolderDeltas { changed: inbox, afresh: true, ..Default::default() };
    ews_mail::apply_ews_mail(&mut s, EwsMailResult { folders: Some(deltas), hierarchy: "HIER-1".into(), ..piece() }).unwrap();
    let first = EwsMessagePage { folder: "F1".into(), afresh: true, last: true, state: "S-1".into(), ..Default::default() };
    ews_mail::apply_ews_mail(&mut s, EwsMailResult { pages: vec![first], ..piece() }).unwrap();
    ews_mail::apply_ews_mail(&mut s, EwsMailResult { whole: true, partial: false, ..piece() }).unwrap();
    let at = |id: &str, received: i64| GraphMessage { sent: received - 60, received, ..listed(id, "F1", "Plan") };
    let delta = EwsMessagePage {
        folder: "F1".into(),
        messages: vec![at("NEW", NOW - DAY), at("OLD", NOW - 400 * DAY)],
        last: true,
        from: "S-1".into(),
        state: "S-2".into(),
        ..Default::default()
    };
    ews_mail::apply_ews_mail(&mut s, EwsMailResult { pages: vec![delta], ..piece() }).unwrap();
    assert!(here(&s, account, "NEW").rules_waiting.is_some() && here(&s, account, "OLD").rules_waiting.is_some());
    assert_eq!(ews_mail::ews_fetch_request(&s, account, None, 100).unwrap().ids, ["NEW"], "the old one isn't fetched in the background");
    let old = here(&s, account, "OLD").id;
    assert_eq!(ews_mail::ews_fetch_request(&s, account, Some(old), 1).unwrap().ids, ["OLD"], "opened, it is");
    let fetched = vec![whole("OLD", "Plan", "", "The invoice.")];
    ews_mail::apply_ews_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    let m = here(&s, account, "OLD");
    assert!(m.read && m.rules_waiting.is_none());
}

#[test]
fn a_fetch_names_the_messages_a_waiting_rule_shows_a_desktop_alert_for() {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": ME, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), caught_up: true, ..Default::default() });
    // An alert and nothing else, for mail whose text says "invoice"; and one for the subject,
    // which tells as the mail arrives.
    let text =
        json!({"name": "Invoices", "server": false, "conditions": [{"type": "bodyContains", "value": "invoice"}], "actions": [], "alert": true});
    s.run("rules.add", &text).unwrap();
    let subject =
        json!({"name": "Urgent", "server": false, "conditions": [{"type": "subjectContains", "value": "urgent"}], "actions": [], "alert": true});
    s.run("rules.add", &subject).unwrap();
    let messages = vec![listed("N1", "F-in", "Plan"), listed("N2", "F-in", "Plan"), listed("N3", "F-in", "Urgent")];
    let page = MessagePage { folder: "F-in".into(), messages, ..Default::default() };
    graph_mail::apply_graph_mail(&mut s, GraphMailResult { account, email: ME.into(), pages: vec![page], partial: true, ..Default::default() })
        .unwrap();
    assert!(!here(&s, account, "N1").rule_alert);
    assert!(here(&s, account, "N3").rule_alert, "told of as it arrived");
    let fetched = vec![
        whole("N1", "Plan", "", "The invoice for May."),
        whole("N2", "Plan", "", "Nothing to pay."),
        whole("N3", "Urgent", "", "The invoice, urgently."),
    ];
    let said = graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    // The one that arrived told of already isn't named again.
    assert_eq!(said["alerts"], json!([here(&s, account, "N1").id]));
    assert!(here(&s, account, "N1").rule_alert && !here(&s, account, "N2").rule_alert);
    // Fetched again, nothing more.
    let again = vec![whole("N1", "Plan", "", "The invoice for May.")];
    let said = graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched: again, ..Default::default() }).unwrap();
    assert_eq!(said["alerts"], json!([]));
}

/// A Microsoft account with Inbox and a folder `name` on its server; their ids here.
fn microsoft_with(s: &mut Session, name: &str) -> (Id, Id, Id) {
    let account = s.run("account.add", &json!({"email": ME, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), caught_up: true, ..Default::default() });
    let kept = s.mailbox.add_folder(account, None, name).unwrap();
    s.mailbox.folder_mut(kept).unwrap().server = Some(ServerFolder { id: "F-kept".into(), name: name.into(), ..Default::default() });
    (account, inbox, kept)
}

/// Whether a message `subject` of `account` lies in its Deleted Items, where a rule's Delete puts it.
fn binned(s: &Session, account: Id, subject: &str) -> bool {
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    s.mailbox.messages.values().any(|m| m.subject == subject && m.folder == deleted)
}

fn arrives(s: &mut Session, account: Id, subject: &str) {
    let page = MessagePage { folder: "F-in".into(), messages: vec![listed("N1", "F-in", subject)], ..Default::default() };
    graph_mail::apply_graph_mail(s, GraphMailResult { account, email: ME.into(), pages: vec![page], partial: true, ..Default::default() }).unwrap();
}

/// An IMAP account beside the one under test, read once: its Inbox, which a rule of the other
/// account copies Zoe's mail into, and the account.
fn imap_inbox(s: &mut Session) -> (Id, Id) {
    use mailcraft_model::sync::{FolderDelta, SyncResult};
    let servers = json!({"email": "blake@other.example", "incoming": {"host": "imap.other.example"}, "outgoing": {"host": "smtp.other.example"}});
    let b = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let delta = FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 9, ..Default::default() };
    crate::cmd::account::apply_sync(s, SyncResult { account: b, folders: vec![delta], ..Default::default() }).unwrap();
    (s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap(), b)
}

/// A rule of `account`'s, on this computer, that copies Zoe's mail into `to`.
fn copy_out(s: &mut Session, account: Id, to: Id) {
    let rule = json!({"name": "Zoe to Blake", "account": account, "server": false,
        "conditions": [{"type": "fromContains", "value": "zoe"}], "actions": [{"type": "copyTo", "value": to}]});
    s.run("rules.add", &rule).unwrap();
}

/// The copies in `to`, and those waiting to be made on the server of `b`.
fn copies(s: &Session, to: Id, b: Id) -> (usize, usize) {
    let made = s.mailbox.messages.values().filter(|m| m.folder == to).count();
    let placed = s.mailbox.queued.iter().filter(|q| matches!(q, mailcraft_model::Queued::PlaceMessage { account, .. } if *account == b)).count();
    (made, placed)
}

/// New mail is listed before it is fetched, and a copy in another mailbox is made of the whole
/// message: a rule on this computer that copies it into another account's folder waits for the
/// download and copies it then, as Outlook's rule copies new mail into another account while it
/// is open (#566). For a Microsoft account, an Exchange server account and a JMAP account's fetch.
fn copies_once_fetched(s: &mut Session, account: Id, to: Id, b: Id, fetch: impl Fn(&mut Session, Vec<GraphFetched>)) {
    assert_eq!(copies(s, to, b), (0, 0), "nothing is copied of mail not here yet");
    assert!(here(s, account, "N1").rules_waiting.is_some());
    fetch(s, vec![whole("N1", "Plan", "", "The plan.")]);
    assert!(!here(s, account, "N1").unfetched && here(s, account, "N1").rules_waiting.is_none());
    assert_eq!(copies(s, to, b), (1, 1), "copied once it is here whole");
    // The original stays where it came.
    assert_eq!(s.mailbox.folder(here(s, account, "N1").folder).map(|f| f.kind), Some(FolderKind::Inbox));
}

#[test]
fn a_microsoft_rule_copying_new_mail_into_another_account_copies_it_once_it_is_fetched() {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": ME, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), caught_up: true, ..Default::default() });
    let (to, b) = imap_inbox(&mut s);
    copy_out(&mut s, account, to);
    arrives(&mut s, account, "Plan");
    copies_once_fetched(&mut s, account, to, b, |s, fetched| {
        graph_mail::apply_graph_fetch(s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    });
}

#[test]
fn an_exchange_rule_copying_new_mail_into_another_account_copies_it_once_it_is_fetched() {
    let mut s = Session::empty();
    let url = "https://exchange.example.org/EWS/Exchange.asmx";
    let account = s.run("account.add", &json!({"email": ME, "kind": "exchange", "ews": {"url": url}})).unwrap()["id"].as_u64().unwrap();
    let (to, b) = imap_inbox(&mut s);
    copy_out(&mut s, account, to);
    let piece = || EwsMailResult { account, email: ME.into(), url: url.into(), partial: true, ..Default::default() };
    let inbox = vec![GraphFolder { id: "F1".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let deltas = EwsFolderDeltas { changed: inbox, afresh: true, ..Default::default() };
    ews_mail::apply_ews_mail(&mut s, EwsMailResult { folders: Some(deltas), hierarchy: "HIER-1".into(), ..piece() }).unwrap();
    let first = EwsMessagePage { folder: "F1".into(), afresh: true, last: true, state: "S-1".into(), ..Default::default() };
    ews_mail::apply_ews_mail(&mut s, EwsMailResult { pages: vec![first], ..piece() }).unwrap();
    ews_mail::apply_ews_mail(&mut s, EwsMailResult { whole: true, partial: false, ..piece() }).unwrap();
    let delta = EwsMessagePage {
        folder: "F1".into(),
        messages: vec![listed("N1", "F1", "Plan")],
        last: true,
        from: "S-1".into(),
        state: "S-2".into(),
        ..Default::default()
    };
    ews_mail::apply_ews_mail(&mut s, EwsMailResult { pages: vec![delta], ..piece() }).unwrap();
    copies_once_fetched(&mut s, account, to, b, |s, fetched| {
        ews_mail::apply_ews_fetch(s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    });
}

/// A JMAP account's runs don't put new mail through the rules as it arrives yet (as on main); run
/// on what was listed, as they are on a Microsoft account's new mail, a copy into another account
/// waits for the JMAP fetch and is made then.
#[test]
fn a_jmap_rule_copying_listed_mail_into_another_account_copies_it_once_it_is_fetched() {
    let mut s = Session::empty();
    let jmap = json!({"email": ME, "kind": "jmap", "jmap": {"session": "https://jmap.example.org/jmap/session", "bearer": true}});
    let account = s.run("account.add", &jmap).unwrap()["id"].as_u64().unwrap();
    let (to, b) = imap_inbox(&mut s);
    copy_out(&mut s, account, to);
    let folders = vec![GraphFolder { id: "M1".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let piece = || JmapMailResult { account, email: ME.into(), partial: true, ..Default::default() };
    let run = |s: &mut Session, messages: Vec<GraphMessage>, afresh: bool| {
        let listing = Some(FolderListing { folders: folders.clone(), complete: true });
        jmap_mail::apply_jmap_mail(s, JmapMailResult { listing, ..piece() }).unwrap();
        let window = MessagePage { folder: "M1".into(), messages, afresh, ..Default::default() };
        jmap_mail::apply_jmap_mail(s, JmapMailResult { pages: vec![window], ..piece() }).unwrap();
        jmap_mail::apply_jmap_mail(s, JmapMailResult { whole: true, partial: false, ..piece() }).unwrap();
    };
    run(&mut s, vec![], true);
    run(&mut s, vec![listed("N1", "M1", "Plan")], false);
    let (id, clock) = (here(&s, account, "N1").id, s.rule_clock());
    s.mailbox.apply_rules(id, &clock);
    copies_once_fetched(&mut s, account, to, b, |s, fetched| {
        jmap_mail::apply_jmap_fetch(s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    });
}

/// The download never comes: the server says it has no such message, then lists it as gone (it
/// was deleted on a phone, say). Nothing was copied meanwhile, nothing waits in the queue for it:
/// the message goes as the server says, and that is all.
#[test]
fn a_copy_into_another_account_that_waits_for_a_download_that_never_comes_leaves_nothing_waiting() {
    use mailcraft_model::Queued;
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": ME, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), caught_up: true, ..Default::default() });
    let (to, b) = imap_inbox(&mut s);
    copy_out(&mut s, account, to);
    arrives(&mut s, account, "Plan");
    let id = here(&s, account, "N1").id;
    let about = |s: &Session| {
        s.mailbox.queued.iter().any(|q| match q {
            Queued::PlaceMessage { message, .. } => *message == id,
            Queued::DeleteMessage { account: a, .. } => *a == account,
            _ => false,
        })
    };
    // The copy waits: nothing goes before the download.
    assert_eq!(here(&s, account, "N1").rules_waiting.as_ref().map(|w| w.rules.len()), Some(1));
    assert!(!about(&s) && super::graph_up::message_changes(&s, account, 100).0.is_empty());
    assert_eq!(copies(&s, to, b), (0, 0));
    // The fetch says the server has no such message.
    let missing = GraphFetchResult { account, email: ME.into(), missing: vec!["N1".into()], ..Default::default() };
    graph_mail::apply_graph_fetch(&mut s, missing).unwrap();
    assert!(!about(&s) && super::graph_up::message_changes(&s, account, 100).0.is_empty());
    // Then its listing drops it.
    let page = MessagePage { folder: "F-in".into(), removed: vec!["N1".into()], link: "L2".into(), caught_up: true, ..Default::default() };
    graph_mail::apply_graph_mail(&mut s, GraphMailResult { account, email: ME.into(), pages: vec![page], whole: true, ..Default::default() })
        .unwrap();
    assert!(s.mailbox.message(id).is_none());
    assert!(!about(&s) && super::graph_up::message_changes(&s, account, 100).0.is_empty());
    assert_eq!(copies(&s, to, b), (0, 0));
    assert!(s.mailbox.messages.values().all(|m| m.rules_waiting.is_none()));
}

/// A rule deletes new mail, except if a header holds a word or the sender marked it
/// private. A message whose header fields run past what is kept of them (a mailing list's long
/// trace, say) can't be told by the exception: the rule leaves it alone, though the one with the
/// header near the top is told by it as ever.
#[test]
fn an_exception_in_a_header_past_what_is_kept_doesnt_let_the_rule_delete() {
    let mut s = Session::empty();
    let (account, _, _) = microsoft_with(&mut s, "Kept");
    let gone = json!({"name": "Plans", "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
        "exceptions": [{"type": "headerContains", "value": "x-keep: yes"}, {"type": "sensitivity", "value": "private"}],
        "actions": [{"type": "delete"}]});
    s.run("rules.add", &gone).unwrap();
    let page = MessagePage {
        folder: "F-in".into(),
        messages: ["N1", "N2", "N3"].iter().map(|id| listed(id, "F-in", "Plan")).collect(),
        ..Default::default()
    };
    graph_mail::apply_graph_mail(&mut s, GraphMailResult { account, email: ME.into(), pages: vec![page], partial: true, ..Default::default() })
        .unwrap();
    let padding: String = (0..200).map(|i| format!("X-Trace-{i}: hop {i}\r\n")).collect();
    let fetched = vec![
        whole("N1", "Plan", "X-Keep: yes\r\n", "Its header is near the top."),
        whole("N2", "Plan", &format!("{padding}X-Keep: yes\r\n"), "The same header, after the trace."),
        whole("N3", "Plan", &format!("{padding}Sensitivity: Private\r\n"), "Private, after the trace."),
    ];
    graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    assert!(here(&s, account, "N2").headers_cut && !here(&s, account, "N1").headers_cut);
    assert!(!binned(&s, account, "Plan"), "deleted past an exception it couldn't tell, or one that holds");
}

/// A Microsoft delta gives no size (`graph/mail/down.rs`: size 0, "not
/// said"), unlike [`listed`]'s 1,200. A size rule on such new mail waits for the download, and
/// then acts.
#[test]
fn a_size_rule_on_microsoft_new_mail_listed_without_a_size_waits_for_the_download() {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": ME, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), caught_up: true, ..Default::default() });
    let small = json!({"name": "Small", "account": account, "server": false,
        "conditions": [{"type": "sizeBetween", "value": {"min": 0, "max": 64}}], "actions": [{"type": "flag"}]});
    s.run("rules.add", &small).unwrap();
    let mut as_listed = listed("N1", "F-in", "Plan");
    as_listed.server.size = 0;
    let page = MessagePage { folder: "F-in".into(), messages: vec![as_listed], ..Default::default() };
    graph_mail::apply_graph_mail(&mut s, GraphMailResult { account, email: ME.into(), pages: vec![page], partial: true, ..Default::default() })
        .unwrap();
    let m = here(&s, account, "N1");
    assert!(m.unfetched && m.flag == FlagStatus::None && m.rules_waiting.is_some(), "{m:?}");
    let fetched = vec![whole("N1", "Plan", "", "The plan.")];
    graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    let m = here(&s, account, "N1");
    assert!(!m.unfetched && m.rules_waiting.is_none() && m.flag == FlagStatus::Flagged, "{m:?}");
}

/// The owner files new mail away before it is downloaded, which lets the rule waiting on it
/// alone. An agent then moves it back into the Inbox (a move within the account, asked about
/// nothing), or undoes the owner's move: the rule still lets it alone once it is downloaded,
/// rather than deleting it.
#[test]
fn mail_moved_back_after_its_owner_filed_it_isnt_deleted_by_a_rule_that_waited() {
    use crate::agent::{Verdict, check};
    let mut lost = Vec::new();
    for agent in [None, Some("mail.move"), Some("edit.undo")] {
        let mut s = Session::empty();
        let (account, inbox, kept) = microsoft_with(&mut s, "Kept");
        let gone = json!({"name": "Invoices", "server": false,
            "conditions": [{"type": "bodyContains", "value": "invoice"}], "actions": [{"type": "delete"}]});
        s.run("rules.add", &gone).unwrap();
        arrives(&mut s, account, "Plan");
        let m = here(&s, account, "N1").id;
        assert!(s.mailbox.message(m).unwrap().rules_waiting.is_some());
        // The owner files it away before it is downloaded.
        s.run("mail.move", &json!({"ids": [m], "folder": kept})).unwrap();
        assert_eq!(s.mailbox.message(m).unwrap().folder, kept);
        // The agent moves it back, or undoes the owner's move: asked about nothing.
        if let Some(id) = agent {
            let params = if id == "edit.undo" { json!({}) } else { json!({"ids": [m], "folder": inbox}) };
            assert_eq!(check(&s, id, &params, &[]), Verdict::Run, "{id} isn't asked about");
            s.agent = Some(Vec::new());
            s.run(id, &params).unwrap();
            s.agent = None;
            let back = s.mailbox.message(m).unwrap();
            assert!(back.folder == inbox && back.unfetched, "{id}: {back:?}");
        }
        let fetched = vec![whole("N1", "Plan", "", "The invoice for May.")];
        graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
        let after = s.mailbox.message(m);
        let deleted = binned(&s, account, "Plan");
        if !(after.is_some_and(|x| !x.unfetched) && !deleted) {
            lost.push(format!("{agent:?}: here {}, in Deleted Items {deleted}", after.is_some()));
        }
    }
    assert!(lost.is_empty(), "the message the owner filed away is deleted after: {lost:?}");
}

/// The same filing away done on another device (a phone) on a Microsoft account, whose ids don't
/// change with a move: a run sees the message in Kept, a later one back in the Inbox, and the rule
/// that waited lets it alone once it is downloaded. Downloaded while it is in Kept, it is let
/// alone too.
#[test]
fn mail_filed_away_and_back_on_a_phone_isnt_deleted_by_a_rule_that_waited() {
    for back in [true, false] {
        let mut s = Session::empty();
        let (account, inbox, kept) = microsoft_with(&mut s, "Kept");
        let gone = json!({"name": "Invoices", "server": false,
            "conditions": [{"type": "bodyContains", "value": "invoice"}], "actions": [{"type": "delete"}]});
        s.run("rules.add", &gone).unwrap();
        arrives(&mut s, account, "Plan");
        let m = here(&s, account, "N1").id;
        assert!(s.mailbox.message(m).unwrap().rules_waiting.is_some());
        let run = |s: &mut Session, from: &str, to: &str| {
            let left = MessagePage { folder: from.into(), removed: vec!["N1".into()], ..Default::default() };
            let there = MessagePage { folder: to.into(), messages: vec![listed("N1", to, "Plan")], ..Default::default() };
            let pages = vec![left, there];
            graph_mail::apply_graph_mail(s, GraphMailResult { account, email: ME.into(), pages, partial: true, ..Default::default() }).unwrap();
        };
        run(&mut s, "F-in", "F-kept");
        assert_eq!(s.mailbox.message(m).unwrap().folder, kept, "filed away");
        if back {
            run(&mut s, "F-kept", "F-in");
            let back = s.mailbox.message(m).unwrap();
            assert!(back.folder == inbox && back.unfetched, "{back:?}");
        }
        let fetched = vec![whole("N1", "Plan", "", "The invoice for May.")];
        graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
        assert!(s.mailbox.message(m).is_some_and(|x| !x.unfetched) && !binned(&s, account, "Plan"), "moved back: {back}");
    }
}

/// An agent's edit of mail waiting for its rules: the last rule copies "Plan" into another
/// account's folder. The rule "Keep", which stops the others,
/// would have kept the message off the other server; the agent's unasked Clear All Categories
/// makes it not apply once the text is here. With the marks changed meanwhile, the rules that
/// waited copy and move nothing onto another account's server, with or without the agent's edit
/// (an agent's own copy there asks `send`).
#[test]
fn an_agents_edit_of_mail_waiting_for_its_rules_doesnt_let_a_later_rule_copy_it_onto_another_server() {
    use crate::agent::{Verdict, check};
    let mut outcome = Vec::new();
    for agent in [false, true] {
        let mut s = Session::empty();
        let (account, _, _) = microsoft_with(&mut s, "Kept");
        let (to, b) = imap_inbox(&mut s);
        let boss = json!({"name": "Boss", "account": account, "server": false, "conditions": [{"type": "fromContains", "value": "zoe"}],
            "actions": [{"type": "categorize", "value": "Boss"}]});
        s.run("rules.add", &boss).unwrap();
        let keep = json!({"name": "Keep", "account": account, "server": false, "stop": true,
            "conditions": [{"type": "category", "value": "Boss"}, {"type": "bodyContains", "value": "keep"}], "actions": [{"type": "flag"}]});
        s.run("rules.add", &keep).unwrap();
        let plans = json!({"name": "Plans to Blake", "account": account, "server": false,
            "conditions": [{"type": "subjectContains", "value": "Plan"}], "actions": [{"type": "copyTo", "value": to}]});
        s.run("rules.add", &plans).unwrap();
        arrives(&mut s, account, "Plan");
        let m = here(&s, account, "N1").id;
        assert!(s.mailbox.message(m).is_some_and(|x| x.rules_waiting.is_some() && x.has_category("Boss")));
        if agent {
            let params = json!({"ids": [m]});
            assert_eq!(check(&s, "mail.clearCategories", &params, &[]), Verdict::Run, "asked about nothing");
            s.agent = Some(Vec::new());
            s.run("mail.clearCategories", &params).unwrap();
            s.agent = None;
        }
        let fetched = vec![whole("N1", "Plan", "", "Please keep this.")];
        graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
        outcome.push(format!("agent {agent}: copies here and on their way to the other server {:?}", copies(&s, to, b)));
    }
    assert_eq!(
        outcome,
        [
            "agent false: copies here and on their way to the other server (0, 0)",
            "agent true: copies here and on their way to the other server (0, 0)"
        ],
        "an agent's unasked Clear All Categories made a rule copy the message onto another account's server"
    );
}

/// The owner's first rule keeps Private mail where it is ("sensitivity is private: flag it, stop
/// processing more rules"); the second files Zoe's mail into an IMAP account's Inbox. New mail
/// on a Microsoft account is listed before it is fetched, so the first rule can't tell yet: a
/// move into the other account waits for the download and for that rule's say, as a copy does,
/// and doesn't go onto the other server on a guess (#566).
#[test]
fn a_move_onto_another_server_waits_for_an_earlier_stop_rule_that_cant_tell_yet() {
    for action in ["copyTo", "moveTo"] {
        let mut s = Session::empty();
        let (account, _, _) = microsoft_with(&mut s, "Kept");
        let (to, b) = imap_inbox(&mut s);
        let private = json!({"name": "Private stays", "account": account, "server": false, "stop": true,
            "conditions": [{"type": "sensitivity", "value": "private"}], "actions": [{"type": "flag"}]});
        s.run("rules.add", &private).unwrap();
        let zoe = json!({"name": "Zoe to Blake", "account": account, "server": false,
            "conditions": [{"type": "fromContains", "value": "zoe"}], "actions": [{"type": action, "value": to}]});
        s.run("rules.add", &zoe).unwrap();
        arrives(&mut s, account, "Plan");
        let here = s.mailbox.messages.values().find(|m| m.subject == "Plan" && m.folder != to).map(|m| m.account == account);
        let waits = s.mailbox.messages.values().any(|m| m.subject == "Plan" && m.rules_waiting.is_some());
        assert_eq!((here, waits, copies(&s, to, b)), (Some(true), true, (0, 0)), "{action}: gone onto the other server on a guess");
    }
}

/// The same two rules run by Run Rules Now over a Microsoft Inbox whose mail isn't fetched yet
/// (older than Mail to keep offline): the first can't tell, so the second doesn't move the mail
/// onto the other account's server (#566).
#[test]
fn run_rules_now_doesnt_move_onto_another_server_past_a_stop_rule_that_cant_tell() {
    let mut s = Session::empty();
    let (account, inbox, _) = microsoft_with(&mut s, "Kept");
    let (to, b) = imap_inbox(&mut s);
    arrives(&mut s, account, "Plan");
    let private = json!({"name": "Private stays", "account": account, "server": false, "stop": true,
        "conditions": [{"type": "sensitivity", "value": "private"}], "actions": [{"type": "flag"}]});
    s.run("rules.add", &private).unwrap();
    let zoe = json!({"name": "Zoe to Blake", "account": account, "server": false,
        "conditions": [{"type": "fromContains", "value": "zoe"}], "actions": [{"type": "moveTo", "value": to}]});
    s.run("rules.add", &zoe).unwrap();
    assert!(s.mailbox.messages.values().any(|m| m.subject == "Plan" && m.unfetched && m.folder == inbox));
    s.run("rules.run", &json!({"folder": inbox})).unwrap();
    assert_eq!(copies(&s, to, b), (0, 0), "moved onto another server past a stop rule that couldn't tell");
}

/// An agent asks to Run Rules Now over the Inbox while a message there has only its preview: "To
/// Blake" (move it into another account's Inbox, except if the text says "keep") can't tell, so it
/// leaves the message alone. While the question is up the message is downloaded (the agent opened
/// it, say): the text doesn't say "keep". Every condition and exception holds of it as before; only
/// whether the exception can be told changed. The yes runs nothing (#566).
#[test]
fn a_download_while_run_rules_now_is_asked_doesnt_ride_the_yes() {
    use crate::agent::bind::Shown;
    use crate::agent::{Class, Verdict, check};
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": ME, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), caught_up: true, ..Default::default() });
    let page = MessagePage { folder: "F-in".into(), messages: vec![listed("N1", "F-in", "Plan")], ..Default::default() };
    graph_mail::apply_graph_mail(&mut s, GraphMailResult { account, email: ME.into(), pages: vec![page], partial: true, ..Default::default() })
        .unwrap();
    let (to, _) = imap_inbox(&mut s);
    let away = json!({"name": "To Blake", "account": account, "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
        "exceptions": [{"type": "bodyContains", "value": "keep"}], "actions": [{"type": "moveTo", "value": to}]});
    s.run("rules.add", &away).unwrap();
    let m = here(&s, account, "N1").id;
    assert!(s.mailbox.message(m).unwrap().unfetched);
    let params = json!({"folder": inbox});
    let Verdict::Ask(Class::Send, what) = check(&s, "rules.run", &params, &[]) else { panic!("asked as a send") };
    let shown = Shown::new(&s, "rules.run", &params, &what).unwrap();
    let fetched = vec![whole("N1", "Plan", "", "The plan.")];
    graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    assert!(s.mailbox.message(m).is_some_and(|m| !m.unfetched), "downloaded, and still here");
    let reach = shown.check(&s, "rules.run", &params);
    // What the yes would do now: "To Blake" takes it.
    s.run("rules.run", &params).unwrap();
    assert!(s.mailbox.messages.values().any(|m| m.subject == "Plan" && m.folder == to), "Run Rules Now now moves it onto Blake's server");
    assert!(reach.is_err(), "the yes runs, and puts on another server a message the rule left alone when asked: {what}");
}

/// The same two rules, and the message is private, but its Sensitivity line comes after 200
/// trace lines (a mailing list's, say), past what is kept of a header: once it is downloaded,
/// "Private stays" still can't tell, and the move or copy into the other account stays held
/// behind it, as at Run Rules Now (#566).
#[test]
fn a_move_onto_another_server_stays_held_after_the_download_when_the_stop_rule_still_cant_tell() {
    let mut outcome = Vec::new();
    for action in ["copyTo", "moveTo"] {
        let mut s = Session::empty();
        let (account, _, _) = microsoft_with(&mut s, "Kept");
        let (to, b) = imap_inbox(&mut s);
        let private = json!({"name": "Private stays", "account": account, "server": false, "stop": true,
            "conditions": [{"type": "sensitivity", "value": "private"}], "actions": [{"type": "flag"}]});
        s.run("rules.add", &private).unwrap();
        let zoe = json!({"name": "Zoe to Blake", "account": account, "server": false,
            "conditions": [{"type": "fromContains", "value": "zoe"}], "actions": [{"type": action, "value": to}]});
        s.run("rules.add", &zoe).unwrap();
        arrives(&mut s, account, "Plan");
        assert_eq!(copies(&s, to, b), (0, 0), "{action}: held as it arrived");
        let padding: String = (0..200).map(|i| format!("X-Trace-{i}: hop {i}\r\n")).collect();
        let fetched = vec![whole("N1", "Plan", &format!("{padding}Sensitivity: Private\r\n"), "Between us.")];
        graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
        let cut = s.mailbox.messages.values().any(|m| m.subject == "Plan" && m.headers_cut);
        let in_b = s.mailbox.messages.values().filter(|m| m.subject == "Plan" && m.account == b).count();
        outcome.push(format!("{action}: headers cut {cut}, in the other account {in_b}, (made, placed) {:?}", copies(&s, to, b)));
    }
    assert_eq!(
        outcome,
        [
            "copyTo: headers cut true, in the other account 0, (made, placed) (0, 0)",
            "moveTo: headers cut true, in the other account 0, (made, placed) (0, 0)"
        ],
        "a private message went onto another account's server past a stop rule that still can't tell"
    );
}

/// The move that waited for a rule before it that couldn't tell is made once the message is here
/// and that rule doesn't apply (no Sensitivity line): held, not dropped (#566).
#[test]
fn a_move_that_waited_past_a_rule_that_couldnt_tell_is_made_once_downloaded() {
    let mut s = Session::empty();
    let (account, _, _) = microsoft_with(&mut s, "Kept");
    let (to, _) = imap_inbox(&mut s);
    let private = json!({"name": "Private stays", "account": account, "server": false, "stop": true,
        "conditions": [{"type": "sensitivity", "value": "private"}], "actions": [{"type": "flag"}]});
    s.run("rules.add", &private).unwrap();
    let zoe = json!({"name": "Zoe to Blake", "account": account, "server": false,
        "conditions": [{"type": "fromContains", "value": "zoe"}], "actions": [{"type": "moveTo", "value": to}]});
    s.run("rules.add", &zoe).unwrap();
    arrives(&mut s, account, "Plan");
    assert!(!s.mailbox.messages.values().any(|m| m.folder == to), "not moved on a guess");
    let fetched = vec![whole("N1", "Plan", "", "The plan.")];
    graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    let plan: Vec<(Id, bool)> = s.mailbox.messages.values().filter(|m| m.subject == "Plan").map(|m| (m.folder, m.unfetched)).collect();
    assert_eq!(plan, [(to, false)], "moved into Blake's Inbox once the first rule had its say");
}

/// Run Rules Now over an IMAP Inbox holding mail downloaded before its header lines were kept
/// (0.12.0): a stop rule on a header can't tell, so a later rule copies nothing into another
/// account (#566).
#[test]
fn run_rules_now_copies_nothing_into_another_account_past_a_header_rule_that_cant_tell() {
    use mailcraft_model::sync::{FolderDelta, SyncResult};
    let mut s = Session::empty();
    let servers = json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "imap.mail.example"}});
    let a = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let delta = FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 9, complete: true, ..Default::default() };
    crate::cmd::account::apply_sync(&mut s, SyncResult { account: a, folders: vec![delta], ..Default::default() }).unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let (theirs, _) = imap_inbox(&mut s);
    let m = Message {
        folder: inbox,
        uid: 5,
        from: Address::new("Zoe", "zoe@example.org"),
        subject: "Plan".into(),
        message_id: "plan@example.org".into(),
        ..Default::default()
    };
    let id = s.mailbox.add_message(m).unwrap();
    assert!(s.mailbox.message(id).unwrap().read_header.is_empty());
    let private = json!({"name": "Private stays", "account": a, "stop": true,
        "conditions": [{"type": "headerContains", "value": "x-private"}], "actions": [{"type": "flag"}]});
    s.run("rules.add", &private).unwrap();
    let zoe = json!({"name": "Zoe to Blake", "account": a,
        "conditions": [{"type": "fromContains", "value": "zoe"}], "actions": [{"type": "copyTo", "value": theirs}]});
    s.run("rules.add", &zoe).unwrap();
    s.run("rules.run", &json!({"folder": inbox})).unwrap();
    assert_eq!(s.mailbox.messages.values().filter(|m| m.folder == theirs).count(), 0, "copied onto Blake's server past a rule that couldn't tell");
}

/// A rule the Microsoft server has, made in Outlook with a part not known here (it forwards) and
/// "stop processing more rules", whose condition holds of the new mail: the server ran it and
/// stopped. A rule of this computer after it that copies or moves that mail into an IMAP account
/// doesn't put it onto that server past it (#566).
#[test]
fn a_server_rule_with_parts_not_known_here_that_stops_holds_a_copy_onto_another_server() {
    use mailcraft_model::remote_rules::{RemoteRule, RulesDownload};
    use mailcraft_model::{Action, Condition};
    let mut failed = Vec::new();
    for action in ["copyTo", "moveTo"] {
        let mut s = Session::empty();
        let (account, _, _) = microsoft_with(&mut s, "Kept");
        let (to, b) = imap_inbox(&mut s);
        let mut forward = RemoteRule {
            id: "r1".into(),
            name: "Forward plans to legal".into(),
            sequence: 1,
            enabled: true,
            stop: true,
            conditions: vec![Condition::SubjectContains(vec!["PLAN".into()])],
            actions: vec![Action::MarkRead],
            ..Default::default()
        };
        forward.unknown = vec!["forwardTo".into()];
        let download = RulesDownload { account, email: ME.into(), complete: true, rules: vec![forward] };
        crate::cmd::rules_remote::merge_rules(&mut s.mailbox, &download).unwrap();
        let zoe = json!({"name": "Zoe to Blake", "account": account, "server": false,
            "conditions": [{"type": "fromContains", "value": "zoe"}], "actions": [{"type": action, "value": to}]});
        s.run("rules.add", &zoe).unwrap();
        arrives(&mut s, account, "Plan");
        let fetched = vec![whole("N1", "Plan", "", "The plan.")];
        let _ = graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() });
        if copies(&s, to, b) != (0, 0) {
            failed.push(format!("{action}: {:?} (made, placed on {b}'s server)", copies(&s, to, b)));
        }
    }
    assert!(failed.is_empty(), "onto the other server past a server rule that stopped: {failed:?}");
}

/// The same server rule with every part known stops the copy too.
#[test]
fn a_known_server_rule_that_stops_holds_the_copy() {
    use mailcraft_model::remote_rules::{RemoteRule, RulesDownload};
    use mailcraft_model::{Action, Condition};
    let mut s = Session::empty();
    let (account, _, _) = microsoft_with(&mut s, "Kept");
    let (to, b) = imap_inbox(&mut s);
    let known = RemoteRule {
        id: "r1".into(),
        name: "Plans read".into(),
        sequence: 1,
        enabled: true,
        stop: true,
        conditions: vec![Condition::SubjectContains(vec!["PLAN".into()])],
        actions: vec![Action::MarkRead],
        ..Default::default()
    };
    let download = RulesDownload { account, email: ME.into(), complete: true, rules: vec![known] };
    crate::cmd::rules_remote::merge_rules(&mut s.mailbox, &download).unwrap();
    copy_out(&mut s, account, to);
    arrives(&mut s, account, "Plan");
    let fetched = vec![whole("N1", "Plan", "", "The plan.")];
    let _ = graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() });
    assert_eq!(copies(&s, to, b), (0, 0));
}

/// The first rule files Zoe's new mail into "Kept"; the next copies it into an IMAP account (or,
/// behind a rule that stops and can't tell yet, moves it there) and waits for the download. The
/// owner deletes "Kept" meanwhile, and the message goes into Deleted Items with it, the folder
/// keeping its id: once downloaded, it goes onto no other server.
#[test]
fn mail_in_a_folder_deleted_before_the_download_isnt_put_onto_another_server_by_a_rule_that_waited() {
    let mut outcome = Vec::new();
    for action in ["copyTo", "moveTo"] {
        let mut s = Session::empty();
        let (account, _, kept) = microsoft_with(&mut s, "Kept");
        let (to, b) = imap_inbox(&mut s);
        let file = json!({"name": "Zoe to Kept", "account": account, "server": false,
            "conditions": [{"type": "fromContains", "value": "zoe"}], "actions": [{"type": "moveTo", "value": kept}]});
        s.run("rules.add", &file).unwrap();
        if action == "moveTo" {
            let private = json!({"name": "Private stays", "account": account, "server": false, "stop": true,
                "conditions": [{"type": "sensitivity", "value": "private"}], "actions": [{"type": "flag"}]});
            s.run("rules.add", &private).unwrap();
        }
        let out = json!({"name": "Zoe to Blake", "account": account, "server": false,
            "conditions": [{"type": "fromContains", "value": "zoe"}], "actions": [{"type": action, "value": to}]});
        s.run("rules.add", &out).unwrap();
        arrives(&mut s, account, "Plan");
        let m = here(&s, account, "N1").id;
        let waiting = s.mailbox.message(m).map(|m| (m.folder == kept, m.rules_waiting.is_some()));
        assert_eq!(waiting, Some((true, true)), "{action}: filed into Kept, a rule waiting");
        // The owner deletes the folder, and the message with it, before it is downloaded.
        assert_eq!(s.run("folder.delete", &json!({"id": kept})).unwrap()["permanently"], false);
        let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
        assert_eq!(s.mailbox.folder(kept).and_then(|f| f.parent), Some(deleted));
        let fetched = vec![whole("N1", "Plan", "", "The plan.")];
        graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
        let in_b = s.mailbox.messages.values().filter(|m| m.subject == "Plan" && m.account == b).count();
        outcome.push(format!("{action}: in the other account {in_b}, (made, placed) {:?}", copies(&s, to, b)));
    }
    assert_eq!(
        outcome,
        ["copyTo: in the other account 0, (made, placed) (0, 0)", "moveTo: in the other account 0, (made, placed) (0, 0)"],
        "mail the owner deleted with its folder went onto another account's server"
    );
}
