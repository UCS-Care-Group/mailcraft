// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sending through the account's outgoing (SMTP) server.

use std::borrow::Cow;
use std::sync::atomic::AtomicBool;

use mailcraft_mime::WriteOptions;
use mailcraft_model::{Account, Message, Security};
use mailcraft_proto::smtp::Smtp;

use crate::transport::{self, Patience, Service, Stream};
use crate::{NetError, not_signed_in};

/// What the client calls itself in EHLO: an address literal that says nothing about this machine.
const CLIENT_NAME: &str = "[127.0.0.1]";
/// Longest address sent in an envelope.
const MAX_ADDRESS: usize = 254;

/// Who a message goes from and to, as SMTP is told (not as the message's headers say).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Envelope {
    pub(crate) from: String,
    pub(crate) recipients: Vec<String>,
}

impl Envelope {
    /// The account's address, and To + Cc + Bcc, each address once.
    pub(crate) fn of(account: &Account, message: &Message) -> Result<Envelope, NetError> {
        let from = account.email.trim();
        if !is_address(from) {
            return Err(NetError::Config("the account has no usable e-mail address to send from".into()));
        }
        let mut recipients: Vec<String> = Vec::new();
        for address in message.recipients() {
            let email = address.email.trim();
            if email.is_empty() {
                continue;
            }
            // Leaving someone out silently would be worse than not sending.
            if !is_address(email) {
                return Err(NetError::Config(format!("\"{}\" isn't an address mail can be sent to", shown(email))));
            }
            if !recipients.iter().any(|r| r.eq_ignore_ascii_case(email)) {
                recipients.push(email.to_string());
            }
        }
        if recipients.is_empty() {
            return Err(NetError::Config("the message has no recipients".into()));
        }
        Ok(Envelope { from: from.to_string(), recipients })
    }
}

/// `local@domain` in visible ASCII, safe to put on an SMTP command line.
fn is_address(email: &str) -> bool {
    let plain = email.len() <= MAX_ADDRESS && email.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'<' | b'>' | b',' | b';' | b'"'));
    plain && email.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty() && !domain.contains('@'))
}

/// An address for an error message: short, and without control characters.
fn shown(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(80).collect()
}

/// Connect to the account's outgoing server, encrypt as it asks, and sign in when there is a
/// user name, a password, and a server that offers a way. An account that signs in with OAuth
/// always does, with `secret` as its access token (XOAUTH2).
pub(crate) fn open_smtp<'a>(account: &Account, secret: &str) -> Result<Smtp<Stream<'a>>, NetError> {
    open_smtp_with(account, secret, &Patience::default(), None)
}

/// [`open_smtp`], waiting as `patience` says, and giving up at once when `cancel` is set while
/// it is under way.
pub(crate) fn open_smtp_with<'a>(
    account: &Account,
    secret: &str,
    patience: &Patience,
    cancel: Option<&'a AtomicBool>,
) -> Result<Smtp<Stream<'a>>, NetError> {
    let server = account.outgoing.as_ref().ok_or_else(|| NetError::Config("the account has no outgoing server".into()))?;
    let user = account.login();
    if account.oauth.is_some() {
        if user.is_empty() {
            return Err(NetError::Config("the account has no user name or e-mail address to sign in with".into()));
        }
        // Without a token there is nothing to show the server: no connection is made.
        if secret.is_empty() {
            return Err(not_signed_in(account));
        }
        crate::sync::token_may_go_to(server)?;
    }
    let (mut stream, host) = transport::open(server, Service::Smtp, patience)?;
    stream.watch(cancel);
    let mut smtp = Smtp::connect(stream)?;
    let mut offered = smtp.hello(CLIENT_NAME)?;
    if server.security == Security::StartTls {
        // Never carry on in the clear when encryption was asked for.
        if !offered.starttls {
            return Err(NetError::Tls { host, reason: "the server doesn't offer STARTTLS".into() });
        }
        let plain = smtp.starttls()?;
        smtp = Smtp::resume(transport::upgrade(plain, &host)?);
        offered = smtp.hello(CLIENT_NAME)?;
    }
    if account.oauth.is_some() {
        // A server that doesn't take tokens is an error, not a reason to go on unsigned.
        smtp.auth_xoauth2(user, secret)?;
    } else if !user.is_empty() && !secret.is_empty() && (offered.auth_plain || offered.auth_login) {
        smtp.auth(user, secret)?;
    }
    Ok(smtp)
}

