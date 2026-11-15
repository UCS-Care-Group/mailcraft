// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A remembered secret belongs to one account as it is (AU3-02, AU3-03, AU3-18): it is read and
//! written by that account, never found by its address; an account that has the address of
//! another, with other servers, or that trusts a certificate the other doesn't, never gets it; a
//! mailbox where two accounts share an address reads nothing from the store until the owner has
//! seen where each connects; and Remove all data finds every entry that was read.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_model::sync::{SyncRequest, SyncResult};
use mailcraft_model::{Account, AccountKind, EwsServer, Mailbox, OAuth, OAuthProvider, Server};
use serde_json::json;

use super::*;
use crate::{SecretStore, Services};

const NOW: i64 = 1_791_993_600;
const OWNER: &str = "owner@mail.example";
const URL: &str = "https://mail.example.org/EWS/Exchange.asmx";
const SECRET: &str = "dummy-owner-secret";

#[derive(Clone, Default)]
struct Store(Arc<Mutex<HashMap<String, String>>>);

impl SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), mailcraft_host::SecretError> {
        self.0.lock().unwrap().insert(key.into(), secret.into());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

impl Store {
    fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.0.lock().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    }
}

/// What reached the host's Send/Receive: the account, its IMAP host and the password.
type Seen = Arc<Mutex<Vec<(u64, String, String)>>>;

fn imap(id: u64, email: &str, host: &str) -> Account {
    Account {
        id,
        kind: AccountKind::Imap,
        email: email.into(),
        name: email.into(),
        incoming: Some(Server { host: format!("imap.{host}"), port: 993, security: Default::default() }),
        outgoing: Some(Server { host: format!("smtp.{host}"), port: 465, security: Default::default() }),
        ..Default::default()
    }
}

fn exchange(id: u64, email: &str, trusted: &str) -> Account {
    Account {
        id,
        kind: AccountKind::Exchange,
        email: email.into(),
        name: email.into(),
        ews: Some(EwsServer { url: URL.into(), username: String::new(), trusted: trusted.into(), hierarchy: String::new(), parents: false }),
        ..Default::default()
    }
}

fn app(store: &Store, seen: &Seen) -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.ui.remember_sign_in = true;
    let s2 = seen.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        let host = req.account.incoming.as_ref().map(|s| s.host.clone()).unwrap_or_default();
        s2.lock().unwrap().push((req.account.id, host, req.password.clone()));
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, ..Default::default() }).unwrap();
        rx
    }));
    a.services.secrets = Some(Arc::new(store.clone()));
    a
}

/// The sample mailbox with `accounts` added, as it comes back from a file.
fn from_file(accounts: Vec<Account>) -> Mailbox {
    let mut mb = mailcraft_engine::sample::mailbox(NOW, 0);
    mb.accounts.extend(accounts);
    mailcraft_engine::io::read_mailbox(&mailcraft_engine::io::write_mailbox(&mb).unwrap()).unwrap()
}

fn open(a: &mut MailApp, mailbox: Mailbox) {
    a.session.replace_mailbox(mailbox);
    a.drop_secrets_if_replaced();
}

fn passwords(seen: &Seen) -> Vec<(u64, String, String)> {
    seen.lock().unwrap().clone()
}

// ---- AU3-03: the exact account, never one found by its address (run 3's v21) ----

/// A file adds an account with the owner's address (in another case) and servers of its own; with
/// or without a copy of the owner's own account before it, that account never gets the owner's
/// password, before the servers are confirmed or after.
#[test]
fn an_account_with_the_owners_address_and_other_servers_never_gets_the_owners_password() {
    for with_copy in [true, false] {
        let (store, seen) = (Store::default(), Seen::default());
        let owner = imap(7001, OWNER, "mail.example");
        store.set(&MailApp::secret_key("password", &owner), SECRET).unwrap();
        let mut a = app(&store, &seen);
        let mut accounts = if with_copy { vec![owner.clone()] } else { Vec::new() };
        accounts.push(imap(7002, "OWNER@mail.example", "attacker.example"));
        open(&mut a, from_file(accounts));
        assert!(a.is_unconfirmed(), "{with_copy}");
        // Before the servers are confirmed: nothing goes, and nothing is read.
        let _ = a.execute("ui.sync", json!({"account": 7002}));
        crate::secret_read_tests::settle(&mut a);
        assert!(passwords(&seen).is_empty() && a.secret_cache.is_empty(), "{with_copy}");
        // Confirmed: the copy's own key holds the owner's password, the other's holds nothing.
        a.execute("ui.confirmServers", json!({})).unwrap();
        a.secrets_asked = None;
        crate::secret_read_tests::settle(&mut a);
        assert_eq!(a.remembered("password", 7002), None, "{with_copy}");
        assert_eq!(a.remembered("password", 7001).is_some(), with_copy);
        let _ = a.execute("ui.sync", json!({"account": 7002}));
        crate::secret_read_tests::settle(&mut a);
        a.poll_sync();
        assert!(!passwords(&seen).iter().any(|(_, _, password)| password == SECRET), "{with_copy}: {:?}", passwords(&seen));
    }
}

