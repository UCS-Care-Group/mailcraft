// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail to keep offline (#143): the setting, which mail is within it, the background fetch
//! within it, and mail outside it going back to its headers and preview.

use mailcraft_model::{Id, KeepOffline};
use serde_json::json;

use crate::Session;

/// A session with a local account and a Microsoft account; their ids.
fn session() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let owner = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    (s, local, owner)
}

#[test]
fn a_new_account_keeps_a_year_and_account_edit_changes_it() {
    let (mut s, _, owner) = session();
    assert_eq!(s.mailbox.account(owner).unwrap().keep_offline, KeepOffline::Months12);
    s.run("account.edit", &json!({"id": owner, "keepOffline": "3m"})).unwrap();
    assert_eq!(s.mailbox.account(owner).unwrap().keep_offline, KeepOffline::Months3);
    let listed = s.run("account.list", &json!({})).unwrap();
    let mine = listed["accounts"].as_array().unwrap().iter().find(|a| a["id"] == owner).unwrap().clone();
    assert_eq!(mine["keepOffline"], "3m");
    for nonsense in [json!("forever"), json!(3), json!(null), json!(""), json!("12 months")] {
        let before = s.mailbox.account(owner).unwrap().keep_offline;
        let r = s.run("account.edit", &json!({"id": owner, "keepOffline": nonsense}));
        assert!(r.is_err() || !nonsense.is_string(), "{nonsense}");
        assert_eq!(s.mailbox.account(owner).unwrap().keep_offline, before, "{nonsense}");
    }
    // Undo puts the setting back.
    s.run("account.edit", &json!({"id": owner, "keepOffline": "all"})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.account(owner).unwrap().keep_offline, KeepOffline::Months3);
}

/// 2027-01-15.
const NOW: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

/// A session at [`NOW`] with a Microsoft account keeping `keep`, whose Inbox (on the server) has
/// a message whole received each of `days_ago`, of a few kilobytes; its id and theirs.
fn aged(keep: &str, days_ago: &[i64]) -> (Session, Id, Vec<Id>) {
    use mailcraft_model::sync::ServerMessage;
    use mailcraft_model::{Body, FolderKind, Mailbox, Message, ServerFolder};
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("account.edit", &json!({"id": account, "keepOffline": keep})).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "in".into(), ..Default::default() });
    let ids = days_ago
        .iter()
        .map(|d| {
            let server = ServerMessage { id: format!("M{d}"), folder: "in".into(), ..Default::default() };
            let body = Body { text: format!("Received {d} days ago. ").repeat(100), html: None };
            s.mailbox
                .add_message(Message { account, folder: inbox, received: NOW - d * DAY, server: Some(server), body, ..Default::default() })
                .unwrap()
        })
        .collect();
    (s, account, ids)
}

#[test]
fn the_space_a_shorter_window_frees_is_said_before_anything_goes() {
    let (mut s, account, ids) = aged("all", &[1, 20, 100, 400]);
    let size = |s: &mut Session, keep: &str| s.run("account.offlineSize", &json!({"account": account, "keepOffline": keep})).unwrap();
    let a = size(&mut s, "1m");
    assert_eq!(a["messages"], 2);
    assert_eq!(a["window"], "1 month");
    let bytes = a["bytes"].as_u64().unwrap();
    assert!(bytes > 2 * 2_000, "{a}");
    assert!(a["size"].as_str().unwrap().ends_with("KB"), "{a}");
    assert_eq!(size(&mut s, "3d")["messages"], 3);
    assert_eq!(size(&mut s, "all")["messages"], 0);
    assert!(ids.iter().all(|id| !s.mailbox.message(*id).unwrap().unfetched), "nothing went");
    assert!(s.run("account.offlineSize", &json!({"account": account, "keepOffline": "soon"})).is_err());
    assert!(s.run("account.offlineSize", &json!({"keepOffline": "1m"})).is_err());
}

