// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Info › Automatic Replies: what is read, what is to change, and what a server's answer
//! comes to. The server's side is written out by hand, as `mailcraft-net` brings it.

use mailcraft_model::Id;
use mailcraft_model::auto_replies::MAX_REPLY_TEXT;
use serde_json::{Value, json};

use super::auto_replies::{NOT_MICROSOFT, NOT_READ, SHARED, any_on};
use crate::Session;
use crate::agent::{self, Access, Class};

/// 2026-09-21T13:46:40Z.
const NOW: i64 = 1_790_000_000;

fn session() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn server_setting() -> Value {
    json!({
        "status": "scheduled",
        "externalAudience": "contactsOnly",
        "scheduledStartDateTime": {"dateTime": "2026-09-21T08:00:00.0000000", "timeZone": "UTC"},
        "scheduledEndDateTime": {"dateTime": "2026-09-28T09:00:00.0000000", "timeZone": "W. Europe Standard Time"},
        "internalReplyMessage": "<html><head><style>p{color:red}</style></head><body><p>Away until <b>Monday</b>.</p><script>steal()</script><img src=\"https://tracker.example/p.gif\"></body></html>",
        "externalReplyMessage": "<div>Out of the office.</div><div>Ask bo@example.org.</div>",
    })
}

#[test]
fn what_the_server_has_is_kept_as_plain_text() {
    let (mut s, account) = session();
    // Nothing read yet: the window is told to read them.
    let asked = run(&mut s, "account.automaticReplies", json!({"account": account}));
    assert_eq!((asked["known"].clone(), asked["ask"]["account"].as_u64()), (Value::Null, Some(account)));
    // The first account that can have them is the default.
    assert_eq!(run(&mut s, "account.automaticReplies", json!({}))["account"].as_u64(), Some(account));

    let known = run(&mut s, "sync.applyAutomaticReplies", json!({"account": account, "setting": server_setting()}));
    assert_eq!(known["status"], "scheduled");
    assert_eq!(known["audience"], "contactsOnly");
    // Times in the calendar's zone (UTC here); a zone by its Windows name is known.
    assert_eq!((known["start"].as_str(), known["end"].as_str()), (Some("2026-09-21T08:00"), Some("2026-09-28T07:00")));
    // Nothing of the HTML runs, is fetched or is kept: its text alone.
    let internal = known["internal"].as_str().unwrap();
    assert!(internal.contains("Away until Monday."), "{internal}");
    for gone in ["<", "steal", "color:red", "tracker"] {
        assert!(!internal.contains(gone), "{gone} in {internal}");
    }
    assert!(known["external"].as_str().unwrap().contains("Ask bo@example.org."));
    assert_eq!(known["on"].as_bool(), Some(true));
    assert!(any_on(&s));
    // What is known is what `account.automaticReplies` says.
    assert_eq!(run(&mut s, "account.automaticReplies", json!({"account": account}))["known"], known);

    // Turned off on the server: off here, and the status bar says nothing.
    run(&mut s, "sync.applyAutomaticReplies", json!({"account": account, "setting": {"status": "disabled"}}));
    assert!(!any_on(&s));
    // Never kept in the mailbox: another mailbox knows nothing of them.
    run(&mut s, "sync.applyAutomaticReplies", json!({"account": account, "setting": {"status": "alwaysEnabled"}}));
    assert!(any_on(&s));
    s.replace_mailbox(s.mailbox.clone());
    assert!(!any_on(&s) && s.auto_replies.is_empty());
}