/// Two accounts with one address: the mailbox opens, but nothing is read from the store for it
/// until the owner has confirmed its servers, the owner's own mailbox at start included; and the
/// question says that two accounts have the same address.
#[test]
fn a_mailbox_where_two_accounts_share_an_address_reads_nothing_until_its_servers_are_confirmed() {
    let store = Store::default();
    let owner = imap(7001, OWNER, "mail.example");
    store.set(&MailApp::secret_key("password", &owner), SECRET).unwrap();
    // The window starts with it (no file to confirm: its own mailbox).
    let mut mb = mailcraft_engine::sample::mailbox(NOW, 0);
    mb.accounts.extend([owner.clone(), imap(7002, OWNER, "attacker.example")]);
    let mut a = MailApp::new(Session::at(mb, NOW, 0), Services::default());
    a.autosave = false;
    a.ui.remember_sign_in = true;
    a.services.secrets = Some(Arc::new(store.clone()));
    a.drop_secrets_if_replaced();
    crate::secret_read_tests::settle(&mut a);
    assert!(a.is_unconfirmed());
    assert!(a.secret_cache.is_empty(), "nothing read before the owner says");
    let asked = a.confirm_servers_question().text;
    assert!(asked.contains("the same address"), "{asked}");
    a.execute("ui.confirmServers", json!({})).unwrap();
    crate::secret_read_tests::settle(&mut a);
    assert_eq!(a.remembered("password", 7001).as_deref(), Some(SECRET));
    assert_eq!(a.remembered("password", 7002), None);
    // A mailbox whose addresses are each its own is not held back for it.
    let mut b = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    b.drop_secrets_if_replaced();
    assert!(!b.is_unconfirmed());
}

/// Every way a secret is read and written takes the account: two accounts with one address and
/// different servers never share a secret, whichever kind they are.
#[test]
fn every_kind_of_account_reads_and_writes_its_own_secret_only() {
    let oauth = |mut account: Account, provider| {
        account.oauth = Some(OAuth { provider, ..Default::default() });
        account
    };
    let pairs: Vec<(&str, Account, Account)> = vec![
        ("password", imap(7001, OWNER, "mail.example"), imap(7002, OWNER, "attacker.example")),
        (
            "refresh",
            oauth(imap(7001, OWNER, "gmail.com"), OAuthProvider::Google),
            oauth(imap(7002, OWNER, "attacker.example"), OAuthProvider::Google),
        ),
        ("password", exchange(7001, OWNER, ""), exchange(7002, OWNER, &"ab".repeat(32))),
    ];
    for (kind, mine, theirs) in pairs {
        let (store, seen) = (Store::default(), Seen::default());
        let mut a = app(&store, &seen);
        open(&mut a, from_file(vec![mine.clone(), theirs.clone()]));
        a.execute("ui.confirmServers", json!({})).unwrap();
        crate::secret_read_tests::settle(&mut a);
        a.remember(kind, 7001, SECRET);
        assert_eq!(a.remembered(kind, 7001).as_deref(), Some(SECRET), "{kind} {mine:?}");
        assert_eq!(a.remembered(kind, 7002), None, "{kind} {theirs:?}");
        assert_ne!(MailApp::secret_key(kind, &mine), MailApp::secret_key(kind, &theirs));
        assert_eq!(store.keys(), vec![MailApp::secret_key(kind, &mine)]);
    }
}

// ---- AU3-02: a trusted certificate is part of what is confirmed and of the secret's name (v07) ----

