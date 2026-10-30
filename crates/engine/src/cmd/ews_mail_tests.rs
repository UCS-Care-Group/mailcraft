// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Exchange server account (#252, E6), as the engine works out what a host should read and
//! takes in what it brings back, with results made by hand. (Against `mailcraft-ews`'s stand-in:
//! `apps/mailcraft/src/ews_mail_flow.rs`.)

use mailcraft_model::sync::{EwsFolderDeltas, EwsMessagePage, GraphFetched, GraphFolder, GraphMessage, ReadState, ServerMessage};
use mailcraft_model::{Address, FlagStatus, Message};
use serde_json::json;

use super::*;

const ME: &str = "ada@example.org";
const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";

fn session() -> (Session, Id) {
    let mut s = Session::empty();
    s.ews = true;
    let added = s.run("account.add", &json!({"email": ME, "kind": "exchange", "ews": {"url": URL}})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn folder(id: &str, parent: &str, name: &str, kind: FolderKind) -> GraphFolder {
    GraphFolder { id: id.into(), parent: parent.into(), name: name.into(), kind, ..Default::default() }
}

fn folders() -> Vec<GraphFolder> {
    vec![
        folder("F1", "", "Inbox", FolderKind::Inbox),
        folder("F2", "", "Sent Items", FolderKind::Sent),
        folder("F3", "F5", "Apollo", FolderKind::Custom),
        folder("F5", "", "Projects", FolderKind::Custom),
    ]
}

fn mail(id: &str, subject: &str, received: i64) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), size: 1_200, change_key: "CK1".into(), ..Default::default() },
        message_id: format!("{id}@example.org"),
        subject: subject.into(),
        from: Address::new("Zoe Park", "zoe@example.org"),
        to: vec![Address::new("Ada", ME)],
        sent: received - 60,
        received,
        preview: format!("About {subject}"),
        ..Default::default()
    }
}

fn piece(account: Id) -> EwsMailResult {
    EwsMailResult { account, email: ME.into(), url: URL.into(), partial: true, ..Default::default() }
}

fn page(folder: &str, messages: Vec<GraphMessage>, afresh: bool, last: bool) -> EwsMessagePage {
    EwsMessagePage { folder: folder.into(), messages, afresh, last, ..Default::default() }
}

/// The folder list, as a first read brings it.
fn list(s: &mut Session, account: Id) {
    let deltas = EwsFolderDeltas { changed: folders(), afresh: true, ..Default::default() };
    apply_ews_mail(s, EwsMailResult { folders: Some(deltas), hierarchy: "HIER-1".into(), ..piece(account) }).unwrap();
}

/// The page as the host makes it: read from where the folder is now (unless it begins afresh) and
/// going on from `state_after`.
fn next(s: &Session, account: Id, mut page: EwsMessagePage, state_after: &str) -> EwsMessagePage {
    if !page.afresh && page.from.is_empty() {
        page.from = s.mailbox.folder_by_server(account, &page.folder).map(|_| state_of(s, account, &page.folder)).unwrap_or_default();
    }
    page.state = state_after.into();
    page
}

/// One page of a folder with its state, more to come.
fn read(s: &mut Session, account: Id, page: EwsMessagePage, state_after: &str) -> Value {
    let page = next(s, account, page, state_after);
    apply_ews_mail(s, EwsMailResult { pages: vec![page], ..piece(account) }).unwrap()
}

/// The run's last piece.
fn end(s: &mut Session, account: Id, whole: bool) -> Value {
    apply_ews_mail(s, EwsMailResult { whole, partial: false, ..piece(account) }).unwrap()
}

fn here<'s>(s: &'s Session, account: Id, id: &str) -> Option<&'s Message> {
    s.mailbox.messages.values().map(|m| &**m).find(|m| m.account == account && m.server.as_ref().is_some_and(|server| server.id == id))
}

fn state_of(s: &Session, account: Id, server: &str) -> String {
    let id = s.mailbox.folder_by_server(account, server).unwrap();
    s.mailbox.folder(id).unwrap().server.as_ref().unwrap().sync_state.clone()
}

#[test]
fn the_first_request_reads_everything_from_the_start() {
    let (s, account) = session();
    let request = ews_mail_request(&s, account, false).unwrap();
    assert_eq!((request.url.as_str(), request.email.as_str()), (URL, ME));
    assert!(request.list_folders && request.hierarchy.is_empty() && request.states.is_empty());
    assert_eq!(request.scope, EwsScope::All);
    // A light run of an account with nothing yet still reads the folder list.
    assert!(ews_mail_request(&s, account, true).unwrap().list_folders);
    assert!(ews_inbox_request(&s, account).is_none(), "no Inbox of the server's yet");
    // The request names no password and, in debug output, no address.
    assert!(!format!("{request:?}").contains("exchange.example.org"));
}

