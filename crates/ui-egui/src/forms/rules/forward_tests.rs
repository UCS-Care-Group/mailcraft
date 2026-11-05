// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A rule that forwards or redirects mail on the rule page (#566): offered only where a server
//! forwards it, kept on that server, the people it goes to, and what the page says meanwhile.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use super::*;
use crate::rule_sync::Permission;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

const NOOR: &str = "noor@example.org";

/// A window with an account on this computer and, when `microsoft`, Noor's Microsoft account
/// too, the Rules dialog open on the new rule page for the mail of the last of them: the window
/// and that account.
fn window(microsoft: bool) -> (Harness<'static, MailApp>, Id) {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let mut account = app.session.mailbox.accounts[0].id;
    if microsoft {
        account = app.execute("account.add", json!({"email": NOOR, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    }
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    let mb = h.state().session.mailbox.clone();
    form(&mut h).choose(&mb, account);
    h.run_steps(2);
    // With no rule yet, the dialog opens on the new rule page.
    if h.query_by_label("Add Rule").is_none() {
        h.get_by_label("New Rule").click();
        h.run_steps(3);
    }
    (h, account)
}

/// The Rules form the dialog shows.
fn form<'h>(h: &'h mut Harness<'_, MailApp>) -> &'h mut RulesForm {
    match h.state_mut().dialog.as_mut() {
        Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(f))) => f,
        _ => panic!("the Rules dialog isn't open"),
    }
}

/// The new rule forwards the mail with "invoice" in its subject to `people`.
fn forwarding(h: &mut Harness<'_, MailApp>, kind: Do, people: &str) {
    let f = form(h);
    f.name = "To the bookkeeper".into();
    f.conds[0] = Cond { kind: When::Subject, text: "invoice".into(), ..Default::default() };
    f.acts[0] = Act { kind, people: people.into(), ..f.fresh_action() };
    h.run_steps(3);
}

/// Whether each item of the menu of what the first action does that is called `label` is greyed.
fn greyed(h: &mut Harness<'_, MailApp>, label: &str) -> bool {
    // The drop-down shows what the action does now; it has no name of its own.
    h.get_all_by(|n| n.role() == egui::accesskit::Role::ComboBox && n.value().as_deref() == Some("Move to folder")).next().unwrap().click();
    h.run_steps(3);
    let greyed = h.get_by_label(label).accesskit_node().is_disabled();
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    greyed
}

/// How many texts on screen start with `start` (a label and the text in it count apart).
fn said(h: &Harness<'_, MailApp>, start: &str) -> usize {
    h.query_all_by(|n| n.label().or(n.value()).is_some_and(|l| l.starts_with(start))).count()
}

#[test]
fn forwarding_is_greyed_with_why_where_no_server_would_forward() {
    let (mut h, _) = window(false);
    for label in ["Forward to", "Forward as an attachment to", "Redirect to"] {
        assert!(greyed(&mut h, label), "{label}");
    }
    assert!(!greyed(&mut h, "Mark as read"));
    // Chosen anyway, through a script of the form: said, and not added.
    forwarding(&mut h, Do::Forward, "books@example.org");
    assert!(
        h.query_by_label("A rule kept on this computer doesn't forward mail: only a server does. Take the forwarding out to save the rule.")
            .is_some()
    );
    assert!(h.get_by_label("Add Rule").accesskit_node().is_disabled());
}

#[test]
fn a_microsoft_accounts_rule_that_forwards_is_kept_on_its_server_and_takes_whole_addresses() {
    let (mut h, account) = window(true);
    assert!(!greyed(&mut h, "Forward to"));
    forwarding(&mut h, Do::Forward, "Books <books@example.org>; ann@example.org");
    // The box for the server ticked and greyed: only a server forwards mail.
    let keep = format!("Keep on the server of {NOOR}");
    let node = h.get_by_label(&keep).accesskit_node();
    assert!(node.is_disabled() && node.toggled() == Some(egui::accesskit::Toggled::True));
    assert!(h.query_by_label("It runs there whether MailCraft is open or not, and shows in Outlook on the web and on your phone.").is_some());
    // Unticked before: the rule still goes to the server, and isn't run over the mail already
    // here, which only the server could forward.
    form(&mut h).server = false;
    form(&mut h).then_run = true;
    h.run_steps(2);
    assert!(h.get_by_label(&keep).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True));
    // People are whole addresses, ten at most in all.
    for odd in ["", "Ann", &(0..11).map(|n| format!("p{n}@example.org")).collect::<Vec<_>>().join("; ")] {
        form(&mut h).acts[0].people = odd.into();
        h.run_steps(2);
        assert!(h.get_by_label("Add Rule").accesskit_node().is_disabled(), "{odd:?}");
    }
    form(&mut h).acts[0].people = "Books <books@example.org>; ann@example.org".into();
    h.run_steps(2);
    h.get_by_label("Add Rule").click();
    h.run_steps(4);
    let mb = &h.state().session.mailbox;
    let rule = mb.rules.iter().find(|r| r.name == "To the bookkeeper").unwrap();
    assert_eq!((rule.account, rule.keep), (account, Keep::Server));
    assert_eq!(rule.actions, [Action::ForwardTo(vec!["books@example.org".into(), "ann@example.org".into()])]);
    assert!(mb.messages.values().all(|m| !m.read || m.account != account), "nothing ran here");
    // Its row says it doesn't run until the server has it, and Run Rule Now is greyed.
    let marked = mark(h.state(), rule, account);
    assert!(marked.starts_with("Not running yet: only the server forwards mail"), "{marked}");
    h.get_all_by_label("More for this rule").next().unwrap().click();
    h.run_steps(3);
    assert!(h.get_by_label("Run Rule Now").accesskit_node().is_disabled());
}

