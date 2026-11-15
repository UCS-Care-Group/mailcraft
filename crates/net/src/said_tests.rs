// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a host's errors say in English, word for word (#872). The CLI, MCP and the log read
//! these lines; the window says them in its own language from the texts they are made of, and
//! that must not change a word of the English.
use crate::fake::{Fake, ImapWorld, SmtpWorld};
use crate::{NetError, test_account};
use mailcraft_model::sync::SyncRequest;
use mailcraft_model::{Account, Address, Message, Security, Server};
use mailcraft_proto::ProtoError;

const PASSWORD: &str = "invented-horse";

fn account(imap: Option<u16>, smtp: Option<u16>, security: Security) -> Account {
    let server = |port: u16| Server { host: "127.0.0.1".into(), port, security };
    Account { email: "ada@example.org".into(), username: "ada".into(), incoming: imap.map(server), outgoing: smtp.map(server), ..Default::default() }
}

fn hello() -> Message {
    Message { id: 9, to: vec![Address::new("Bob", "bob@example.org")], subject: "Hello".into(), ..Default::default() }
}

#[test]
fn a_server_that_takes_no_password_here_is_said_as_before() {
    let imap = Fake::imap(ImapWorld { user: "ada".into(), password: PASSWORD.into(), login_disabled: true, ..Default::default() });
    let account = account(Some(imap.port), None, Security::None);
    let result = crate::sync(&SyncRequest { account: account.clone(), password: PASSWORD.into(), ..Default::default() });
    assert_eq!(result.errors, ["Couldn't check for new mail: sign-in failed: the server doesn't take a password on this connection"]);
    let error = test_account(&account, PASSWORD).unwrap_err();
    assert_eq!(error.to_string(), "sign-in failed: the server doesn't take a password on this connection");
    assert!(error.is_refused_sign_in());
}

#[test]
fn a_server_s_own_refusal_is_said_as_before() {
    let imap = Fake::imap(ImapWorld { user: "ada".into(), password: PASSWORD.into(), plain: true, ..Default::default() });
    let smtp = Fake::smtp(SmtpWorld { user: "ada".into(), password: PASSWORD.into(), auth: true, ..Default::default() });
    let account = account(Some(imap.port), Some(smtp.port), Security::None);
    let mut request = SyncRequest { account: account.clone(), password: "wrong-horse".into(), ..Default::default() };
    request.outbox = vec![hello()];
    let result = crate::sync(&request);
    assert_eq!(
        result.errors,
        [
            "Couldn't send mail: sign-in failed: 535 5.7.8 no user with the password ***",
            "Couldn't check for new mail: sign-in failed: [AUTHENTICATIONFAILED] refused",
        ]
    );
    assert!(!format!("{result:?}").contains("wrong-horse"));
}

#[test]
fn a_folder_the_server_refuses_is_said_as_before() {
    let mut world = ImapWorld {
        user: "ada".into(),
        password: PASSWORD.into(),
        plain: true,
        refuse: vec!["SELECT".into(), "EXAMINE".into()],
        ..Default::default()
    };
    world.mailboxes = vec![crate::fake::FakeMailbox {
        raw: "INBOX".into(),
        flags: "\\HasNoChildren".into(),
        uid_validity: 11,
        messages: Vec::new(),
        refuse_open: false,
        next_uid: 0,
    }];
    let imap = Fake::imap(world);
    let account = account(Some(imap.port), None, Security::None);
    let result = crate::sync(&SyncRequest { account, password: PASSWORD.into(), ..Default::default() });
    assert_eq!(result.errors, ["Couldn't open \"INBOX\": the server said: [SERVERBUG] that can't be done"]);
}

#[test]
fn the_host_s_own_reasons_are_said_as_before() {
    // No outgoing server for what waits in the Outbox.
    let request = SyncRequest { account: account(None, None, Security::None), outbox: vec![hello()], ..Default::default() };
    assert_eq!(crate::sync(&request).errors, ["Couldn't send mail: the account has no outgoing server"]);
    // No password, no recipients, no servers.
    let imap = Fake::imap(ImapWorld::default());
    let with_imap = account(Some(imap.port), None, Security::None);
    assert_eq!(test_account(&with_imap, "").unwrap_err().to_string(), "enter a password to sign in");
    assert_eq!(test_account(&Account::default(), PASSWORD).unwrap_err().to_string(), "the account has no mail servers to check");
    let smtp = Fake::smtp(SmtpWorld::default());
    let with_smtp = account(None, Some(smtp.port), Security::None);
    assert_eq!(crate::send(&with_smtp, PASSWORD, &Message::default()).unwrap_err().to_string(), "the message has no recipients");
    // A server that offers no STARTTLS.
    let plain = Fake::imap(ImapWorld::default());
    let starttls = account(Some(plain.port), None, Security::StartTls);
    assert_eq!(
        test_account(&starttls, PASSWORD).unwrap_err().to_string(),
        "couldn't make a secure connection to 127.0.0.1: the server doesn't offer STARTTLS"
    );
    // Nobody listening: the system's own words follow.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let nobody = test_account(&account(Some(closed), None, Security::None), PASSWORD).unwrap_err();
    assert!(nobody.to_string().starts_with("couldn't reach 127.0.0.1: "), "{nobody}");
}

#[test]
fn proto_errors_read_as_before() {
    let said = |e: ProtoError| NetError::Proto(e).to_string();
    assert_eq!(said(ProtoError::Io("the line went".into())), "connection problem: the line went");
    assert_eq!(said(ProtoError::Protocol("odd".into())), "unexpected server response: odd");
    assert_eq!(said(ProtoError::Rejected("no such mailbox".into())), "the server said: no such mailbox");
    assert_eq!(said(ProtoError::Auth("Invalid credentials".into())), "sign-in failed: Invalid credentials");
    assert_eq!(NetError::Config("a reason".into()).to_string(), "a reason");
    assert_eq!(
        NetError::Connect { host: "mail.example.org".into(), reason: "refused".into() }.to_string(),
        "couldn't reach mail.example.org: refused"
    );
}
