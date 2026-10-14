//! What a shared mailbox is told when its server says no ([`mailcraft_model::Account::shared_of`]).
//!
//! A shared mailbox is opened with its owner's own sign-in: over IMAP with the shared mailbox's
//! address as the user and the owner's access token (SASL XOAUTH2), and mail from it goes out
//! through SMTP signed in as the owner with the shared mailbox's address as the sender. Whether
//! the server lets that happen is a matter of rights an administrator gives (Full Access, Send
//! As), not of signing in again; the server's refusal is said in those words.

use mailcraft_model::sync::SyncRequest;
use mailcraft_proto::ProtoError;

use crate::NetError;

/// A shared mailbox's address for a line of text: one line, not too long.
fn shown(address: &str) -> String {
    address.trim().chars().filter(|c| !c.is_control()).take(80).collect()
}

/// What to say when the incoming server wouldn't open a shared mailbox, in place of the usual
/// line; `None` for any other account, or for any other trouble (a server that can't be reached
/// is no refusal).
pub(crate) fn opening_refused(request: &SyncRequest, error: &NetError) -> Option<String> {
    if !request.account.is_shared() || !error.is_refused_sign_in() {
        return None;
    }
    Some(format!(
        "Couldn't open the shared mailbox {}: the server didn't let your account in. Opening it takes Full Access to it, which an administrator of your organisation gives ({error})",
        shown(&request.account.email)
    ))
}

/// What to say when the outgoing server wouldn't send a message from a shared mailbox's
/// address, in place of the usual line; `None` for any other account or any other trouble.
pub(crate) fn sending_refused(request: &SyncRequest, subject: &str, error: &NetError) -> Option<String> {
    if !request.account.is_shared() || request.send_login.trim().is_empty() {
        return None;
    }
    // Exchange Online says it on MAIL FROM (`550 5.7.60 … Client does not have permissions to
    // send as this sender`) or after the message (`554 5.2.252 SendAsDenied`). A recipient it
    // won't take is another matter, and keeps its own line.
    let NetError::Proto(ProtoError::Rejected(said)) = error else { return None };
    let said = said.to_ascii_lowercase();
    if !["5.7.60", "5.2.252", "sendasdenied", "send as", "send-as"].iter().any(|sign| said.contains(sign)) {
        return None;
    }
    Some(format!(
        "Couldn't send \"{subject}\" from {}: the server didn't let your account send as that mailbox. Sending from it takes the Send As right on it, which an administrator of your organisation gives ({error})",
        shown(&request.account.email)
    ))
}
