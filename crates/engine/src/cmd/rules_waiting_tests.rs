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

/// A rule copies new Microsoft mail into a folder of the account as it arrives (the server makes
/// the copy); a later rule deletes for good mail whose text says "purge", which can't be told until
/// the download, so it waits. The next Send/Receive sends the copy before the download (a large
/// message, say) and the server refuses it. Once the message is downloaded the rule that waited
/// deletes nothing for good, as after a copy made as it arrived it never does: nothing would hold
/// the delete, and the message would be gone with no copy (#566).
#[test]
fn a_delete_for_good_that_waited_doesnt_go_after_a_copy_made_as_it_arrived() {
    use mailcraft_model::sync::{MessageChange, MessageChangeDone, MessageOutcome};
    let mut s = Session::empty();
    let (account, _, kept) = microsoft_with(&mut s, "Kept");
    let copy = json!({"name": "Copy", "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
        "actions": [{"type": "copyTo", "value": kept}]});
    s.run("rules.add", &copy).unwrap();
    let purge = json!({"name": "Purge", "server": false, "conditions": [{"type": "bodyContains", "value": "purge"}],
        "actions": [{"type": "deleteForGood"}]});
    s.run("rules.add", &purge).unwrap();
    arrives(&mut s, account, "Plan");
    assert!(here(&s, account, "N1").rules_waiting.is_some(), "Purge waits for the download");
    let changes = super::graph_up::message_changes(&s, account, 100).0;
    let [copied @ MessageChange::Copy { .. }] = changes.as_slice() else { panic!("the copy made as it arrived goes: {changes:?}") };
    // The server refuses the copy (the mailbox is full, say).
    let refused = MessageChangeDone { change: copied.clone(), outcome: MessageOutcome::Refused { status: 403 }, there: None };
    graph_mail::apply_graph_mail(&mut s, GraphMailResult { account, email: ME.into(), messages: vec![refused], ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.messages.values().filter(|m| m.subject == "Plan").count(), 1, "the copy is taken away again");
    // Downloaded later: the rule that waited has its say.
    let fetched = vec![whole("N1", "Plan", "", "Please purge this after reading.")];
    graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    let left = s.mailbox.messages.values().filter(|m| m.subject == "Plan").count();
    let deletes: Vec<MessageChange> =
        super::graph_up::message_changes(&s, account, 100).0.into_iter().filter(|c| matches!(c, MessageChange::Delete { .. })).collect();
    assert!(
        left > 0 || deletes.is_empty(),
        "after a rule that copied it as it arrived, a later rule deleted it for good: {left} left here, the server told {deletes:?}"
    );
}

/// [`a_delete_for_good_that_waited_doesnt_go_after_a_copy_made_as_it_arrived`] the other way
/// round: the download comes before the copy's answer, and a refused copy leaves the original.
#[test]
fn a_delete_for_good_that_waited_doesnt_go_when_the_copy_is_refused_after_the_download() {
    use mailcraft_model::sync::{MessageChange, MessageChangeDone, MessageOutcome};
    let mut s = Session::empty();
    let (account, _, kept) = microsoft_with(&mut s, "Kept");
    let copy = json!({"name": "Copy", "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
        "actions": [{"type": "copyTo", "value": kept}]});
    s.run("rules.add", &copy).unwrap();
    let purge = json!({"name": "Purge", "server": false, "conditions": [{"type": "bodyContains", "value": "purge"}],
        "actions": [{"type": "deleteForGood"}]});
    s.run("rules.add", &purge).unwrap();
    arrives(&mut s, account, "Plan");
    let fetched = vec![whole("N1", "Plan", "", "Please purge this after reading.")];
    graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    let changes = super::graph_up::message_changes(&s, account, 100).0;
    let [copied @ MessageChange::Copy { .. }] = changes.as_slice() else { panic!("the copy goes, and no delete: {changes:?}") };
    let refused = MessageChangeDone { change: copied.clone(), outcome: MessageOutcome::Refused { status: 403 }, there: None };
    graph_mail::apply_graph_mail(&mut s, GraphMailResult { account, email: ME.into(), messages: vec![refused], ..Default::default() }).unwrap();
    let left = s.mailbox.messages.values().filter(|m| m.subject == "Plan").count();
    let deletes: Vec<MessageChange> =
        super::graph_up::message_changes(&s, account, 100).0.into_iter().filter(|c| matches!(c, MessageChange::Delete { .. })).collect();
    assert_eq!((left, deletes), (1, vec![]), "the original stays and no delete is sent");
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
/// rather than deleting it permanently here and on the server.
#[test]
fn mail_moved_back_after_its_owner_filed_it_isnt_deleted_for_good_by_a_rule_that_waited() {
    use crate::agent::{Verdict, check};
    let mut lost = Vec::new();
    for agent in [None, Some("mail.move"), Some("edit.undo")] {
        let mut s = Session::empty();
        let (account, inbox, kept) = microsoft_with(&mut s, "Kept");
        let gone = json!({"name": "Invoices", "server": false,
            "conditions": [{"type": "bodyContains", "value": "invoice"}], "actions": [{"type": "deleteForGood"}]});
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
        let deletes = s.mailbox.queued.iter().any(|q| matches!(q, mailcraft_model::Queued::DeleteMessage { .. }));
        if !(after.is_some_and(|x| !x.unfetched) && !deletes) {
            lost.push(format!("{agent:?}: here {}, delete for the server queued {deletes}", after.is_some()));
        }
    }
    assert!(lost.is_empty(), "the message the owner filed away is deleted for good after: {lost:?}");
}

/// The same filing away done on another device (a phone) on a Microsoft account, whose ids don't
/// change with a move: a run sees the message in Kept, a later one back in the Inbox, and the rule
/// that waited lets it alone once it is downloaded. Downloaded while it is in Kept, it is let
/// alone too.
#[test]
fn mail_filed_away_and_back_on_a_phone_isnt_deleted_for_good_by_a_rule_that_waited() {
    for back in [true, false] {
        let mut s = Session::empty();
        let (account, inbox, kept) = microsoft_with(&mut s, "Kept");
        let gone = json!({"name": "Invoices", "server": false,
            "conditions": [{"type": "bodyContains", "value": "invoice"}], "actions": [{"type": "deleteForGood"}]});
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
        let deletes = s.mailbox.queued.iter().any(|q| matches!(q, mailcraft_model::Queued::DeleteMessage { .. }));
        assert!(s.mailbox.message(m).is_some_and(|x| !x.unfetched) && !deletes, "moved back: {back}");
    }
}

/// The Safe and Blocked Senders sort new mail before the rules: an agent taking a sender off the
/// Blocked Senders lets their mail reach a rule that deletes for good, as one taking a rule out of
/// its way would, and is asked as a delete, the question naming the rule; one that blocks more
/// isn't (#566).
#[test]
fn unblocking_a_sender_beside_a_rule_that_deletes_for_good_asks_as_a_delete() {
    use crate::agent::{Access, Class, classify};
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": ME, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), caught_up: true, ..Default::default() });
    let junk = s.mailbox.folder_of_kind(account, FolderKind::Junk).unwrap();
    // The owner's: Zoe is blocked, and a rule deletes her mail for good.
    let purge = json!({"name": "Purge", "account": account, "conditions": [{"type": "fromContains", "value": "zoe"}], "actions": [{"type": "deleteForGood"}]});
    s.run("rules.add", &purge).unwrap();
    s.run("junk.add", &json!({"list": "blocked", "entry": "zoe@example.org", "account": account})).unwrap();
    let arrive = |s: &mut Session, id: &str| {
        let page = MessagePage { folder: "F-in".into(), messages: vec![listed(id, "F-in", "Weekly")], ..Default::default() };
        graph_mail::apply_graph_mail(s, GraphMailResult { account, email: ME.into(), pages: vec![page], partial: true, ..Default::default() })
            .unwrap();
    };
    let lies = |s: &Session, id: &str| s.mailbox.messages.values().find(|m| m.server.as_ref().is_some_and(|x| x.id == id)).map(|m| m.folder);
    arrive(&mut s, "N1");
    assert_eq!(lies(&s, "N1"), Some(junk), "blocked: her mail waits in Junk E-mail, where it can be got back");
    // Any change to the rules for the same mail asks as a delete while Purge is on (#566)...
    let other =
        json!({"name": "Read", "account": account, "server": false, "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "markRead"}]});
    assert_eq!(classify(&s, "rules.add", &other), Access::Owner(Class::Delete));
    // ...but taking her off Blocked Senders, which lets her mail reach Purge, asks as settings.
    let unblock = json!({"list": "blocked", "entry": "zoe@example.org", "account": account});
    let class = classify(&s, "junk.remove", &unblock);
    let asked = crate::agent::describe(&s, "junk.remove", &unblock);
    s.run("junk.remove", &unblock).unwrap();
    arrive(&mut s, "N2");
    assert_eq!(lies(&s, "N2"), None, "unblocked: her next message is deleted for good as it arrives");
    assert!(s.mailbox.queued.iter().any(|q| matches!(q, mailcraft_model::Queued::DeleteMessage { id, .. } if id == "N2")), "and on the server");
    assert_eq!(class, Access::Owner(Class::Delete), "junk.remove beside a rule that deletes for good; question: {asked}");
    assert!(asked.contains("Purge") && asked.contains("deletes mail permanently"), "{asked}");
    // Putting a sender on the Safe Senders lets mail the server put in Junk E-mail through too;
    // blocking one, or taking one off the Safe Senders, lets less through.
    let safe = json!({"list": "safe", "entry": "ann@example.org", "account": account});
    assert_eq!(classify(&s, "junk.add", &safe), Access::Owner(Class::Delete));
    let block = json!({"list": "blocked", "entry": "bob@example.org", "account": account});
    assert_eq!(classify(&s, "junk.add", &block), Access::Owner(Class::Settings));
    assert_eq!(classify(&s, "junk.remove", &safe), Access::Owner(Class::Settings));
    // With the rule switched off, nothing deletes for good: a change to the lists is settings.
    let purge = s.mailbox.rules.iter().find(|r| r.name == "Purge").unwrap().id;
    s.run("rules.enable", &json!({"id": purge, "value": false})).unwrap();
    assert_eq!(classify(&s, "junk.add", &safe), Access::Owner(Class::Settings));
    // Beside a rule that moves mail onto another account's server, it is asked as a send.
    let other = s.run("account.add", &json!({"email": "other@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let theirs = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(theirs).unwrap().server = Some(ServerFolder { id: "O-in".into(), ..Default::default() });
    let onto = json!({"name": "Onto", "account": account, "server": false, "conditions": [{"type": "fromContains", "value": "ann"}], "actions": [{"type": "moveTo", "value": theirs}]});
    s.run("rules.add", &onto).unwrap();
    assert_eq!(classify(&s, "junk.add", &safe), Access::Owner(Class::Send));
    assert!(crate::agent::describe(&s, "junk.add", &safe).contains("Onto"));
}

/// The owner's rules: Zoe's mail gets the category Boss; Boss mail whose text says "keep" is
/// flagged and stops the rules after it (it reads the text, so it waits for the download); mail
/// about a "Plan" is deleted for good, after it. A Plan from Zoe saying "keep" is kept. An agent's
/// Clear All Categories on it while it waits for the download (a command asked about nothing)
/// changes which mail reaches the rule that deletes for good, as a change to the rule before it
/// would, which is asked as a delete. Once something changed what the rules read of it, those that
/// waited delete nothing for good.
#[test]
fn an_agents_edit_of_mail_waiting_for_its_rules_doesnt_let_a_later_rule_delete_it_for_good() {
    use crate::agent::{Verdict, check};
    let mut outcome = Vec::new();
    for agent in [false, true] {
        let mut s = Session::empty();
        let (account, _, _) = microsoft_with(&mut s, "Kept");
        let boss = json!({"name": "Boss", "server": false, "conditions": [{"type": "fromContains", "value": "zoe"}],
            "actions": [{"type": "categorize", "value": "Boss"}]});
        s.run("rules.add", &boss).unwrap();
        let keep = json!({"name": "Keep", "server": false, "stop": true,
            "conditions": [{"type": "category", "value": "Boss"}, {"type": "bodyContains", "value": "keep"}], "actions": [{"type": "flag"}]});
        s.run("rules.add", &keep).unwrap();
        let plans = json!({"name": "Plans", "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
            "actions": [{"type": "deleteForGood"}]});
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
        let deletes = s.mailbox.queued.iter().any(|q| matches!(q, mailcraft_model::Queued::DeleteMessage { id, .. } if id == "N1"));
        outcome.push(format!("agent {agent}: here {}, deleted on the server {deletes}", s.mailbox.message(m).is_some()));
    }
    assert_eq!(
        outcome,
        ["agent false: here true, deleted on the server false", "agent true: here true, deleted on the server false"],
        "an agent's unasked Clear All Categories made the rule delete the message for good"
    );
}

/// The same agent edit as the test above, with the last rule copying "Plan" into another
/// account's folder instead of deleting it for good. The rule "Keep", which stops the others,
/// would have kept the message off the other server; the agent's unasked Clear All Categories
/// makes it not apply once the text is here. With the marks changed meanwhile, the rules that
/// waited copy and move nothing onto another account's server, as they delete nothing for good,
/// with or without the agent's edit (an agent's own copy there asks `send`).
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

/// An agent asks to Run Rules Now over the Inbox while a message there has only its preview: "Purge" (delete
/// permanently, except if the text says "keep") can't tell, so it leaves the message alone. While
/// the question is up the message is downloaded (the agent opened it, say): the text doesn't say
/// "keep". Every condition and exception holds of it as before; only whether the exception can be
/// told changed. The yes runs nothing (#566).
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
    let purge = json!({"name": "Purge", "account": account, "server": false, "conditions": [{"type": "subjectContains", "value": "Plan"}],
        "exceptions": [{"type": "bodyContains", "value": "keep"}], "actions": [{"type": "deleteForGood"}]});
    s.run("rules.add", &purge).unwrap();
    let m = here(&s, account, "N1").id;
    assert!(s.mailbox.message(m).unwrap().unfetched);
    let params = json!({"folder": inbox});
    let Verdict::Ask(Class::Delete, what) = check(&s, "rules.run", &params, &[]) else { panic!("asked as a delete") };
    let shown = Shown::new(&s, "rules.run", &params, &what).unwrap();
    let fetched = vec![whole("N1", "Plan", "", "The plan.")];
    graph_mail::apply_graph_fetch(&mut s, GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }).unwrap();
    assert!(s.mailbox.message(m).is_some_and(|m| !m.unfetched), "downloaded, and still here");
    let reach = shown.check(&s, "rules.run", &params);
    // What the yes would do now: Purge takes it.
    s.run("rules.run", &params).unwrap();
    assert!(s.mailbox.message(m).is_none(), "Run Rules Now now deletes it for good");
    assert!(reach.is_err(), "the yes runs, and deletes for good a message the rule left alone when asked: {what}");
}
