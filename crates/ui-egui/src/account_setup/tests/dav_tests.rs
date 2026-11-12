// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! CalDAV and CardDAV: an account of calendars and address books alone, and calendars kept at
//! another provider than the mail, with a sign-in of their own.

use super::*;

const CALDAV: &str = "https://cloud.example.org/remote.php/dav/calendars/ada/";
const CARDDAV: &str = "https://cloud.example.org/remote.php/dav/addressbooks/ada/";

fn wizard_mut<'a>(h: &'a mut Harness<'static, MailApp>) -> &'a mut AccountSetup {
    match h.state_mut().dialog.as_mut() {
        Some(Dialog::Form(Form::Account(w))) => w,
        _ => panic!("the wizard isn't open"),
    }
}

/// A host that tries a CalDAV and CardDAV sign-in: it answers `answer`, or leaves it waiting.
fn dav_checks(app: &mut MailApp, answer: Option<Result<(), &str>>) -> Log<(DavServer, String, String)> {
    let asked: Log<(DavServer, String, String)> = Default::default();
    let log = asked.clone();
    let answer = answer.map(|a| a.map_err(mailcraft_i18n::Text::verbatim));
    let silent: Log<mpsc::Sender<Result<(), mailcraft_i18n::Text>>> = Default::default();
    app.services.test_dav = Some(Box::new(move |dav, username, secret| {
        log.lock().unwrap().push((dav, username, secret));
        let (tx, rx) = mpsc::channel();
        match &answer {
            Some(answer) => tx.send(answer.clone()).unwrap(),
            None => silent.lock().unwrap().push(tx),
        }
        rx
    }));
    asked
}

/// A host that reads calendars and address books at once, so that a first sync starts.
fn reads_dav(app: &mut MailApp) -> Log<Id> {
    let asked: Log<Id> = Default::default();
    let log = asked.clone();
    app.services.dav = Some(Box::new(move |job: crate::dav_sync::DavJob| {
        log.lock().unwrap().push(job.account.id);
        let (tx, rx) = mpsc::channel();
        tx.send(crate::dav_sync::DavResult { account: job.account.id, ..Default::default() }).unwrap();
        rx
    }));
    asked
}

#[test]
fn a_caldav_and_carddav_account_is_added_without_mail_and_signs_in_on_its_own() {
    let mut a = app(false);
    let checked = dav_checks(&mut a, Some(Ok(())));
    let read = reads_dav(&mut a);
    let mut h = open(a, json!({}));
    type_text(&mut h, "noor@example.org");
    connect_manually(&mut h);
    click(&mut h, Kind::CalDav.title());
    assert_eq!(page(&h), Some(Page::Password));
    assert!(says(&h, "keeps calendars and address books only"), "the page says what the account is for");
    {
        let w = wizard_mut(&mut h);
        w.dav.caldav = CALDAV.into();
        w.dav.carddav = CARDDAV.into();
        w.dav.username = "ada-cloud".into();
    }
    h.run_steps(2);
    type_password(&mut h);
    click(&mut h, "Connect");
    assert_ne!(page(&h), Some(Page::Checking), "the host answered at once");
    assert_eq!(page(&h), Some(Page::Done));
    let w = wizard(&h).unwrap();
    assert_eq!(w.outcome, Outcome::Calendars);

    // The host was given the two roots, the name the calendars know, and what was typed.
    let asked = checked.lock().unwrap();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        (asked[0].0.caldav.as_str(), asked[0].0.carddav.as_str(), asked[0].1.as_str(), asked[0].2.as_str()),
        (CALDAV, CARDDAV, "ada-cloud", TYPED)
    );

    let account = h.state().session.mailbox.accounts.iter().find(|a| a.email == "noor@example.org").unwrap().clone();
    assert_eq!(account.kind, AccountKind::Dav);
    assert!(account.incoming.is_none() && account.outgoing.is_none());
    assert!(account.dav.as_ref().is_some_and(|dav| dav.username == "ada-cloud"));
    // Its password is for the calendars, not for a mail account's Send/Receive.
    assert_eq!(h.state().dav_passwords.get(&account.id).map(String::as_str), Some(TYPED));
    assert!(!h.state().passwords.contains_key(&account.id));

    // Its calendars are read at once, without a mail sync to wait for.
    h.state_mut().auto_sync = true;
    h.state_mut().dav_sync_soon(account.id);
    assert!(h.state_mut().dav_tick_at(1_800_000_000_000.0), "the first read starts");
    assert_eq!(read.lock().unwrap().as_slice(), [account.id]);
}