/// A file's copy of the owner's Exchange server account that trusts a certificate the system
/// doesn't: the question names the certificate, the confirmation is not the unpinned one's, and
/// the password remembered for the unpinned account is never handed to it.
#[test]
fn a_certificate_a_file_trusts_is_named_confirmed_and_never_inherits_the_password() {
    let (store, seen) = (Store::default(), Seen::default());
    let pin = "ab".repeat(32);
    let unpinned = exchange(7001, OWNER, "");
    store.set(&MailApp::secret_key("password", &unpinned), SECRET).unwrap();
    let mut a = app(&store, &seen);
    let plain = from_file(vec![unpinned.clone()]);
    let pinned = from_file(vec![exchange(7001, OWNER, &pin)]);
    assert_ne!(unconfirmed::fingerprint(&plain), unconfirmed::fingerprint(&pinned), "the pin is part of what is confirmed");
    assert_ne!(MailApp::secret_key("password", &unpinned), MailApp::secret_key("password", &pinned.accounts[pinned.accounts.len() - 1]));
    open(&mut a, pinned);
    assert!(a.is_unconfirmed());
    let asked = a.confirm_servers_question().text;
    assert!(asked.contains("trusting the certificate AB:AB:AB"), "{asked}");
    assert!(a.account_servers().iter().any(|line| line.contains("trusting the certificate")));
    crate::secret_read_tests::settle(&mut a);
    assert_eq!(a.ews_secret(7001), None, "nothing read before the yes");
    a.execute("ui.confirmServers", json!({})).unwrap();
    a.secrets_asked = None;
    crate::secret_read_tests::settle(&mut a);
    assert_eq!(a.ews_secret(7001), None, "the unpinned account's password is not the pinned one's");
    // The owner's own unpinned account still has its own, under the name it always had.
    let mut b = app(&store, &seen);
    open(&mut b, plain);
    b.execute("ui.confirmServers", json!({})).unwrap();
    crate::secret_read_tests::settle(&mut b);
    assert_eq!(b.ews_secret(7001).as_deref(), Some(SECRET));
}

/// An agent that sets a certificate to trust is asked about by naming it.
#[test]
fn an_agents_change_of_the_trusted_certificate_names_it_in_the_question() {
    let mut s = Session::empty();
    let id = s.run("account.add", &json!({"email": OWNER, "kind": "exchange", "ews": {"url": URL}})).unwrap()["id"].as_u64().unwrap();
    for (params, said) in [
        (json!({"id": id, "ews": {"trusted": "cd".repeat(32)}}), "trusting the certificate CD:CD:CD"),
        (json!({"email": "kit@mail.example", "kind": "exchange", "ews": {"url": URL, "trusted": "ef".repeat(32)}}), "trusting the certificate EF:EF"),
    ] {
        let command = if params.get("id").is_some() { "account.edit" } else { "account.add" };
        match mailcraft_engine::agent::check(&s, command, &params, &[]) {
            mailcraft_engine::agent::Verdict::Ask(_, text) => assert!(text.contains(said), "{command}: {text}"),
            other => panic!("{command}: {other:?}"),
        }
    }
}

// ---- AU3-18: every entry read is noted, so Remove all data finds it (v20) ----

#[test]
fn an_entry_read_for_one_mailbox_is_removed_with_all_data_after_another_is_opened() {
    let (store, seen) = (Store::default(), Seen::default());
    let mut a = app(&store, &seen);
    a.ui.sign_ins_bound = true;
    a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap();
    let account = a.session.mailbox.accounts.iter().find(|x| x.email == "avery@mail.example").cloned().unwrap();
    // Written by a version that didn't note what it wrote: in the store, and nowhere else.
    store.set(&MailApp::secret_key("password", &account), "dummy-pw").unwrap();
    a.secrets_asked = None;
    crate::secret_read_tests::settle(&mut a);
    assert_eq!(a.remembered("password", account.id).as_deref(), Some("dummy-pw"));
    assert!(a.ui.remembered_keys.contains(&MailApp::secret_key("password", &account)));
    // Another mailbox takes its place; then everything is removed.
    a.execute("file.new", json!({})).unwrap();
    a.drop_secrets_if_replaced();
    let left = a.forget_every_secret();
    assert!(left.is_empty(), "{left:?}");
    assert!(store.keys().is_empty(), "{:?}", store.keys());
}

// ---- review: every name an account's secret may have goes with it ----

