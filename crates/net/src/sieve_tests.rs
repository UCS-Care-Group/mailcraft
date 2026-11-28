// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sieve scripts through ManageSieve, against the fake server.

use std::time::{Duration, Instant};

use mailcraft_model::{Account, AccountKind, OAuth, OAuthProvider, Security, Server};

use crate::NetError;
use crate::fake::Fake;
use crate::fake_sieve::{FakeSieve, Opening, SieveWorld};
use crate::sieve::{Checked, LooksLike, Reach, check, check_with, read_with};
use crate::sieve::{OwnChange, OwnWritten, write_own};
use crate::sieve::{PORT, ScriptChange, SieveError, Written, deactivate, digest, read, server_for, write};
use crate::transport::Patience;

const PASSWORD: &str = "correct horse";
const TOKEN: &str = "tok.en-42";
const OURS: &str = "require \"vacation\";\r\nvacation :days 1 \"I am away.\";\r\n";

fn world() -> SieveWorld {
    SieveWorld { user: "ada".into(), password: PASSWORD.into(), ..Default::default() }
}

/// An IMAP account whose server is on this computer, and the fake's address next to it.
fn account(fake: &FakeSieve) -> (Account, Server) {
    let account = Account {
        id: 7,
        email: "ada@example.org".into(),
        kind: AccountKind::Imap,
        incoming: Some(Server { host: "127.0.0.1".into(), port: 1, security: Security::None }),
        username: "ada".into(),
        ..Default::default()
    };
    let server = Server { host: "127.0.0.1".into(), port: fake.port, security: Security::None };
    (account, server)
}

/// A change of MailCraft's script to `text`, MailCraft having last written the digest `last_written`.
fn change<'a>(text: &'a str, last_written: Option<&'a str>) -> ScriptChange<'a> {
    ScriptChange { name: "mailcraft", text, last_written, activate: true }
}

