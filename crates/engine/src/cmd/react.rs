// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! React (#1544): a message answered with one emoji, without writing a reply. It goes out as a
//! small mail through the account's normal sending (RFC 9078): `In-Reply-To` and `References`
//! naming the original, and the emoji as a part marked `Content-Disposition: reaction` with the
//! same emoji as plain text beside it for a program that doesn't know the disposition. Nothing
//! Exchange-specific: a Microsoft or Exchange server account is refused, whatever the server
//! could do on its own.

use mailcraft_model::{AccountKind, Address, Body, FolderKind, Id, Message};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mail.react", "React", "Home › Respond", react)
            .params(r#"{"id"?: number, "emoji": "👍"|"❤️"|"🎉"|"😂"|"😮"|"😢"} (default `id`: the selected message; an `id` that is null or isn't an id, or `ids` naming other than one message, is refused) — answers the message with one emoji, as a short mail to its sender (RFC 9078: In-Reply-To, References and a part marked Content-Disposition: reaction beside the same emoji as plain text), sent like any mail and filed in Sent Items; no message is opened to write. Only those six emoji; a draft, mail the owner sent, mail in Junk E-mail, mail without a sender and mail of a Microsoft or Exchange server account are refused. Goes to one address (the message's Reply-To, else its sender; mail from a mailing list goes to the sender alone); a second reaction with the same emoji is refused. Answers {"id", "queued", "emoji"}"#)
            .selection(needs_reactable),
    ]
}

/// The emoji a reaction may be, as they are sent, with the name the window shows for each.
pub const REACTIONS: [(&str, &str); 6] = [
    ("\u{1F44D}", "Like"),
    ("\u{2764}\u{FE0F}", "Love"),
    ("\u{1F389}", "Celebrate"),
    ("\u{1F602}", "Laugh"),
    ("\u{1F62E}", "Surprised"),
    ("\u{1F622}", "Sad"),
];

/// Why a message can't be reacted to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// A draft, or mail the owner wrote (Drafts, Outbox, Sent Items, or from one of their addresses).
    Own,
    /// Junk E-mail: an answer would tell a spammer the address is read.
    Junk,
    /// Nobody to send it to (no valid sender).
    NoSender,
    /// Nothing for `In-Reply-To` to name.
    NoMessageId,
    /// A Microsoft or Exchange server account, or one of a kind this version doesn't know.
    Account,
}

const OWN: &str = "a reaction can't be sent to a draft or to mail you wrote";
const JUNK: &str = "a reaction isn't sent to mail in Junk E-mail";
const NO_SENDER: &str = "this message has no sender to send a reaction to";
const NO_MESSAGE_ID: &str = "this message has no Message-ID, so a reaction can't say what it answers";
const ACCOUNT: &str = "a reaction is sent only from IMAP, JMAP and local accounts, not from a Microsoft or Exchange server account";

/// Whether `m` can be reacted to, or why not. The window greys its button by the same word the
/// command refuses by.
pub fn why_not(s: &Session, m: &Message) -> Option<Why> {
    let mb = &s.mailbox;
    let own_folder = mb.folder(m.folder).is_some_and(|f| matches!(f.kind, FolderKind::Drafts | FolderKind::Sent | FolderKind::Outbox));
    let mine = mb.accounts.iter().any(|a| !a.email.is_empty() && a.email.eq_ignore_ascii_case(&m.from.email));
    if m.draft || own_folder || mine {
        return Some(Why::Own);
    }
    if mb.in_junk(m.folder) {
        return Some(Why::Junk);
    }
    if recipient(s, m).is_none() {
        return Some(Why::NoSender);
    }
    if m.message_id.trim().is_empty() {
        return Some(Why::NoMessageId);
    }
    match mb.account(m.account).map(|a| a.kind) {
        Some(AccountKind::Local | AccountKind::Imap | AccountKind::Jmap) => None,
        _ => Some(Why::Account),
    }
}

/// Whom the reaction goes to: one address. The message's Reply-To (the first that isn't the
/// owner's), else its sender, as a reply goes (#736); mail from a mailing list (`List-Id`) goes to
/// its sender alone, never to the list its Reply-To names.
fn recipient(s: &Session, m: &Message) -> Option<Address> {
    let mine = |a: &Address| s.mailbox.accounts.iter().any(|x| !x.email.is_empty() && x.email.eq_ignore_ascii_case(&a.email));
    let list = m.has_header("List-Id");
    let replies = if list { None } else { m.reply_to.iter().find(|a| a.is_valid() && !mine(a)) };
    replies.or(Some(&m.from)).filter(|a| a.is_valid() && !mine(a)).cloned()
}