#[test]
fn the_page_says_what_waits_for_permission_and_what_a_work_account_may_not_forward() {
    let (mut h, account) = window(true);
    h.state_mut().rules.perm.insert(account, Permission::Needed);
    forwarding(&mut h, Do::Redirect, "books@example.net");
    let waits = "It goes to the server once MailCraft may change the server's rules (File › Info › Ask again); until then it doesn't run: only the server forwards mail.";
    assert!(h.query_by_label(waits).is_some());
    // Out of the organisation, from a work account: the sender, not the owner, hears of a block.
    let note = "Your organisation may not let mail be forwarded outside it. Then the server doesn't forward it, and the sender, not you, gets a notice that it wasn't delivered.";
    assert!(h.query_by_label(note).is_some());
    form(&mut h).acts[0].people = "ann@EXAMPLE.org".into();
    h.run_steps(2);
    assert!(h.query_by_label(note).is_none(), "within the organisation");
    // A personal account has no organisation to stop it.
    form(&mut h).acts[0].people = "books@example.net".into();
    let a = h.state_mut().session.mailbox.accounts.iter_mut().find(|a| a.id == account).unwrap();
    a.oauth.get_or_insert_with(Default::default).directory = mailcraft_model::Directory::Personal;
    h.run_steps(2);
    assert!(h.query_by_label(note).is_none(), "a personal account");
    // What the server can't keep with it is said, and the rule isn't added.
    let f = form(&mut h);
    let flag = Act { kind: Do::Flag, ..f.fresh_action() };
    f.acts.push(flag);
    h.run_steps(2);
    assert!(said(&h, "A rule that forwards mail is kept on the server, which can't keep this one") > 0);
    assert!(h.get_by_label("Add Rule").accesskit_node().is_disabled());
}

