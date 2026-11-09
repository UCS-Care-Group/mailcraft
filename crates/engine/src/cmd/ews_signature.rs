// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The signature an Exchange server keeps for its owner, taken in (#484).
//!
//! Outlook on the web keeps the owner's signature in the mailbox's options; a run of an Exchange
//! server account that reads every folder asks the server for it ([`mailcraft_ews::GetUserOptions`])
//! and this takes it in as a signature of this mailbox, named `<address> (Exchange)`:
//!
//! - **Read, never written.** Nothing is sent back to the server; a signature written here stays
//!   here, and the one on the server is only read.
//! - **The server's changes come in, the owner's edits stay.** A fingerprint of what the server last
//!   gave is kept with the account ([`mailcraft_model::EwsServer::signature_seen`]). The signature
//!   is brought in again only when the server's is another, so a signature the owner changed here
//!   stays until the server's own changes; and one the server no longer has is **not** removed.
//! - **Defaults only the first time.** When the account has no signature chosen yet, the one taken
//!   in becomes what it starts new mail (and replies) with, as the server's options say; after that
//!   File › Options › Signatures is the owner's.
//! - **Hostile text.** The HTML goes through the same reader as any pasted signature (no script, no
//!   style sheet, no picture fetched); what the signature's size or the mailbox's count of
//!   signatures does not allow is left out and the account is left as it was.

use mailcraft_model::rich::RichText;
use mailcraft_model::signature::Signature;
use mailcraft_model::sync::{EwsSignature, fingerprint};
use mailcraft_model::{Id, Mailbox, clean_name};

/// Most signatures one mailbox keeps (`signature.set`'s).
const MAX_SIGNATURES: usize = 100;

/// What a signature taken from the account `of` is called.
fn name_of(email: &str) -> Option<String> {
    clean_name(&format!("{} (Exchange)", email.trim()))
}

/// A fingerprint of everything in `found` that is kept.
fn seen(found: &EwsSignature) -> u64 {
    let mut bytes = Vec::with_capacity(found.html.len().saturating_add(found.text.len()).saturating_add(8));
    bytes.extend_from_slice(found.html.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(found.text.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&[u8::from(found.on_new), u8::from(found.on_reply)]);
    // (0 is "nothing seen yet".)
    fingerprint(&bytes).max(1)
}

/// Take in the signature the server gave for `account`. Whether the mailbox changed.
pub(crate) fn take(mb: &mut Mailbox, account: Id, found: &EwsSignature) -> bool {
    if found.is_empty() || !found.within_bounds() {
        return false;
    }
    let Some(a) = mb.account(account) else { return false };
    let Some(server) = a.ews.as_ref() else { return false };
    let fingerprint = seen(found);
    if server.signature_seen == fingerprint {
        return false;
    }
    let Some(name) = name_of(&a.email) else { return false };
    let first = server.signature_seen == 0;
    let (html, text) = (found.html.trim(), found.text.trim());
    // Its words and look as rich text; HTML with no words (a logo alone) shows its pictures'
    // descriptions, as `signature.html` does.
    let body = if html.is_empty() {
        RichText::from_plain(text)
    } else {
        let rich = mailcraft_mime::rich::from_html(html);
        if rich.to_plain().trim().is_empty() {
            RichText::from_plain(if text.is_empty() { mailcraft_html::to_text(html) } else { text.to_owned() }.trim())
        } else {
            rich
        }
    };
    let at = mb.signatures.iter().position(|x| x.name.eq_ignore_ascii_case(&name));
    if at.is_none() && mb.signatures.len() >= MAX_SIGNATURES {
        return false;
    }
    let mut signature = at.and_then(|i| mb.signatures.get(i).cloned()).unwrap_or_else(|| Signature::plain_text(name.clone(), ""));
    if signature.set_body(body).is_err() || (!html.is_empty() && signature.set_html(Some(html.to_owned())).is_err()) {
        return false;
    }
    match at {
        Some(i) => {
            if let Some(slot) = mb.signatures.get_mut(i) {
                *slot = signature;
            }
        }
        None => mb.signatures.push(signature),
    }
    if let Ok(a) = mb.account_mut(account) {
        if first && a.signature.is_empty() && a.reply_signature.is_none() {
            a.signature = if found.on_new { name.clone() } else { String::new() };
            a.reply_signature = Some(if found.on_reply { name } else { String::new() });
        }
        if let Some(server) = a.ews.as_mut() {
            server.signature_seen = fingerprint;
        }
    }
    true
}

#[cfg(test)]
#[path = "ews_signature_tests.rs"]
mod tests;
