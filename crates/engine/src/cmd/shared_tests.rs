//! Shared mailboxes: `account.addShared` and `account.removeShared`, what a synchronisation of
//! one is asked to do, where what it brings lands, and what a mailbox file keeps of one.

use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{Address, FolderKind, Id, Mailbox, Message};
use serde_json::{Value, json};

use super::super::account::{apply_sync, sync_request};
use super::{MAX_SHARED, signer};
use crate::{CmdError, Session};

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn refused(s: &mut Session, id: &str, params: Value) -> String {
    match s.run(id, &params) {
        Ok(v) => panic!("{id} {params} went through: {v}"),
        Err(e) => e.to_string(),
    }
}

/// A mailbox with the owner's local account and a Microsoft 365 account (work or school).
fn setup() -> (Session, Id) {
    let mut s = Session::at(Mailbox::with_local_account("Avery Quinn", "avery@lumen.example"), 1_791_993_600, 0);
    let added = run(
        &mut s,
        "account.add",
        json!({
            "email": "avery@harbor.example", "displayName": "Avery Quinn",
            "incoming": {"host": "outlook.office365.com", "port": 993},
            "outgoing": {"host": "smtp.office365.com", "port": 587, "security": "starttls"},
            "auth": "oauth2", "provider": "microsoft", "directory": "organisation",
        }),
    );
    let account = added["id"].as_u64().unwrap();
    (s, account)
}

fn add_support(s: &mut Session, account: Id) -> Id {
    run(s, "account.addShared", json!({"account": account, "address": "support@harbor.example", "displayName": "Harbor Support"}))["id"]
        .as_u64()
        .unwrap()
}

#[test]
fn a_shared_mailbox_opens_under_its_account_with_folders_of_its_own() {
    let (mut s, account) = setup();
    let support = add_support(&mut s, account);
    let shared = s.mailbox.account(support).unwrap().clone();
    assert_eq!(shared.shared_of, Some(account));
    assert_eq!((shared.email.as_str(), shared.display_name.as_str()), ("support@harbor.example", "Harbor Support"));
    // No sign-in of its own: it uses its account's.
    assert!(shared.oauth.is_none() && shared.username.is_empty());
    assert_eq!(signer(&s, support), account);
    assert_eq!(signer(&s, account), account);
    // Its own standard folders; its Inbox isn't put among the Favorites by itself.
    for kind in FolderKind::STANDARD {
        assert!(s.mailbox.folder_of_kind(support, kind).is_some(), "{kind:?}");
    }
    let inbox = s.mailbox.folder_of_kind(support, FolderKind::Inbox).unwrap();
    assert!(!s.mailbox.folder(inbox).unwrap().favorite);
    // The list of accounts says what it is.
    let list = run(&mut s, "account.list", json!({}));
    let entry = list["accounts"].as_array().unwrap().iter().find(|a| a["id"] == support).unwrap().clone();
    assert_eq!(entry["sharedOf"], account);
    // Listed right after its account, whatever is added after it.
    run(&mut s, "account.add", json!({"email": "avery@fabrikam.example"}));
    let order: Vec<&str> = s.mailbox.accounts_in_pane_order().iter().map(|a| a.email.as_str()).collect();
    assert_eq!(order, ["avery@lumen.example", "avery@harbor.example", "support@harbor.example", "avery@fabrikam.example"]);
    // Without naming the account: the first that can open one.
    let other = run(&mut s, "account.addShared", json!({"address": "Sales@harbor.example"}));
    assert_eq!(other["account"], account);
    assert_eq!(s.mailbox.shared_mailboxes(account).len(), 2);
}

