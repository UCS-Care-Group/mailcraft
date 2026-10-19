//! A JMAP account through `account.add` and `account.edit`: the kind, the server it names and
//! what is refused, without any network.

use mailcraft_model::{AccountKind, Id, JmapServer};
use serde_json::{Value, json};

use crate::agent::{Access, Class, access};
use crate::{CmdError, Session};

const ME: &str = "ada@example.org";
const SESSION: &str = "https://mail.example.org/jmap/session?account=ada@example.org";

fn add(s: &mut Session, params: Value) -> Result<Id, CmdError> {
    s.run("account.add", &params).map(|answer| answer["id"].as_u64().unwrap_or(0))
}

fn edit(s: &mut Session, id: Id, mut params: Value) -> Result<Value, CmdError> {
    params["id"] = json!(id);
    s.run("account.edit", &params)
}

/// The words of a refusal of the parameters.
fn refusal(result: Result<impl std::fmt::Debug, CmdError>) -> String {
    match result {
        Err(CmdError::Params(why)) => why,
        other => panic!("expected a refusal of the parameters, got {other:?}"),
    }
}

fn jmap_params(session: &str) -> Value {
    json!({"email": ME, "kind": "jmap", "jmap": {"session": session}})
}

/// A session with one JMAP account; its id.
fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let id = add(&mut s, json!({"email": ME, "displayName": "Ada", "kind": "jmap", "jmap": {"session": SESSION, "bearer": true}})).unwrap();
    (s, id)
}

fn server(s: &Session, id: Id) -> Option<JmapServer> {
    s.mailbox.account(id).and_then(|a| a.jmap.clone())
}

#[test]
fn a_jmap_account_is_added_with_its_server() {
    let (mut s, id) = session();
    let a = s.mailbox.account(id).unwrap();
    assert_eq!((a.kind, a.email.as_str(), a.name.as_str(), a.display_name.as_str()), (AccountKind::Jmap, ME, ME, "Ada"));
    assert_eq!(a.jmap, Some(JmapServer { session: SESSION.into(), bearer: true }));
    // JMAP alone: no mail servers and no sign-in with a provider; the secret is nowhere in it.
    assert!(a.incoming.is_none() && a.outgoing.is_none() && a.oauth.is_none());
    // The first real account makes way for the placeholder, and has the standard folders.
    assert_eq!(s.mailbox.accounts.len(), 1);
    assert_eq!(s.mailbox.folders.iter().filter(|f| f.account == id).count(), mailcraft_model::FolderKind::STANDARD.len());
    // It is among those with a server (the window synchronises it) whose folders and messages are
    // known by server ids; nothing sends its Outbox yet.
    assert!(a.kind.has_server() && a.kind.server_ids() && !a.kind.sends_mail());
    let listed = s.run("account.list", &json!({})).unwrap();
    let account = &listed["accounts"][0];
    assert_eq!((account["kind"].as_str(), account["auth"].as_str()), (Some("jmap"), Some("password")));
    assert_eq!(account["jmap"], json!({"session": SESSION, "bearer": true}));
    assert!(account["incoming"].is_null() && account["oauth"].is_null());
    // Any case, and the password as the way to send the secret unless it is said to be a token.
    let other = add(&mut s, json!({"email": "bob@example.org", "kind": " JMAP ", "jmap": {"session": "https://mail.example.org/s"}})).unwrap();
    assert_eq!(server(&s, other), Some(JmapServer { session: "https://mail.example.org/s".into(), bearer: false }));
    // The other kinds say nothing of it.
    let local = add(&mut s, json!({"email": "cy@example.org"})).unwrap();
    assert_eq!((s.mailbox.account(local).map(|a| a.kind), server(&s, local)), (Some(AccountKind::Local), None));
    assert!(s.run("account.list", &json!({})).unwrap()["accounts"][2]["jmap"].is_null());
    // The same address twice is still refused.
    assert!(add(&mut s, json!({"email": "ADA@example.org", "kind": "jmap", "jmap": {"session": SESSION}})).is_err());
}

#[test]
fn plain_http_is_for_this_computer_only() {
    let mut s = Session::empty();
    for (n, good) in ["http://127.0.0.1:8080/jmap/session", "http://localhost:3000/session", "http://[::1]:8080/session"].into_iter().enumerate() {
        add(&mut s, json!({"email": format!("user{n}@example.org"), "kind": "jmap", "jmap": {"session": good}})).unwrap();
    }
    let before = s.mailbox.clone();
    for bad in ["http://mail.example.org/jmap/session", "http://10.0.0.5/jmap/session", "http://localhost.example.org/s"] {
        let why = refusal(add(&mut s, jmap_params(bad)));
        assert!(why.contains("`jmap.session`") && why.contains("plain http://") && why.contains("use https://"), "{bad}: {why}");
    }
    assert_eq!(s.mailbox, before, "a refusal changes nothing");
}

