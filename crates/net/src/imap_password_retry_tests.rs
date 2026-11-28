// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Initial password authentication retries never replay delivery or mailbox work.

use crate::fake::{Fake, ImapWorld, SmtpWorld};
use crate::sync::{Host, Limits, sync_as};
use mailcraft_model::sync::{SyncRequest, SyncResult};
use mailcraft_model::{Account, Address, Message, OAuth, Security, Server};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

const PASSWORD: &str = "synthetic-password";
const REFUSAL: &str = "NO [AUTHENTICATIONFAILED] synthetic refusal";

fn peer(replies: &[&str], plain: bool) -> crate::fake::FakeImap {
    Fake::imap(ImapWorld {
        user: "user@example.org".into(),
        password: PASSWORD.into(),
        plain,
        sasl_ir: true,
        password_replies: replies.iter().map(|reply| reply.to_string()).collect(),
        ..Default::default()
    })
}

fn request(port: u16) -> SyncRequest {
    SyncRequest {
        account: Account {
            id: 7,
            email: "user@example.org".into(),
            incoming: Some(Server { host: "127.0.0.1".into(), port, security: Security::None }),
            ..Default::default()
        },
        password: PASSWORD.into(),
        ..Default::default()
    }
}

fn limits(delay: Duration) -> Limits {
    Limits { password_retry_delay: Some(delay), ..Limits::in_pieces() }
}

fn run(request: &SyncRequest, limits: Limits, stop: &AtomicBool, files_sent: bool) -> (SyncResult, Vec<SyncResult>) {
    let mut result = SyncResult::default();
    let mut pieces = Vec::new();
    sync_as(request, limits, Host { files_sent, ..Default::default() }, stop, &mut |piece| {
        pieces.push(piece.clone());
        result.absorb(piece);
    });
    (result, pieces)
}

#[test]
fn a_first_password_refusal_reconnects_once_after_a_delay_without_replaying_smtp() {
    for (plain, files_sent) in [(false, false), (false, true), (true, false), (true, true)] {
        let imap = peer(&[REFUSAL], plain);
        if !files_sent {
            imap.world().mailboxes.push(crate::fake::FakeMailbox {
                raw: "Sent".into(),
                flags: "\\Sent".into(),
                uid_validity: 11,
                messages: Vec::new(),
                refuse_open: false,
                next_uid: 1,
            });
        }
        let smtp = Fake::smtp(SmtpWorld { user: "user@example.org".into(), password: PASSWORD.into(), auth: true, ..Default::default() });
        let mut request = request(imap.port);
        request.account.outgoing = Some(Server { host: "127.0.0.1".into(), port: smtp.port, security: Security::None });
        request.outbox = vec![Message {
            id: 41,
            to: vec![Address::new("Recipient", "recipient@example.org")],
            subject: "Synthetic delivery".into(),
            ..Default::default()
        }];
        let start = Instant::now();
        let (result, pieces) = run(&request, limits(Duration::from_millis(20)), &AtomicBool::new(false), files_sent);
        assert!(start.elapsed() >= Duration::from_millis(20));
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.auth_refused, Some(false));
        assert!(pieces.iter().all(|piece| piece.auth_refused == Some(false) && piece.errors.is_empty()));
        assert_eq!(result.sent, [41]);
        assert_eq!(smtp.world().delivered.len(), 1);
        assert_eq!(smtp.world().connections, 1);
        let world = imap.world();
        assert_eq!(world.connections, 2);
        assert_eq!(world.log.iter().filter(|line| line.contains(" APPEND ")).count(), usize::from(!files_sent));
        let command = if plain { " AUTHENTICATE PLAIN" } else { " LOGIN " };
        assert_eq!(world.log.iter().filter(|line| line.contains(command)).count(), 2);
        let second_auth = world.log.iter().rposition(|line| line.contains(command)).unwrap();
        assert!(!world.log.iter().take(second_auth).any(|line| line.contains(" LIST ") || line.contains(" SELECT ") || line.contains(" APPEND ")));
    }
}

#[test]
fn a_second_refusal_is_final_but_io_and_service_failures_are_not_credential_refusals() {
    for (replies, refused, expected_connections) in [
        (vec![REFUSAL, REFUSAL], true, 2),
        (vec![REFUSAL, "DROP"], false, 2),
        (vec!["NO [UNAVAILABLE] sign-in service unavailable"], false, 1),
        (vec!["BAD unsupported authentication"], false, 1),
    ] {
        let imap = peer(&replies, true);
        let (result, _) = run(&request(imap.port), limits(Duration::from_millis(1)), &AtomicBool::new(false), false);
        assert_eq!(imap.world().connections, expected_connections);
        assert_eq!(result.auth_refused, Some(refused), "{:?}", result.errors);
        assert_eq!(result.errors.len(), 1);
        assert!(!format!("{result:?}").contains(PASSWORD));
    }
}