/// Enabled with a message selected that can be reacted to.
fn needs_reactable(s: &Session) -> Option<&'static str> {
    let id = s.sel.focus()?;
    s.mailbox.message(id).and_then(|m| why_not(s, m)).map(text)
}

fn text(why: Why) -> &'static str {
    match why {
        Why::Own => OWN,
        Why::Junk => JUNK,
        Why::NoSender => NO_SENDER,
        Why::NoMessageId => NO_MESSAGE_ID,
        Why::Account => ACCOUNT,
    }
}

/// `emoji` as a reaction is sent, when it is one of the six (a heart with or without the variation
/// selector); `None` for anything else.
pub fn canonical(emoji: &str) -> Option<&'static str> {
    let emoji = emoji.trim();
    let bare = emoji.trim_end_matches('\u{FE0F}');
    REACTIONS.iter().map(|(e, _)| *e).find(|e| *e == emoji || e.trim_end_matches('\u{FE0F}') == bare)
}

fn react(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id", "ids", "emoji"])?;
    let id = p::message_strict(v)?.or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let emoji = canonical(p::req_str(v, "emoji")?).ok_or_else(|| bad("`emoji` is one of 👍 ❤️ 🎉 😂 😮 😢"))?;
    let original = s.mailbox.message(id).cloned().ok_or_else(|| failed!("no message with id {id}", id))?;
    match why_not(s, &original) {
        None => {}
        Some(Why::Own) => return Err(fail(OWN)),
        Some(Why::Junk) => return Err(fail(JUNK)),
        Some(Why::NoSender) => return Err(fail(NO_SENDER)),
        Some(Why::NoMessageId) => return Err(fail(NO_MESSAGE_ID)),
        Some(Why::Account) => return Err(fail(ACCOUNT)),
    }
    let to = recipient(s, &original).ok_or_else(|| fail(NO_SENDER))?;
    // The same reaction twice (a double click) is one mail: refused while the first is in this
    // computer's Outbox or Sent Items.
    let again = s.mailbox.messages.values().any(|m| {
        m.reaction
            && m.account == original.account
            && m.in_reply_to == original.message_id
            && m.body.text.trim() == emoji
            && s.mailbox.folder(m.folder).is_some_and(|f| matches!(f.kind, FolderKind::Outbox | FolderKind::Sent))
    });
    if again {
        return Err(failed!("you already reacted to this message with {emoji}", emoji = emoji.to_string()));
    }
    let account = s.mailbox.account(original.account).cloned().ok_or_else(|| fail("the account is gone"))?;
    let queued = account.kind.sends_mail();
    let kind = if queued { FolderKind::Outbox } else { FolderKind::Sent };
    let folder = s
        .mailbox
        .folder_of_kind(account.id, kind)
        .ok_or_else(|| failed!("this account has no {folder} folder", folder = mailcraft_i18n::Arg::folder(kind.default_name())))?;
    let now = s.now();
    let domain = account.email.rsplit_once('@').map_or("", |(_, d)| d);
    let message_id = mailcraft_mime::new_message_id(domain, now, s.rev().wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ id.rotate_left(32) ^ emoji.len() as u64);
    let m = Message {
        account: account.id,
        folder,
        message_id,
        in_reply_to: original.message_id.clone(),
        references: super::compose::references_of(&original),
        from: account.address(),
        to: vec![to],
        subject: super::compose::prefixed("RE", &original.subject),
        date: now,
        received: now,
        body: Body { text: emoji.to_string(), html: None },
        read: true,
        reaction: true,
        ..Default::default()
    };
    let sent = s.mailbox.add_message(m.clone())?;
    // A local account has nowhere to send it: filed as sent, and delivered to the local accounts
    // it names, as Send does.
    if !queued {
        let locals: Vec<(Id, String)> = s
            .mailbox
            .accounts
            .iter()
            .filter(|a| a.kind == AccountKind::Local && !a.email.is_empty())
            .map(|a| (a.id, a.email.to_lowercase()))
            .collect();
        for (local, email) in locals {
            if !m.recipients().any(|r| r.email.to_lowercase() == email) {
                continue;
            }
            let Some(inbox) = s.mailbox.folder_of_kind(local, FolderKind::Inbox) else { continue };
            let copy = Message { id: 0, folder: inbox, read: false, reaction: false, ..m.clone() };
            let cid = s.mailbox.add_message(copy)?;
            s.mailbox.apply_rules(cid);
        }
    }
    Ok(json!({"id": sent, "queued": queued, "emoji": emoji}))
}
