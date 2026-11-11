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

/// What a server's list gives of a new message: its headers and a preview, no text.
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

fn arrives(s: &mut Session, account: Id, subject: &str) {
    let page = MessagePage { folder: "F-in".into(), messages: vec![listed("N1", "F-in", subject)], ..Default::default() };
    graph_mail::apply_graph_mail(s, GraphMailResult { account, email: ME.into(), pages: vec![page], partial: true, ..Default::default() }).unwrap();
}

/// A rule copies new mail into a folder of its own Microsoft account and deletes it for good,
/// after a rule that stops the others and can't tell yet (it reads the text). The rule waits
/// whole for the download; then its copy and its delete go together, the delete held behind the
/// copy, and when the server refuses the copy the message is still here and isn't deleted there.
#[test]
fn a_rule_that_copies_and_deletes_for_good_after_one_that_cant_tell_keeps_the_message_when_the_copy_is_refused() {
    use mailcraft_model::sync::{MessageChange, MessageChangeDone, MessageOutcome};
    let mut s = Session::empty();
    let (account, _, kept) = microsoft_with(&mut s, "Kept");
    let first = json!({"name": "Drafts", "server": false, "stop": true,
        "conditions": [{"type": "bodyContains", "value": "draft"}], "actions": [{"type": "flag"}]});
    s.run("rules.add", &first).unwrap();
    let keep = json!({"name": "Keep", "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
        "actions": [{"type": "copyTo", "value": kept}, {"type": "deleteForGood"}]});
    s.run("rules.add", &keep).unwrap();
    arrives(&mut s, account, "Plan");
    assert!(super::graph_up::message_changes(&s, account, 100).0.is_empty(), "nothing goes before the download");
    assert_eq!(here(&s, account, "N1").rules_waiting.as_ref().map(|w| w.rules.len()), Some(2));
    let fetched = vec![whole("N1", "Plan", "", "The plan.")];
    graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    let changes = super::graph_up::message_changes(&s, account, 100).0;
    let [copy @ MessageChange::Copy { .. }] = changes.as_slice() else { panic!("the copy goes first, the delete waits behind it: {changes:?}") };
    let refused = MessageChangeDone { change: copy.clone(), outcome: MessageOutcome::Refused { status: 403 }, there: None };
    graph_mail::apply_graph_mail(&mut s, GraphMailResult { account, email: ME.into(), messages: vec![refused], ..Default::default() }).unwrap();
    let left: Vec<Id> = s.mailbox.messages.values().filter(|m| m.subject == "Plan").map(|m| m.folder).collect();
    assert_eq!(left.len(), 1, "the original is here again: {left:?}");
    assert_ne!(left[0], kept);
    let after = super::graph_up::message_changes(&s, account, 100).0;
    assert!(!after.iter().any(|c| matches!(c, MessageChange::Delete { .. })), "no delete goes: {after:?}");
}

