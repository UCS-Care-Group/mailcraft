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
//! - **The server's changes come in, the owner's edits stay.** A fingerprint of the words the
//!   server last gave (its HTML and its text) is kept with the account
//!   ([`mailcraft_model::EwsServer::signature_seen`]). The signature is brought in again only when
//!   the server's words are others, so a signature the owner changed here stays until the server's
//!   own change; and one the server no longer has is **not** removed. Whether the server adds it to
//!   new mail and replies is no part of the fingerprint: a change of that alone brings nothing in.
//! - **Only its own signature is written.** The one taken in is known by its name
//!   ([`mailcraft_model::EwsServer::signature_name`]), which follows it when the owner renames it
//!   here. A signature the owner made is never written over, whatever it is called: when
//!   `<address> (Exchange)` is the owner's own, the server's is called `<address> (Exchange 2)`, and
//!   so on. One the owner removed here comes back only with a change of the server's, as a new
//!   signature.
//! - **An open copy follows.** When File › Options › Signatures has it open with no changes, the
//!   open copy shows what came in; one with changes is the owner's, to save or to discard.
//! - **Defaults only the first time.** When the account has no signature chosen yet, the one taken
//!   in becomes what it starts new mail (and replies) with, as the server's options say; after that
//!   File › Options › Signatures is the owner's.
//! - **Hostile text.** The HTML goes through the same reader as any pasted signature (no script, no
//!   style sheet, no picture fetched); what the signature's size or the mailbox's count of
//!   signatures does not allow is left out and the account is left as it was.
//! - **Pictures the server keeps apart.** HTML that shows a picture by `cid:` (a part of the
//!   message Outlook on the web would send with it, which this account is not given) is not sent as
//!   written: it would go out with a broken picture. It is kept as rich text instead, its words and
//!   their look without those pictures.

use mailcraft_model::rich::RichText;
use mailcraft_model::signature::Signature;
use mailcraft_model::sync::{EwsSignature, fingerprint};
use mailcraft_model::{Id, Mailbox, clean_name};

/// Most signatures one mailbox keeps (`signature.set`'s).
const MAX_SIGNATURES: usize = 100;

/// Most names tried for the signature taken in when the ones before are the owner's own.
const MAX_NAMES: usize = 9;

/// What a signature taken from the account `email` is called: the `n`th name tried, from 1.
fn name_of(email: &str, n: usize) -> Option<String> {
    let email = email.trim();
    clean_name(&if n <= 1 { format!("{email} (Exchange)") } else { format!("{email} (Exchange {n})") })
}

/// A fingerprint of the words of `found`, its HTML and its text. (Whether the server adds it is
/// only read the first time, so it is left out: a change of that alone is no new signature.)
fn seen(found: &EwsSignature) -> u64 {
    let mut bytes = Vec::with_capacity(found.html.len().saturating_add(found.text.len()).saturating_add(1));
    bytes.extend_from_slice(found.html.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(found.text.as_bytes());
    // (0 is "nothing seen yet".)
    fingerprint(&bytes).max(1)
}

/// Whether `html` shows a picture by `cid:`, a part of the message it would have to be sent with.
fn shows_parts(html: &str) -> bool {
    let blocks = mailcraft_html::parse_text(html);
    let cid = |src: &str| src.trim_start().get(..4).is_some_and(|scheme| scheme.eq_ignore_ascii_case("cid:"));
    mailcraft_html::images(&blocks).iter().any(|b| matches!(&b.kind, mailcraft_html::BlockKind::Image { src, .. } if cid(src)))
        || mailcraft_html::bg_pictures(&blocks).iter().any(|p| cid(&p.src))
}

/// Whether `html` shows anything at all: words, or a picture of any kind. HTML that shows
/// nothing (empty paragraphs, a bare `&nbsp;`) would go out as a blank signature.
fn shows_anything(html: &str, rich: &RichText) -> bool {
    if !rich.to_plain().trim().is_empty() {
        return true;
    }
    let blocks = mailcraft_html::parse_text(html);
    !mailcraft_html::images(&blocks).is_empty() || !mailcraft_html::bg_pictures(&blocks).is_empty()
}

/// Take in the signature the server gave for `account`: the name of the signature it wrote, or
/// `None` when the mailbox is as it was.
pub(crate) fn take(mb: &mut Mailbox, account: Id, found: &EwsSignature) -> Option<String> {
    if found.is_empty() || !found.within_bounds() {
        return None;
    }
    let a = mb.account(account)?;
    let server = a.ews.as_ref()?;
    let fingerprint = seen(found);
    if server.signature_seen == fingerprint {
        return None;
    }
    let first = server.signature_seen == 0;
    // The one taken in before, while it is there; else a name nobody's signature has.
    let ours = Some(server.signature_name.as_str()).filter(|name| !name.is_empty()).and_then(|name| mb.signature(name)).map(|x| x.name.clone());
    let name = match ours {
        Some(name) => name,
        None => (1..=MAX_NAMES).filter_map(|n| name_of(&a.email, n)).find(|name| mb.signature(name).is_none())?,
    };
    let (html, text) = (found.html.trim(), found.text.trim());
    let rich = (!html.is_empty()).then(|| mailcraft_mime_model::rich::from_html(html));
    // HTML sent as written, unless it shows pictures it would have to be sent with, or shows
    // nothing at all (then the plain text goes, as it does in the body below).
    let as_written = rich.as_ref().is_some_and(|rich| !shows_parts(html) && shows_anything(html, rich));
    // Its words and look as rich text; HTML with no words (a logo alone) shows its pictures'
    // descriptions, as `signature.html` does.
    let body = match rich {
        None => RichText::from_plain(text),
        Some(rich) if rich.to_plain().trim().is_empty() => {
            RichText::from_plain(if text.is_empty() { mailcraft_html::to_text(html) } else { text.to_owned() }.trim())
        }
        Some(rich) => rich,
    };
    let at = mb.signatures.iter().position(|x| x.name.eq_ignore_ascii_case(&name));
    if at.is_none() && mb.signatures.len() >= MAX_SIGNATURES {
        return None;
    }
    let mut signature = at.and_then(|i| mb.signatures.get(i).cloned()).unwrap_or_else(|| Signature::plain_text(name.clone(), ""));
    if signature.set_body(body).is_err() || signature.set_html(as_written.then(|| html.to_owned())).is_err() {
        return None;
    }
    match at.and_then(|i| mb.signatures.get_mut(i)) {
        Some(slot) => *slot = signature,
        None => mb.signatures.push(signature),
    }
    if let Ok(a) = mb.account_mut(account) {
        if first && a.signature.is_empty() && a.reply_signature.is_none() {
            a.signature = if found.on_new { name.clone() } else { String::new() };
            a.reply_signature = Some(if found.on_reply { name.clone() } else { String::new() });
        }
        if let Some(server) = a.ews.as_mut() {
            server.signature_seen = fingerprint;
            server.signature_name.clone_from(&name);
        }
    }
    Some(name)
}

#[cfg(test)]
#[path = "ews_signature_tests.rs"]
mod tests;