#[test]
fn a_name_that_is_not_an_email_address_offers_only_calendars_and_address_books() {
    let mut a = app(false);
    let checked = dav_checks(&mut a, Some(Ok(())));
    let mut h = open(a, json!({}));
    type_text(&mut h, "Ada's Nextcloud");
    assert!(says(&h, "calendars and address books alone"), "the page says what the name can set up");
    press(&mut h, Key::Enter);
    assert_eq!(page(&h), Some(Page::Choose));
    assert!(says(&h, "only calendars and address books"), "the page says only those can be set up");
    assert!(!says(&h, "Microsoft 365"), "no mail kinds are offered");
    assert!(!says(&h, "This device only"), "not a local account either");
    click(&mut h, Kind::CalDav.title());
    assert_eq!(page(&h), Some(Page::Password));
    {
        let w = wizard_mut(&mut h);
        w.dav.caldav = CALDAV.into();
        w.dav.carddav = CARDDAV.into();
        w.dav.username = "ada-cloud".into();
    }
    h.run_steps(2);
    type_password(&mut h);
    click(&mut h, "Connect");
    assert_eq!(page(&h), Some(Page::Done));
    let account = h.state().session.mailbox.accounts.iter().find(|a| a.email == "Ada's Nextcloud").unwrap().clone();
    assert_eq!(account.kind, AccountKind::Dav);
    assert_eq!(checked.lock().unwrap().len(), 1);
}

#[test]
fn calendars_at_another_provider_are_saved_with_their_own_sign_in() {
    let mut a = app(false);
    let id = a
        .execute(
            "account.add",
            json!({"email": "noor@example.org", "incoming": {"host": "imap.example.org", "port": 993}, "outgoing": {"host": "smtp.example.org", "port": 465}}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let mut h = open(a, json!({"id": id}));
    assert_eq!(page(&h), Some(Page::Settings));
    {
        let w = wizard_mut(&mut h);
        w.dav.caldav = CALDAV.into();
        w.dav.carddav = CARDDAV.into();
        w.dav.username = "ada-cloud".into();
        w.dav_password.0 = "a calendar password".into();
    }
    h.run_steps(2);
    click(&mut h, "Save");
    let account = h.state().session.mailbox.account(id).unwrap().clone();
    let dav = account.dav.as_ref().expect("the calendars are kept");
    assert_eq!((dav.caldav.as_str(), dav.carddav.as_str(), dav.username.as_str()), (CALDAV, CARDDAV, "ada-cloud"));
    assert_eq!(h.state().dav_passwords.get(&id).map(String::as_str), Some("a calendar password"));
    // The mail account's own sign-in is untouched.
    assert!(!h.state().passwords.contains_key(&id));

    // Clearing both addresses takes the services away again, and their password with them.
    h.state_mut().execute("ui.dialog", json!({"name": "account", "id": id})).unwrap();
    h.run_steps(3);
    {
        let w = wizard_mut(&mut h);
        w.dav = DavFields::default();
    }
    click(&mut h, "Save");
    assert!(h.state().session.mailbox.account(id).unwrap().dav.is_none());
    assert!(!h.state().dav_passwords.contains_key(&id));
}

#[test]
fn a_calendar_only_accounts_settings_have_no_mail_servers() {
    let mut a = app(false);
    let id = a
        .execute(
            "account.add",
            json!({
                "email": "ada@example.org",
                "kind": "dav",
                "dav": {"caldav": CALDAV, "carddav": CARDDAV, "username": "ada-cloud"}
            }),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let mut h = open(a, json!({"id": id}));
    assert_eq!(page(&h), Some(Page::Settings));
    assert!(says(&h, "Keeps calendars and address books over CalDAV and CardDAV"), "the page says what it is");
    assert!(!says(&h, "Incoming mail (IMAP)"), "a calendar account has no mail servers to show");
    assert!(says(&h, "User name for calendars and address books"));
    assert!(h.query_by_role(Role::PasswordInput).is_some(), "its calendar password can be changed");
    assert!(h.get_all_by_role(Role::TextInput).any(|field| field.value().as_deref() == Some(CALDAV)));

    // A CalDAV and CardDAV account without its addresses can't be saved, and stays as it was.
    {
        let w = wizard_mut(&mut h);
        w.dav = DavFields::default();
    }
    h.run_steps(2);
    click(&mut h, "Save");
    assert_eq!(page(&h), Some(Page::Settings));
    assert!(says(&h, "Type the CalDAV and CardDAV addresses"));
    assert!(h.state().session.mailbox.account(id).unwrap().dav.is_some());
}

#[test]
fn both_calendar_addresses_are_needed_and_the_account_is_a_calendar_one() {
    let a = app(false);
    let mut w = AccountSetup::new(&a, &json!({"email": "ada@example.org"}));
    w.dav_only = true;
    w.dav.caldav = CALDAV.into();
    assert!(w.settings_problem(true).is_some(), "one address is not the two services");
    w.dav.carddav = CARDDAV.into();
    w.dav.username = "ada-cloud".into();
    assert_eq!(w.settings_problem(true), None);
    let (command, params) = w.add_command("").unwrap();
    assert_eq!(command, "account.add");
    assert_eq!(params["kind"].as_str(), Some("dav"));
    assert!(params.get("incoming").is_none() && params.get("outgoing").is_none());
    assert_eq!(params["dav"]["username"].as_str(), Some("ada-cloud"));
    assert_eq!(w.account().map(|a| (a.kind, a.incoming.is_none(), a.dav.is_some())), Some((AccountKind::Dav, true, true)));

    // A service root that isn't https is refused, with the address named.
    w.dav.caldav = "http://cloud.example.org/dav/".into();
    let why = w.settings_problem(true).unwrap();
    assert!(why.contains("CalDAV"), "{why}");
}