#[test]
fn the_folders_and_the_first_pages_come_in_with_the_states_that_go_on_from_them() {
    let (mut s, account) = session();
    list(&mut s, account);
    assert_eq!(s.mailbox.account(account).unwrap().ews.as_ref().unwrap().hierarchy, "HIER-1");
    let apollo = s.mailbox.folder_by_server(account, "F3").unwrap();
    let projects = s.mailbox.folder_by_server(account, "F5").unwrap();
    assert_eq!(s.mailbox.folder(apollo).unwrap().parent, Some(projects), "a folder lies in the one it is listed under, whichever comes first");
    let said = read(&mut s, account, page("F1", vec![mail("A1", "Kickoff", 1_000), mail("A2", "Budget", 2_000)], true, false), "S-1");
    assert_eq!(said["new"], 2);
    let kickoff = here(&s, account, "A1").unwrap();
    assert!(kickoff.unfetched && kickoff.body.text == "About Kickoff" && kickoff.subject == "Kickoff");
    assert_eq!(kickoff.server.as_ref().unwrap().change_key, "CK1");
    assert_eq!(state_of(&s, account, "F1"), "S-1");
    // The next page goes on from it, and a folder whose pages haven't ended hasn't had its first round.
    let request = ews_mail_request(&s, account, false).unwrap();
    assert!(request.states.iter().any(|st| st.folder == "F1" && st.state == "S-1"));
    assert!(request.states.iter().any(|st| st.folder == "F2" && st.state.is_empty()), "a folder not read yet is read from its start");
    assert_eq!(request.hierarchy, "HIER-1");
    read(&mut s, account, page("F1", vec![mail("A3", "Agenda", 3_000)], false, true), "S-2");
    assert_eq!(state_of(&s, account, "F1"), "S-2");
    let inbox = s.mailbox.folder_by_server(account, "F1").unwrap();
    assert!(s.mailbox.folder(inbox).unwrap().server.as_ref().unwrap().round_ended);
    assert_eq!(s.mailbox.messages.values().filter(|m| m.folder == inbox).count(), 3);
    // A light run reads the Inbox and the folder on screen, and its states are those alone.
    let light = ews_mail_request(&s, account, true).unwrap();
    assert!(!light.list_folders);
    assert_eq!(light.states.iter().map(|st| st.folder.as_str()).collect::<Vec<_>>(), vec!["F1"]);
    let pass = ews_inbox_request(&s, account).unwrap();
    assert!(pass.inbox_pass && pass.states.len() == 1 && !pass.list_folders);
}

#[test]
fn a_delta_changes_what_changed_and_what_left_goes_when_the_run_ends_well() {
    let (mut s, account) = session();
    list(&mut s, account);
    read(
        &mut s,
        account,
        page("F1", vec![mail("A1", "Kickoff", 1_000), mail("A2", "Budget", 2_000), mail("A3", "Agenda", 3_000)], true, true),
        "S-1",
    );
    end(&mut s, account, true);
    let mut edited = mail("A1", "Kickoff (moved)", 1_000);
    edited.server.change_key = "CK2".into();
    edited.server.flag = FlagStatus::Flagged;
    let delta = EwsMessagePage {
        folder: "F1".into(),
        messages: vec![edited, mail("A4", "Late", 4_000)],
        removed: vec!["A3".into()],
        read: vec![ReadState { id: "A2".into(), read: true }],
        last: true,
        ..Default::default()
    };
    let said = read(&mut s, account, delta, "S-2");
    assert_eq!((said["new"].as_u64(), said["changed"].as_u64(), said["gone"].as_u64()), (Some(1), Some(2), Some(1)));
    // Matched by the item id: one message, with the new change key, not a second one.
    let kickoff = here(&s, account, "A1").unwrap();
    assert_eq!((kickoff.subject.as_str(), kickoff.flag), ("Kickoff", FlagStatus::Flagged));
    assert_eq!(kickoff.server.as_ref().unwrap().change_key, "CK2");
    assert!(here(&s, account, "A2").unwrap().read);
    // Kept until a run that read every folder ends well; then gone.
    assert!(here(&s, account, "A3").is_some_and(|m| m.server.as_ref().unwrap().gone));
    end(&mut s, account, false);
    assert!(here(&s, account, "A3").is_some(), "a run that did not go well removes nothing");
    let said = end(&mut s, account, true);
    assert!(here(&s, account, "A3").is_none());
    assert_eq!(said["removedMessages"], 1);
    // Mail that arrives from now on is new mail, which the lists and the rules sort.
    assert_eq!(state_of(&s, account, "F1"), "S-2");
}