#[test]
fn cancelling_the_delay_prevents_the_second_connection_even_when_a_sent_copy_is_owed() {
    for sent_copy in [false, true] {
        let imap = peer(&[REFUSAL], true);
        let smtp = Fake::smtp(SmtpWorld { user: "user@example.org".into(), password: PASSWORD.into(), auth: true, ..Default::default() });
        let mut request = request(imap.port);
        if sent_copy {
            request.account.outgoing = Some(Server { host: "127.0.0.1".into(), port: smtp.port, security: Security::None });
            request.outbox.push(Message {
                id: 41,
                to: vec![Address::new("Recipient", "recipient@example.org")],
                subject: "Synthetic delivery".into(),
                ..Default::default()
            });
        }
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let task = scope.spawn(|| run(&request, limits(Duration::from_secs(2)), &stop, false));
            let deadline = Instant::now() + Duration::from_secs(10);
            while !imap.world().password_replies.is_empty() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            std::thread::sleep(Duration::from_millis(25));
            stop.store(true, Ordering::Relaxed);
            let (result, _) = task.join().unwrap();
            assert!(result.stopped);
            assert_eq!(result.auth_refused, Some(false));
            assert!(result.errors.is_empty());
            if sent_copy {
                assert_eq!(result.unfiled, [41]);
            }
        });
        assert_eq!(imap.world().connections, 1);
        assert_eq!(smtp.world().delivered.len(), usize::from(sent_copy));
    }
}

#[test]
fn cancelling_the_replacement_greeting_never_submits_the_password_even_when_a_sent_copy_is_owed() {
    for sent_copy in [false, true] {
        let imap = peer(&[REFUSAL], true);
        let greeting = Arc::new(Barrier::new(2));
        imap.world().greeting_barrier_after_first = Some(greeting.clone());
        let smtp = Fake::smtp(SmtpWorld { user: "user@example.org".into(), password: PASSWORD.into(), auth: true, ..Default::default() });
        let mut request = request(imap.port);
        if sent_copy {
            request.account.outgoing = Some(Server { host: "127.0.0.1".into(), port: smtp.port, security: Security::None });
            request.outbox.push(Message {
                id: 41,
                to: vec![Address::new("Recipient", "recipient@example.org")],
                subject: "Synthetic delivery".into(),
                ..Default::default()
            });
        }
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let task = scope.spawn(|| run(&request, limits(Duration::ZERO), &stop, false));
            let deadline = Instant::now() + Duration::from_secs(10);
            while imap.world().connections < 2 {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            stop.store(true, Ordering::Relaxed);
            greeting.wait();
            let (result, _) = task.join().unwrap();
            assert!(result.stopped);
            assert_eq!(result.auth_refused, Some(false));
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            assert_eq!(result.unfiled, if sent_copy { vec![41] } else { Vec::new() });
        });
        assert_eq!(imap.world().connections, 2);
        assert_eq!(imap.world().log.iter().filter(|line| line.contains(" AUTHENTICATE PLAIN")).count(), 1);
        assert_eq!(smtp.world().delivered.len(), usize::from(sent_copy));
    }
}

#[test]
fn cancellation_during_replacement_tls_setup_is_stopped_not_a_connection_error() {
    crate::tls_test_support::with_tls(|config| {
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cancel = stop.clone();
        let handshakes = seen.clone();
        let imap = peer(&[REFUSAL], true);
        {
            let mut world = imap.world();
            world.offer_starttls = true;
            world.starttls_upgrade = Some(Arc::new(move |mut stream| -> Box<dyn crate::fake::MailIo> {
                if handshakes.fetch_add(1, Ordering::Relaxed) == 0 {
                    let connection = rustls::ServerConnection::new(config.clone()).unwrap();
                    return Box::new(rustls::StreamOwned::new(connection, stream));
                }
                // Wait for the replacement's ClientHello, then cancel and leave its
                // handshake unanswered. Its short I/O timeout must remain a stop.
                let mut first_byte = [0];
                std::io::Read::read_exact(&mut stream, &mut first_byte).unwrap();
                cancel.store(true, Ordering::Relaxed);
                let _ = std::io::copy(&mut stream, &mut std::io::sink());
                stream
            }));
        }
        let smtp = Fake::smtp(SmtpWorld { user: "user@example.org".into(), password: PASSWORD.into(), auth: true, ..Default::default() });
        let mut request = request(imap.port);
        request.account.incoming.as_mut().unwrap().security = Security::StartTls;
        request.account.outgoing = Some(Server { host: "127.0.0.1".into(), port: smtp.port, security: Security::None });
        request.outbox.push(Message {
            id: 41,
            to: vec![Address::new("Recipient", "recipient@example.org")],
            subject: "Synthetic delivery".into(),
            ..Default::default()
        });
        let mut limits = limits(Duration::ZERO);
        limits.patience.io = Duration::from_millis(500);
        let (result, _) = run(&request, limits, &stop, false);
        assert_eq!(seen.load(Ordering::Relaxed), 2);
        assert!(result.stopped);
        assert_eq!(result.auth_refused, Some(false));
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.unfiled, [41]);
        assert_eq!(smtp.world().delivered.len(), 1);
        assert_eq!(imap.world().log.iter().filter(|line| line.contains(" AUTHENTICATE PLAIN")).count(), 1);
    });
}

