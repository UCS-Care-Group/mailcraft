// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Device limits (MB3.16): the setting.

use mailcraft_model::device_limits::{MIN_STORAGE, PHONE_STORAGE};
use serde_json::json;

use crate::Session;

#[test]
fn a_computer_has_none_until_they_are_set_and_off_takes_them_away() {
    let mut s = Session::empty();
    let said = s.run("account.deviceLimits", &json!({})).unwrap();
    assert_eq!(said["on"], false);
    assert!(s.mailbox.device_limits.is_none(), "reading changes nothing");
    let said = s.run("account.deviceLimits", &json!({"messages": 3000})).unwrap();
    assert_eq!((said["on"].clone(), said["storage"].clone(), said["messages"].clone()), (json!(true), json!(PHONE_STORAGE), json!(3000)));
    s.run("account.deviceLimits", &json!({"storage": MIN_STORAGE})).unwrap();
    assert_eq!(s.mailbox.device_limits.unwrap().storage, MIN_STORAGE);
    assert_eq!(s.mailbox.device_limits.unwrap().messages, 3000, "what isn't named stays");
    assert_eq!(s.run("account.deviceLimits", &json!({"off": true})).unwrap()["on"], false);
    assert!(s.mailbox.device_limits.is_none());
}

#[test]
fn values_out_of_bounds_are_refused_and_change_nothing() {
    let mut s = Session::empty();
    s.run("account.deviceLimits", &json!({"storage": PHONE_STORAGE})).unwrap();
    let before = s.mailbox.device_limits;
    for bad in [
        json!({"storage": 1}),
        json!({"storage": u64::MAX}),
        json!({"storage": "500"}),
        json!({"messages": 10}),
        json!({"messages": -1}),
        json!({"defaults": 1.5}),
        json!({"off": "yes"}),
        json!([1, 2]),
    ] {
        assert!(s.run("account.deviceLimits", &bad).is_err(), "{bad}");
        assert_eq!(s.mailbox.device_limits, before, "{bad}");
    }
}

#[test]
fn account_edit_takes_the_one_megabyte_step() {
    let mut s = Session::empty();
    let id = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("account.edit", &json!({"id": id, "downloadInFull": "upTo1Mb"})).unwrap();
    assert_eq!(s.mailbox.account(id).unwrap().download_in_full, mailcraft_model::DownloadInFull::UpTo1Mb);
}

/// 2027-01-15.
pub(super) const NOW: i64 = 1_800_000_000;

/// A session at [`NOW`] with a Microsoft account keeping a month, whose Inbox the server has.
pub(super) fn graph_session() -> (Session, u64, u64) {
    use mailcraft_model::{FolderKind, Mailbox, ServerFolder};
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("account.edit", &json!({"id": account, "keepOffline": "1m"})).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder {
        id: "in".into(),
        delta: "https://graph.microsoft.com/v1.0/x?$deltatoken=1".into(),
        caught_up: true,
        ..Default::default()
    });
    (s, account, inbox)
}

#[test]
fn only_a_phone_asks_its_server_for_the_windows_mail_alone() {
    let (mut s, account, _) = graph_session();
    let request = crate::cmd::graph_mail::graph_mail_request(&s, account).unwrap();
    assert_eq!(request.since, None, "a computer lists everything");
    s.run("account.deviceLimits", &json!({"storage": PHONE_STORAGE})).unwrap();
    let request = crate::cmd::graph_mail::graph_mail_request(&s, account).unwrap();
    assert_eq!(request.since, mailcraft_model::KeepOffline::Month1.since(NOW));
    assert_eq!(crate::cmd::graph_mail::graph_inbox_request(&s, account).unwrap().since, request.since);
}

#[test]
fn a_longer_window_on_a_phone_lists_the_folders_afresh_and_a_shorter_one_does_not() {
    let (mut s, account, inbox) = graph_session();
    let relisting = |s: &Session| s.mailbox.folder(inbox).unwrap().server.as_ref().unwrap().relist;
    s.run("account.edit", &json!({"id": account, "keepOffline": "3m"})).unwrap();
    assert!(!relisting(&s), "a computer has listed everything already");
    s.run("account.deviceLimits", &json!({"storage": PHONE_STORAGE})).unwrap();
    s.run("account.edit", &json!({"id": account, "keepOffline": "1w"})).unwrap();
    assert!(!relisting(&s));
    s.run("account.edit", &json!({"id": account, "keepOffline": "6m"})).unwrap();
    assert!(relisting(&s));
    let request = crate::cmd::graph_mail::graph_mail_request(&s, account).unwrap();
    let link = request.links.iter().find(|l| l.folder == "in").unwrap();
    assert!(link.afresh && link.link.is_empty(), "the old link is never followed");
    assert!(crate::cmd::graph_mail::graph_inbox_request(&s, account).is_none(), "the whole run lists it");
    // A page of the new listing taken in: it goes on from there.
    let page = mailcraft_model::sync::MessagePage {
        folder: "in".into(),
        link: "https://graph.microsoft.com/v1.0/x?$deltatoken=2".into(),
        afresh: true,
        relist_gen: link.relist_gen,
        caught_up: true,
        ..Default::default()
    };
    let mut index = crate::cmd::graph_down::ServerIndex::default();
    crate::cmd::graph_down::take_page(&mut s.mailbox, &mut index, account, &page, &mut Default::default());
    assert!(!relisting(&s));
}

