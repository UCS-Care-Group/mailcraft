// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Rules dialog and where a rule is kept (#566): the box for the server, and Always Move
//! Messages From This Sender filling the new rule in and adding it only when asked.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::*;
use crate::dialogs::Dialog;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn session() -> Session {
    Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0)
}

fn app() -> MailApp {
    let mut app = MailApp::new(session(), Services::default());
    app.autosave = false;
    app
}

/// The whole window, laid out a few frames so the dialog is up.
fn window(app: MailApp) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(6);
    harness
}

#[test]
fn the_owner_says_where_a_new_rule_is_kept() {
    let mut s = session();
    let noor = s.run("account.add", &json!({"email": "noor@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let mut f = RulesForm::new(&s, &json!({}));
    f.choose(&s.mailbox, noor);
    let fill = |f: &mut RulesForm, kind: Do, folder: Option<Id>| {
        f.name = "Club".into();
        f.conds[0].text = "news@club.example".into();
        f.acts[0] = Act { kind, folder: folder.unwrap_or(f.first_folder), ..f.fresh_action() };
    };
    // Ticked to start with, and the server can keep it: the command leaves it there.
    fill(&mut f, Do::MarkRead, None);
    assert!(f.server);
    let params = f.command().1;
    assert!(params.get("server").is_none());
    assert_eq!(server_cannot_draft(&s, &params), None);
    // Unticked, it stays on this computer.
    f.server = false;
    let (command, params) = f.command();
    assert_eq!(params["server"], false);
    assert_eq!(s.run(command, &params).unwrap()["keep"], "here");
    f.reset();
    assert!(f.server);
    // What the server can't do greys it, and the rule is made here: a flag, or a move to
    // another account's folder, which the list names with its account.
    fill(&mut f, Do::Flag, None);
    assert_eq!(server_cannot_draft(&s, &f.command().1), Some(NotOnServer::Flag));
    assert!(f.folders.first().is_some_and(|(id, _)| s.mailbox.folder(*id).is_some_and(|x| x.account == noor)));
    let (elsewhere, label) = f.folders.iter().find(|(id, _)| s.mailbox.folder(*id).is_some_and(|x| x.account != noor)).cloned().unwrap();
    let theirs = s.mailbox.account(s.mailbox.folder(elsewhere).unwrap().account).unwrap().email.clone();
    assert!(label.ends_with(&format!(" - {theirs}")), "{label}");
    fill(&mut f, Do::Move, Some(elsewhere));
    assert!(server_cannot_draft(&s, &f.command().1).is_some());
    let (command, params) = f.command();
    let made = s.run(command, &params).unwrap();
    assert_eq!(made["keep"], "here");
    let rule = s.mailbox.rules.iter().find(|r| r.id == made["id"].as_u64().unwrap()).unwrap().clone();
    assert!(describe(&s.mailbox, &rule).ends_with(&format!(" - {theirs}")), "{}", describe(&s.mailbox, &rule));
    // An account without an address is named by its name.
    let theirs = s.mailbox.folder(elsewhere).unwrap().account;
    let account = s.mailbox.accounts.iter_mut().find(|a| a.id == theirs).unwrap();
    (account.email, account.name) = (String::new(), "Archive here".into());
    assert!(describe(&s.mailbox, &rule).ends_with(" - Archive here"), "{}", describe(&s.mailbox, &rule));
}

#[test]
fn always_move_messages_from_this_sender_fills_the_new_rule_in() {
    use mailcraft_model::{Address, Message};
    let mut s = session();
    let noor = s.run("account.add", &json!({"email": "noor@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(noor, FolderKind::Inbox).unwrap();
    let (to, _) = folder_choices(&s.mailbox, noor).into_iter().find(|(id, _)| *id != inbox).unwrap();
    s.mailbox.folder_mut(to).unwrap().server = Some(mailcraft_model::ServerFolder { id: "news".into(), ..Default::default() });
    let from = Address::new("Weekly Digest", "digest@news.example");
    let m = s.mailbox.add_message(Message { folder: inbox, from, subject: "This week".into(), ..Default::default() }).unwrap();
    s.sel.messages = vec![m];
    let f = RulesForm::new(&s, &json!({"folder": to}));
    // Unfolded, for the account the message came to, with the box ticked.
    let to_name = s.mailbox.folder(to).unwrap().name.clone();
    assert!(f.open && f.server && f.then_run);
    assert_eq!((f.account, f.name.clone()), (noor, format!("Weekly Digest to {to_name}")));
    assert_eq!(f.conds, [Cond { kind: When::From, text: "digest@news.example".into(), ..Default::default() }]);
    assert_eq!((f.acts.len(), f.acts[0].kind, f.acts[0].folder), (1, Do::Move, to));
    assert_eq!(f.run_in.as_ref().map(|(id, _)| *id), Some(inbox));
    // Another account picked in Rules for: the mail already in the folder is the first's.
    let mut other = f.clone();
    let (elsewhere, _) = rule_accounts(&s.mailbox).into_iter().find(|(id, _)| *id != noor).unwrap();
    other.choose(&s.mailbox, elsewhere);
    assert!(other.run_in.is_none() && !other.then_run);
    // Nothing is made until Add Rule, and then on the server, which can keep it.
    assert!(s.mailbox.rules.iter().all(|r| r.account != noor));
    let (command, params) = f.command();
    assert_eq!(server_cannot_draft(&s, &params), None);
    assert_eq!(s.run(command, &params).unwrap()["keep"], "server");
    // A folder or a message that isn't there, or a send queue, leaves the dialog as it opens.
    let plain = RulesForm::new(&s, &json!({}));
    let outbox = s.mailbox.folder_of_kind(noor, FolderKind::Outbox).unwrap();
    for p in [json!({"folder": 99_999}), json!({"folder": to, "id": 99_999}), json!({"folder": outbox})] {
        assert_eq!(RulesForm::new(&s, &p), plain, "{p}");
    }
}

#[test]
fn always_move_messages_from_this_sender_adds_the_rule_only_when_asked_then_runs_it() {
    let mut a = app();
    let inbox = a.session.view.folder;
    let mb = &a.session.mailbox;
    let m = mb.messages_in(inbox).find(|m| !m.from.email.is_empty()).map(|m| m.id).unwrap();
    let sender = mb.message(m).unwrap().from.email.clone();
    let account = mb.message(m).unwrap().account;
    let to = mb.folder_of_kind(account, mailcraft_model::FolderKind::Archive).unwrap();
    let before = mb.rules.len();
    a.session.sel.messages = vec![m];
    a.execute("ui.dialog", json!({"name": "rules", "folder": to})).unwrap();
    let mut h = window(a);
    assert_eq!(h.state().session.mailbox.rules.len(), before, "nothing is made before Add Rule");
    assert!(h.query_by_label("Then run it over the mail already in Inbox").is_some());
    h.get_by_label("Add Rule").click();
    h.run_steps(4);
    let mb = &h.state().session.mailbox;
    let rule = mb.rules.last().unwrap();
    assert_eq!(rule.conditions, [mailcraft_model::Condition::FromContains(sender)]);
    assert_eq!(rule.actions, [mailcraft_model::Action::MoveTo(to)]);
    // Then the rules ran over the Inbox, as the menu said: the message is there now.
    assert_eq!(mb.message(m).unwrap().folder, to);
    assert_eq!(h.state().dialog.as_ref().map(Dialog::name), Some("rules"));
}

/// #702: the dialog lists the accounts in the order the owner put them in the folder pane.
#[test]
fn the_accounts_are_listed_in_the_owners_order() {
    let mut s = session();
    let first = s.mailbox.accounts[0].id;
    let other = s.run("account.add", &serde_json::json!({"email": "other@example.org"})).unwrap()["id"].as_u64().unwrap();
    let ids = |s: &Session| rule_accounts(&s.mailbox).into_iter().map(|(id, _)| id).collect::<Vec<_>>();
    assert_eq!(ids(&s), [first, other]);
    s.run("account.move", &serde_json::json!({"id": other, "before": first})).unwrap();
    assert_eq!(ids(&s), [other, first]);
}

/// #702: with a shared mailbox's folder in view (it has no rules of its own), the dialog opens
/// on the first account added, wherever the owner moved the accounts.
#[test]
fn with_no_account_in_view_the_dialog_opens_on_the_first_added() {
    let mut s = session();
    let first = s.mailbox.accounts[0].id;
    let work = s.run("account.add", &serde_json::json!({"email": "work@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let team =
        s.run("account.addSharedMailbox", &serde_json::json!({"account": work, "address": "team@example.org"})).unwrap()["id"].as_u64().unwrap();
    s.view.folder = s.mailbox.folders.iter().find(|f| f.account == team).unwrap().id;
    s.run("account.move", &serde_json::json!({"id": work, "before": first})).unwrap();
    assert_eq!(rule_accounts(&s.mailbox).first().map(|(id, _)| *id), Some(work), "listed first");
    assert_eq!(account_in_view(&s), first);
}
