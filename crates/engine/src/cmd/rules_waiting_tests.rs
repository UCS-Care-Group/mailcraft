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