/// A pinned Exchange server account with its password still under the name from before the pin:
/// removing the account leaves nothing of it in the store, nor its names in the settings.
#[test]
fn removing_a_pinned_account_forgets_its_password_under_every_name_it_had() {
    let (store, seen) = (Store::default(), Seen::default());
    let pinned = exchange(7001, OWNER, &"ab".repeat(32));
    let unpinned = exchange(7001, OWNER, "");
    let mut a = app(&store, &seen);
    open(&mut a, from_file(vec![pinned.clone()]));
    a.execute("ui.confirmServers", json!({})).unwrap();
    for key in [MailApp::secret_key("password", &unpinned), MailApp::secret_key("password", &pinned), MailApp::legacy_secret_key("password", OWNER)] {
        store.set(&key, SECRET).unwrap();
        a.index_secret_key(key);
    }
    a.execute("account.remove", json!({"id": 7001})).unwrap();
    assert!(store.keys().is_empty(), "{:?}", store.keys());
    assert!(a.ui.remembered_keys.is_empty(), "{:?}", a.ui.remembered_keys);
}

/// On an install from before secrets were bound to servers, the password under the old name is
/// moved to neither of two accounts with one address: the old name doesn't say whose it was.
#[test]
fn the_old_name_moves_to_neither_of_two_accounts_with_one_address() {
    let store = Store::default();
    store.set(&MailApp::legacy_secret_key("password", OWNER), SECRET).unwrap();
    let mut mb = mailcraft_engine::sample::mailbox(NOW, 0);
    mb.accounts.extend([imap(7001, OWNER, "mail.example"), imap(7002, OWNER, "attacker.example")]);
    let mut a = MailApp::new(Session::at(mb, NOW, 0), Services::default());
    a.autosave = false;
    a.ui.remember_sign_in = true;
    a.ui.sign_ins_bound = false;
    a.services.secrets = Some(Arc::new(store.clone()));
    a.drop_secrets_if_replaced();
    // Own mailbox held back: the question says why, not that it came from a file.
    let asked = a.confirm_servers_question().text;
    assert!(!asked.contains("opened from a file"), "{asked}");
    a.execute("ui.confirmServers", json!({})).unwrap();
    crate::secret_read_tests::settle(&mut a);
    assert_eq!(a.remembered("password", 7001), None);
    assert_eq!(a.remembered("password", 7002), None);
    assert_eq!(store.keys(), vec![MailApp::legacy_secret_key("password", OWNER)]);
}

/// A shared mailbox with its owner's address keeps no secret of its own: it doesn't hold the
/// owner's mailbox back.
#[test]
fn a_shared_mailbox_is_not_an_account_with_the_same_address() {
    let mut owner = imap(7001, OWNER, "mail.example");
    owner.kind = AccountKind::Microsoft;
    let shared = Account { id: 7002, shared_of: Some(7001), ..owner.clone() };
    let mut mb = mailcraft_engine::sample::mailbox(NOW, 0);
    mb.accounts.extend([owner, shared]);
    let a = MailApp::new(Session::at(mb, NOW, 0), Services::default());
    assert!(!a.has_shared_address());
}

/// File › Info's look at a certificate connects to the account's server: not for a mailbox whose
/// servers nobody has confirmed.
#[test]
fn the_certificate_look_waits_for_the_servers_to_be_confirmed() {
    let (store, seen) = (Store::default(), Seen::default());
    let mut a = app(&store, &seen);
    open(&mut a, from_file(vec![exchange(7001, OWNER, "")]));
    assert!(a.is_unconfirmed());
    let page = json!({"id": 7001, "page": "certificate"});
    assert!(crate::account_setup::AccountSetup::open(&a, &page).is_none());
    a.execute("ui.confirmServers", json!({})).unwrap();
    assert!(crate::account_setup::AccountSetup::open(&a, &page).is_some());
}

/// The store is read through `remembered` and the background read alone, and the old names are
/// made in one place: any new place that does either has to be added here, on purpose.
#[test]
fn the_secret_cache_and_the_old_names_are_used_only_where_they_are_listed() {
    let allowed: &[(&str, &str, usize)] = &[
        ("lib.rs", "legacy_secret_key(", 2),
        ("secret_read.rs", "legacy_secret_key(", 1),
        ("lib.rs", "secret_cache.get(", 2),
        ("secret_read.rs", "secret_cache.contains_key(", 2),
    ];
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![root.clone()];
    let mut found = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
            if !name.ends_with(".rs") || name.ends_with("tests.rs") || name.contains("/tests/") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for needle in ["legacy_secret_key(", "secret_cache.get(", "secret_cache.contains_key("] {
                let n = text.matches(needle).count();
                if n > 0 {
                    found.push((name.clone(), needle, n));
                }
            }
        }
    }
    found.sort();
    let mut want: Vec<(String, &str, usize)> = allowed.iter().map(|(f, n, c)| (f.to_string(), *n, *c)).collect();
    want.sort();
    assert_eq!(found, want);
}
