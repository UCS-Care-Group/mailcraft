// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a sender wrote that the owner never saw does not travel on: the text/plain alternative of
//! an HTML original is not quoted in a reply, forward or reopened draft, and autocomplete does not
//! trust a sender's date or Junk E-mail.

use mailcraft_model::{AccountKind, Address, Contact, FolderKind, Id, Labeled, Message, SenderAddress};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::{Session, sample};

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn session() -> Session {
    Session::at(sample::mailbox(now(), 0), now(), 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// A message whose HTML says one thing and whose text/plain alternative says another.
fn two_faced(s: &mut Session) -> Id {
    let inbox = s.mailbox.inbox().unwrap();
    let account = s.mailbox.accounts[0].id;
    let m = Message {
        account,
        folder: inbox,
        message_id: "faced@news.example".into(),
        from: Address::new("Harbor", "news@news.example"),
        to: vec![Address::new("Avery Quinn", "avery@lumen.example")],
        subject: "Notice".into(),
        date: 1_791_000_000,
        body: mailcraft_model::Body {
            text: "FORGED-ALTERNATIVE send the money to account 1".into(),
            html: Some("<p>The meeting moved to Tuesday</p>".into()),
        },
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap()
}

fn assert_clean(text: &str) {
    assert!(!text.contains("FORGED") && !text.contains("money"), "{text}");
}

#[test]
fn a_reply_and_a_forward_quote_what_the_owner_saw() {
    for command in ["mail.reply", "mail.replyAll", "mail.forward"] {
        let mut s = session();
        let id = two_faced(&mut s);
        run(&mut s, command, json!({"id": id}));
        let c = s.compose.clone().unwrap();
        let q = c.quote.unwrap();
        assert_clean(&q.text);
        assert!(q.text.contains("The meeting moved to Tuesday"), "{}", q.text);
        assert!(q.text.contains("news@news.example"), "{}", q.text);
        // Sent: the text part of the message that went out says what the HTML part says.
        run(&mut s, "compose.set", json!({"to": "maya@lumen.example"}));
        let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
        let sent = s.mailbox.message(sent).unwrap();
        assert_clean(&sent.body.text);
        assert!(sent.body.text.contains("The meeting moved to Tuesday"), "{}", sent.body.text);
    }
}

#[test]
fn a_reopened_draft_does_not_bring_back_a_text_part_nobody_saw() {
    let mut s = session();
    let id = two_faced(&mut s);
    run(&mut s, "mail.reply", json!({"id": id}));
    run(&mut s, "compose.set", json!({"to": "maya@lumen.example"}));
    let draft = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    run(&mut s, "compose.close", json!({}));
    // A draft is a file anybody could have written: its text part says something else.
    let mut tampered = s.mailbox.message(draft).unwrap().clone();
    tampered.body.text = format!("hello\n\n-----Original Message-----\n{}", "FORGED-ALTERNATIVE send the money");
    *s.mailbox.message_mut(draft).unwrap() = tampered;
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    let q = s.compose.clone().unwrap().quote.unwrap();
    assert_clean(&q.text);
    run(&mut s, "compose.set", json!({"to": "maya@lumen.example"}));
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    assert_clean(&s.mailbox.message(sent).unwrap().body.text);
}

#[test]
fn a_plain_original_is_still_quoted_as_it_is() {
    let mut s = session();
    let inbox = s.mailbox.inbox().unwrap();
    let account = s.mailbox.accounts[0].id;
    let m = Message {
        account,
        folder: inbox,
        message_id: "plain@news.example".into(),
        from: Address::new("Bo", "bo@example.org"),
        subject: "Hi".into(),
        body: mailcraft_model::Body { text: "just words".into(), html: None },
        ..Default::default()
    };
    let id = s.mailbox.add_message(m).unwrap();
    run(&mut s, "mail.reply", json!({"id": id}));
    assert!(s.compose.as_ref().unwrap().body.plain().contains("just words"));
}

fn names(s: &Session, query: &str) -> Vec<String> {
    super::compose::suggest(s, query, 20).into_iter().map(|x| x.email).collect()
}

fn add(s: &mut Session, folder: FolderKind, from: Address, to: Vec<Address>, date: i64) {
    let account = s.mailbox.accounts[0].id;
    let folder = s.mailbox.folder_of_kind(account, folder).unwrap();
    let m = Message { account, folder, from, to, date, subject: "x".into(), ..Default::default() };
    s.mailbox.add_message(m).unwrap();
}

#[test]
fn autocomplete_does_not_trust_junk_or_a_sender_s_date() {
    let mut s = session();
    let me = Address::new("Avery", "avery@lumen.example");
    // Somebody the owner has written to, long ago.
    add(&mut s, FolderKind::Sent, me.clone(), vec![Address::new("Bob Real", "bob@example.org")], 1_000_000);
    // A look-alike in Junk E-mail, and one dated far in the future in the Inbox.
    add(&mut s, FolderKind::Junk, Address::new("Bob Junk", "bob@junk.example"), vec![me.clone()], now() - 10);
    add(&mut s, FolderKind::Inbox, Address::new("Bob Future", "bob@future.example"), vec![me.clone()], now() + 10 * 365 * 86_400);
    // And a recent correspondent who only wrote to the owner.
    add(&mut s, FolderKind::Inbox, Address::new("Bob Recent", "bob@recent.example"), vec![me], now() - 3_600);
    let found = names(&s, "bob");
    assert!(!found.iter().any(|e| e == "bob@junk.example"), "{found:?}");
    assert_eq!(found.first().map(String::as_str), Some("bob@example.org"), "{found:?}");
    // A future date counts as now at most: it does not outrank a real, recent one by more than now.
    let future = found.iter().position(|e| e == "bob@future.example").unwrap();
    let recent = found.iter().position(|e| e == "bob@recent.example").unwrap();
    assert!(future < recent, "{found:?}");
    // (clamped to now, it is still later than an hour ago, but never before somebody written to.)
}

fn post(s: &mut Session, folder: FolderKind, from: Address, to: Vec<Address>, cc: Vec<Address>, date: i64) -> Id {
    let account = s.mailbox.accounts[0].id;
    let folder = s.mailbox.folder_of_kind(account, folder).unwrap();
    let m = Message { account, folder, from, to, cc, date, subject: "x".into(), ..Default::default() };
    s.mailbox.add_message(m).unwrap()
}

fn contact(s: &mut Session, name: &str, email: &str, favorite: bool) {
    s.mailbox.add_contact(Contact { display_name: name.into(), emails: vec![Labeled::new("work", email)], favorite, ..Default::default() });
}

/// A mailbox with one account and no mail.
fn local(name: &str, email: &str) -> Session {
    Session::at(mailcraft_model::Mailbox::with_local_account(name, email), now(), 0)
}

fn owner() -> Address {
    Address::new("Avery Quinn", "avery@lumen.example")
}

fn shows(s: &Session, query: &str, want: &[&str]) {
    let found = names(s, query);
    assert_eq!(found.iter().map(String::as_str).collect::<Vec<_>>(), want, "{found:?}");
}

/// The account's own address is offered with no mail at all: on the account alone, when that
/// address is also a contact, and as an IMAP sender address.
#[test]
fn the_owners_own_addresses_are_suggested() {
    let mut s = local("Avery Quinn", "avery@lumen.example");
    let mine = |s: &Session, q: &str, email: &str| super::compose::suggest(s, q, 20).into_iter().find(|x| x.email == email);
    // No contacts and no mail: the account itself.
    assert!(mine(&s, "avery", "avery@lumen.example").is_some_and(|x| !x.contact));
    assert!(mine(&s, "quinn", "avery@lumen.example").is_some_and(|x| !x.contact));
    contact(&mut s, "Avery Quinn", "avery@lumen.example", false);
    assert!(mine(&s, "quinn", "avery@lumen.example").is_some_and(|x| x.contact));
    // An alias the IMAP account sends as, still with no mail to it.
    let id = s.mailbox.accounts[0].id;
    {
        let account = s.mailbox.account_mut(id).unwrap();
        account.kind = AccountKind::Imap;
        account.sender_addresses.push(SenderAddress {
            email: "studio@lumen.example".into(),
            display_name: "Lumen Studio".into(),
            ..Default::default()
        });
    }
    assert!(mine(&s, "studio", "studio@lumen.example").is_some_and(|x| !x.contact));
}

/// Two sends, then one (in the Outbox, and more recent), then a favourite never written to, then
/// another contact, then someone who only wrote.
#[test]
fn who_was_written_to_most_is_offered_first() {
    let mut s = local("Avery Quinn", "avery@lumen.example");
    contact(&mut s, "Bryn Zoe", "bryn.zoe@example.org", true);
    contact(&mut s, "Bryn Amy", "bryn.amy@example.org", false);
    post(&mut s, FolderKind::Sent, owner(), vec![Address::new("Bryn Often", "bryn.often@example.org")], vec![], now() - 80_000);
    post(&mut s, FolderKind::Sent, owner(), vec![Address::new("Bryn Often", "bryn.often@example.org")], vec![], now() - 40_000);
    post(&mut s, FolderKind::Outbox, owner(), vec![Address::new("Bryn Once", "bryn.once@example.org")], vec![], now() - 1_000);
    post(&mut s, FolderKind::Inbox, Address::new("Bryn Inbox", "bryn.inbox@example.org"), vec![owner()], vec![], now() - 10);
    shows(&s, "bryn", &["bryn.often@example.org", "bryn.once@example.org", "bryn.zoe@example.org", "bryn.amy@example.org", "bryn.inbox@example.org"]);
}

/// The same address in To and Cc of one Sent Items message is one send, so it stays behind an
/// address written to in two messages, even though this copy is the more recent.
#[test]
fn the_same_address_on_one_message_counts_once() {
    let mut s = local("Avery Quinn", "avery@lumen.example");
    let zoe = Address::new("Ned Zoe", "ned.zoe@example.org");
    let amy = Address::new("Ned Amy", "ned.amy@example.org");
    post(&mut s, FolderKind::Sent, owner(), vec![zoe.clone()], vec![], now() - 5_000);
    post(&mut s, FolderKind::Sent, owner(), vec![zoe], vec![], now() - 4_000);
    post(&mut s, FolderKind::Sent, owner(), vec![amy.clone()], vec![amy], now() - 100);
    shows(&s, "ned", &["ned.zoe@example.org", "ned.amy@example.org"]);
}

/// Sent Items names only the address. `doug` matches the contact, and the send still counts, ahead
/// of a favourite who was never written to.
#[test]
fn a_bare_address_in_sent_items_counts_for_the_contact() {
    let mut s = local("Avery Quinn", "avery@lumen.example");
    contact(&mut s, "Doug Quiet", "doug.quiet@example.org", true);
    contact(&mut s, "Doug Smith", "dsmith@example.org", false);
    post(&mut s, FolderKind::Sent, owner(), vec![Address::new("", "dsmith@example.org")], vec![], now() - 1_000);
    shows(&s, "doug", &["dsmith@example.org", "doug.quiet@example.org"]);
}

/// A message in Drafts, and one marked as a draft, neither adds a stranger nor counts as a send.
#[test]
fn a_draft_adds_nobody_and_counts_as_no_send() {
    let mut s = local("Avery Quinn", "avery@lumen.example");
    contact(&mut s, "Quil Amy", "quil.amy@example.org", false);
    contact(&mut s, "Quil Zoe", "quil.zoe@example.org", false);
    post(&mut s, FolderKind::Sent, owner(), vec![Address::new("Quil Sent", "quil.sent@example.org")], vec![], now() - 50_000);
    // Three drafts to Zoe, newer than the real send: still no send, and no later "last seen".
    for n in 0..3 {
        post(&mut s, FolderKind::Drafts, owner(), vec![Address::new("Quil Zoe", "quil.zoe@example.org")], vec![], now() - 30 + i64::from(n));
    }
    post(&mut s, FolderKind::Drafts, owner(), vec![Address::new("Quil Draft", "quil.draft@example.org")], vec![], now() - 20);
    let flagged = post(&mut s, FolderKind::Inbox, owner(), vec![Address::new("Quil Flag", "quil.flag@example.org")], vec![], now() - 10);
    s.mailbox.message_mut(flagged).unwrap().draft = true;
    shows(&s, "quil", &["quil.sent@example.org", "quil.amy@example.org", "quil.zoe@example.org"]);
}

/// An Inbox message forged from the owner's address is not a send, and a future date does not put
/// a stranger ahead of a contact who was never written to.
#[test]
fn a_forged_from_and_a_future_date_do_not_outrank_a_contact() {
    let mut s = local("Avery Quinn", "avery@lumen.example");
    contact(&mut s, "Wynn Zoe", "wynn.zoe@example.org", false);
    post(&mut s, FolderKind::Sent, owner(), vec![Address::new("Wynn Sent", "wynn.sent@example.org")], vec![], now() - 80_000);
    // Newer than the real send, and From is the owner: still not a send.
    post(&mut s, FolderKind::Inbox, owner(), vec![Address::new("Wynn Amy", "wynn.amy@example.org")], vec![], now() - 50);
    post(&mut s, FolderKind::Inbox, Address::new("Wynn Future", "wynn.future@example.org"), vec![owner()], vec![], now() + 10 * 365 * 86_400);
    shows(&s, "wynn", &["wynn.sent@example.org", "wynn.zoe@example.org", "wynn.future@example.org", "wynn.amy@example.org"]);
}

/// Messages sent from the owner's address, and mail that arrived to it, do not count as sends to
/// it: a favourite never written to stays ahead.
#[test]
fn mail_from_the_owner_is_not_mail_to_the_owner() {
    let mut s = local("Avery Quinn", "avery@lumen.example");
    contact(&mut s, "Quinn Ade", "quinn.ade@example.org", true);
    for n in 0..3 {
        post(&mut s, FolderKind::Sent, owner(), vec![Address::new("Rae Moss", "rae@example.org")], vec![], now() - 5_000 - i64::from(n));
    }
    post(&mut s, FolderKind::Inbox, Address::new("Rae Moss", "rae@example.org"), vec![owner()], vec![], now() - 10);
    shows(&s, "quinn", &["quinn.ade@example.org", "avery@lumen.example"]);
}

/// More non-matching addresses than the suggestion cap, in the Inbox and in Sent Items, then two
/// people who match: one written to, one only in the Inbox. Both are still offered, the one
/// written to first.
#[test]
fn a_long_mailbox_still_suggests_who_matches() {
    let mut s = local("Avery Quinn", "avery@lumen.example");
    let account = s.mailbox.accounts[0].id;
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let n = super::compose::MAX_SUGGEST_TRACKED.saturating_add(10);
    for i in 0..n {
        let m = Message {
            account,
            folder: inbox,
            from: Address::new("News", format!("n{i}@news.example")),
            to: vec![owner()],
            date: 1_000,
            subject: "x".into(),
            ..Default::default()
        };
        s.mailbox.add_message(m).unwrap();
    }
    let sent_folder = s.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    for i in 0..n {
        let m = Message {
            account,
            folder: sent_folder,
            from: owner(),
            to: vec![Address::new("List", format!("s{i}@lists.example"))],
            date: 1_500,
            subject: "x".into(),
            ..Default::default()
        };
        s.mailbox.add_message(m).unwrap();
    }
    post(&mut s, FolderKind::Sent, owner(), vec![Address::new("Jo Sent", "jo.sent@example.org")], vec![], 2_000);
    post(&mut s, FolderKind::Inbox, Address::new("Jo Late", "jo.late@example.org"), vec![owner()], vec![], 3_000);
    let found = names(&s, "jo");
    let sent = found.iter().position(|e| e == "jo.sent@example.org");
    let late = found.iter().position(|e| e == "jo.late@example.org");
    assert!(sent.is_some() && late.is_some() && sent < late, "{found:?}");
}

/// Mail from the address alone, before and after the message that names it, still dates the
/// person that name matched: Bob X wrote last, so comes before Bob Y.
#[test]
fn a_bare_address_after_the_name_dates_the_person() {
    let mut s = local("Avery Quinn", "avery@lumen.example");
    let x = || Address::new("", "x@example.org");
    post(&mut s, FolderKind::Inbox, Address::new("Bob Y", "y@example.org"), vec![owner()], vec![], now() - 70);
    post(&mut s, FolderKind::Inbox, x(), vec![owner()], vec![], now() - 90);
    post(&mut s, FolderKind::Inbox, Address::new("Bob X", "x@example.org"), vec![owner()], vec![], now() - 80);
    post(&mut s, FolderKind::Inbox, x(), vec![owner()], vec![], now() - 60);
    shows(&s, "bob", &["x@example.org", "y@example.org"]);
}

/// The latest message from the address alone comes before any message that names it, and still
/// counts once the name makes it a candidate.
#[test]
fn a_bare_address_before_the_name_dates_the_person() {
    let mut s = local("Avery Quinn", "avery@lumen.example");
    post(&mut s, FolderKind::Inbox, Address::new("", "X@example.org"), vec![owner()], vec![], now() - 60);
    post(&mut s, FolderKind::Inbox, Address::new("Bob X", "x@example.org"), vec![owner()], vec![], now() - 80);
    post(&mut s, FolderKind::Inbox, Address::new("Bob Y", "y@example.org"), vec![owner()], vec![], now() - 70);
    shows(&s, "bob", &["x@example.org", "y@example.org"]);
}