#[test]
fn each_way_of_forwarding_is_made_as_rules_add_takes_it_and_comes_back_to_the_page() {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let noor = s.run("account.add", &json!({"email": NOOR, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let mut f = RulesForm::new(&s, &json!({}));
    f.choose(&s.mailbox, noor);
    f.name = "Books".into();
    f.conds[0] = Cond { kind: When::Subject, text: "invoice".into(), ..Default::default() };
    let people = vec!["books@example.org".to_string()];
    let ways = [
        (Do::Forward, Action::ForwardTo(people.clone())),
        (Do::ForwardAttached, Action::ForwardAsAttachmentTo(people.clone())),
        (Do::Redirect, Action::RedirectTo(people.clone())),
    ];
    for (kind, action) in ways {
        f.acts = vec![Act { kind, people: "Books <books@example.org>".into(), ..f.fresh_action() }];
        let id = s.run("rules.add", &f.command().1).unwrap()["id"].as_u64().unwrap();
        let rule = s.mailbox.rules.iter().find(|r| r.id == id).unwrap().clone();
        assert_eq!(rule.actions, [action]);
        let mut back = RulesForm::new(&s, &json!({}));
        assert!(back.change(&s.mailbox, &rule));
        assert_eq!((back.acts[0].kind, back.acts[0].addresses()), (kind, people.clone()));
    }
}

/// The rule page forwarding a work account's invoices out of its organisation, and the menu of
/// what an action does on an account whose rules stay on this computer, in light and dark, as PNG
/// files to look at:
/// `MAILCRAFT_SHOTS_DIR=<dir> cargo test -p mailcraft-ui-egui shots_of_forwarding -- --ignored`
#[test]
#[ignore = "writes screenshots, for looking at them"]
fn shots_of_forwarding() {
    let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") else {
        eprintln!("set MAILCRAFT_SHOTS_DIR to the directory to write to");
        return;
    };
    for theme in ["light", "dark"] {
        for (name, microsoft) in [("rule-forward-work", true), ("rule-forward-greyed", false)] {
            let mut app = MailApp::new(Session::at(Mailbox::with_local_account("Me", "me@example.org"), NOW, 0), Services::default());
            app.autosave = false;
            app.execute("ui.theme", json!({"value": theme})).unwrap();
            let mut account = app.session.mailbox.accounts[0].id;
            if microsoft {
                account = app.execute("account.add", json!({"email": NOOR, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
            }
            app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
            let mut h = Harness::builder()
                .with_accessibility_check(false)
                .allow_missing_glyphs()
                .with_size(egui::vec2(1280.0, 800.0))
                .with_step_dt(1.0 / 60.0)
                .wgpu()
                .build_ui_state(
                    |ui, app: &mut MailApp| {
                        let ctx = ui.ctx().clone();
                        app.logic(&ctx);
                        app.ui(ui);
                    },
                    app,
                );
            h.input_mut().max_texture_side = Some(8192);
            h.run_steps(2);
            let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
            h.run_steps(4);
            let mb = h.state().session.mailbox.clone();
            form(&mut h).choose(&mb, account);
            h.run_steps(2);
            if h.query_by_label("Add Rule").is_none() {
                h.get_by_label("New Rule").click();
                h.run_steps(3);
            }
            if microsoft {
                let a = h.state_mut().session.mailbox.accounts.iter_mut().find(|a| a.id == account).unwrap();
                a.oauth.get_or_insert_with(Default::default).directory = mailcraft_model::Directory::Organisation;
                forwarding(&mut h, Do::Forward, "Books <books@example.net>; ann@example.org");
                let f = form(&mut h);
                let read = Act { kind: Do::MarkRead, ..f.fresh_action() };
                f.acts.push(read);
            } else {
                h.get_all_by(|n| n.role() == egui::accesskit::Role::ComboBox && n.value().as_deref() == Some("Move to folder"))
                    .next()
                    .unwrap()
                    .click();
            }
            h.run_steps(12);
            let path = std::path::Path::new(&dir).join(format!("{name}-{theme}.png"));
            h.render().unwrap().save(&path).unwrap();
            println!("{}", path.display());
        }
    }
}