#[test]
fn an_address_that_may_not_be_used_is_refused_and_says_why() {
    let mut s = Session::empty();
    let before = s.mailbox.clone();
    let long = format!("https://mail.example.org/{}", "a".repeat(2048));
    for (bad, why) in [
        ("", "is empty"),
        ("mail.example.org/jmap", "isn't an https:// address"),
        ("ftp://mail.example.org/jmap", "isn't an https:// address"),
        ("javascript:alert(1)", "isn't an https:// address"),
        ("https://ada@mail.example.org/jmap", "carries a user name or a password"),
        ("https://ada:secret@mail.example.org/jmap", "carries a user name or a password"),
        ("https://mail.example.org@evil.example/jmap", "carries a user name or a password"),
        ("https://mail.example.org/jmap\n", "has a control character"),
        ("https://mail.example.org/jmap\0", "has a control character"),
        ("https://mail.example.org/\u{7f}", "has a control character"),
        ("https://mail.example.org/\r\nHost: evil.example", "has a control character"),
        (" https://mail.example.org/jmap", "has a control character"),
        ("https://mail.example.org\\@evil.example/", "has a backslash"),
        ("https://", "names no server"),
        ("https://mail_example.org/jmap", "names a server that isn't"),
        ("https://mail.example.org:99999/jmap", "has a port"),
        (long.as_str(), "is longer than 2048 characters"),
    ] {
        let said = refusal(add(&mut s, jmap_params(bad)));
        assert!(said.starts_with("`jmap.session` ") && said.contains(why), "{bad:?}: {said}");
        assert_eq!(s.mailbox, before, "{bad:?}: a refusal changes nothing");
    }
    // As long as it may be, it is not refused for that.
    let longest = format!("https://mail.example.org/{}", "a".repeat(2048 - "https://mail.example.org/".len()));
    assert_eq!(longest.chars().count(), 2048);
    let id = add(&mut s, jmap_params(&longest)).unwrap();
    assert_eq!(server(&s, id).map(|j| j.session.len()), Some(2048), "kept whole, not cut short like a name");
}

#[test]
fn what_is_not_the_right_type_is_refused() {
    let mut s = Session::empty();
    let before = s.mailbox.clone();
    for (params, why) in [
        // The server is required, and is an object.
        (json!({"email": ME, "kind": "jmap"}), "is required"),
        (json!({"email": ME, "kind": "jmap", "jmap": {}}), "is required"),
        (json!({"email": ME, "kind": "jmap", "jmap": null}), "is required"),
        (json!({"email": ME, "kind": "jmap", "jmap": 5}), "`jmap` is {"),
        (json!({"email": ME, "kind": "jmap", "jmap": SESSION}), "`jmap` is {"),
        (json!({"email": ME, "kind": "jmap", "jmap": [SESSION]}), "`jmap` is {"),
        (json!({"email": ME, "kind": "jmap", "jmap": true}), "`jmap` is {"),
        // Its parts are of the type they are said to be.
        (json!({"email": ME, "kind": "jmap", "jmap": {"session": 5}}), "`jmap.session` (string)"),
        (json!({"email": ME, "kind": "jmap", "jmap": {"session": null}}), "`jmap.session` (string)"),
        (json!({"email": ME, "kind": "jmap", "jmap": {"session": [SESSION]}}), "`jmap.session` (string)"),
        (json!({"email": ME, "kind": "jmap", "jmap": {"session": {"url": SESSION}}}), "`jmap.session` (string)"),
        (json!({"email": ME, "kind": "jmap", "jmap": {"session": SESSION, "bearer": "yes"}}), "`jmap.bearer`"),
        (json!({"email": ME, "kind": "jmap", "jmap": {"session": SESSION, "bearer": 1}}), "`jmap.bearer`"),
        (json!({"email": ME, "kind": "jmap", "jmap": {"session": SESSION, "bearer": null}}), "`jmap.bearer`"),
        // Nothing that is for another kind of account.
        (json!({"email": ME, "kind": "jmap", "jmap": {"session": SESSION}, "incoming": {"host": "imap.example.org"}}), "no `incoming` mail server"),
        (json!({"email": ME, "kind": "jmap", "jmap": {"session": SESSION}, "outgoing": {"host": "smtp.example.org"}}), "no `outgoing` mail server"),
        (json!({"email": ME, "kind": "jmap", "jmap": {"session": SESSION}, "auth": "oauth2"}), "not with OAuth 2.0"),
        (json!({"email": ME, "kind": "jmap", "jmap": {"session": SESSION}, "auth": "other"}), "`auth` is password or oauth2"),
        // And a server only for a JMAP account.
        (json!({"email": ME, "jmap": {"session": SESSION}}), "only for an account whose `kind` is"),
        (json!({"email": ME, "kind": "imap", "jmap": {"session": SESSION}}), "only for an account whose `kind` is"),
        (json!({"email": ME, "kind": "microsoft", "jmap": {"session": SESSION}}), "only for an account whose `kind` is"),
        (json!({"email": ME, "kind": "local", "jmap": 5}), "only for an account whose `kind` is"),
        (json!({"email": ME, "kind": 5, "jmap": {"session": SESSION}}), "only for an account whose `kind` is"),
        (json!({"email": ME, "kind": "exchange"}), "`kind` is imap, local, microsoft or jmap"),
    ] {
        let said = refusal(add(&mut s, params.clone()));
        assert!(said.contains(why), "{params}: {said}");
        assert_eq!(s.mailbox, before, "{params}: a refusal changes nothing");
    }
    // Said to be nothing for the other kinds, it is nothing; and no incoming server said to be none.
    let id = add(&mut s, json!({"email": ME, "jmap": null})).unwrap();
    assert_eq!((s.mailbox.account(id).map(|a| a.kind), server(&s, id)), (Some(AccountKind::Local), None));
    let id = add(
        &mut s,
        json!({"email": "bob@example.org", "kind": "jmap", "jmap": {"session": SESSION}, "incoming": null, "outgoing": null, "auth": "password"}),
    )
    .unwrap();
    assert_eq!(s.mailbox.account(id).map(|a| a.kind), Some(AccountKind::Jmap));
}

