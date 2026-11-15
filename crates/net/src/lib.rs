// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft's network side, for the desktop app and the CLI.
//!
//! Sockets and TLS under the protocol clients of `mailcraft-proto`: [`test_account`] checks a
//! sign-in, [`send`] sends one message through SMTP, and [`sync`] synchronises with the account's
//! servers both ways: it sends the Outbox, pushes the flag changes, moves and deletions made
//! here to IMAP, and downloads new mail, flag changes and removals from there. [`oauth`]
//! signs the owner in on the provider's own page, for accounts whose servers take an access
//! token (SASL XOAUTH2) instead of a password, and [`lookup_mx`] finds out where a domain's mail
//! goes, which tells its provider. [`graph`] asks Microsoft Graph for what a Microsoft 365
//! account has that IMAP and SMTP don't reach: sending where SMTP is switched off, the calendar,
//! the contacts, and which folders the owner keeps under Favorites in Outlook. [`report`] sends
//! a problem report the owner wrote to the project's report relay, on their click. All of them
//! block, so a host calls them on a background thread; every connection has timeouts.
//!
//! A password or a token is only ever sent inside TLS, or to a server on this machine. It never
//! appears in an error or a log line.
//!
//! On the web this crate is empty but for [`AVAILABLE`]: a browser can't open sockets.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Whether this build can talk to mail servers.
#[cfg(target_arch = "wasm32")]
pub const AVAILABLE: bool = false;

#[cfg(not(target_arch = "wasm32"))]
mod anchors;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod anchors_nss;
#[cfg(not(target_arch = "wasm32"))]
mod autoconfig;
#[cfg(not(target_arch = "wasm32"))]
mod cert;
#[cfg(not(target_arch = "wasm32"))]
mod discover;
#[cfg(not(target_arch = "wasm32"))]
mod dns;
#[cfg(not(target_arch = "wasm32"))]
mod dns_srv;
#[cfg(not(target_arch = "wasm32"))]
mod favorite_paths;
#[cfg(not(target_arch = "wasm32"))]
pub mod gmail;
#[cfg(not(target_arch = "wasm32"))]
pub mod graph;
#[cfg(not(target_arch = "wasm32"))]
mod http;
#[cfg(not(target_arch = "wasm32"))]
pub mod https;
#[cfg(not(target_arch = "wasm32"))]
pub mod idle;
#[cfg(test)]
mod idle_tests;
#[cfg(not(target_arch = "wasm32"))]
mod kept;
#[cfg(not(target_arch = "wasm32"))]
mod kinds;
#[cfg(not(target_arch = "wasm32"))]
pub mod oauth;
#[cfg(not(target_arch = "wasm32"))]
pub mod pictures;
#[cfg(not(target_arch = "wasm32"))]
mod pinned;
#[cfg(not(target_arch = "wasm32"))]
mod push;
#[cfg(not(target_arch = "wasm32"))]
pub mod report;
#[cfg(not(target_arch = "wasm32"))]
mod send;
#[cfg(not(target_arch = "wasm32"))]
mod sync;
#[cfg(not(target_arch = "wasm32"))]
mod tls;
#[cfg(not(target_arch = "wasm32"))]
mod transport;
#[cfg(not(target_arch = "wasm32"))]
pub mod update;
#[cfg(not(target_arch = "wasm32"))]
pub mod update_download;
#[cfg(not(target_arch = "wasm32"))]
mod xml_text;

// (`cfg(test)` on its own line: that is how clippy knows test code, which may unwrap.)
#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod fake;
#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests;

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

