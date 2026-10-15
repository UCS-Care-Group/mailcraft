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
//! the contacts, and which folders the owner keeps under Favorites in Outlook. [`ews`] reads
//! those Favorites through Exchange Web Services instead, for an account whose owner chose
//! that way (it exists until 1 April 2027, and reads nothing else). [`idle`] keeps a connection
//! waiting on the Inbox between two runs, so that new mail is heard of at once. All of them
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
mod dns;
#[cfg(not(target_arch = "wasm32"))]
pub mod ews;
#[cfg(not(target_arch = "wasm32"))]
mod favorite_paths;
#[cfg(not(target_arch = "wasm32"))]
pub mod graph;
#[cfg(not(target_arch = "wasm32"))]
mod http;
#[cfg(not(target_arch = "wasm32"))]
pub mod idle;
#[cfg(not(target_arch = "wasm32"))]
mod kinds;
#[cfg(not(target_arch = "wasm32"))]
pub mod oauth;
#[cfg(not(target_arch = "wasm32"))]
pub mod pictures;
#[cfg(not(target_arch = "wasm32"))]
mod push;
#[cfg(not(target_arch = "wasm32"))]
mod send;
#[cfg(not(target_arch = "wasm32"))]
mod shared;
#[cfg(not(target_arch = "wasm32"))]
mod sync;
#[cfg(not(target_arch = "wasm32"))]
mod transport;

// (`cfg(test)` on its own line: that is how clippy knows test code, which may unwrap.)
#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod fake;
#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod folder_tests;
#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests;

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use mailcraft_model::Account;

    pub use crate::dns::{lookup_mx, lookup_mx_at};
    pub use crate::send::send;
    pub use crate::sync::{sync, sync_with, sync_with_updates};

    /// Whether this build can talk to mail servers.
    pub const AVAILABLE: bool = true;

    #[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
    pub enum NetError {
        #[error("couldn't reach {host}: {reason}")]
        Connect { host: String, reason: String },
        #[error("couldn't make a secure connection to {host}: {reason}")]
        Tls { host: String, reason: String },
        #[error(transparent)]
        Proto(#[from] mailcraft_proto::ProtoError),
        #[error("{0}")]
        Config(String),
        /// The owner called the sign-in on the provider's page off ([`oauth::sign_in`](crate::oauth::sign_in)).
        #[error("the sign-in was cancelled")]
        SignInCancelled,
        /// The sign-in on the provider's page wasn't finished in the time it is waited for.
        #[error("the sign-in wasn't finished in time")]
        SignInTimedOut,
        /// The sign-in is good, but it doesn't cover what was asked for until the owner agrees to
        /// that on the provider's page: a token for Microsoft Graph, for an owner who was never
        /// asked ([`oauth::refresh_for`](crate::oauth::refresh_for)). Signing in to mail again is
        /// not the cure; [`oauth::sign_in_for`](crate::oauth::sign_in_for) is.
        #[error("permission is still to be given: {0}")]
        ConsentNeeded(String),
        /// The service took the sign-in and won't do this for the account all the same (Microsoft
        /// Graph's 403): asking again changes nothing.
        #[error("{0}")]
        Forbidden(String),
        /// "Not now, later" (Microsoft Graph's 429, 503 and 504). `retry_after` is how many
        /// seconds the service asked to be left alone for, when it said.
        #[error("{text}")]
        Busy { text: String, retry_after: Option<u32> },
        /// The organisation the account belongs to doesn't allow the service, for anybody or for
        /// this program (Exchange Web Services' 403 with an `X-EWS-Policy-Reason` header: the
        /// service is switched off there, or the program is not on its list). Asking again
        /// changes nothing; only the organisation's administrator can.
        #[error("{0}")]
        Blocked(String),
    }

    impl NetError {
        /// The same error with `secret` (a password, a token) blanked out, should a server have written it back.
        pub(crate) fn without(self, secret: &str) -> NetError {
            use mailcraft_proto::ProtoError as P;
            let clean = |text: String| scrub(&text, secret);
            match self {
                NetError::Connect { host, reason } => NetError::Connect { host, reason: clean(reason) },
                NetError::Tls { host, reason } => NetError::Tls { host, reason: clean(reason) },
                NetError::Config(text) => NetError::Config(clean(text)),
                NetError::Proto(e) => NetError::Proto(match e {
                    P::Io(text) => P::Io(clean(text)),
                    P::Protocol(text) => P::Protocol(clean(text)),
                    P::Rejected(text) => P::Rejected(clean(text)),
                    P::Auth(text) => P::Auth(clean(text)),
                }),
                NetError::ConsentNeeded(text) => NetError::ConsentNeeded(clean(text)),
                NetError::Forbidden(text) => NetError::Forbidden(clean(text)),
                NetError::Busy { text, retry_after } => NetError::Busy { text: clean(text), retry_after },
                NetError::Blocked(text) => NetError::Blocked(clean(text)),
                unworded @ (NetError::SignInCancelled | NetError::SignInTimedOut) => unworded,
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
        NetError::Config(if who.is_empty() { "sign in to the account first".into() } else { format!("sign in to {who} first") })
    }

    /// Why a shared mailbox couldn't be opened ([`check_shared`]).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum SharedCheck {
        /// The server didn't let the account in to it, or it has no Inbox to show the account:
        /// opening it takes Full Access, which an administrator gives. What the server said.
        NoAccess(String),
        /// It couldn't be found out this time (the server couldn't be reached, say).
        Failed(NetError),
    }

    /// Whether the account whose access token `token` is may open the shared mailbox `account`
    /// (as [`mailcraft_model::Account::shared_of`] signs in to one: IMAP with the shared mailbox's
    /// address as the user and the owner's token): signed in to, and its Inbox opened to read.
    /// Nothing is changed and nothing is downloaded. The token never appears in the answer.
    pub fn check_shared(account: &Account, token: &str) -> Result<(), SharedCheck> {
        if account.incoming.is_none() {
            return Err(SharedCheck::Failed(NetError::Config("the account has no incoming mail server".into())));
        }
        let mut imap = match crate::sync::open_imap(account, token) {
            Ok(imap) => imap,
            Err(e) => {
                let e = e.without(token);
                return Err(if e.is_refused_sign_in() { SharedCheck::NoAccess(e.to_string()) } else { SharedCheck::Failed(e) });
            }
        };
        let opened = imap.select("INBOX", true);
        let _ = imap.logout();
        match opened {
            Ok(_) => Ok(()),
            Err(e @ (mailcraft_proto::ProtoError::Rejected(_) | mailcraft_proto::ProtoError::Auth(_))) => {
                Err(SharedCheck::NoAccess(NetError::from(e).without(token).to_string()))
            }
            Err(e) => Err(SharedCheck::Failed(NetError::from(e).without(token))),
        }
    }

    /// Check that the account's servers accept the sign-in (IMAP login, then SMTP EHLO/AUTH when an outgoing server is set).
    ///
    /// `secret` is the account's password, or the access token of an account that signs in with
    /// OAuth ([`Account::oauth`]).
    pub fn test_account(account: &Account, secret: &str) -> Result<(), NetError> {
        if account.incoming.is_none() && account.outgoing.is_none() {
            return Err(NetError::Config("the account has no mail servers to check".into()));
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