#[test]
fn mail_ages_out_with_time_by_its_received_date() {
    let (mut s, account, ids) = aged("1m", &[1, 20, 40]);
    let whole = |s: &Session| ids.iter().map(|id| !s.mailbox.message(*id).unwrap().unfetched).collect::<Vec<_>>();
    let thin = |s: &mut Session| s.run("account.thinOffline", &json!({"account": account})).unwrap();
    let said = thin(&mut s);
    assert_eq!((said["thinned"].as_u64(), said["left"].as_u64()), (Some(1), Some(0)), "{said}");
    assert_eq!(whole(&s), [true, true, false]);
    // Not an undo step: an undo is of the window's change, and brings no content back.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.keep_offline_of(account), KeepOffline::Months12);
    assert_eq!(whole(&s), [true, true, false]);
    s.run("edit.redo", &json!({})).unwrap();
    // Fifteen days on, the twenty-day-old one is past a month.
    s.clock.now = Some(NOW + 15 * DAY);
    assert_eq!(thin(&mut s)["thinned"], 1);
    assert_eq!(whole(&s)[..2], [true, false]);
    assert_eq!(thin(&mut s)["thinned"], 0, "nothing twice");
    // At most `most` at a time; the rest is said.
    let (mut s, _, _) = aged("3d", &[10, 11, 12, 13, 14]);
    let said = s.run("account.thinOffline", &json!({"most": 2})).unwrap();
    assert_eq!((said["thinned"].as_u64(), said["left"].as_u64()), (Some(2), Some(3)));
    assert_eq!(s.run("account.thinOffline", &json!({})).unwrap()["thinned"], 3);
}

#[test]
fn a_shared_mailbox_has_no_setting_of_its_own() {
    let (mut s, _, owner) = session();
    let shared = s.run("account.addSharedMailbox", &json!({"account": owner, "address": "support@example.org"})).unwrap()["id"].as_u64().unwrap();
    s.run("account.edit", &json!({"id": owner, "keepOffline": "1w"})).unwrap();
    assert_eq!(s.mailbox.keep_offline_of(shared), KeepOffline::Week1);
    let refused = s.run("account.edit", &json!({"id": shared, "keepOffline": "all"})).unwrap_err().to_string();
    assert!(refused.contains("opened through"), "{refused}");
    let listed = s.run("account.list", &json!({})).unwrap();
    let it = listed["accounts"].as_array().unwrap().iter().find(|a| a["id"] == shared).unwrap().clone();
    assert_eq!(it["keepOffline"], "1w", "as its owner's");
}

/// An Exchange server account keeps its mail to keep offline too (#966): only mail within it is
/// fetched whole in the background, older mail that is here whole goes back to its headers and
/// preview, and the window's commands take it.
#[test]
fn an_exchange_server_account_fetches_and_keeps_only_its_window() {
    use mailcraft_model::sync::ServerMessage;
    use mailcraft_model::{Body, FolderKind, Mailbox, Message, ServerFolder};
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let ews = json!({"url": "https://exchange.example.org/EWS/Exchange.asmx"});
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "exchange", "ews": ews})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.account(account).unwrap().keep_offline, KeepOffline::Months12);
    s.run("account.edit", &json!({"id": account, "keepOffline": "1m"})).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "in".into(), ..Default::default() });
    let add = |s: &mut Session, d: i64, whole: bool| {
        let server = ServerMessage { id: format!("E{d}"), folder: "in".into(), ..Default::default() };
        let body = Body { text: format!("Received {d} days ago. ").repeat(100), html: None };
        let m = Message { account, folder: inbox, received: NOW - d * DAY, server: Some(server), body, unfetched: !whole, ..Default::default() };
        s.mailbox.add_message(m).unwrap()
    };
    // Headers and a preview only: the background fetches the one within the month, not the rest.
    for d in [1, 100, 400] {
        add(&mut s, d, false);
    }
    let asked = crate::cmd::ews_mail::ews_fetch_request(&s, account, None, 100).unwrap();
    assert_eq!(asked.ids, vec!["E1".to_owned()]);
    // Here whole from before: what is outside the month goes back to its headers and preview.
    let old = add(&mut s, 200, true);
    let new = add(&mut s, 2, true);
    let size = s.run("account.offlineSize", &json!({"account": account, "keepOffline": "1m"})).unwrap();
    assert_eq!(size["messages"], 1, "{size}");
    assert_eq!(s.run("account.thinOffline", &json!({"account": account})).unwrap()["thinned"], 1);
    assert!(s.mailbox.message(old).unwrap().unfetched);
    assert!(!s.mailbox.message(new).unwrap().unfetched);
}
