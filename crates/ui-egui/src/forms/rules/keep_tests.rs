// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Rules dialog (#566): the box for the server, Always Move Messages From This Sender filling
//! the new rule in and adding it only when asked, and the new conditions, exceptions and actions.

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

fn folder_id(s: &Session, path: &str) -> Id {
    s.mailbox.folders.iter().find(|f| s.mailbox.folder_path(f.id) == path).map(|f| f.id).unwrap()
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
    // From that sender, by the whole address (#566).
    assert_eq!(f.conds, [Cond { kind: When::Person, text: "digest@news.example".into(), ..Default::default() }]);
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
    // A sender's that isn't one whole address is looked for as words, as the command does,
    // and the rule can be added as it is filled in.
    let odd = Message { folder: inbox, from: Address::new("", "noreply"), subject: "x".into(), ..Default::default() };
    let odd = s.mailbox.add_message(odd).unwrap();
    let f = RulesForm::new(&s, &json!({"folder": to, "id": odd}));
    assert_eq!(f.conds, [Cond { kind: When::From, text: "noreply".into(), ..Default::default() }]);
    assert_eq!(f.lacks(), None);
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
    assert_eq!(rule.conditions, [mailcraft_model::Condition::From(vec![sender])]);
    assert_eq!(rule.actions, [mailcraft_model::Action::MoveTo(to)]);
    // Then the rules ran over the Inbox, as the menu said: the message is there now.
    assert_eq!(mb.message(m).unwrap().folder, to);
    assert_eq!(h.state().dialog.as_ref().map(Dialog::name), Some("rules"));
}

#[test]
fn what_each_new_part_needs_is_said() {
    let s = session();
    let mut f = RulesForm::new(&s, &json!({}));
    f.name = "News".into();
    let needs = |f: &RulesForm| f.lacks().unwrap_or("");
    // People are whole addresses, any one of them.
    f.conds[0] = Cond { kind: When::Person, text: "news@club.example; Noor".into(), ..Default::default() };
    assert!(needs(&f).contains("whole addresses"), "{}", needs(&f));
    f.conds[0].text = "news@club.example; noor@example.org".into();
    assert_eq!(f.lacks(), None);
    // Two sizes, the smaller first.
    f.conds[0] = Cond { kind: When::Size, min: "500".into(), max: "20".into(), ..Default::default() };
    assert!(needs(&f).contains("sizes in KB"), "{}", needs(&f));
    f.conds[0].max = "2147483648".into();
    assert!(needs(&f).contains("sizes in KB"), "more than a server keeps");
    f.conds[0].max = "2000".into();
    assert_eq!(f.lacks(), None);
    // One day or two, the earlier first.
    f.conds[0] = Cond { kind: When::Received, ..Default::default() };
    assert!(needs(&f).contains("one day or two"));
    f.conds[0].to = "2026-10-01".into();
    assert_eq!(f.lacks(), None);
    f.conds[0].from = "2026-10-02".into();
    assert!(needs(&f).contains("the earlier first"));
    f.conds[0].from = "yesterday".into();
    assert!(needs(&f).contains("one day or two"));
    f.conds[0].from = "2026-09-01".into();
    assert_eq!(f.lacks(), None);
    // A category to look for.
    f.conds[0] = Cond { kind: When::Category, ..Default::default() };
    assert!(f.lacks().is_some());
    f.conds[0].category = "Red category".into();
    assert_eq!(f.lacks(), None);
    // An exception needs what a condition does.
    f.unless = vec![Cond { kind: When::Subject, ..Default::default() }];
    assert!(needs(&f).contains("condition and exception"), "{}", needs(&f));
    f.unless[0].text = "unsubscribe".into();
    assert_eq!(f.lacks(), None);
    // Something to do: an action, or the alert alone.
    f.acts.clear();
    assert!(needs(&f).contains("something to do"));
    f.alert = true;
    assert_eq!(f.lacks(), None);
    // A copy needs a folder, as a move does.
    f.acts = vec![Act { kind: Do::Copy, folder: 0, ..f.fresh_action() }];
    assert!(needs(&f).contains("need a folder"), "{}", needs(&f));
    // And a name.
    f.acts[0].folder = folder_id(&s, "Receipts");
    assert_eq!(f.lacks(), None);
    f.name = " ".into();
    assert!(needs(&f).contains("a name"), "{}", needs(&f));
}

