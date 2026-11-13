// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The names secrets are kept under in the platform's credential store. One place makes them, for
//! the desktop window and the phones alike, so that the rules about whose secret goes to which
//! server don't drift apart.

use mailcraft_model::{Account, AccountKind, Server};

/// The name of a Microsoft account's secret of this kind (`password` or `refresh`): such an
/// account is kept in step through Graph, which goes to Microsoft alone, so its name holds the
/// address and nothing about servers.
pub fn microsoft_secret_key(kind: &str, address: &str) -> String {
    format!("{kind}:{}|graph", address.trim().to_ascii_lowercase())
}

/// The name a secret is kept under in the system's credential store: its kind (`password` or
/// `refresh`), the account's address and the servers it goes to, so that an account that names
/// other servers (a mailbox from a file, a changed setting) is not given the secret.
/// A Microsoft account kept in step through Graph goes to Microsoft alone: its sign-in page and
/// Graph are fixed (a shared mailbox is such an account too; one of another kind that a file calls
/// shared is not, and gets the name of its servers, never the owner's token).
pub fn secret_key(kind: &str, a: &Account) -> String {
    let address = a.email.trim().to_ascii_lowercase();
    let server = |s: &Option<Server>| match s {
        Some(s) => format!("{}:{}", s.host.trim().to_ascii_lowercase(), s.port),
        None => "none".to_string(),
    };
    if a.kind == AccountKind::Microsoft {
        microsoft_secret_key(kind, &address)
    } else if a.kind == AccountKind::Jmap {
        // (Its whole session address: another one, on the same server or not, is another server;
        // and a bearer token is no password.)
        let (session, bearer) = a.jmap.as_ref().map_or(("none", false), |j| (j.session.trim(), j.bearer));
        format!("{kind}:{address}|jmap={session}|bearer={bearer}")
    } else if a.kind == AccountKind::Exchange {
        // (Its whole EWS address: another one, on the same server or not, is another server; the
        // user name it signs in with: a password is for one name; and the certificate it trusts
        // besides the system's, when it does: a password remembered for the server the system
        // vouches for is never handed to a connection that a pin lets through, AU3-02.)
        let url = a.ews.as_ref().map_or("none", |e| e.url.trim());
        let user = a.ews.as_ref().map_or_else(String::new, |e| e.username.trim().to_ascii_lowercase());
        match a.ews.as_ref().map(|e| e.trusted.trim().to_ascii_lowercase()).filter(|pin| !pin.is_empty()) {
            Some(pin) => format!("{kind}:{address}|ews={url}|user={user}|pin={pin}"),
            None => format!("{kind}:{address}|ews={url}|user={user}"),
        }
    } else {
        format!("{kind}:{address}|imap={}|smtp={}", server(&a.incoming), server(&a.outgoing))
    }
}

/// The name secrets were kept under before they were bound to servers.
pub fn legacy_secret_key(kind: &str, address: &str) -> String {
    format!("{kind}:{}", address.trim().to_ascii_lowercase())
}