#[test]
fn the_server_of_an_account_is_changed_by_editing_it() {
    let (mut s, id) = session();
    // Its name is changed without a word of kind: it stays a JMAP account, with its server.
    edit(&mut s, id, json!({"displayName": "Ada Lovelace", "name": "Work"})).unwrap();
    let a = s.mailbox.account(id).unwrap();
    assert_eq!((a.kind, a.name.as_str(), a.display_name.as_str()), (AccountKind::Jmap, "Work", "Ada Lovelace"));
    assert_eq!(a.jmap, Some(JmapServer { session: SESSION.into(), bearer: true }));
    // Only what is said changes: the secret's way, and then the address.
    edit(&mut s, id, json!({"jmap": {"bearer": false}})).unwrap();
    assert_eq!(server(&s, id), Some(JmapServer { session: SESSION.into(), bearer: false }));
    edit(&mut s, id, json!({"kind": "jmap", "jmap": {"session": "https://jmap.example.net/session"}})).unwrap();
    assert_eq!(server(&s, id), Some(JmapServer { session: "https://jmap.example.net/session".into(), bearer: false }));
    // What can't be used leaves the account as it is.
    let before = s.mailbox.clone();
    for bad in [
        json!({"jmap": {"session": "http://jmap.example.net/session"}}),
        json!({"jmap": {"session": "https://ada@jmap.example.net/session"}}),
        json!({"jmap": {"session": "https://jmap.example.net/\u{0}"}}),
        json!({"jmap": {"session": "x".repeat(5000)}}),
        json!({"jmap": {"session": 5}}),
        json!({"jmap": {"bearer": "yes"}}),
        json!({"jmap": 5}),
        json!({"incoming": {"host": "imap.example.org"}}),
        json!({"auth": "oauth2"}),
        json!({"displayName": "Someone", "jmap": {"session": "ftp://jmap.example.net/session"}}),
    ] {
        let said = refusal(edit(&mut s, id, bad.clone()));
        assert!(!said.is_empty(), "{bad}");
        assert_eq!(s.mailbox, before, "{bad}");
    }
    // A change of the name wasn't held up by anything that came before.
    edit(&mut s, id, json!({"displayName": "Ada"})).unwrap();
    assert_eq!(s.mailbox.account(id).map(|a| a.display_name.as_str()), Some("Ada"));
}