/// The message as it goes out: one that was never dated is dated now.
pub(crate) fn dated(message: &Message) -> Cow<'_, Message> {
    if message.date == 0 { Cow::Owned(Message { date: mailcraft_time::now(), ..message.clone() }) } else { Cow::Borrowed(message) }
}

/// Hand one message to an open session.
pub(crate) fn deliver(smtp: &mut Smtp<Stream<'_>>, envelope: &Envelope, message: &Message) -> Result<(), NetError> {
    let message = dated(message);
    // Bcc recipients are in the envelope only.
    let options = WriteOptions { include_bcc: false, offset_minutes: mailcraft_time::local_offset_minutes(message.date) };
    let bytes = mailcraft_mime::write(&message, &options);
    let recipients: Vec<&str> = envelope.recipients.iter().map(String::as_str).collect();
    smtp.send(&envelope.from, &recipients, &bytes)?;
    Ok(())
}

/// Send one message through the account's outgoing server.
///
/// `secret` is the account's password, or the access token of an account that signs in with
/// OAuth ([`Account::oauth`]).
pub fn send(account: &Account, secret: &str, message: &Message) -> Result<(), NetError> {
    // A message that can't be sent is found out before any connection is made.
    let envelope = Envelope::of(account, message)?;
    let mut smtp = open_smtp(account, secret).map_err(|e| e.without(secret))?;
    let sent = deliver(&mut smtp, &envelope, message).map_err(|e| e.without(secret));
    let _ = smtp.quit();
    sent
}

#[cfg(test)]
mod tests {
    use mailcraft_model::Address;

    use super::*;

    fn account() -> Account {
        Account { email: " ada@example.org ".into(), ..Default::default() }
    }

    fn to(addresses: &[&str]) -> Vec<Address> {
        addresses.iter().map(|a| Address::new("Someone", *a)).collect()
    }

    #[test]
    fn the_envelope_has_every_recipient_once() {
        let message = Message {
            to: to(&["bob@example.org", "carol@example.org"]),
            cc: to(&["BOB@example.org", "dave@example.org", ""]),
            bcc: to(&["erin@example.org", "carol@example.org"]),
            ..Default::default()
        };
        let envelope = Envelope::of(&account(), &message).unwrap();
        assert_eq!(envelope.from, "ada@example.org");
        assert_eq!(envelope.recipients, ["bob@example.org", "carol@example.org", "dave@example.org", "erin@example.org"]);
    }

    #[test]
    fn messages_that_cannot_be_sent_are_config_errors() {
        let config = |account: &Account, message: &Message| match Envelope::of(account, message) {
            Err(NetError::Config(why)) => why,
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert_eq!(config(&account(), &Message::default()), "the message has no recipients");
        assert_eq!(config(&account(), &Message { to: to(&["", "  "]), ..Default::default() }), "the message has no recipients");
        for bad in
            ["bob", "bob@", "@example.org", "bob smith@example.org", "bob@example.org>\r\nRCPT TO:<eve@example.org", "bøb@example.org", "a@b@c"]
        {
            let why = config(&account(), &Message { to: to(&["carol@example.org", bad]), ..Default::default() });
            assert!(why.contains("isn't an address"), "{bad:?}: {why}");
            assert!(!why.contains('\r') && !why.contains('\n'));
        }
        let nobody = Account::default();
        assert!(config(&nobody, &Message { to: to(&["bob@example.org"]), ..Default::default() }).contains("send from"));
        // An intranet address without a dot is fine.
        assert!(Envelope::of(&account(), &Message { to: to(&["root@localhost"]), ..Default::default() }).is_ok());
    }
}