#[test]
fn a_rule_looks_for_no_more_words_than_rules_add_takes_and_the_box_for_the_server_says_so_of_the_rest() {
    // The reason says 20.
    assert_eq!(MAX_WORDS_EACH, 20);
    let mut s = session();
    let noor = s.run("account.add", &json!({"email": "noor@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let mut f = RulesForm::new(&s, &json!({}));
    f.choose(&s.mailbox, noor);
    f.name = "News".into();
    let words = |from: usize, to: usize| (from..to).map(|n| format!("w{n}")).collect::<Vec<_>>().join("; ");
    // A condition.
    f.conds[0] = Cond { kind: When::Subject, text: words(0, 21), ..Default::default() };
    assert_eq!(f.lacks(), Some(too_many_words()));
    f.conds[0].text = words(0, 20);
    assert_eq!(f.lacks(), None);
    // Exceptions of one kind are one: a word both look for counts once, and other kinds apart.
    f.unless = vec![
        Cond { kind: When::Body, text: words(0, 12), ..Default::default() },
        Cond { kind: When::Body, text: words(4, 20).to_uppercase(), ..Default::default() },
        Cond { kind: When::Subject, text: words(30, 45), ..Default::default() },
    ];
    assert_eq!(f.lacks(), None);
    let (command, params) = f.command();
    s.run(command, &params).unwrap();
    f.unless[1].text = words(4, 21);
    assert_eq!(f.lacks(), Some(too_many_words()));
    let (command, params) = f.command();
    assert!(s.run(command, &params).is_err(), "as `rules.add` refuses it");
    // Meanwhile the box for the server judges the exceptions as they are, never as none: a name
    // the server can't look for is said.
    f.unless = vec![
        Cond { kind: When::From, text: format!("{}; Noor Haddad", words(0, 12)), ..Default::default() },
        Cond { kind: When::From, text: words(12, 21), ..Default::default() },
    ];
    assert_eq!(f.lacks(), Some(too_many_words()));
    assert_eq!(server_cannot_draft(&s, &f.command().1), Some(NotOnServer::Names));
}

#[test]
fn the_new_conditions_exceptions_and_actions_are_spelled_out_and_made() {
    let mut s = session();
    let mut f = RulesForm::new(&s, &json!({}));
    let receipts = folder_id(&s, "Receipts");
    f.name = "Club".into();
    f.conds = vec![
        Cond { kind: When::Person, text: "news@club.example".into(), ..Default::default() },
        Cond { kind: When::SubjectOrBody, text: "meeting; agenda".into(), ..Default::default() },
        Cond { kind: When::ToOrCcMe, ..Default::default() },
        Cond { kind: When::Size, min: "0".into(), max: "512".into(), ..Default::default() },
        Cond { kind: When::Received, from: "2026-10-01".into(), ..Default::default() },
    ];
    f.unless = vec![
        Cond { kind: When::Sensitivity, sensitivity: Sensitivity::Private, ..Default::default() },
        Cond { kind: When::Category, category: "Red category".into(), ..Default::default() },
        Cond { kind: When::Header, text: "X-Spam-Flag: YES".into(), ..Default::default() },
    ];
    f.acts = vec![
        Act { kind: Do::Copy, folder: receipts, ..f.fresh_action() },
        Act { kind: Do::Flag, due: Some(FollowUp::Tomorrow), ..f.fresh_action() },
        Act { kind: Do::ClearCategories, ..f.fresh_action() },
    ];
    f.alert = true;
    assert_eq!(f.lacks(), None);
    let (command, params) = f.command();
    assert_eq!(
        params,
        json!({
            "name": "Club",
            "account": f.account,
            "conditions": [
                {"type": "from", "value": "news@club.example"}, {"type": "subjectOrBodyContains", "value": ["meeting", "agenda"]},
                {"type": "toOrCcMe"}, {"type": "sizeBetween", "value": {"min": 0, "max": 512}},
                {"type": "receivedBetween", "value": {"from": "2026-10-01", "to": null}},
            ],
            "exceptions": [
                {"type": "sensitivity", "value": "private"}, {"type": "category", "value": "Red category"},
                {"type": "headerContains", "value": "X-Spam-Flag: YES"},
            ],
            "actions": [{"type": "copyTo", "value": receipts}, {"type": "flagFor", "value": "tomorrow"}, {"type": "clearCategories"}],
            "stop": false,
            "alert": true,
        })
    );
    let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
    let rule = s.mailbox.rules.iter().find(|r| r.id == id).unwrap();
    assert!(rule.alert);
    assert_eq!(rule.exceptions.len(), 3);
    assert_eq!(
        describe(&s.mailbox, rule),
        "When from \"news@club.example\" and subject or body contains \"meeting\" or \"agenda\" and my address is in To or Cc and \
         size is 0 KB to 512 KB and received on or after 2026-10-01, except if sensitivity is private or category is \"Red category\" \
         or header contains \"X-Spam-Flag: YES\": copy it to Receipts, flag it for tomorrow, clear its categories, show a desktop alert"
    );
    // The rest: a plain flag, clearing it, deleting for good, and the other conditions that
    // take nothing.
    f.conds = [When::SentOnlyToMe, When::ToMe, When::CcMe, When::NotToMe, When::Meeting]
        .into_iter()
        .map(|kind| Cond { kind, ..Default::default() })
        .collect();
    f.unless = vec![Cond { kind: When::AutomaticReply, ..Default::default() }, Cond { kind: When::Flagged, ..Default::default() }];
    f.acts = [Do::Flag, Do::ClearFlag, Do::DeleteForGood].into_iter().map(|kind| Act { kind, ..f.fresh_action() }).collect();
    f.alert = false;
    let params = f.command().1;
    assert_eq!(
        params["conditions"],
        json!([{"type": "sentOnlyToMe"}, {"type": "toMe"}, {"type": "ccMe"}, {"type": "notToMe"}, {"type": "meetingRequest"}])
    );
    assert_eq!(params["exceptions"], json!([{"type": "automaticReply"}, {"type": "flagged"}]));
    assert_eq!(params["actions"], json!([{"type": "flag"}, {"type": "clearFlag"}, {"type": "deleteForGood"}]));
    assert!(params.get("alert").is_none());
    assert!(s.run("rules.add", &params).is_ok());
    // Every kind the builder offers is one `rules.add` takes.
    for kind in When::ALL {
        f.conds = vec![Cond {
            kind,
            text: "news@club.example".into(),
            min: "1".into(),
            max: "2".into(),
            from: "2026-10-01".into(),
            category: "Red category".into(),
            ..Default::default()
        }];
        f.unless.clear();
        f.acts = vec![f.fresh_action()];
        assert_eq!(f.lacks(), None, "{kind:?}");
        assert!(s.run("rules.add", &f.command().1).is_ok(), "{kind:?}");
    }
    for kind in Do::ALL {
        f.acts = vec![Act { kind, ..f.fresh_action() }];
        assert!(s.run("rules.add", &f.command().1).is_ok(), "{kind:?}");
    }
}