#[test]
fn the_kind_of_an_account_changes_with_its_server() {
    let mut s = Session::empty();
    let local = add(&mut s, json!({"email": ME})).unwrap();
    // To JMAP: its server is required.
    assert!(refusal(edit(&mut s, local, json!({"kind": "jmap"}))).contains("is required"));
    assert_eq!(s.mailbox.account(local).map(|a| a.kind), Some(AccountKind::Local));
    edit(&mut s, local, json!({"kind": "jmap", "jmap": {"session": SESSION, "bearer": true}})).unwrap();
    assert_eq!((s.mailbox.account(local).map(|a| a.kind), server(&s, local).map(|j| j.bearer)), (Some(AccountKind::Jmap), Some(true)));
    // From JMAP: the server is dropped with it.
    edit(&mut s, local, json!({"kind": "imap", "incoming": {"host": "imap.example.org"}})).unwrap();
    let a = s.mailbox.account(local).unwrap();
    assert_eq!((a.kind, a.jmap.is_none(), a.incoming.is_some()), (AccountKind::Imap, true, true));
    edit(&mut s, local, json!({"kind": "jmap", "jmap": {"session": SESSION}})).unwrap();
    let a = s.mailbox.account(local).unwrap();
    assert_eq!((a.kind, a.incoming.is_none(), a.outgoing.is_none()), (AccountKind::Jmap, true, true), "no mail servers of the account's past");
    edit(&mut s, local, json!({"kind": "local"})).unwrap();
    assert_eq!((s.mailbox.account(local).map(|a| a.kind), server(&s, local)), (Some(AccountKind::Local), None));
    // A Microsoft account has no sign-in with Microsoft as a JMAP account.
    let microsoft = add(&mut s, json!({"email": "noor@example.org", "kind": "microsoft"})).unwrap();
    assert!(s.mailbox.account(microsoft).is_some_and(|a| a.oauth.is_some()));
    edit(&mut s, microsoft, json!({"kind": "jmap", "jmap": {"session": SESSION}})).unwrap();
    let a = s.mailbox.account(microsoft).unwrap();
    assert_eq!((a.kind, a.oauth.is_none()), (AccountKind::Jmap, true));
    // A shared mailbox is a Microsoft account's, whatever it is told.
    let owner = add(&mut s, json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap();
    let shared = s.mailbox.add_shared_mailbox(owner, "support@example.org").unwrap();
    let _ = edit(&mut s, shared, json!({"kind": "jmap", "jmap": {"session": SESSION}}));
    let a = s.mailbox.account(shared).unwrap();
    assert_eq!((a.kind, a.jmap.is_none(), a.shared_of), (AccountKind::Microsoft, true, Some(owner)));
}

#[test]
fn account_add_is_still_the_owners_to_say_yes_to() {
    assert_eq!(access("account.add"), Some(Access::Owner(Class::Accounts)));
    assert_eq!(access("account.edit"), Some(Access::Owner(Class::Accounts)));
    // Whatever it is told to add, an agent needs the owner's yes.
    let s = Session::empty();
    for params in [jmap_params(SESSION), json!({"email": ME}), json!({"kind": "jmap", "jmap": 5})] {
        assert_eq!(crate::agent::classify(&s, "account.add", &params), Access::Owner(Class::Accounts), "{params}");
    }
}

#[test]
fn junk_for_the_server_never_panics() {
    let (mut s, id) = session();
    let huge = "é".repeat(200_000);
    let junk = [
        json!(null),
        json!([]),
        json!("https://mail.example.org/"),
        json!(-1),
        json!({}),
        json!({"session": huge, "bearer": huge}),
        json!({"session": "https://[::1", "bearer": [[]]}),
        json!({"session": "https://:::/", "bearer": {"deep": [[[]]]}}),
        json!({"session": "https://\u{202e}mail.example.org/\u{0}", "bearer": 1e300}),
        json!({"session": "http://[::ffff:127.0.0.1]:99999999999999999999/", "bearer": -1}),
        json!({"session": SESSION, "bearer": true, "unknown": {"x": 1}}),
    ];
    for kind in [json!("jmap"), json!("JMAP"), json!(null), json!(5), json!(["jmap"]), json!("imap"), json!("local"), json!("microsoft"), json!("")] {
        for jmap in &junk {
            let _ = add(&mut s, json!({"email": "junk@example.org", "kind": kind, "jmap": jmap}));
            let _ = edit(&mut s, id, json!({"kind": kind, "jmap": jmap}));
            let _ = edit(&mut s, id, json!({"jmap": jmap, "incoming": jmap, "outgoing": jmap, "auth": jmap}));
        }
    }
    // Whatever was left, a JMAP account has the server of one that may be used or none at all.
    for a in &s.mailbox.accounts {
        assert!(a.kind == AccountKind::Jmap || a.jmap.is_none(), "{:?}", a.email);
        if a.kind == AccountKind::Jmap {
            assert!(a.jmap.as_ref().is_some_and(|j| j.problem().is_none()), "{:?}", a.email);
        }
    }
}