#[test]
fn a_folder_read_afresh_keeps_what_the_server_still_has_and_drops_the_rest() {
    let (mut s, account) = session();
    list(&mut s, account);
    read(&mut s, account, page("F1", vec![mail("A1", "Kickoff", 1_000), mail("A2", "Budget", 2_000)], true, true), "S-1");
    // The one fetched whole keeps its body through a fresh read: it is matched by item id.
    let kickoff = here(&s, account, "A1").unwrap().id;
    let mime = b"From: Zoe Park <zoe@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: Kickoff\r\nMessage-ID: <A1@example.org>\r\n\r\nThe whole text.\r\n";
    apply_ews_fetch(
        &mut s,
        GraphFetchResult { account, email: ME.into(), fetched: vec![GraphFetched { id: "A1".into(), mime: mime.to_vec() }], ..Default::default() },
    )
    .unwrap();
    assert!(!s.mailbox.message(kickoff).unwrap().unfetched);
    // The server lost its state: the fresh read lists A1 and A3, and not A2.
    let fresh = read(&mut s, account, page("F1", vec![mail("A1", "Kickoff", 1_000), mail("A3", "Agenda", 3_000)], true, true), "S-9");
    assert_eq!((fresh["new"].as_u64(), fresh["gone"].as_u64()), (Some(1), Some(0)));
    assert!(here(&s, account, "A2").is_some_and(|m| m.server.as_ref().unwrap().gone));
    end(&mut s, account, true);
    assert!(here(&s, account, "A2").is_none() && here(&s, account, "A3").is_some());
    let kickoff = here(&s, account, "A1").unwrap();
    assert_eq!(kickoff.id, s.mailbox.message(kickoff.id).unwrap().id);
    assert!(!kickoff.unfetched && kickoff.body.text.contains("The whole text."), "the message fetched whole stays whole");
    assert_eq!(state_of(&s, account, "F1"), "S-9");
}

#[test]
fn a_deleted_folder_goes_with_its_mail_and_a_renamed_one_keeps_where_it_went_on_from() {
    let (mut s, account) = session();
    list(&mut s, account);
    read(&mut s, account, page("F5", vec![mail("P1", "Plan", 1_000)], true, true), "S-P");
    let delta = EwsFolderDeltas { changed: vec![folder("F5", "", "Projects (old)", FolderKind::Custom)], deleted: vec!["F3".into()], afresh: false };
    apply_ews_mail(&mut s, EwsMailResult { folders: Some(delta), hierarchy: "HIER-2".into(), ..piece(account) }).unwrap();
    let projects = s.mailbox.folder_by_server(account, "F5").unwrap();
    assert_eq!(s.mailbox.folder(projects).unwrap().name, "Projects (old)");
    assert_eq!(state_of(&s, account, "F5"), "S-P", "a rename leaves where the folder goes on from");
    assert!(s.mailbox.folder_by_server(account, "F3").is_none(), "deleted on the server");
    assert!(s.mailbox.folder_by_server(account, "F1").is_some(), "what the delta doesn't mention stays");
    assert_eq!(s.mailbox.account(account).unwrap().ews.as_ref().unwrap().hierarchy, "HIER-2");
    // Read afresh, the folder list is every folder: one it lacks is gone, but not a standard folder.
    let all = EwsFolderDeltas { changed: vec![folder("F1", "", "Inbox", FolderKind::Inbox)], afresh: true, ..Default::default() };
    apply_ews_mail(&mut s, EwsMailResult { folders: Some(all), hierarchy: "HIER-3".into(), ..piece(account) }).unwrap();
    assert!(s.mailbox.folder_by_server(account, "F5").is_none());
    assert!(s.mailbox.folder_of_kind(account, FolderKind::Inbox).is_some());
}

#[test]
fn a_page_for_a_folder_that_is_not_here_is_passed_over_with_its_state() {
    let (mut s, account) = session();
    let said = read(&mut s, account, page("F1", vec![mail("A1", "Kickoff", 1_000)], true, true), "S-1");
    assert_eq!(said["new"], 0);
    assert!(here(&s, account, "A1").is_none());
}