#[test]
fn a_change_says_what_changes_in_graphs_names() {
    let (mut s, account) = session();
    run(&mut s, "sync.applyAutomaticReplies", json!({"account": account, "setting": server_setting()}));
    // Always on, with a new reply inside the organisation: the one outside, unchanged, isn't sent.
    let said = run(
        &mut s,
        "account.setAutomaticReplies",
        json!({"account": account, "status": "alwaysEnabled", "internal": "Back on the 28th.\n\n<b>Not</b> before & not after.", "external": "Out of the office.\n\nAsk bo@example.org.", "audience": "contactsOnly"}),
    );
    assert_eq!(said["pending"], "automaticReplies");
    let change = &said["change"];
    assert_eq!(change["status"], "alwaysEnabled");
    let html = change["internalReplyMessage"].as_str().unwrap();
    assert!(html.starts_with("<p>Back on the 28th.</p>") && html.contains("&lt;b&gt;Not&lt;/b&gt; before &amp; not after."), "{html}");
    assert!(change.get("externalReplyMessage").is_none() && change.get("externalAudience").is_none(), "{change}");
    assert!(change.get("scheduledStartDateTime").is_none());
    // Nothing changed here before the server has said so.
    assert_eq!(s.auto_replies.get(&account).map(|k| k.status.code()), Some("scheduled"));

    // During a time range, in the calendar's zone (two hours east here), sent in UTC.
    let mut east = Session::at(crate::sample::mailbox(NOW, 0), NOW, 120);
    let added = run(&mut east, "account.add", json!({"email": "ada@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}));
    let account = added["id"].as_u64().unwrap();
    run(&mut east, "sync.applyAutomaticReplies", json!({"account": account, "setting": {"status": "disabled"}}));
    let said = run(
        &mut east,
        "account.setAutomaticReplies",
        json!({"account": account, "status": "scheduled", "start": "2026-10-01T09:00", "end": "2026-10-08T17:30", "audience": "none"}),
    );
    assert_eq!(said["change"]["scheduledStartDateTime"], json!({"dateTime": "2026-10-01T07:00:00", "timeZone": "UTC"}));
    assert_eq!(said["change"]["scheduledEndDateTime"], json!({"dateTime": "2026-10-08T15:30:00", "timeZone": "UTC"}));
    assert_eq!(said["change"]["externalAudience"], "none");
    // Off: the status alone.
    assert_eq!(
        run(&mut east, "account.setAutomaticReplies", json!({"account": account, "status": "disabled"}))["change"],
        json!({"status": "disabled"})
    );
}

#[test]
fn what_cant_be_is_refused_with_a_reason() {
    let (mut s, account) = session();
    let refused = |s: &mut Session, params: Value| s.run("account.setAutomaticReplies", &params).unwrap_err().to_string();
    // Before the server's setting is read, nothing is changed: empty replies would be written
    // over the owner's.
    let blind = json!({"account": account, "status": "disabled", "internal": "", "external": "", "audience": "all"});
    assert_eq!(refused(&mut s, blind), NOT_READ);
    run(&mut s, "sync.applyAutomaticReplies", json!({"account": account, "setting": {"status": "disabled"}}));
    // A time range with no end, an end before its start, times that aren't.
    assert!(refused(&mut s, json!({"account": account, "status": "scheduled", "start": "2026-10-01T09:00"})).contains("`end`"));
    assert!(
        refused(&mut s, json!({"account": account, "status": "scheduled", "start": "2026-10-08T09:00", "end": "2026-10-01T09:00"}))
            .contains("after its start")
    );
    for odd in [json!("next Tuesday"), json!(7), json!("0001-01-01T00:00"), json!("2026-13-01T00:00"), json!("9".repeat(5000))] {
        assert!(
            s.run("account.setAutomaticReplies", &json!({"account": account, "status": "scheduled", "start": odd, "end": "2026-10-08T09:00"}))
                .is_err(),
            "{odd}"
        );
    }
    assert!(refused(&mut s, json!({"account": account, "status": "sometimes"})).contains("status"));
    assert!(refused(&mut s, json!({"account": account, "status": "alwaysEnabled", "audience": "everybody"})).contains("audience"));
    assert!(refused(&mut s, json!({"account": account, "status": "alwaysEnabled", "audience": 3})).contains("audience"));
    let long = "x".repeat(MAX_REPLY_TEXT + 1);
    assert!(refused(&mut s, json!({"account": account, "status": "alwaysEnabled", "internal": long})).contains("characters"));
    // Control characters are dropped from what is typed, line breaks kept.
    let said = run(&mut s, "account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled", "internal": "a\u{0}b\r\nc\u{1b}"}));
    assert_eq!(said["change"]["internalReplyMessage"], "<p>ab<br>\nc</p>\n");

    // An account that doesn't sign in with Microsoft, and a mailbox on this computer: why not.
    let imap = run(&mut s, "account.add", json!({"email": "bo@example.org", "incoming": {"host": "imap.example.org"}}));
    let imap = imap["id"].as_u64().unwrap();
    for id in ["account.automaticReplies", "account.setAutomaticReplies"] {
        let said = s.run(id, &json!({"account": imap, "status": "disabled"})).unwrap_err().to_string();
        assert!(said.contains("IMAP has no standard way") && said.contains("Sieve"), "{said}");
    }
    assert!(s.run("account.automaticReplies", &json!({"account": 999_999})).is_err());
    let mut alone = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    assert_eq!(alone.run("account.automaticReplies", &json!({})).unwrap_err().to_string(), NOT_MICROSOFT);
    // A shared mailbox's are its managers'.
    if let Some(a) = s.mailbox.accounts.iter_mut().find(|a| a.id == account) {
        a.kind = mailcraft_model::AccountKind::Microsoft;
    }
    let shared = run(&mut s, "account.addSharedMailbox", json!({"account": account, "address": "team@example.org"}));
    let id = shared["id"].as_u64().unwrap();
    assert_eq!(s.run("account.automaticReplies", &json!({"account": id})).unwrap_err().to_string(), SHARED);
    // (The account kept in step through Graph has them as before.)
    run(&mut s, "account.automaticReplies", json!({"account": account}));

    // Working offline: what is known is said, the server isn't to be asked, and nothing is changed.
    run(&mut s, "sync.offline", json!({"value": true}));
    assert!(run(&mut s, "account.automaticReplies", json!({"account": account})).get("ask").is_none());
    assert!(refused(&mut s, json!({"account": account, "status": "disabled"})).contains("offline"));
}

#[test]
fn a_hostile_answer_is_cut_or_refused() {
    let (mut s, account) = session();
    for odd in [json!(null), json!("on"), json!([1])] {
        assert!(s.run("sync.applyAutomaticReplies", &json!({"account": account, "setting": odd})).is_err(), "{odd}");
    }
    // What is no setting in it is none: off, no replies.
    for odd in [json!({"status": 7}), json!({"internalReplyMessage": {"html": 1}})] {
        let known = run(&mut s, "sync.applyAutomaticReplies", json!({"account": account, "setting": odd}));
        assert_eq!((known["status"].as_str(), known["internal"].as_str()), (Some("disabled"), Some("")));
    }
    // Unknown words are no status; a zone nobody knows is UTC; times that aren't are none.
    let odd = json!({
        "status": "sometimes",
        "scheduledStartDateTime": {"dateTime": "2026-09-21T08:00:00", "timeZone": "Atlantis Standard Time"},
        "scheduledEndDateTime": {"dateTime": "the day after", "timeZone": "UTC"},
        "internalReplyMessage": "<p>".repeat(100_000),
        "externalReplyMessage": format!("<p>{}</p>", "word ".repeat(40_000)),
    });
    let known = run(&mut s, "sync.applyAutomaticReplies", json!({"account": account, "setting": odd}));
    assert_eq!((known["status"].as_str(), known["start"].as_str(), known["end"].clone()), (Some("disabled"), Some("2026-09-21T08:00"), Value::Null));
    assert!(known["external"].as_str().unwrap().chars().count() <= MAX_REPLY_TEXT);
    assert_eq!(known["on"], false);
    // Only the host's to say.
    assert_eq!(agent::access("sync.applyAutomaticReplies"), Some(Access::Never));
}

#[test]
fn agents_may_read_the_replies_and_need_the_owner_to_change_them() {
    assert_eq!(agent::access("account.automaticReplies"), Some(Access::Safe));
    assert_eq!(agent::access("account.setAutomaticReplies"), Some(Access::Owner(Class::Send)));
    let (s, account) = session();
    let what = agent::describe(&s, "account.setAutomaticReplies", &json!({"account": account, "status": "alwaysEnabled", "internal": "Away"}));
    assert!(what.contains("ada@example.org") && what.contains("on the owner's behalf") && what.contains("Away"), "{what}");
    let off = agent::describe(&s, "account.setAutomaticReplies", &json!({"account": account, "status": "disabled"}));
    assert!(off.contains("turn off"), "{off}");
}