#[test]
fn ordinary_sync_oauth_tls_and_local_validation_do_not_gain_password_retries() {
    let imap = peer(&[REFUSAL], true);
    let (result, _) = run(&request(imap.port), Limits::in_pieces(), &AtomicBool::new(false), false);
    assert_eq!(imap.world().connections, 1);
    assert_eq!(result.auth_refused, Some(true));
    let imap = peer(&[], true);
    let mut ask = request(imap.port);
    ask.account.oauth = Some(OAuth::default());
    ask.access_token = "synthetic-token".into();
    let (result, _) = run(&ask, limits(Duration::ZERO), &AtomicBool::new(false), false);
    assert_eq!(imap.world().connections, 1);
    assert!(!result.errors.is_empty());
    let imap = peer(&[], false);
    imap.world().login_disabled = true;
    let _ = run(&request(imap.port), limits(Duration::ZERO), &AtomicBool::new(false), false);
    assert_eq!(imap.world().connections, 1);
    let mut ask = request(imap.port);
    ask.password.clear();
    let _ = run(&ask, limits(Duration::ZERO), &AtomicBool::new(false), false);
    assert_eq!(imap.world().connections, 1);
    let imap = peer(&[], true);
    let mut ask = request(imap.port);
    ask.account.incoming.as_mut().unwrap().security = Security::StartTls;
    let (result, _) = run(&ask, limits(Duration::ZERO), &AtomicBool::new(false), false);
    assert_eq!(imap.world().connections, 1);
    assert_eq!(result.auth_refused, Some(false));
    assert!(!result.errors.is_empty());
}

#[test]
fn successful_authentication_allows_a_fresh_single_retry_in_a_new_run() {
    let imap = peer(&[REFUSAL], true);
    let ask = request(imap.port);
    for _ in 0..2 {
        let (result, _) = run(&ask, limits(Duration::ZERO), &AtomicBool::new(false), false);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.auth_refused, Some(false));
        imap.world().password_replies = vec![REFUSAL.into()];
    }
    assert_eq!(imap.world().connections, 4);
}

#[test]
fn desktop_entrypoint_uses_the_bounded_two_second_retry() {
    let imap = peer(&[REFUSAL], true);
    let mut result = SyncResult::default();
    let start = Instant::now();
    crate::sync_with_password_retry(&request(imap.port), &AtomicBool::new(false), &mut |piece| result.absorb(piece));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.auth_refused, Some(false));
    assert_eq!(imap.world().connections, 2);
    assert!(start.elapsed() >= Duration::from_secs(2));
}

#[test]
fn changed_password_methods_never_downgrade_or_misclassify_the_retry() {
    for (first_plain, second_plain, login_disabled) in [(true, false, false), (false, true, false), (false, false, true)] {
        let imap = peer(&[REFUSAL], first_plain);
        imap.world().password_methods_after_reply = Some((second_plain, login_disabled));
        let (result, _) = run(&request(imap.port), limits(Duration::ZERO), &AtomicBool::new(false), false);
        assert_eq!(imap.world().connections, 2);
        assert_eq!(result.auth_refused, Some(false), "{:?}", result.errors);
        assert_eq!(result.errors.len(), 1);
        let world = imap.world();
        assert_eq!(world.log.iter().filter(|line| line.contains(" AUTHENTICATE PLAIN") || line.contains(" LOGIN ")).count(), 1);
        if first_plain {
            assert!(!world.log.iter().any(|line| line.contains(" LOGIN ")));
        }
    }
}

#[test]
fn oauth_post_authentication_token_expiry_keeps_the_existing_renewal_verdict() {
    let imap = Fake::imap(ImapWorld {
        user: "user@example.org".into(),
        token: "synthetic-token".into(),
        sasl_ir: true,
        answers: vec![(" LIST ".into(), "{tag} NO [AUTHENTICATIONFAILED] AccessTokenExpired\r\n".into())],
        ..Default::default()
    });
    let mut ask = request(imap.port);
    ask.account.oauth = Some(OAuth::default());
    ask.access_token = "synthetic-token".into();
    let (result, pieces) = run(&ask, limits(Duration::ZERO), &AtomicBool::new(false), false);
    assert_eq!(imap.world().connections, 1, "OAuth never gains the password retry");
    assert_eq!(result.auth_refused, None, "provider expiry handling remains authoritative for OAuth");
    assert!(pieces.iter().all(|piece| piece.auth_refused.is_none()));
    assert!(result.errors.iter().any(|error| error.contains("AccessTokenExpired")), "{:?}", result.errors);
    assert!(!format!("{result:?}").contains("synthetic-token"));
}