const DAY: i64 = 86_400;

/// A message of `account` in `folder` (the server's "in" unless it is another), received
/// `days_ago`, with `body` bytes of text; its id.
pub(super) fn add(s: &mut Session, account: u64, folder: u64, days_ago: i64, body: usize) -> u64 {
    use mailcraft_model::sync::ServerMessage;
    use mailcraft_model::{Body, Message};
    let server_folder = s.mailbox.folder(folder).and_then(|f| f.server.as_ref()).map_or("in".to_string(), |f| f.id.clone());
    let server = ServerMessage { id: format!("M{}", s.mailbox.messages.len() + 1), folder: server_folder, ..Default::default() };
    let body = Body { text: "x".repeat(body), html: None };
    s.mailbox.add_message(Message { account, folder, received: NOW - days_ago * DAY, server: Some(server), body, ..Default::default() }).unwrap()
}

#[test]
fn mail_older_than_the_window_leaves_a_phone_but_never_what_the_server_lacks() {
    use mailcraft_model::FolderKind;
    let (mut s, account, inbox) = graph_session();
    let recent = add(&mut s, account, inbox, 2, 10);
    let old = add(&mut s, account, inbox, 60, 10);
    let read_here = add(&mut s, account, inbox, 60, 10);
    s.mailbox.message_mut(read_here).unwrap().read = true; // the server still has it unread
    let drafts = s.mailbox.folder_of_kind(account, FolderKind::Drafts).unwrap();
    let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    let draft = add(&mut s, account, drafts, 60, 10);
    let waiting = add(&mut s, account, outbox, 60, 10);
    let mut unsent = mailcraft_model::Message { account, folder: inbox, received: NOW - 90 * DAY, ..Default::default() };
    unsent.subject = "only here".into();
    let unsent = s.mailbox.add_message(unsent).unwrap();
    // A computer keeps it all.
    let said = s.run("account.trimDevice", &json!({})).unwrap();
    assert_eq!(said["dropped"], 0);
    s.run("account.deviceLimits", &json!({"storage": PHONE_STORAGE})).unwrap();
    let said = s.run("account.trimDevice", &json!({})).unwrap();
    assert_eq!((said["dropped"].clone(), said["thinned"].clone()), (json!(1), json!(0)));
    assert!(s.mailbox.message(old).is_none());
    for kept in [recent, read_here, draft, waiting, unsent] {
        assert!(s.mailbox.message(kept).is_some(), "{kept}");
    }
    // Nothing goes to the server for it.
    assert!(s.mailbox.queued.is_empty());
    assert_eq!(crate::cmd::graph_up::message_changes(&s, account, 100).0.len(), 1, "only the read state waits to go");
}

#[test]
fn past_the_cap_of_messages_the_oldest_leave() {
    let (mut s, account, inbox) = graph_session();
    s.run("account.edit", &json!({"id": account, "keepOffline": "all"})).unwrap();
    s.run("account.deviceLimits", &json!({"messages": 2000})).unwrap();
    let ids: Vec<u64> = (0..2003).map(|n| add(&mut s, account, inbox, 2003 - n, 0)).collect();
    let said = s.run("account.trimDevice", &json!({"most": 2})).unwrap();
    assert_eq!((said["dropped"].clone(), said["left"].clone()), (json!(2), json!(1)), "a batch at a time");
    s.run("account.trimDevice", &json!({})).unwrap();
    assert!(ids[..3].iter().all(|id| s.mailbox.message(*id).is_none()), "the three oldest");
    assert!(ids[3..].iter().all(|id| s.mailbox.message(*id).is_some()));
    assert_eq!(s.mailbox.messages.len(), 2000);
}

#[test]
fn past_the_cap_of_storage_the_oldest_go_back_to_their_headers_first() {
    let (mut s, account, inbox) = graph_session();
    s.run("account.deviceLimits", &json!({"storage": MIN_STORAGE})).unwrap();
    let mb = 1 << 20;
    let ids: Vec<u64> = [20, 15, 10, 5].iter().map(|d| add(&mut s, account, inbox, *d, 20 * mb)).collect();
    let said = s.run("account.trimDevice", &json!({})).unwrap();
    assert_eq!((said["thinned"].clone(), said["dropped"].clone()), (json!(2), json!(0)));
    assert!(said["bytes"].as_u64().unwrap() <= MIN_STORAGE);
    let len = |id: u64| s.mailbox.message(id).unwrap().body.text.len();
    assert!(len(ids[0]) < 1000 && len(ids[1]) < 1000, "the two oldest are headers and preview");
    assert_eq!((len(ids[2]), len(ids[3])), (20 * mb, 20 * mb));
}