/// A Google mail access token is for Google's IMAP/SMTP servers alone. Never send it to
/// servers copied from a mailbox file or chosen by a plugin/agent. Loopback is only permitted
/// inside this crate's synthetic protocol tests.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn google_destination(account: &mailcraft_model::Account, host: &str, expected: &str) -> Result<(), NetError> {
    if account.oauth.as_ref().is_some_and(|oauth| oauth.provider == mailcraft_model::OAuthProvider::Google) {
        let name = host.trim().trim_end_matches('.');
        if !(name.eq_ignore_ascii_case(expected) || (cfg!(test) && name.eq_ignore_ascii_case("127.0.0.1"))) {
            return Err(NetError::Config(mailcraft_i18n::said!(
                "Google OAuth access tokens may only be sent to Google's Gmail IMAP and SMTP servers"
            )));
        }
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use mailcraft_i18n::{Arg, Text, said};
    use mailcraft_model::Account;

    pub use crate::anchors::system_trust_anchors;
    pub use crate::cert::{CertificateFault, CertificateInfo, CertificateProblem, describe as describe_certificate};
    pub use crate::discover::{PLACES, Places, discover, discover_at};
    pub use crate::dns::{lookup_mx, lookup_mx_at};
    pub use crate::dns_srv::{
        AutodiscoverTargets, SrvTarget, autodiscover_hosts, autodiscover_hosts_at, autodiscover_targets, autodiscover_targets_at,
        lookup_autodiscover, lookup_autodiscover_at,
    };
    pub use crate::pinned::pin_check_under_way;
    pub use crate::send::send;
    pub use crate::sync::{sync, sync_with, token_may_go_to};

    /// Whether this build can talk to mail servers.
    pub const AVAILABLE: bool = true;

    /// What went wrong. The reasons that are MailCraft's own words are texts the window says in
    /// its language ([`Text`], made with `said!`); what a server or the system said is kept as it
    /// said it. The English ([`Display`](std::fmt::Display)) is what the CLI, MCP and the log read
    /// (#872).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum NetError {
        Connect {
            host: String,
            reason: Text,
        },
        Tls {
            host: String,
            reason: Text,
        },
        Proto(mailcraft_proto::ProtoError),
        Config(Text),
        /// The owner called the sign-in on the provider's page off ([`oauth::sign_in`](crate::oauth::sign_in)).
        SignInCancelled,
        /// The sign-in on the provider's page wasn't finished in the time it is waited for.
        SignInTimedOut,
        /// The sign-in is good, but it doesn't cover what was asked for until the owner agrees to
        /// that on the provider's page: a token for Microsoft Graph, for an owner who was never
        /// asked ([`oauth::refresh_for`](crate::oauth::refresh_for)). Signing in to mail again is
        /// not the cure; [`oauth::sign_in_for`](crate::oauth::sign_in_for) is.
        ConsentNeeded(String),
        /// The service took the sign-in and won't do this for the account all the same (Microsoft
        /// Graph's 403): asking again changes nothing.
        Forbidden(String),
        /// "Not now, later" (Microsoft Graph's 429, 503 and 504). `retry_after` is how many
        /// seconds the service asked to be left alone for, when it said.
        Busy {
            text: String,
            retry_after: Option<u32>,
        },
        /// The system does not trust the server's certificate, or it is one nobody may trust
        /// ([`https`](crate::https)): what setup shows is in it. It holds nothing of the
        /// certificate in its text.
        Certificate(Box<CertificateProblem>),
        /// A shared mailbox Microsoft won't open for the owner of the token (#70), by Graph's own
        /// code: asking again by itself changes nothing.
        SharedRefused(mailcraft_model::sync::SharedRefusal),
    }

    impl From<mailcraft_proto::ProtoError> for NetError {
        fn from(e: mailcraft_proto::ProtoError) -> NetError {
            NetError::Proto(e)
        }
    }

    impl std::fmt::Display for NetError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            std::fmt::Display::fmt(&self.said(), f)
        }
    }

    impl std::error::Error for NetError {}

    impl NetError {
        /// The error as the owner is told it, in words the window can say in its language: those
        /// of the mail servers' sign-in and connection, and the account's settings. (What
        /// Microsoft's sign-in and Graph say is still English for now.)
        pub fn said(&self) -> Text {
            match self {
                NetError::Connect { host, reason } => said!("couldn't reach {host}: {reason}", host, reason = Arg::said(reason.clone())),
                NetError::Tls { host, reason } => {
                    said!("couldn't make a secure connection to {host}: {reason}", host, reason = Arg::said(reason.clone()))
                }
                NetError::Proto(e) => e.said(),
                NetError::Config(why) => why.clone(),
                NetError::SignInCancelled => Text::verbatim("the sign-in was cancelled"),
                NetError::SignInTimedOut => Text::verbatim("the sign-in wasn't finished in time"),
                NetError::ConsentNeeded(text) => Text::verbatim(format!("permission is still to be given: {text}")),
                NetError::Forbidden(text) | NetError::Busy { text, .. } => Text::verbatim(text.clone()),
                NetError::Certificate(problem) => Text::verbatim(problem.to_string()),
                NetError::SharedRefused(refusal) => Text::verbatim(refusal.said()),
            }
        }

        /// The same error with `secret` (a password, a token) blanked out, should a server have written it back.
        pub(crate) fn without(self, secret: &str) -> NetError {
            use mailcraft_proto::ProtoError as P;
            let clean = |text: String| scrub(&text, secret);
            let blank = |text: Text| text.map_plain(&|said: String| scrub(&said, secret));
            match self {
                NetError::Connect { host, reason } => NetError::Connect { host, reason: blank(reason) },
                NetError::Tls { host, reason } => NetError::Tls { host, reason: blank(reason) },
                NetError::Config(text) => NetError::Config(blank(text)),
                NetError::Proto(e) => NetError::Proto(match e {
                    P::Io(text) => P::Io(blank(text)),
                    P::Protocol(text) => P::Protocol(blank(text)),
                    P::Rejected(text) => P::Rejected(blank(text)),
                    P::Auth(text) => P::Auth(blank(text)),
                }),
                NetError::ConsentNeeded(text) => NetError::ConsentNeeded(clean(text)),
                NetError::Forbidden(text) => NetError::Forbidden(clean(text)),
                NetError::Busy { text, retry_after } => NetError::Busy { text: clean(text), retry_after },
                unworded @ (NetError::SignInCancelled | NetError::SignInTimedOut | NetError::SharedRefused(_) | NetError::Certificate(_)) => unworded,
            }
        }

        /// Whether a sign-in that is good doesn't cover what was asked for yet
        /// ([`NetError::ConsentNeeded`]): the owner has to agree to it on the provider's page,
        /// which is not signing in again.
        pub fn is_consent_needed(&self) -> bool {
            matches!(self, NetError::ConsentNeeded(_))
        }

        /// Whether the other side refused the sign-in itself (a wrong password, a token or refresh
        /// token it no longer takes), as against trouble reaching it: only then is asking the
        /// owner to sign in again the cure.
        pub fn is_refused_sign_in(&self) -> bool {
            matches!(self, NetError::Proto(mailcraft_proto::ProtoError::Auth(_)))
        }

        /// A Google refresh grant was explicitly revoked or expired; a transient failure
        /// or a denied browser consent must never make us erase an otherwise usable token.
        pub fn is_revoked_google_refresh(&self) -> bool {
            matches!(self, NetError::Proto(mailcraft_proto::ProtoError::Auth(reason)) if reason.to_string() == crate::oauth::GOOGLE_REFRESH_REVOKED)
        }

        /// Whether the other side couldn't be reached or stopped answering, as against an answer
        /// that says no: trying again later is what there is to do.
        pub fn is_unreachable(&self) -> bool {
            matches!(self, NetError::Connect { .. } | NetError::Tls { .. } | NetError::Proto(mailcraft_proto::ProtoError::Io(_)))
        }

        /// Whether the connection the error came from can't be used any more.
        pub(crate) fn is_fatal(&self) -> bool {
            use mailcraft_proto::ProtoError as P;
            !matches!(self, NetError::Proto(P::Rejected(_) | P::Auth(_)) | NetError::Config(_))
        }
    }

    /// `text` with `secret` blanked out.
    pub(crate) fn scrub(text: &str, secret: &str) -> String {
        if secret.is_empty() || !text.contains(secret) { text.to_string() } else { text.replace(secret, "***") }
    }

    /// What an account that signs in with OAuth is told while there is no token for it.
    pub(crate) fn not_signed_in(account: &Account) -> NetError {
        let who: String = account.email.trim().chars().filter(|c| !c.is_control()).take(80).collect();
        NetError::Config(if who.is_empty() { said!("sign in to the account first") } else { said!("sign in to {who} first", who) })
    }

    /// Check that the account's servers accept the sign-in (IMAP login, then SMTP EHLO/AUTH when an outgoing server is set).
    ///
    /// `secret` is the account's password, or the access token of an account that signs in with
    /// OAuth ([`Account::oauth`]).
    pub fn test_account(account: &Account, secret: &str) -> Result<(), NetError> {
        if account.incoming.is_none() && account.outgoing.is_none() {
            return Err(NetError::Config(said!("the account has no mail servers to check")));
        }
        if account.incoming.is_some() {
            let imap = crate::sync::open_imap(account, secret).map_err(|e| e.without(secret))?;
            let _ = imap.logout();
        }
        if account.outgoing.is_some() {
            let smtp = crate::send::open_smtp(account, secret).map_err(|e| e.without(secret))?;
            let _ = smtp.quit();
        }
        Ok(())
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod password_tests;

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod report_tests;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod said_tests;

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "tls_test_support_tests.rs"]
mod tls_test_support;

// (`cfg(test)` on its own line, so that clippy knows the helpers that unwrap are test code.)
#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod verifier_tests;

#[cfg(test)]
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod anchors_nss_tests;
#[cfg(test)]
#[cfg(all(unix, not(target_os = "macos")))]
mod anchors_tests;