#[test]
fn a_piece_that_is_too_large_is_refused_and_one_that_is_odd_is_cut_and_removes_nothing() {
    let (mut s, account) = session();
    list(&mut s, account);
    read(&mut s, account, page("F1", vec![mail("A1", "Kickoff", 1_000), mail("A2", "Budget", 2_000)], true, true), "S-1");
    end(&mut s, account, true);
    let before = s.mailbox.clone();
    // A subject, a preview, a name or a list far beyond any message: refused whole.
    for odd in [
        GraphMessage { subject: "x".repeat(20_000), ..mail("B1", "s", 1) },
        GraphMessage { preview: "x".repeat(20_000), ..mail("B1", "s", 1) },
        GraphMessage { from: Address::new("x".repeat(20_000), "a@example.org"), ..mail("B1", "s", 1) },
        GraphMessage { to: vec![Address::new("x", "a@example.org"); 2_000], ..mail("B1", "s", 1) },
        GraphMessage { message_id: "x".repeat(20_000), ..mail("B1", "s", 1) },
    ] {
        let why =
            apply_ews_mail(&mut s, EwsMailResult { pages: vec![page("F1", vec![odd], false, true)], ..piece(account) }).unwrap_err().to_string();
        assert!(why.contains("longer than is taken in"), "{why}");
    }
    let long_folder = EwsFolderDeltas { changed: vec![folder("F9", "", &"x".repeat(5_000), FolderKind::Custom)], ..Default::default() };
    assert!(apply_ews_mail(&mut s, EwsMailResult { folders: Some(long_folder), ..piece(account) }).is_err());
    assert!(s.mailbox == before);
    // An id that is no id (it has a control character): the piece is refused whole, with its state.
    let odd_id = mail("A\u{1}B", "Odd", 5_000);
    let refused = next(&s, account, page("F1", vec![odd_id, mail("A3", "Agenda", 3_000)], false, false), "S-2");
    assert!(apply_ews_mail(&mut s, EwsMailResult { pages: vec![refused], ..piece(account) }).is_err());
    assert!(here(&s, account, "A3").is_none() && s.mailbox == before);
    // The page after it was read from the state that was never kept (S-2): it is not taken in,
    // and the folder stays where it is. Nothing is skipped over.
    let after = EwsMessagePage { from: "S-2".into(), ..page("F1", vec![mail("A5", "Later", 5_000)], false, true) };
    let said = apply_ews_mail(&mut s, EwsMailResult { pages: vec![EwsMessagePage { state: "S-3".into(), ..after }], ..piece(account) }).unwrap();
    assert_eq!(said["new"], 0);
    assert!(here(&s, account, "A5").is_none());
    assert_eq!(state_of(&s, account, "F1"), "S-1");
    // Long text that is within the limits is cut to what is shown.
    let long = GraphMessage { subject: format!("{}\r\n{}", "y".repeat(2_000), "z"), ..mail("A4", "s", 4_000) };
    read(&mut s, account, page("F1", vec![long], false, true), "S-3");
    let subject = &here(&s, account, "A4").unwrap().subject;
    assert!(subject.chars().count() <= 998 && !subject.contains(['\r', '\n']));
}

#[test]
fn a_state_is_taken_in_only_after_the_page_it_followed_and_a_run_that_left_one_out_removes_nothing() {
    let (mut s, account) = session();
    list(&mut s, account);
    read(&mut s, account, page("F1", vec![mail("A1", "Kickoff", 1_000), mail("A2", "Budget", 2_000)], true, true), "S-1");
    // The state was lost: the fresh read's first page is refused (the host went on with S-2 in
    // memory), its second page is read from S-2, and the run ends "well".
    let first = page("F1", vec![mail("A1", "Kickoff", 1_000), mail("A\u{2}", "Odd", 1)], true, false);
    let first = EwsMessagePage { state: "S-2".into(), ..first };
    assert!(apply_ews_mail(&mut s, EwsMailResult { pages: vec![first], ..piece(account) }).is_err());
    let second = EwsMessagePage { from: "S-2".into(), state: "S-3".into(), ..page("F1", vec![mail("A3", "Agenda", 3_000)], false, true) };
    apply_ews_mail(&mut s, EwsMailResult { pages: vec![second], ..piece(account) }).unwrap();
    end(&mut s, account, true);
    // Nothing that was here is gone, and the folder is where it was: it is read again.
    assert!(here(&s, account, "A1").is_some() && here(&s, account, "A2").is_some() && here(&s, account, "A3").is_none());
    assert!(!here(&s, account, "A2").unwrap().server.as_ref().unwrap().gone);
    assert_eq!(state_of(&s, account, "F1"), "S-1");
}