/// A rule that copies mail whose text says "invoice" can't tell yet as new mail is listed, and
/// doesn't stop the others; a rule after it deletes that mail for good by its subject. As Outlook
/// runs them, the invoice is copied and then the original deleted: here the delete waits for the
/// download, and then goes behind the copy.
#[test]
fn a_delete_for_good_waits_for_an_earlier_copy_rule_that_cant_tell_yet() {
    use mailcraft_model::sync::MessageChange;
    let mut s = Session::empty();
    let (account, _, kept) = microsoft_with(&mut s, "Invoices");
    let copy = json!({"name": "Invoices", "server": false,
        "conditions": [{"type": "bodyContains", "value": "invoice"}], "actions": [{"type": "copyTo", "value": kept}]});
    s.run("rules.add", &copy).unwrap();
    let gone = json!({"name": "Plans", "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
        "actions": [{"type": "deleteForGood"}]});
    s.run("rules.add", &gone).unwrap();
    arrives(&mut s, account, "Plan");
    assert!(super::graph_up::message_changes(&s, account, 100).0.is_empty(), "nothing is deleted before the copy rule can tell");
    let fetched = vec![whole("N1", "Plan", "", "The invoice for May.")];
    graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    let changes = super::graph_up::message_changes(&s, account, 100).0;
    assert!(matches!(changes.as_slice(), [MessageChange::Copy { .. }]), "the copy goes, the delete waits behind it: {changes:?}");
    assert!(s.mailbox.queued.iter().any(|q| matches!(q, mailcraft_model::Queued::DeleteMessage { id, .. } if id == "N1")));
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
/// was deleted on a phone, say). Nothing was copied or deleted meanwhile, nothing waits in the
/// queue for it, and a rule after the copy that deletes for good deleted nothing either: the
/// message goes as the server says, and that is all.
#[test]
fn a_copy_into_another_account_that_waits_for_a_download_that_never_comes_leaves_nothing_waiting() {
    use mailcraft_model::Queued;
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": ME, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), caught_up: true, ..Default::default() });
    let (to, b) = imap_inbox(&mut s);
    copy_out(&mut s, account, to);
    let gone = json!({"name": "Gone", "account": account, "server": false,
        "conditions": [{"type": "subjectContains", "value": "Plan"}], "actions": [{"type": "deleteForGood"}]});
    s.run("rules.add", &gone).unwrap();
    arrives(&mut s, account, "Plan");
    let id = here(&s, account, "N1").id;
    let about = |s: &Session| {
        s.mailbox.queued.iter().any(|q| match q {
            Queued::PlaceMessage { message, source, .. } => *message == id || *source == Some(id),
            Queued::DeleteMessage { account: a, .. } => *a == account,
            _ => false,
        })
    };
    // The delete for good waits with the copy: nothing is deleted before the download.
    assert_eq!(here(&s, account, "N1").rules_waiting.as_ref().map(|w| w.rules.len()), Some(2));
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

/// After a rule of this computer that copied new mail as it arrived, a later delete for good
/// doesn't wait behind a rule that can't tell yet: it deletes nothing for good, and the message
/// stays where the rules left it with its copy beside it, as the docs say (#566).
#[test]
fn a_delete_for_good_after_a_copy_made_as_mail_arrived_deletes_nothing() {
    let mut s = Session::empty();
    let (account, _, kept) = microsoft_with(&mut s, "Kept");
    let copy = json!({"name": "Copy", "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
        "actions": [{"type": "copyTo", "value": kept}]});
    s.run("rules.add", &copy).unwrap();
    let unsure =
        json!({"name": "Drafts", "server": false, "conditions": [{"type": "bodyContains", "value": "draft"}], "actions": [{"type": "flag"}]});
    s.run("rules.add", &unsure).unwrap();
    let gone = json!({"name": "Plans", "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
        "actions": [{"type": "deleteForGood"}]});
    s.run("rules.add", &gone).unwrap();
    arrives(&mut s, account, "Plan");
    let fetched = vec![whole("N1", "Plan", "", "The plan.")];
    graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    let left: Vec<bool> = s.mailbox.messages.values().filter(|m| m.subject == "Plan").map(|m| m.folder == kept).collect();
    assert_eq!(left.len(), 2, "the original and its copy: {left:?}");
    assert!(left.contains(&true) && left.contains(&false), "{left:?}");
}

/// A rule deletes new mail for good on one page of a run; a later page of the run lists it again
/// (changed on the server meanwhile): it isn't taken in again while its delete is still to go.
#[test]
fn mail_a_rule_deleted_for_good_on_one_page_isnt_taken_in_again_from_the_next() {
    let mut s = Session::empty();
    let (account, _, _) = microsoft_with(&mut s, "Kept");
    let gone = json!({"name": "Plans", "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
        "actions": [{"type": "deleteForGood"}]});
    s.run("rules.add", &gone).unwrap();
    let first = MessagePage { folder: "F-in".into(), messages: vec![listed("N1", "F-in", "Plan")], link: "L1".into(), ..Default::default() };
    let again = MessagePage { folder: "F-in".into(), messages: vec![listed("N1", "F-in", "Notes")], link: "L2".into(), ..Default::default() };
    let result = GraphMailResult { account, email: ME.into(), pages: vec![first, again], partial: true, ..Default::default() };
    graph_mail::apply_graph_mail(&mut s, result).unwrap();
    let back = s.mailbox.messages.values().any(|m| m.server.as_ref().is_some_and(|server| server.id == "N1"));
    assert!(!back, "taken in again");
    assert!(s.mailbox.queued.iter().any(|q| matches!(q, mailcraft_model::Queued::DeleteMessage { id, .. } if id == "N1")));
}

/// A rule deletes new mail for good, except if a header holds a word or the sender marked it
/// private. A message whose header fields run past what is kept of them (a mailing list's long
/// trace, say) can't be told by the exception: the rule leaves it alone, though the one with the
/// header near the top is told by it as ever.
#[test]
fn an_exception_in_a_header_past_what_is_kept_doesnt_let_the_rule_delete_for_good() {
    let mut s = Session::empty();
    let (account, _, _) = microsoft_with(&mut s, "Kept");
    let gone = json!({"name": "Plans", "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
        "exceptions": [{"type": "headerContains", "value": "x-keep: yes"}, {"type": "sensitivity", "value": "private"}],
        "actions": [{"type": "deleteForGood"}]});
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
    for id in ["N1", "N2", "N3"] {
        let _ = here(&s, account, id);
    }
    assert!(!s.mailbox.queued.iter().any(|q| matches!(q, mailcraft_model::Queued::DeleteMessage { .. })));
}