#[test]
fn a_digest_is_sha256_in_hex() {
    assert_eq!(digest(""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(digest("keep;").len(), 64);
}

#[test]
fn the_server_is_the_imap_host_on_port_4190_with_starttls() {
    let mut account =
        Account { incoming: Some(Server { host: " mail.example.org ".into(), port: 993, security: Security::Tls }), ..Default::default() };
    assert_eq!(server_for(&account).unwrap(), Server { host: "mail.example.org".into(), port: PORT, security: Security::StartTls });
    account.incoming = Some(Server { host: "mail.example.org".into(), port: 143, security: Security::StartTls });
    assert_eq!(server_for(&account).unwrap().security, Security::StartTls);
    // Unencrypted only where IMAP is unencrypted on this computer.
    account.incoming = Some(Server { host: "127.0.0.1".into(), port: 143, security: Security::None });
    assert_eq!(server_for(&account).unwrap().security, Security::None);
    account.incoming = Some(Server { host: "mail.example.org".into(), port: 143, security: Security::None });
    assert_eq!(server_for(&account).unwrap().security, Security::StartTls);
    account.incoming = None;
    assert!(matches!(server_for(&account), Err(NetError::Config(_))));
    // The host and port the owner named; plain only where both are on this computer.
    account.incoming = Some(Server { host: "mail.example.org".into(), port: 993, security: Security::Tls });
    account.sieve_host = "sieve.example.org".into();
    account.sieve_port = 2000;
    assert_eq!(server_for(&account).unwrap(), Server { host: "sieve.example.org".into(), port: 2000, security: Security::StartTls });
    account.incoming = Some(Server { host: "127.0.0.1".into(), port: 143, security: Security::None });
    assert_eq!(server_for(&account).unwrap().security, Security::StartTls);
    account.sieve_host = "localhost".into();
    assert_eq!(server_for(&account).unwrap().security, Security::None);
    // Port 0 means ManageSieve's own.
    assert_eq!(
        crate::transport::port(&Server { host: "x".into(), port: 0, security: Security::StartTls }, crate::transport::Service::ManageSieve),
        PORT
    );
}

#[test]
fn reading_signs_in_and_brings_the_scripts_and_the_one_asked_for() {
    let fake = Fake::sieve(SieveWorld {
        scripts: vec![("mine".into(), "keep;".into()), ("mailcraft".into(), OURS.into())],
        active: Some("mailcraft".into()),
        ..world()
    });
    let (account, server) = account(&fake);
    let state = read(&account, &server, PASSWORD, "mailcraft").unwrap();
    assert_eq!(state.script.as_deref(), Some(OURS));
    assert_eq!(state.active(), Some("mailcraft"));
    assert_eq!(state.active_script, None);
    assert_eq!(state.scripts.len(), 2);
    assert!(state.has_extension("vacation") && state.has_extension("date"));
    assert_eq!(state.implementation, "Fake Sieve");
    // A script that isn't there is not asked for; the active one, another's, is read to be shown.
    let state = read(&account, &server, PASSWORD, "other").unwrap();
    assert_eq!(state.script, None);
    assert_eq!(state.active_script, Some(("mailcraft".to_string(), OURS.to_string())));
    let world = fake.world();
    assert_eq!(world.sign_ins, 2);
    assert_eq!(world.log.iter().filter(|l| l.starts_with("GETSCRIPT")).count(), 2);
    // Nothing of the scripts is in debug output.
    assert!(!format!("{state:?}").contains("mine"));
}

#[test]
fn a_first_write_makes_the_script_and_makes_it_active() {
    let fake = Fake::sieve(world());
    let (account, server) = account(&fake);
    let written = write(&account, &server, PASSWORD, &change(OURS, None)).unwrap();
    assert_eq!(written, Written::Done { active: true, warnings: None });
    let world = fake.world();
    assert_eq!(world.script("mailcraft"), Some(OURS));
    assert_eq!(world.active.as_deref(), Some("mailcraft"));
}

#[test]
fn a_script_mailcraft_wrote_is_written_over_and_stays_active() {
    let fake = Fake::sieve(SieveWorld { scripts: vec![("mailcraft".into(), OURS.into())], active: Some("mailcraft".into()), ..world() });
    let (account, server) = account(&fake);
    let new = "require \"vacation\";\r\nvacation \"Back on Monday.\";\r\n";
    let ours = digest(OURS);
    assert_eq!(write(&account, &server, PASSWORD, &change(new, Some(&ours))).unwrap(), Written::Done { active: true, warnings: None });
    assert_eq!(fake.world().script("mailcraft"), Some(new));
    // Writing what is there already writes nothing.
    assert_eq!(write(&account, &server, PASSWORD, &change(new, Some(&ours))).unwrap(), Written::Done { active: true, warnings: None });
    assert_eq!(fake.world().log.iter().filter(|l| l.starts_with("PUTSCRIPT")).count(), 1);
}

#[test]
fn a_script_changed_elsewhere_is_never_written_over() {
    let theirs = "require \"fileinto\";\r\nfileinto \"Lists\";\r\n";
    let fake = Fake::sieve(SieveWorld { scripts: vec![("mailcraft".into(), theirs.into())], active: Some("mailcraft".into()), ..world() });
    let (account, server) = account(&fake);
    // Changed since MailCraft wrote it.
    let written = write(&account, &server, PASSWORD, &change(OURS, Some(&digest("keep;")))).unwrap();
    assert_eq!(written, Written::ChangedElsewhere { now: Some(theirs.into()) });
    // The text itself where the digest should be is no digest of it.
    let written = write(&account, &server, PASSWORD, &change(OURS, Some(theirs))).unwrap();
    assert_eq!(written, Written::ChangedElsewhere { now: Some(theirs.into()) });
    // Made by somebody else before MailCraft ever wrote one.
    let written = write(&account, &server, PASSWORD, &change(OURS, None)).unwrap();
    assert_eq!(written, Written::ChangedElsewhere { now: Some(theirs.into()) });
    let world = fake.world();
    assert_eq!(world.script("mailcraft"), Some(theirs));
    assert!(!world.heard("PUTSCRIPT") && !world.heard("SETACTIVE"));
}

#[test]
fn a_script_removed_elsewhere_is_not_brought_back_unasked() {
    let fake = Fake::sieve(world());
    let (account, server) = account(&fake);
    assert_eq!(write(&account, &server, PASSWORD, &change(OURS, Some(&digest("keep;")))).unwrap(), Written::ChangedElsewhere { now: None });
    assert!(fake.world().scripts.is_empty());
}

#[test]
fn the_owners_active_script_stays_active() {
    let fake = Fake::sieve(SieveWorld { scripts: vec![("roundcube".into(), "keep;".into())], active: Some("roundcube".into()), ..world() });
    let (account, server) = account(&fake);
    let written = write(&account, &server, PASSWORD, &change(OURS, None)).unwrap();
    assert_eq!(written, Written::AnotherActive { active: "roundcube".into(), warnings: None });
    let world = fake.world();
    assert_eq!(world.active.as_deref(), Some("roundcube"));
    assert_eq!(world.script("mailcraft"), Some(OURS));
    assert!(!world.heard("SETACTIVE"));
    drop(world);
    // Asked to stop: the owner's script is not touched.
    assert!(!deactivate(&account, &server, PASSWORD, "mailcraft").unwrap());
    assert_eq!(fake.world().active.as_deref(), Some("roundcube"));
}

#[test]
fn a_script_not_to_be_activated_is_only_written() {
    let fake = Fake::sieve(world());
    let (account, server) = account(&fake);
    let written = write(&account, &server, PASSWORD, &ScriptChange { activate: false, ..change(OURS, None) }).unwrap();
    assert_eq!(written, Written::Done { active: false, warnings: None });
    assert_eq!(fake.world().active, None);
}

#[test]
fn deactivating_stops_only_mailcrafts_own_script() {
    let fake = Fake::sieve(SieveWorld { scripts: vec![("mailcraft".into(), OURS.into())], active: Some("mailcraft".into()), ..world() });
    let (account, server) = account(&fake);
    assert!(deactivate(&account, &server, PASSWORD, "mailcraft").unwrap());
    let world = fake.world();
    assert_eq!(world.active, None);
    assert_eq!(world.script("mailcraft"), Some(OURS));
}

#[test]
fn a_script_the_server_refuses_changes_nothing() {
    let fake = Fake::sieve(SieveWorld { refuse_scripts_with: Some("bogus".into()), ..world() });
    let (account, server) = account(&fake);
    let err = write(&account, &server, PASSWORD, &change("bogus;", None)).unwrap_err();
    assert!(
        matches!(&err, SieveError::Net(NetError::Proto(mailcraft_proto::ProtoError::Rejected(why))) if why.to_string().contains("unknown command")),
        "{err:?}"
    );
    let world = fake.world();
    assert!(world.scripts.is_empty() && world.active.is_none());
}

#[test]
fn a_wrong_password_is_a_refused_sign_in_that_doesnt_repeat_it() {
    let fake = Fake::sieve(world());
    let (account, server) = account(&fake);
    let err = read(&account, &server, "wrong horse battery", "mailcraft").unwrap_err();
    assert!(err.is_refused_sign_in(), "{err:?}");
    assert!(!err.to_string().contains("wrong horse"), "{err}");
}

#[test]
fn an_account_with_a_token_signs_in_with_oauthbearer() {
    let fake = Fake::sieve(SieveWorld { user: "ada@example.org".into(), token: TOKEN.into(), ..world() });
    let (mut account, server) = account(&fake);
    account.username.clear();
    account.oauth = Some(OAuth { provider: OAuthProvider::Microsoft, ..Default::default() });
    read(&account, &server, TOKEN, "mailcraft").unwrap();
    assert!(fake.world().heard("AUTHENTICATE \"OAUTHBEARER\""));
    let err = read(&account, &server, "expired", "mailcraft").unwrap_err();
    assert!(err.is_refused_sign_in(), "{err:?}");
    // No token: no connection.
    let before = fake.world().connections;
    assert!(matches!(read(&account, &server, "", "mailcraft"), Err(SieveError::Net(NetError::Config(_)))));
    assert_eq!(fake.world().connections, before);
}

#[test]
fn a_google_token_never_goes_to_another_server() {
    let fake = Fake::sieve(SieveWorld { user: "ada@example.org".into(), token: TOKEN.into(), ..world() });
    let (mut account, mut server) = account(&fake);
    account.oauth = Some(OAuth { provider: OAuthProvider::Google, ..Default::default() });
    // (The tests let Google's token go to 127.0.0.1 alone; `localhost` stands for any other server.)
    account.incoming = Some(Server { host: "localhost".into(), port: 1, security: Security::None });
    server.host = "localhost".into();
    assert!(matches!(read(&account, &server, TOKEN, "mailcraft"), Err(SieveError::Net(NetError::Config(_)))));
    assert_eq!(fake.world().connections, 0);
}

#[test]
fn the_sign_in_never_goes_to_a_host_other_than_the_mail_servers() {
    let fake = Fake::sieve(world());
    let (mut account, server) = account(&fake);
    account.incoming = Some(Server { host: "localhost".into(), port: 1, security: Security::None });
    for result in [
        read(&account, &server, PASSWORD, "mailcraft").map(|_| ()),
        write(&account, &server, PASSWORD, &change(OURS, None)).map(|_| ()),
        deactivate(&account, &server, PASSWORD, "mailcraft").map(|_| ()),
    ] {
        assert!(matches!(result, Err(SieveError::Net(NetError::Config(_)))), "{result:?}");
    }
    account.incoming = None;
    assert!(matches!(read(&account, &server, PASSWORD, "mailcraft"), Err(SieveError::Net(NetError::Config(_)))));
    assert_eq!(fake.world().connections, 0);
    // A host the owner named in Account Settings is one the sign-in may go to.
    account.incoming = Some(Server { host: "localhost".into(), port: 1, security: Security::None });
    account.sieve_host = "127.0.0.1".into();
    read(&account, &server, PASSWORD, "mailcraft").unwrap();
    account.sieve_host.clear();
    // The same host however it is written.
    account.incoming = Some(Server { host: "[127.0.0.1].".into(), port: 1, security: Security::None });
    read(&account, &server, PASSWORD, "mailcraft").unwrap();
}

#[test]
fn without_starttls_on_offer_nothing_is_sent_in_the_clear() {
    let fake = Fake::sieve(world());
    let (account, mut server) = account(&fake);
    server.security = Security::StartTls;
    let err = read(&account, &server, PASSWORD, "mailcraft").unwrap_err();
    let at = format!("127.0.0.1:{}", fake.port);
    assert!(matches!(&err, SieveError::Reach(Reach::NoStartTls { at: there }) if *there == at), "{err:?}");
    assert!(err.to_string().contains(&at) && err.to_string().contains("STARTTLS"), "{err}");
    assert!(!fake.world().heard("AUTHENTICATE"));
}

#[test]
fn starttls_comes_before_the_sign_in() {
    let fake = Fake::sieve(SieveWorld { offer_starttls: true, ..world() });
    let (account, mut server) = account(&fake);
    server.security = Security::StartTls;
    // The fake's "handshake" is none: the connection fails before anything of the owner's goes.
    let err = read(&account, &server, PASSWORD, "mailcraft").unwrap_err();
    assert!(matches!(&err, SieveError::Reach(Reach::Tls { at, .. }) if *at == format!("127.0.0.1:{}", fake.port)), "{err:?}");
    let world = fake.world();
    assert!(world.heard("STARTTLS") && !world.heard("AUTHENTICATE"));
}

#[test]
fn a_referral_to_another_server_is_not_followed() {
    let fake = Fake::sieve(SieveWorld { referral: true, ..world() });
    let (account, server) = account(&fake);
    let err = read(&account, &server, PASSWORD, "mailcraft").unwrap_err();
    assert!(matches!(&err, SieveError::Net(NetError::Proto(mailcraft_proto::ProtoError::Rejected(_)))), "{err:?}");
    assert!(!err.to_string().contains("elsewhere"));
    assert_eq!(fake.world().connections, 1);
}

/// Patience of a fraction of a second for the greeting: the tests of silent servers are quick.
fn hurried() -> Patience {
    Patience { io: Duration::from_millis(300), ..Patience::default() }
}

/// What reading the scripts of a fake server that begins as `opening` says comes to, quickly,
/// and where it is.
fn opened_by(opening: Opening, security: Security) -> (SieveError, String, FakeSieve) {
    let fake = Fake::sieve(SieveWorld { opening, ..world() });
    let (account, mut server) = account(&fake);
    server.security = security;
    let started = Instant::now();
    let err = read_with(&account, &server, PASSWORD, "mailcraft", &hurried(), None).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    // Whatever it was, nothing of the owner's went there, nor is it said.
    assert!(!fake.world().heard("AUTHENTICATE"));
    assert!(!err.to_string().contains(PASSWORD), "{err}");
    (err, format!("127.0.0.1:{}", fake.port), fake)
}

#[test]
fn a_closed_port_is_where_nothing_answers() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let account = Account {
        email: "ada@example.org".into(),
        kind: AccountKind::Imap,
        incoming: Some(Server { host: "127.0.0.1".into(), port: 1, security: Security::None }),
        username: "ada".into(),
        ..Default::default()
    };
    let server = Server { host: "127.0.0.1".into(), port, security: Security::None };
    let err = read(&account, &server, PASSWORD, "mailcraft").unwrap_err();
    let at = format!("127.0.0.1:{port}");
    assert!(matches!(&err, SieveError::Reach(Reach::NoAnswer { at: there, .. }) if *there == at), "{err:?}");
    assert!(err.to_string().starts_with(&format!("nothing answers at {at} (")), "{err}");
    let SieveError::Reach(reach) = &err else { panic!("{err:?}") };
    assert!(reach.advice().to_string().contains("Sieve server (Automatic Replies)"));
}

#[test]
fn a_server_that_says_nothing_is_said_to_say_nothing() {
    for security in [Security::None, Security::StartTls] {
        let (err, at, fake) = opened_by(Opening::Silent, security);
        assert!(matches!(&err, SieveError::Reach(Reach::Silent { at: there, tls: false }) if *there == at), "{err:?}");
        assert_eq!(err.to_string(), format!("MailCraft connected to {at}, but the server said nothing"));
        let SieveError::Reach(reach) = &err else { panic!("{err:?}") };
        assert!(reach.advice().to_string().contains("ssl = yes") && reach.advice().to_string().contains("proxy"));
        // A TLS hello was the one thing sent, on a connection of its own; nothing went on the first.
        assert_eq!(fake.world().connections, 2);
        assert!(fake.world().log.is_empty());
    }
}

#[test]
fn a_port_that_expects_encryption_from_the_start_is_named() {
    // Dovecot's `ssl = yes` on the sieve listener: silent until a TLS hello comes.
    let (err, at, fake) = opened_by(Opening::ImplicitTls, Security::StartTls);
    assert!(matches!(&err, SieveError::Reach(Reach::Silent { at: there, tls: true }) if *there == at), "{err:?}");
    assert!(err.to_string().contains("expects an encrypted connection from the start"), "{err}");
    let SieveError::Reach(reach) = &err else { panic!("{err:?}") };
    assert!(reach.advice().to_string().contains("ssl = no"));
    assert_eq!(fake.world().connections, 2);
}

#[test]
fn something_else_answering_is_said_to_be_something_else() {
    for (banner, looks_like) in [
        (&b"* OK [CAPABILITY IMAP4rev1 STARTTLS] Dovecot ready.\r\n"[..], Some(LooksLike::Imap)),
        (b"220 mail.example.org ESMTP\r\n", Some(LooksLike::Smtp)),
        (b"+OK POP3 ready\r\n", Some(LooksLike::Pop3)),
        (b"SSH-2.0-OpenSSH_9.6\r\n", Some(LooksLike::Ssh)),
        (b"\x16\x03\x03\x00\x05hello", Some(LooksLike::Tls)),
        (b"\x00\x01garbage that is no protocol\r\n", None),
        (b"(((\r\n", None),
    ] {
        let (err, at, fake) = opened_by(Opening::Says(banner.to_vec()), Security::StartTls);
        assert!(
            matches!(&err, SieveError::Reach(Reach::NotManageSieve { at: there, looks_like: l }) if *there == at && *l == looks_like),
            "{banner:?}: {err:?}"
        );
        let said = err.to_string();
        assert!(said.starts_with(&format!("something answers at {at}, but it isn't a ManageSieve server")), "{said}");
        // What it sent is never repeated.
        assert!(!said.contains("Dovecot") && !said.contains("garbage") && !said.contains("OpenSSH"), "{said}");
        assert_eq!(fake.world().connections, 1);
    }
    let (err, ..) = opened_by(Opening::Says(b"* OK IMAP4rev1\r\n".to_vec()), Security::StartTls);
    assert!(err.to_string().ends_with("it looks like an IMAP server"), "{err}");
}

#[test]
fn a_server_that_hangs_up_at_once_is_said_to() {
    let (err, at, _fake) = opened_by(Opening::Closes, Security::StartTls);
    assert!(matches!(&err, SieveError::Reach(Reach::Closed { at: there }) if *there == at), "{err:?}");
    assert_eq!(err.to_string(), format!("{at} closed the connection without a word"));
}

#[test]
fn a_managesieve_server_that_greets_with_a_refusal_is_its_own_trouble() {
    let (err, ..) = opened_by(Opening::Says(b"NO \"Too many connections.\"\r\n".to_vec()), Security::StartTls);
    assert!(matches!(&err, SieveError::Net(NetError::Proto(mailcraft_proto::ProtoError::Protocol(_)))), "{err:?}");
    assert!(err.to_string().contains("Too many connections"), "{err}");
}

#[test]
fn what_answered_is_known_by_its_first_words_alone() {
    assert_eq!(LooksLike::of(b"* PREAUTH"), Some(LooksLike::Imap));
    assert_eq!(LooksLike::of(b"* ok"), Some(LooksLike::Imap));
    assert_eq!(LooksLike::of(b"HTTP/1.1 400 Bad Request"), Some(LooksLike::Web));
    assert_eq!(LooksLike::of(b"-ERR"), Some(LooksLike::Pop3));
    assert_eq!(LooksLike::of(b"554-no"), Some(LooksLike::Smtp));
    assert_eq!(LooksLike::of(b"\x15\x03\x01\x00\x02"), Some(LooksLike::Tls));
    for nothing in [&b""[..], b"*", b"22", b"\"IMPLEMENTATION\"", b"OK", b"\x16\x02", b"999x"] {
        assert_eq!(LooksLike::of(nothing), None, "{nothing:?}");
    }
}

#[test]
fn checking_a_script_signs_in_sends_it_and_keeps_nothing() {
    let fake = Fake::sieve(world());
    let (account, server) = account(&fake);
    let started = Instant::now();
    assert_eq!(check_with(&account, &server, PASSWORD, OURS, &hurried(), None).unwrap(), Checked::Fine);
    assert!(started.elapsed() < Duration::from_secs(10));
    let world = fake.world();
    assert!(world.heard("AUTHENTICATE") && world.heard("CHECKSCRIPT"));
    assert!(world.scripts.is_empty() && world.active.is_none() && !world.heard("PUTSCRIPT"));
}

#[test]
fn a_check_brings_the_servers_errors_and_warnings_with_their_lines() {
    let refusal = "line 3: error: unknown command 'bogus'.\r\nline 7: error: missing ';'.\r\nyour password correct horse is wrong";
    let fake = Fake::sieve(SieveWorld {
        refuse_scripts_with: Some("bogus".into()),
        refusal: refusal.into(),
        warn_scripts_with: Some(("odd".into(), "line 2: warning: \u{7}odd".into())),
        ..world()
    });
    let (account, server) = account(&fake);
    let Checked::Errors(said) = check(&account, &server, PASSWORD, "bogus;").unwrap() else { panic!("expected errors") };
    assert_eq!(said, "line 3: error: unknown command 'bogus'.\nline 7: error: missing ';'.\nyour password *** is wrong");
    assert_eq!(check(&account, &server, PASSWORD, "# odd\r\nkeep;").unwrap(), Checked::Warnings("line 2: warning: odd".into()));
    // Debug output never shows the server's words.
    assert!(!format!("{:?}", Checked::Errors(said)).contains("bogus"));
    let world = fake.world();
    assert!(world.scripts.is_empty());
}

#[test]
fn a_server_from_before_rfc_5804_isnt_asked_to_check() {
    let fake = Fake::sieve(SieveWorld { version: false, ..world() });
    let (account, server) = account(&fake);
    assert_eq!(check(&account, &server, PASSWORD, OURS).unwrap(), Checked::NotOffered);
    assert!(!fake.world().heard("CHECKSCRIPT"));
}

#[test]
fn a_check_goes_only_to_the_mail_server_and_never_in_the_clear() {
    let fake = Fake::sieve(world());
    let (mut account, mut server) = account(&fake);
    // STARTTLS asked for and not offered: nothing of the owner's is sent.
    server.security = Security::StartTls;
    let err = check_with(&account, &server, PASSWORD, OURS, &hurried(), None).unwrap_err();
    assert!(matches!(err, SieveError::Reach(Reach::NoStartTls { .. })), "{err:?}");
    let heard = fake.world().heard("AUTHENTICATE");
    assert!(!heard);
    // Another host than the account's: no connection at all.
    server.security = Security::None;
    account.incoming = Some(Server { host: "localhost".into(), port: 1, security: Security::None });
    let before = fake.world().connections;
    assert!(matches!(check(&account, &server, PASSWORD, OURS), Err(SieveError::Net(NetError::Config(_)))));
    let after = fake.world().connections;
    assert_eq!(after, before);
    // A wrong password is a refused sign-in, not repeated.
    account.incoming = Some(Server { host: "127.0.0.1".into(), port: 1, security: Security::None });
    let err = check(&account, &server, "wrong horse", OURS).unwrap_err();
    assert!(err.is_refused_sign_in() && !err.to_string().contains("wrong horse"), "{err}");
}

/// The owner's own script, as a webmail wrote it: CRLF and a lone LF, comments, an extension
/// MailCraft doesn't know, a `text:` block, trailing blanks.
const THEIRS: &str = "# rule:[Lists]\r\nrequire [\"fileinto\", \"x-unknown\"];\r\n/* kept\n   as is */\r\nif header :contains \"subject\" \"[list]\" {  \r\n  fileinto \"Lists\";\r\n  x-unknown text:\r\n.. dot\r\n.\r\n;\r\n}\r\n\r\n";

fn theirs_world() -> SieveWorld {
    SieveWorld { scripts: vec![("roundcube".into(), THEIRS.into()), ("spare".into(), "keep;".into())], active: Some("roundcube".into()), ..world() }
}

#[test]
fn the_owners_script_is_read_and_written_back_byte_for_byte_and_stays_active() {
    let fake = Fake::sieve(theirs_world());
    let (account, server) = account(&fake);
    let state = read(&account, &server, PASSWORD, "mailcraft").unwrap();
    let (name, text) = state.active_script.clone().unwrap();
    assert_eq!((name.as_str(), text.as_str()), ("roundcube", THEIRS));
    let edited = text.replace("\"Lists\"", "\"Lists/Old\"");
    let read_digest = digest(&text);
    let change = OwnChange { name: &name, text: &edited, read: &read_digest };
    assert_eq!(write_own(&account, &server, PASSWORD, &change).unwrap(), OwnWritten::Done { warnings: None });
    let world = fake.world();
    assert_eq!(world.script("roundcube").map(str::as_bytes), Some(edited.as_bytes()));
    assert_eq!(world.active.as_deref(), Some("roundcube"));
    // Nothing else: no other script made, switched, renamed or deleted.
    assert_eq!(world.script("spare"), Some("keep;"));
    assert_eq!(world.script("mailcraft"), None);
    assert!(world.heard("HAVESPACE"));
    assert!(!world.heard("SETACTIVE") && !world.heard("DELETESCRIPT") && !world.heard("RENAMESCRIPT"));
    assert_eq!(world.log.iter().filter(|l| l.starts_with("PUTSCRIPT")).count(), 1);
}

#[test]
fn the_owners_script_changed_removed_or_switched_off_since_it_was_read_is_never_written() {
    let fake = Fake::sieve(theirs_world());
    let (account, server) = account(&fake);
    let read_digest = digest(THEIRS);
    let change = OwnChange { name: "roundcube", text: "keep;\r\n", read: &read_digest };
    // Changed in webmail meanwhile.
    let webmail = format!("{THEIRS}discard;\r\n");
    {
        let mut world = fake.world();
        if let Some((_, text)) = world.scripts.iter_mut().find(|(n, _)| n == "roundcube") {
            *text = webmail.clone();
        }
    }
    assert_eq!(write_own(&account, &server, PASSWORD, &change).unwrap(), OwnWritten::Changed { now: Some(webmail.clone()) });
    assert_eq!(fake.world().script("roundcube"), Some(webmail.as_str()));
    // Switched off meanwhile, as it was read.
    {
        let mut world = fake.world();
        if let Some((_, text)) = world.scripts.iter_mut().find(|(n, _)| n == "roundcube") {
            *text = THEIRS.to_string();
        }
        world.active = Some("spare".into());
    }
    assert_eq!(write_own(&account, &server, PASSWORD, &change).unwrap(), OwnWritten::Changed { now: Some(THEIRS.into()) });
    // Removed meanwhile.
    fake.world().scripts.retain(|(n, _)| n != "roundcube");
    assert_eq!(write_own(&account, &server, PASSWORD, &change).unwrap(), OwnWritten::Changed { now: None });
    let world = fake.world();
    assert!(!world.heard("PUTSCRIPT") && !world.heard("SETACTIVE"));
    assert_eq!(world.active.as_deref(), Some("spare"));
}

#[test]
fn the_owners_script_is_not_written_without_room_nor_when_too_large_nor_when_unchanged() {
    let fake = Fake::sieve(SieveWorld { quota: Some(THEIRS.len() + 4), ..theirs_world() });
    let (account, server) = account(&fake);
    let read_digest = digest(THEIRS);
    let longer = format!("{THEIRS}# a comment too many\r\n");
    let change = OwnChange { name: "roundcube", text: &longer, read: &read_digest };
    assert_eq!(write_own(&account, &server, PASSWORD, &change).unwrap(), OwnWritten::NoSpace);
    let huge = "#".repeat(mailcraft_proto::managesieve::MAX_SCRIPT + 1);
    let change = OwnChange { name: "roundcube", text: &huge, read: &read_digest };
    assert_eq!(write_own(&account, &server, PASSWORD, &change).unwrap(), OwnWritten::TooLarge);
    let change = OwnChange { name: "roundcube", text: THEIRS, read: &read_digest };
    assert_eq!(write_own(&account, &server, PASSWORD, &change).unwrap(), OwnWritten::Done { warnings: None });
    let world = fake.world();
    assert!(!world.heard("PUTSCRIPT"));
    assert_eq!(world.script("roundcube"), Some(THEIRS));
    // Only a size is in debug output.
    assert!(!format!("{:?}", OwnWritten::Changed { now: Some(THEIRS.into()) }).contains("Lists"));
}

#[test]
fn the_servers_refusal_of_the_owners_script_leaves_it_as_it_was() {
    let fake =
        Fake::sieve(SieveWorld { refuse_scripts_with: Some("bogus".into()), refusal: "line 2: unknown command 'bogus'".into(), ..theirs_world() });
    let (account, server) = account(&fake);
    let read_digest = digest(THEIRS);
    let change = OwnChange { name: "roundcube", text: "keep;\r\nbogus;\r\n", read: &read_digest };
    let e = write_own(&account, &server, PASSWORD, &change).unwrap_err();
    assert!(e.to_string().contains("unknown command"), "{e}");
    assert_eq!(fake.world().script("roundcube"), Some(THEIRS));
}

/// More scripts on the server than a listing keeps, the owner's active one past the cap: nothing
/// is written, made active or switched off, as the listing can't say which is active or whether
/// MailCraft's script is there (#2058).
#[test]
fn nothing_is_changed_after_a_listing_that_cant_be_kept_whole() {
    let mut scripts: Vec<(String, String)> = (0..256).map(|n| (format!("old {n}"), "keep;".to_string())).collect();
    scripts.push(("roundcube".into(), THEIRS.into()));
    let fake = Fake::sieve(SieveWorld { scripts, active: Some("roundcube".into()), ..world() });
    let (account, server) = account(&fake);
    assert!(write(&account, &server, PASSWORD, &change(OURS, None)).is_err());
    let read_digest = digest(THEIRS);
    assert!(write_own(&account, &server, PASSWORD, &OwnChange { name: "roundcube", text: "keep;\r\n", read: &read_digest }).is_err());
    assert!(deactivate(&account, &server, PASSWORD, "roundcube").is_err());
    let world = fake.world();
    assert!(!world.heard("PUTSCRIPT") && !world.heard("SETACTIVE") && !world.heard("DELETESCRIPT"));
    assert_eq!(world.active.as_deref(), Some("roundcube"));
    assert_eq!(world.script("roundcube"), Some(THEIRS));
}

/// The owner's script already as the save would make it, and still active: a save the server took
/// whose answer was lost, tried again, is done, not a change made elsewhere (#2058).
#[test]
fn a_save_of_the_owners_script_that_reached_the_server_is_known_again() {
    let edited = THEIRS.replace("\"Lists\"", "\"Lists/Old\"");
    let mut world = theirs_world();
    if let Some((_, text)) = world.scripts.iter_mut().find(|(n, _)| n == "roundcube") {
        *text = edited.clone();
    }
    let fake = Fake::sieve(world);
    let (account, server) = account(&fake);
    let read_digest = digest(THEIRS);
    let change = OwnChange { name: "roundcube", text: &edited, read: &read_digest };
    assert_eq!(write_own(&account, &server, PASSWORD, &change).unwrap(), OwnWritten::Done { warnings: None });
    assert!(!fake.world().heard("PUTSCRIPT"));
    // Not when it is no longer the active one.
    fake.world().active = Some("spare".into());
    assert!(matches!(write_own(&account, &server, PASSWORD, &change).unwrap(), OwnWritten::Changed { .. }));
}

/// ManageSieve is made secure with STARTTLS: a server set to be encrypted from the first byte is
/// refused before any connection is made (#2058).
#[test]
fn implicit_tls_is_refused_before_connecting() {
    let fake = Fake::sieve(world());
    let (account, mut server) = account(&fake);
    server.security = Security::Tls;
    let err = read(&account, &server, PASSWORD, "mailcraft").unwrap_err();
    assert!(err.to_string().contains("STARTTLS"), "{err}");
    assert_eq!(fake.world().connections, 0);
}

/// Stopped while a silent server is waited for: no second connection is made to tell why (#2058).
#[test]
fn a_stop_while_waiting_for_a_greeting_makes_no_second_connection() {
    let fake = Fake::sieve(SieveWorld { opening: Opening::Silent, ..world() });
    let (account, server) = account(&fake);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let patience = Patience { io: Duration::from_millis(600), ..Patience::default() };
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        assert!(read_with(&account, &server, PASSWORD, "mailcraft", &patience, Some(&stop)).is_err());
    });
    assert_eq!(fake.world().connections, 1);
}