#[test]
fn each_state_belongs_to_its_own_page_when_a_result_has_several_for_one_folder() {
    let (mut s, account) = session();
    list(&mut s, account);
    let one = EwsMessagePage { state: "S-1".into(), ..page("F1", vec![mail("A1", "Kickoff", 1_000)], true, false) };
    let two = EwsMessagePage { from: "S-1".into(), state: "S-2".into(), ..page("F1", vec![mail("A2", "Budget", 2_000)], false, false) };
    // The third came cut (it has no state): the folder goes on from the second, not past the third.
    let three = EwsMessagePage { from: "S-2".into(), state: String::new(), ..page("F1", vec![mail("A3", "Agenda", 3_000)], false, true) };
    apply_ews_mail(&mut s, EwsMailResult { pages: vec![one, two, three], ..piece(account) }).unwrap();
    assert_eq!(state_of(&s, account, "F1"), "S-2");
    assert!(here(&s, account, "A3").is_some());
    // A page that skips a state (from S-5) is not taken in.
    let skip = EwsMessagePage { from: "S-5".into(), state: "S-6".into(), ..page("F1", vec![mail("A4", "Skipped", 4_000)], false, true) };
    apply_ews_mail(&mut s, EwsMailResult { pages: vec![skip], ..piece(account) }).unwrap();
    assert!(here(&s, account, "A4").is_none());
    assert_eq!(state_of(&s, account, "F1"), "S-2");
}

#[test]
fn an_account_keeps_no_more_sync_state_in_all_than_is_taken_in() {
    let (mut s, account) = session();
    let many: Vec<GraphFolder> = (0..40).map(|n| folder(&format!("X{n}"), "", &format!("Box {n}"), FolderKind::Custom)).collect();
    let deltas = EwsFolderDeltas { changed: many, afresh: true, ..Default::default() };
    apply_ews_mail(&mut s, EwsMailResult { folders: Some(deltas), ..piece(account) }).unwrap();
    for n in 0..40 {
        let id = s.mailbox.folder_by_server(account, &format!("X{n}")).unwrap();
        // (Each state is within its own size; together they are past the account's.)
        if let Some(server) = s.mailbox.folder_mut(id).unwrap().server.as_mut() {
            server.sync_state = "s".repeat(mailcraft_model::sync::MAX_EWS_STATE);
        }
    }
    let before = s.mailbox.clone();
    let big = EwsMessagePage { from: "s".to_string(), ..page("X0", vec![mail("B1", "Hello", 1_000)], false, true) };
    let why = apply_ews_mail(&mut s, EwsMailResult { pages: vec![EwsMessagePage { state: "t".into(), ..big }], ..piece(account) }).unwrap_err();
    assert!(why.to_string().contains("more sync state"), "{why}");
    assert!(s.mailbox == before, "a piece that is refused changes nothing");
}

#[test]
fn a_run_for_another_account_or_another_server_is_not_taken_in() {
    let (mut s, account) = session();
    list(&mut s, account);
    let other_account = EwsMailResult { email: "bob@example.org".into(), ..piece(account) };
    assert!(apply_ews_mail(&mut s, other_account).unwrap_err().to_string().contains("another account"));
    let other_server = EwsMailResult { url: "https://other.example.org/EWS/Exchange.asmx".into(), ..piece(account) };
    assert!(apply_ews_mail(&mut s, other_server).unwrap_err().to_string().contains("named before"));
    let local = s.run("account.add", &json!({"email": "local@example.org"})).unwrap()["id"].as_u64().unwrap();
    assert!(apply_ews_mail(&mut s, EwsMailResult { account: local, email: "local@example.org".into(), ..Default::default() }).is_err());
}

#[test]
fn what_a_run_says_of_trouble_is_told_and_a_certificate_asks() {
    let (mut s, account) = session();
    list(&mut s, account);
    let said = apply_ews_mail(&mut s, EwsMailResult { auth_refused: true, partial: false, ..piece(account) }).unwrap();
    assert_eq!(said["authRefused"], true);
    assert!(said["errors"][0].as_str().unwrap().contains("turned down the sign-in"));
    let print = "ab".repeat(32);
    let said = apply_ews_mail(&mut s, EwsMailResult { untrusted: print.clone(), whole: true, partial: false, ..piece(account) }).unwrap();
    assert_eq!(said["untrusted"], print.as_str());
    assert!(said["errors"][0].as_str().unwrap().contains("needs your say"), "{said}");
    // (A run with a certificate to answer for is not one that went well: nothing is removed.)
    let said = apply_ews_mail(&mut s, EwsMailResult { busy_for: Some(7), partial: false, ..piece(account) }).unwrap();
    assert_eq!(said["busyFor"], 7);
    assert!(s.sync_status.starts_with("Up to date") || !s.sync_status.is_empty());
}

