// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #276: the counts kept with each folder's delta link, and when a Graph run is a full sweep.

use mailcraft_model::sync::{GraphFolder, GraphMailResult, GraphProgress, MessagePage, ServerMessage};
use mailcraft_model::{FolderCounts, FolderKind, Message};
use serde_json::json;

use super::*;

const ACCOUNT_EMAIL: &str = "owner@example.org";
const T: i64 = 1_790_000_000;

/// A session with one Microsoft account whose folders were listed once, with the clock at `T`.
fn session() -> (Session, Id) {
    let mut s = Session::empty();
    s.clock.now = Some(T);
    let added = s.run("account.add", &json!({"email": ACCOUNT_EMAIL, "kind": "microsoft"})).unwrap();
    let account = added["id"].as_u64().unwrap();
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    let folders = vec![folder("in", "Inbox", FolderKind::Inbox), folder("pr", "Projects", FolderKind::Custom)];
    take(&mut s, account, GraphMailResult { listing: Some(FolderListing { folders, complete: true }), partial: true, ..Default::default() });
    (s, account)
}

fn take(s: &mut Session, account: Id, mut result: GraphMailResult) -> Value {
    result.account = account;
    result.email = ACCOUNT_EMAIL.into();
    apply_graph_mail(s, result).unwrap()
}

/// The last piece of a run that read every folder to its end, `skipped` of them passed over.
fn ended(skipped: u32) -> GraphMailResult {
    let mut last = GraphMailResult { whole: true, ..Default::default() };
    last.timings.skipped = skipped;
    last
}

fn full(s: &Session, account: Id) -> bool {
    graph_mail_request(s, account).unwrap().full
}

#[test]
fn the_first_run_after_opening_is_a_full_sweep_then_one_an_hour() {
    let (mut s, account) = session();
    assert!(full(&s, account), "nothing read whole since the mailbox was opened");
    take(&mut s, account, ended(0));
    assert!(!full(&s, account));
    s.clock.now = Some(T + FULL_SWEEP_EVERY - 1);
    assert!(!full(&s, account));
    s.clock.now = Some(T + FULL_SWEEP_EVERY);
    assert!(full(&s, account), "an hour on");
    // Opening a mailbox starts over.
    take(&mut s, account, ended(0));
    assert!(!full(&s, account));
    let mailbox = s.mailbox.clone();
    s.replace_mailbox(mailbox);
    assert!(full(&s, account));
}

#[test]
fn a_run_that_passed_folders_over_or_wasnt_whole_is_not_a_sweep() {
    let (mut s, account) = session();
    take(&mut s, account, ended(1));
    assert!(full(&s, account));
    take(&mut s, account, GraphMailResult { stopped: true, ..Default::default() });
    assert!(full(&s, account));
    // Nor is the Inbox pass, nor a piece of a run under way.
    take(&mut s, account, GraphMailResult { inbox_pass: true, partial: true, ..ended(0) });
    assert!(full(&s, account));
}

/// A message the server said left its folder waits for a whole run: a folder passed over might
/// be where it went, so the next run passes nothing over.
#[test]
fn a_message_waiting_to_be_removed_makes_the_next_run_a_full_sweep() {
    let (mut s, account) = session();
    take(&mut s, account, ended(0));
    assert!(!full(&s, account));
    let projects = s.mailbox.folder_by_server(account, "pr").unwrap();
    let server = ServerMessage { id: "m1".into(), folder: "pr".into(), ..Default::default() };
    let m = s.mailbox.add_message(Message { folder: projects, account, server: Some(server), ..Default::default() }).unwrap();
    take(
        &mut s,
        account,
        GraphMailResult {
            pages: vec![MessagePage { folder: "pr".into(), removed: vec!["m1".into()], ..Default::default() }],
            partial: true,
            ..Default::default()
        },
    );
    assert!(s.mailbox.message(m).unwrap().server.as_ref().unwrap().gone);
    assert!(full(&s, account));
}

#[test]
fn the_counts_go_with_the_link_they_came_with_and_only_a_delta_link() {
    let (mut s, account) = session();
    let counts = FolderCounts { total: 7, unread: 2 };
    let page = |link: &str, caught_up, counts| MessagePage { folder: "pr".into(), link: link.into(), caught_up, counts, ..Default::default() };
    let link_of = |s: &Session| graph_mail_request(s, account).unwrap().links.into_iter().find(|l| l.folder == "pr").unwrap();
    take(
        &mut s,
        account,
        GraphMailResult { pages: vec![page("https://graph.microsoft.com/delta1", true, Some(counts))], partial: true, ..Default::default() },
    );
    assert_eq!(link_of(&s).counts, Some(counts));
    // Kept in the mailbox file with the link.
    let written = String::from_utf8(crate::io::write_mailbox(&s.mailbox).unwrap()).unwrap();
    assert!(written.contains("\"counts\":{\"total\":7,\"unread\":2}"), "{written}");
    // A new link without counts forgets the old ones.
    take(
        &mut s,
        account,
        GraphMailResult { pages: vec![page("https://graph.microsoft.com/next", false, None)], partial: true, ..Default::default() },
    );
    assert_eq!(link_of(&s).counts, None);
    // And counts never go with a link that isn't a delta link.
    let f = s.mailbox.folder_by_server(account, "pr").unwrap();
    s.mailbox.folder_mut(f).unwrap().server.as_mut().unwrap().counts = Some(counts);
    assert_eq!(link_of(&s).counts, None);
    // The listing keeps them as they are.
    let listing =
        FolderListing { folders: vec![GraphFolder { id: "pr".into(), name: "Projects".into(), total: 9, ..Default::default() }], complete: false };
    take(&mut s, account, GraphMailResult { listing: Some(listing), partial: true, ..Default::default() });
    assert_eq!(s.mailbox.folder(f).unwrap().server.as_ref().unwrap().counts, Some(counts));
}

/// A piece with no page (an empty page that wasn't handed on) changes nothing in the mailbox:
/// the file stays as it was saved.
#[test]
fn a_piece_without_pages_leaves_the_mailbox_as_it_was() {
    let (mut s, account) = session();
    s.dirty = false;
    let rev = s.rev();
    let progress = GraphProgress { folder: "in".into(), read: 0, total: 12, number: 1, folders: 2 };
    take(&mut s, account, GraphMailResult { progress: Some(progress), partial: true, ..Default::default() });
    assert!(!s.dirty);
    assert_eq!(s.rev(), rev);
}

/// A listing cut short left folders out, which weren't read: the run is no full sweep, and the
/// next is one, until a run whose listing was whole.
#[test]
fn a_run_whose_listing_was_cut_short_is_not_a_sweep() {
    let (mut s, account) = session();
    let short = |complete| GraphMailResult {
        listing: Some(FolderListing {
            folders: vec![GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }],
            complete,
        }),
        partial: true,
        ..Default::default()
    };
    take(&mut s, account, short(false));
    take(&mut s, account, ended(0));
    assert!(full(&s, account));
    take(&mut s, account, short(true));
    take(&mut s, account, ended(0));
    assert!(!full(&s, account));
}