#[test]
fn only_a_work_or_school_microsoft_account_opens_one_and_only_once() {
    let (mut s, account) = setup();
    let local = s.mailbox.accounts[0].id;
    let password =
        run(&mut s, "account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}}))["id"].as_u64().unwrap();
    let personal = run(
        &mut s,
        "account.add",
        json!({"email": "avery@outlook.example", "incoming": {"host": "outlook.office365.com"}, "auth": "oauth2", "directory": "personal"}),
    )["id"]
        .as_u64()
        .unwrap();
    for (owner, says) in [(local, "signs in with Microsoft"), (password, "signs in with Microsoft"), (personal, "personal Microsoft account")] {
        let error = refused(&mut s, "account.addShared", json!({"account": owner, "address": "support@harbor.example"}));
        assert!(error.contains(says), "{error}");
    }
    let support = add_support(&mut s, account);
    // A shared mailbox opens no other.
    assert!(refused(&mut s, "account.addShared", json!({"account": support, "address": "x@harbor.example"})).contains("shared mailbox can't"));
    // Nor twice, nor under its own account's address, nor an account that is set up already.
    assert!(refused(&mut s, "account.addShared", json!({"account": account, "address": " SUPPORT@harbor.example "})).contains("already set up"));
    assert!(refused(&mut s, "account.addShared", json!({"account": account, "address": "avery@harbor.example"})).contains("own address"));
    assert!(refused(&mut s, "account.addShared", json!({"account": account, "address": "avery@lumen.example"})).contains("already set up"));
    assert!(refused(&mut s, "account.addShared", json!({"account": 999_999, "address": "a@harbor.example"})).contains("no account"));
    // So many and no more.
    for n in 1..MAX_SHARED {
        run(&mut s, "account.addShared", json!({"account": account, "address": format!("team{n}@harbor.example")}));
    }
    assert!(refused(&mut s, "account.addShared", json!({"account": account, "address": "one-more@harbor.example"})).contains("as many"));
}

#[test]
fn an_address_that_isnt_one_is_refused() {
    let (mut s, account) = setup();
    let long = format!("{}@harbor.example", "a".repeat(260));
    for address in [
        "",
        "   ",
        "support",
        "support@",
        "@harbor.example",
        "support@harbor",
        "sup port@harbor.example",
        "support@harbor.example\r\nRCPT TO:<eve@example.org>",
        "support\u{1}auth=Bearer x@harbor.example",
        "a@b@harbor.example",
        "<support@harbor.example>",
        "\"support\"@harbor.example",
        "support@harbor.example;eve@example.org",
        "süpport@harbor.example",
        "support@harbor.example\u{0}",
        long.as_str(),
    ] {
        let error = s.run("account.addShared", &json!({"account": account, "address": address})).unwrap_err();
        assert!(matches!(error, CmdError::Params(_)), "{address:?}: {error}");
        assert!(!error.to_string().contains('\r') && !error.to_string().contains('\u{0}'), "{error:?}");
    }
    assert!(matches!(s.run("account.addShared", &json!({"account": account})), Err(CmdError::Params(_))));
    assert!(matches!(s.run("account.addShared", &json!({"account": account, "address": 5})), Err(CmdError::Params(_))));
    assert!(s.mailbox.shared_mailboxes(account).is_empty());
}

#[test]
fn a_synchronisation_reads_it_as_itself_with_its_accounts_sign_in() {
    let (mut s, account) = setup();
    let support = add_support(&mut s, account);
    // Its account's servers change afterwards: the shared mailbox follows them.
    run(&mut s, "account.edit", json!({"id": account, "incoming": {"host": "outlook.office365.com", "port": 1993}}));
    let request = sync_request(&s, support).unwrap();
    let owner = s.mailbox.account(account).unwrap();
    assert_eq!(request.account.id, support);
    assert_eq!(request.account.email, "support@harbor.example");
    // IMAP signs in as the shared mailbox (XOAUTH2's user), SMTP as its owner, both with the
    // owner's Microsoft sign-in.
    assert_eq!(request.account.login(), "support@harbor.example");
    assert_eq!(request.send_login, "avery@harbor.example");
    assert_eq!(request.sending_account().login(), "avery@harbor.example");
    assert_eq!(request.sending_account().email, "support@harbor.example");
    assert_eq!(request.account.oauth, owner.oauth);
    assert_eq!(request.account.incoming, owner.incoming);
    assert_eq!(request.account.incoming.as_ref().map(|i| i.port), Some(1993));
    assert_eq!(request.account.outgoing, owner.outgoing);
    // The host brings the token; the engine has none to give.
    assert!(request.access_token.is_empty() && request.password.is_empty());
    // Its own folders, none of its account's.
    let own: Vec<Id> = s.mailbox.folders.iter().filter(|f| f.account == support).map(|f| f.id).collect();
    assert_eq!(request.folders.iter().map(|f| f.id).collect::<Vec<_>>(), own);
    // Its account's own run is as it always was.
    let plain = sync_request(&s, account).unwrap();
    assert!(plain.send_login.is_empty() && plain.account.shared_of.is_none());
    assert_eq!(plain.account.login(), "avery@harbor.example");
    // An account that can no longer open it says so instead of signing in as something else.
    run(&mut s, "account.edit", json!({"id": account, "auth": "password"}));
    assert!(sync_request(&s, support).unwrap_err().to_string().contains("signs in with Microsoft"));
}

#[test]
fn mail_written_from_it_waits_in_its_outbox_from_its_address() {
    let (mut s, account) = setup();
    let support = add_support(&mut s, account);
    run(&mut s, "mail.new", json!({"account": support, "to": "customer@example.org", "subject": "Your ticket"}));
    let sent = run(&mut s, "compose.send", json!({}));
    assert_eq!(sent["queued"], true);
    let request = sync_request(&s, support).unwrap();
    assert_eq!(request.outbox.len(), 1);
    let message = &request.outbox[0];
    assert_eq!(message.from, Address::new("Harbor Support", "support@harbor.example"));
    assert_eq!(message.account, support);
    // Nothing of it waits in its account's Outbox.
    assert!(sync_request(&s, account).unwrap().outbox.is_empty());
}

#[test]
fn what_a_synchronisation_brings_lands_in_the_shared_mailbox() {
    let (mut s, account) = setup();
    let support = add_support(&mut s, account);
    let inbox = s.mailbox.folder_of_kind(support, FolderKind::Inbox).unwrap();
    let new = Message { uid: 1, subject: "Printer on fire".into(), from: Address::new("Bob", "bob@example.org"), ..Default::default() };
    let result = SyncResult {
        account: support,
        email: "support@harbor.example".into(),
        folders: vec![FolderDelta {
            id: Some(inbox),
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 11,
            uid_seen: 1,
            new: vec![new],
            ..Default::default()
        }],
        ..Default::default()
    };
    apply_sync(&mut s, result).unwrap();
    let arrived: Vec<&Message> = s.mailbox.messages.values().map(|m| m.as_ref()).filter(|m| m.subject == "Printer on fire").collect();
    assert_eq!(arrived.len(), 1);
    assert_eq!((arrived[0].account, arrived[0].folder), (support, inbox));
    assert_eq!(s.mailbox.folder(inbox).unwrap().remote, "INBOX");
    // Its account's Inbox is untouched.
    let own = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    assert!(s.mailbox.messages.values().all(|m| m.folder != own));
}

#[test]
fn closing_it_or_its_account_removes_what_is_kept_of_it() {
    let (mut s, account) = setup();
    let support = add_support(&mut s, account);
    let sales = run(&mut s, "account.addShared", json!({"account": account, "address": "sales@harbor.example"}))["id"].as_u64().unwrap();
    // Not an account of its own: account.removeShared refuses it.
    assert!(refused(&mut s, "account.removeShared", json!({"id": account})).contains("isn't a shared mailbox"));
    assert!(refused(&mut s, "account.removeShared", json!({"address": "nobody@harbor.example"})).contains("no shared mailbox"));
    assert!(matches!(s.run("account.removeShared", &json!({})), Err(CmdError::Params(_))));
    // By its address, or by its id.
    assert_eq!(run(&mut s, "account.removeShared", json!({"account": account, "address": "SALES@harbor.example"}))["deleted"], sales);
    assert!(s.mailbox.account(sales).is_none() && s.mailbox.folders.iter().all(|f| f.account != sales));
    let again = run(&mut s, "account.addShared", json!({"account": account, "address": "sales@harbor.example"}))["id"].as_u64().unwrap();
    assert_eq!(run(&mut s, "account.removeShared", json!({"id": again}))["deleted"], again);
    // Undone, it is back.
    run(&mut s, "edit.undo", json!({}));
    assert!(s.mailbox.account(again).is_some());
    // Removing its account takes it along.
    run(&mut s, "account.remove", json!({"id": account}));
    assert!(s.mailbox.account(support).is_none() && s.mailbox.account(again).is_none());
    assert!(s.mailbox.folders.iter().all(|f| f.account != support));
    assert!(s.mailbox.accounts.iter().all(|a| !a.is_shared()));
}

#[test]
fn the_last_account_with_its_shared_mailboxes_stays() {
    let mut s = Session::at(Mailbox::default(), 1_791_993_600, 0);
    let account = run(
        &mut s,
        "account.add",
        json!({"email": "avery@harbor.example", "incoming": {"host": "outlook.office365.com"}, "auth": "oauth2", "directory": "organisation"}),
    )["id"]
        .as_u64()
        .unwrap();
    add_support(&mut s, account);
    assert!(refused(&mut s, "account.remove", json!({"id": account})).contains("last account"));
}

#[test]
fn only_its_names_change_in_account_settings() {
    let (mut s, account) = setup();
    let support = add_support(&mut s, account);
    for change in [
        json!({"id": support, "email": "eve@example.org"}),
        json!({"id": support, "incoming": {"host": "imap.example.org"}}),
        json!({"id": support, "auth": "password"}),
        json!({"id": support, "username": "eve"}),
    ] {
        assert!(refused(&mut s, "account.edit", change.clone()).contains("shared mailbox"), "{change}");
    }
    run(&mut s, "account.edit", json!({"id": support, "displayName": "Help Desk\r\n", "name": "Support"}));
    let shared = s.mailbox.account(support).unwrap();
    assert_eq!((shared.display_name.as_str(), shared.name.as_str()), ("Help Desk", "Support"));
    assert_eq!(shared.shared_of, Some(account));
}

#[test]
fn the_mailbox_file_keeps_nothing_secret_of_it() {
    let (mut s, account) = setup();
    let support = add_support(&mut s, account);
    let bytes = crate::io::write_mailbox(&s.mailbox).unwrap();
    let text = String::from_utf8_lossy(&bytes).to_lowercase();
    for secret in ["password", "token", "refresh", "secret", "bearer"] {
        assert!(!text.contains(secret), "the file mentions {secret}");
    }
    // It reads back as it was.
    let back = crate::io::read_mailbox(&bytes).unwrap();
    assert_eq!(back.account(support).unwrap().shared_of, Some(account));
    // And a file from elsewhere that says something else in its place still opens.
    let mut value: Value = serde_json::from_slice(&bytes).unwrap();
    let accounts = value["accounts"].as_array_mut().unwrap();
    for entry in accounts.iter_mut().filter(|a| a["id"] == support) {
        entry["sharedOf"] = json!({"not": "an id"});
    }
    let odd = crate::io::read_mailbox(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(odd.account(support).unwrap().shared_of, None);
}

#[test]
fn a_shared_mailbox_whose_account_is_gone_is_said_not_signed_in_otherwise() {
    let (mut s, account) = setup();
    let support = add_support(&mut s, account);
    // A file that names an account it doesn't have, or the shared mailbox itself.
    for parent in [987_654, support] {
        s.mailbox.account_mut(support).unwrap().shared_of = Some(parent);
        assert!(sync_request(&s, support).unwrap_err().to_string().contains("is gone"));
        // Nobody else's sign-in is used for it.
        assert_eq!(signer(&s, support), support);
        // Still listed in the pane, once.
        assert_eq!(s.mailbox.accounts_in_pane_order().iter().filter(|a| a.id == support).count(), 1);
    }
}

#[test]
fn hostile_params_never_panic() {
    let (mut s, account) = setup();
    add_support(&mut s, account);
    let junk = [
        json!(null),
        json!([]),
        json!("support@harbor.example"),
        json!({"account": -1, "address": "x@harbor.example"}),
        json!({"account": 18446744073709551615u64, "address": "x@harbor.example"}),
        json!({"account": account, "address": null, "displayName": 5}),
        json!({"account": account, "address": ["x@harbor.example"]}),
        json!({"account": account, "address": "x".repeat(100_000)}),
        json!({"account": account, "address": "y@harbor.example", "displayName": "\u{0}\r\n".repeat(10_000)}),
        json!({"id": -1}),
        json!({"id": 1.5}),
        json!({"id": 0, "address": "\u{202e}@harbor.example"}),
        json!({"id": "x", "account": "y", "address": {"deep": [[[]]]}}),
    ];
    for params in &junk {
        for command in ["account.addShared", "account.removeShared"] {
            let _ = s.run(command, params);
        }
    }
    // A display name is one line and not too long.
    for a in s.mailbox.accounts.iter().filter(|a| a.is_shared()) {
        assert!(a.display_name.chars().count() <= 254 && !a.display_name.contains(['\r', '\n', '\u{0}']));
    }
}