#[test]
fn messages_are_fetched_whole_within_what_the_account_keeps() {
    let (mut s, account) = session();
    list(&mut s, account);
    read(&mut s, account, page("F1", vec![mail("A1", "Kickoff", 1_000), mail("A2", "Budget", 2_000)], true, true), "S-1");
    let queue = ews_fetch_request(&s, account, None, 10).unwrap();
    assert_eq!(queue.ids.len(), 2);
    assert!(queue.background && queue.url == URL && queue.email == ME);
    let kickoff = here(&s, account, "A1").unwrap().id;
    let one = ews_fetch_request(&s, account, Some(kickoff), 10).unwrap();
    assert_eq!(one.ids, vec!["A1".to_owned()]);
    assert!(!one.background);
    let mime = b"From: Zoe Park <zoe@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: Kickoff\r\nMessage-ID: <A1@example.org>\r\n\r\nThe whole text.\r\n";
    let said = apply_ews_fetch(
        &mut s,
        GraphFetchResult {
            account,
            email: ME.into(),
            fetched: vec![GraphFetched { id: "A1".into(), mime: mime.to_vec() }, GraphFetched { id: "<bad>".into(), mime: mime.to_vec() }],
            missing: vec!["A2".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(said["fetched"], 1);
    assert!(!here(&s, account, "A1").unwrap().unfetched);
    // The one the server no longer has is not asked for again in the background this session.
    assert!(ews_fetch_request(&s, account, None, 10).unwrap().ids.is_empty());
    // A command that asks for it by the commands' own door gets the same.
    assert_eq!(s.run("sync.ewsFetchRequest", &json!({"account": account, "message": kickoff})).unwrap()["ids"], json!([]));
    let again = s.run("sync.applyEwsFetch", &json!({"result": {"account": account, "email": ME, "fetched": [{"id": "A2", "mime": mime.to_vec()}]}}));
    assert!(again.is_ok());
}

#[test]
fn the_commands_take_a_result_by_json_and_leave_no_server_id_in_their_errors() {
    let (mut s, account) = session();
    let result = json!({"result": {"account": account, "email": ME, "url": URL, "folders": {"changed": [{"id": "F1", "name": "Inbox", "kind": "inbox"}], "afresh": true}, "hierarchy": "H", "partial": true}});
    s.run("sync.applyEws", &result).unwrap();
    assert!(s.mailbox.folder_by_server(account, "F1").is_some());
    let bad = json!({"result": {"account": account, "email": ME, "pages": [{"folder": "F1\u{7}", "messages": []}]}});
    let why = s.run("sync.applyEws", &bad).unwrap_err().to_string();
    assert!(!why.contains("F1"), "{why}");
    assert!(s.run("sync.applyEws", &json!({"result": "x"})).is_err());
}

#[test]
fn a_refused_page_keeps_the_end_of_its_run_from_sweeping() {
    let (mut s, account) = session();
    list(&mut s, account);
    read(&mut s, account, page("F1", vec![mail("A1", "Kickoff", 1_000), mail("A2", "Budget", 2_000)], true, true), "S-1");
    // A run begins (asked for as a host does), and the folder is read afresh: its first page is taken.
    s.run("sync.ewsRequest", &json!({"account": account})).unwrap();
    let first = EwsMessagePage { state: "S-2".into(), ..page("F1", vec![mail("A1", "Kickoff", 1_000)], true, false) };
    apply_ews_mail(&mut s, EwsMailResult { pages: vec![first], ..piece(account) }).unwrap();
    // Its second page is refused...
    let second = EwsMessagePage { from: "S-2".into(), state: "S-3".into(), ..page("F1", vec![mail("A\u{2}", "Odd", 1)], false, true) };
    assert!(apply_ews_mail(&mut s, EwsMailResult { pages: vec![second], ..piece(account) }).is_err());
    // ...and the run's last piece, a clean one on its own, sweeps nothing: A2 was never listed.
    end(&mut s, account, true);
    assert_eq!(state_of(&s, account, "F1"), "S-2");
    assert!(here(&s, account, "A2").is_some(), "A2 is swept although its page was refused");
    // The next run is not held against it: it begins with its own slate.
    s.run("sync.ewsRequest", &json!({"account": account})).unwrap();
    read(&mut s, account, page("F1", vec![mail("A1", "Kickoff", 1_000)], true, true), "S-4");
    end(&mut s, account, true);
    assert!(here(&s, account, "A2").is_none(), "a run that went well sweeps");
}

#[test]
fn a_run_that_ends_on_a_refused_last_piece_does_not_hold_the_next_run_back() {
    let (mut s, account) = session();
    list(&mut s, account);
    read(&mut s, account, page("F1", vec![mail("A1", "Kickoff", 1_000), mail("A2", "Budget", 2_000)], true, true), "S-1");
    // Run 1: the folder is read afresh, and its last piece is the one that is refused.
    s.run("sync.ewsRequest", &json!({"account": account})).unwrap();
    let bad = EwsMessagePage { from: "S-1".into(), state: "S-2".into(), ..page("F1", vec![mail("A\u{2}", "Odd", 1)], true, true) };
    let last = EwsMailResult { pages: vec![bad], whole: true, partial: false, ..piece(account) };
    assert!(apply_ews_mail(&mut s, last).is_err());
    assert!(here(&s, account, "A2").is_some());
    // Run 2 begins with its own slate: it reads the folder, and its end sweeps what the server lacks.
    s.run("sync.ewsRequest", &json!({"account": account})).unwrap();
    read(&mut s, account, page("F1", vec![mail("A1", "Kickoff", 1_000)], true, true), "S-3");
    end(&mut s, account, true);
    assert!(here(&s, account, "A2").is_none(), "the next run sweeps");
}

#[test]
fn a_page_left_out_or_cut_or_a_blocked_folder_each_keep_the_end_of_their_run_from_sweeping() {
    let sweeps = |spoil: &dyn Fn(&mut Session, Id)| {
        let (mut s, account) = session();
        list(&mut s, account);
        read(&mut s, account, page("F1", vec![mail("A1", "Kickoff", 1_000), mail("A2", "Budget", 2_000)], true, true), "S-1");
        s.run("sync.ewsRequest", &json!({"account": account})).unwrap();
        let first = EwsMessagePage { state: "S-2".into(), ..page("F1", vec![mail("A1", "Kickoff", 1_000)], true, true) };
        apply_ews_mail(&mut s, EwsMailResult { pages: vec![first], ..piece(account) }).unwrap();
        spoil(&mut s, account);
        end(&mut s, account, true);
        here(&s, account, "A2").is_none()
    };
    assert!(sweeps(&|_, _| {}), "nothing spoiled: the end sweeps");
    // A page that doesn't follow the folder's state is left out.
    assert!(!sweeps(&|s, account| {
        let lost = EwsMessagePage { from: "S-9".into(), state: "S-10".into(), ..page("F1", vec![mail("A3", "Lost", 3_000)], false, true) };
        apply_ews_mail(s, EwsMailResult { pages: vec![lost], ..piece(account) }).unwrap();
    }));
    // A piece that came cut.
    assert!(!sweeps(&|s, account| {
        // (A folder with no name to show is passed over, and the piece is a cut one.)
        let deltas = EwsFolderDeltas { changed: vec![folder("F8", "", "", FolderKind::Custom)], ..Default::default() };
        apply_ews_mail(s, EwsMailResult { folders: Some(deltas), ..piece(account) }).unwrap();
    }));
    // A folder blocked.
    assert!(!sweeps(&|s, account| {
        apply_ews_mail(s, EwsMailResult { blocked: vec!["F1".into()], ..piece(account) }).unwrap();
    }));
}

#[test]
fn a_blocked_folder_is_named_as_the_owner_knows_it_and_nothing_else_is_said_of_it() {
    let (mut s, account) = session();
    list(&mut s, account);
    let said = apply_ews_mail(
        &mut s,
        EwsMailResult { blocked: vec!["F1".into(), "F3".into(), "NOT-HERE".into()], whole: false, partial: false, ..piece(account) },
    )
    .unwrap();
    assert_eq!(said["unreadableFolders"], json!(["Inbox", "Apollo"]));
    let errors: Vec<&str> = said["errors"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(errors, ["Inbox couldn't be read from the server", "Apollo couldn't be read from the server"]);
    // A folder this mailbox hasn't got is not named (and no id is told).
    assert!(!said.to_string().contains("NOT-HERE") && !said.to_string().contains("\"F1\""));
    // None blocked: none named.
    let said = end(&mut s, account, true);
    assert_eq!(said["unreadableFolders"], json!([]));
}

/// An Exchange account with the folders X0..Xn, each at a state of the sizes given, and the folder
/// list at `hierarchy`.
fn kept(sizes: &[usize], hierarchy: &str) -> (Session, Id) {
    let (mut s, account) = session();
    let many: Vec<GraphFolder> = (0..sizes.len()).map(|n| folder(&format!("X{n}"), "", &format!("Box {n}"), FolderKind::Custom)).collect();
    let deltas = EwsFolderDeltas { changed: many, afresh: true, ..Default::default() };
    apply_ews_mail(&mut s, EwsMailResult { folders: Some(deltas), hierarchy: hierarchy.into(), ..piece(account) }).unwrap();
    for (n, size) in sizes.iter().enumerate() {
        let id = s.mailbox.folder_by_server(account, &format!("X{n}")).unwrap();
        if let Some(server) = s.mailbox.folder_mut(id).unwrap().server.as_mut() {
            server.sync_state = "s".repeat(*size);
        }
    }
    (s, account)
}

#[test]
fn an_account_keeps_exactly_the_most_sync_state_and_not_a_byte_more() {
    let one = mailcraft_model::sync::MAX_EWS_STATE;
    let cap = mailcraft_model::sync::MAX_EWS_STATE_BYTES;
    assert_eq!(cap, 32 * one);
    // 32 MiB exactly (32 folders at 1 MiB): a piece is taken in; 32 MiB and a byte (a 33rd folder
    // with a state of one byte): it is refused.
    let (mut s, account) = kept(&[one; 32], "");
    assert!(apply_ews_mail(&mut s, EwsMailResult { whole: false, ..piece(account) }).is_ok());
    let mut sizes = vec![one; 32];
    sizes.push(1);
    let (mut s, account) = kept(&sizes, "");
    let before = s.mailbox.clone();
    assert!(apply_ews_mail(&mut s, EwsMailResult { ..piece(account) }).unwrap_err().to_string().contains("more sync state"));
    assert!(s.mailbox == before);
}

#[test]
fn the_new_states_of_a_result_are_counted_with_the_ones_kept() {
    let one = mailcraft_model::sync::MAX_EWS_STATE;
    // 31 folders at 1 MiB, one at 1 MiB less 10 bytes: 10 bytes under the most.
    let mut sizes = vec![one; 31];
    sizes.push(one - 10);
    let move_last = |s: &mut Session, account: Id, to: usize| {
        let from = state_of(s, account, "X31");
        let page = EwsMessagePage { folder: "X31".into(), from, state: "t".repeat(to), last: true, ..Default::default() };
        apply_ews_mail(s, EwsMailResult { pages: vec![page], ..piece(account) })
    };
    // The folder's own state is replaced, not added to: 10 more bytes reaches the most exactly...
    let (mut s, account) = kept(&sizes, "");
    assert!(move_last(&mut s, account, one).is_ok());
    assert_eq!(state_of(&s, account, "X31").len(), one);
    // ...and one byte over the most is refused (31 folders at 1 MiB, one at 1 MiB - 10, one at 10
    // bytes: exactly the most; the last goes on to a state of 11).
    let mut sizes = vec![one; 31];
    sizes.extend([one - 10, 10]);
    let (mut s, account) = kept(&sizes, "");
    let page = EwsMessagePage { folder: "X32".into(), from: "s".repeat(10), state: "t".repeat(11), last: true, ..Default::default() };
    let before = s.mailbox.clone();
    let why = apply_ews_mail(&mut s, EwsMailResult { pages: vec![page], ..piece(account) }).unwrap_err();
    assert!(why.to_string().contains("more sync state") && s.mailbox == before, "{why}");
    // The same with 10 bytes: exactly the most, taken in.
    let page = EwsMessagePage { folder: "X32".into(), from: "s".repeat(10), state: "t".repeat(10), last: true, ..Default::default() };
    assert!(apply_ews_mail(&mut s, EwsMailResult { pages: vec![page], ..piece(account) }).is_ok());
}

#[test]
fn the_folder_lists_state_is_counted_too() {
    let one = mailcraft_model::sync::MAX_EWS_STATE;
    // 31 folders at 1 MiB and one at 1 MiB - 1, and a folder list at "H" (a byte): the most exactly.
    let mut sizes = vec![one; 31];
    sizes.push(one - 1);
    let (mut s, account) = kept(&sizes, "H");
    assert!(apply_ews_mail(&mut s, EwsMailResult { ..piece(account) }).is_ok(), "exactly the most");
    // A new folder list state of the same size is taken in; a byte longer is not.
    let (mut s, account) = kept(&sizes, "H");
    assert!(apply_ews_mail(&mut s, EwsMailResult { hierarchy: "J".into(), folders: Some(Default::default()), ..piece(account) }).is_ok());
    assert_eq!(s.mailbox.account(account).unwrap().ews.as_ref().unwrap().hierarchy, "J");
    let before = s.mailbox.clone();
    let why = apply_ews_mail(&mut s, EwsMailResult { hierarchy: "JK".into(), folders: Some(Default::default()), ..piece(account) }).unwrap_err();
    assert!(why.to_string().contains("more sync state") && s.mailbox == before, "{why}");
}