#[test]
fn the_message_shown_never_leaves() {
    let (mut s, account, inbox) = graph_session();
    let old = add(&mut s, account, inbox, 60, 10);
    s.sel.messages = vec![old];
    s.run("account.deviceLimits", &json!({"storage": PHONE_STORAGE})).unwrap();
    s.run("account.trimDevice", &json!({})).unwrap();
    assert!(s.mailbox.message(old).is_some());
}

#[test]
fn a_page_of_an_earlier_listing_never_meets_a_later_request_to_list_afresh() {
    use mailcraft_model::sync::MessagePage;
    let (mut s, account, inbox) = graph_session();
    s.run("account.deviceLimits", &json!({"storage": PHONE_STORAGE})).unwrap();
    let relisting = |s: &Session| s.mailbox.folder(inbox).unwrap().server.as_ref().unwrap().relist;
    let take = |s: &mut Session, page: &MessagePage| {
        let mut index = crate::cmd::graph_down::ServerIndex::default();
        crate::cmd::graph_down::take_page(&mut s.mailbox, &mut index, account, page, &mut Default::default());
    };
    let page = |afresh: bool, relist_gen: u32| MessagePage {
        folder: "in".into(),
        link: "https://graph.microsoft.com/v1.0/x?$skiptoken=2".into(),
        afresh,
        relist_gen,
        ..Default::default()
    };
    // A month's sync is under way; the window is made six months before its next page comes.
    s.run("account.edit", &json!({"id": account, "keepOffline": "6m"})).unwrap();
    assert!(relisting(&s));
    take(&mut s, &page(false, 0));
    assert!(relisting(&s), "a page that followed a link says nothing of the new request");
    // The first page of a listing afresh begun for an earlier request.
    let first = s.mailbox.folder(inbox).unwrap().server.as_ref().unwrap().relist_gen;
    s.run("account.edit", &json!({"id": account, "keepOffline": "12m"})).unwrap();
    assert!(relisting(&s));
    take(&mut s, &page(true, first));
    assert!(relisting(&s), "nor does one made for the request before it");
    let now = s.mailbox.folder(inbox).unwrap().server.as_ref().unwrap().relist_gen;
    take(&mut s, &page(true, now));
    assert!(!relisting(&s));
}

#[test]
fn a_device_with_no_limits_starts_from_the_phones_when_one_is_set() {
    let mut s = Session::empty();
    s.run("account.deviceLimits", &json!({"storage": MIN_STORAGE})).unwrap();
    let l = s.mailbox.device_limits.unwrap();
    assert_eq!(l.defaults, mailcraft_model::PHONE_DEFAULTS, "not moved to the defaults again at the next open");
    assert_eq!(l.messages, mailcraft_model::DeviceLimits::phone(None).messages);
}

#[test]
fn a_message_with_a_change_waiting_stays_however_many_changes_wait() {
    let (mut s, account, inbox) = graph_session();
    // One more than a request carries: the last in line is as safe as the first.
    let ids: Vec<u64> = (0..=mailcraft_model::sync::MAX_MESSAGE_CHANGES)
        .map(|_| {
            let id = add(&mut s, account, inbox, 60, 0);
            s.mailbox.message_mut(id).unwrap().read = true; // the server still has it unread
            id
        })
        .collect();
    s.run("account.deviceLimits", &json!({"storage": PHONE_STORAGE})).unwrap();
    let said = s.run("account.trimDevice", &json!({})).unwrap();
    assert_eq!(said["dropped"], 0);
    assert!(ids.iter().all(|id| s.mailbox.message(*id).is_some()));
}

#[test]
fn thinning_counts_only_what_it_releases_so_the_cap_is_met() {
    let (mut s, account, inbox) = graph_session();
    s.run("account.edit", &json!({"id": account, "keepOffline": "all"})).unwrap();
    s.run("account.deviceLimits", &json!({"storage": MIN_STORAGE})).unwrap();
    // Many short bodies, each keeping most of its text as the preview, and a recent big one that
    // takes the mailbox 10,000 bytes over the cap.
    let small = 100;
    for n in 0..small {
        add(&mut s, account, inbox, 30 + n, 1000);
    }
    let held: u64 = s.mailbox.messages.values().map(|m| super::device_limits::local_bytes(m)).sum();
    let big = usize::try_from(MIN_STORAGE + 10_000 - held - super::device_limits::HEADER_BYTES).unwrap();
    add(&mut s, account, inbox, 1, big);
    let said = s.run("account.trimDevice", &json!({})).unwrap();
    assert!(said["thinned"].as_u64().unwrap() >= 14, "{said}");
    assert!(said["bytes"].as_u64().unwrap() <= MIN_STORAGE, "{said}");
}
