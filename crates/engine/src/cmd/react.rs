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
            .params(r#"{"id"?: number, "emoji": "👍"|"❤️"|"🎉"|"😂"|"😮"|"😢"} (default `id`: the selected message; an `id` that is null or isn't an id, or `ids` naming other than one message, is refused) — answers the message with one emoji, as a short mail to its sender (RFC 9078: In-Reply-To, References and a part marked Content-Disposition: reaction beside the same emoji as plain text), sent like any mail (with the wait of Undo Send, if the owner set one) and filed in Sent Items; no message is opened to write. Only those six emoji; a draft, mail the owner sent, mail in Junk E-mail, mail without a sender, automated mail (Auto-Submitted, Precedence: bulk/list/junk, a no-reply sender) and mail of a Microsoft or Exchange server account are refused. Goes to one address, the message's sender (From; its Reply-To is never used, so the owner sees whom it goes to: the window shows it beside the emoji); a message with a sender warning (a lookalike domain, a name that shows another address) is refused; a second reaction with the same emoji is refused. Answers {"id", "queued", "emoji", "sendAt", "wait"}"#)
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
    /// Mail a program wrote (`Auto-Submitted`, `Precedence: bulk`, a no-reply address, a list's own
    /// address as the sender): nobody reads an answer to it.
    Automated,
    /// Nothing for `In-Reply-To` to name.
    NoMessageId,
    /// A Microsoft or Exchange server account, or one of a kind this version doesn't know.
    Account,
    /// The message has a warning about its sender (a lookalike domain, a name that shows another
    /// address): an answer would tell whoever is behind it that the address is read.
    Warned,
}

const OWN: &str = "a reaction can't be sent to a draft or to mail you wrote";
const JUNK: &str = "a reaction isn't sent to mail in Junk E-mail";
const NO_SENDER: &str = "this message has no sender to send a reaction to";
const AUTOMATED: &str = "a reaction isn't sent to automated mail (a no-reply address, a mailing list, bulk mail)";
const NO_MESSAGE_ID: &str = "this message has no Message-ID, so a reaction can't say what it answers";
const WARNED: &str = "this message has a warning about its sender, so no reaction is sent to it";
const ACCOUNT: &str = "a reaction is sent only from IMAP, JMAP and local accounts, not from a Microsoft or Exchange server account";

/// [`why_not`], and a message with a warning about its sender refused too: the warnings that say
/// the sender may not be who it seems (a failed check, a name that hides another address, a
/// look-alike or made-up domain, a brand's name on another domain). A warning about a link in the
/// body, or the External tag, is no reason: an ordinary mail from a colleague is not refused for
/// them. These are worked out and kept by the session, so it takes `&mut`: the window and the
/// command ask this; only the menu's enabled state (which has the session read-only) asks
/// [`why_not`] alone, and the command still refuses.
pub fn refusal(s: &mut Session, m: &Message) -> Option<Why> {
    use mailcraft_model::sender_warnings::Warning;
    if let Some(why) = why_not(s, m) {
        return Some(why);
    }
    let about_sender = |w: &Warning| !matches!(w, Warning::External { .. } | Warning::LinkMismatch { .. } | Warning::LinksUnchecked);
    s.sender_warnings(m, false).iter().any(about_sender).then_some(Why::Warned)
}

/// Whether `m` can be reacted to, apart from a sender warning ([`refusal`]), or why not. The
/// window greys its button by the same word the command refuses by.
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
    if automated(m) {
        return Some(Why::Automated);
    }
    if m.message_id.trim().is_empty() {
        return Some(Why::NoMessageId);
    }
    match mb.account(m.account).map(|a| a.kind) {
        Some(AccountKind::Local | AccountKind::Imap | AccountKind::Jmap) => None,
        _ => Some(Why::Account),
    }
}

/// Whom the reaction goes to: the message's sender, one address, never its Reply-To (a reaction is
/// sent without the owner seeing the address, so it must not go to one the message only names: a
/// `Reply-To` of another domain would learn that the address is read). The window shows it beside
/// the emoji. `None` for a sender that isn't a valid address or is the owner's own.
pub fn recipient(s: &Session, m: &Message) -> Option<Address> {
    let mine = s.mailbox.accounts.iter().any(|x| !x.email.is_empty() && x.email.eq_ignore_ascii_case(&m.from.email));
    (m.from.is_valid() && !mine).then(|| m.from.clone())
}

/// Enabled with a message selected that can be reacted to.
fn needs_reactable(s: &Session) -> Option<&'static str> {
    let id = s.sel.focus()?;
    s.mailbox.message(id).and_then(|m| why_not(s, m)).map(text)
}

pub fn text(why: Why) -> &'static str {
    match why {
        Why::Own => OWN,
        Why::Junk => JUNK,
        Why::NoSender => NO_SENDER,
        Why::Automated => AUTOMATED,
        Why::NoMessageId => NO_MESSAGE_ID,
        Why::Account => ACCOUNT,
        Why::Warned => WARNED,
    }
}

/// Whether `m` was written by a program or a list rather than a person: `Auto-Submitted` other than
/// `no` (RFC 3834), `Precedence` bulk, list or junk, `List-Id`/`List-Post` with the list's own
/// address as the sender (a list that rewrites From), or a sender whose name says nobody reads
/// what comes back (`noreply`, `no-reply`, `do_not_reply`, `mailer-daemon`, `postmaster`). Only
/// headers kept on the message by name are looked at ([`mailcraft_model::headers::WORKING`]): mail
/// that was read before this was added keeps them in its block and is judged by its sender alone.
fn automated(m: &Message) -> bool {
    let auto = m.header_values("Auto-Submitted").any(|v| {
        let keyword = v.split(';').next().unwrap_or("").trim();
        !keyword.is_empty() && !keyword.eq_ignore_ascii_case("no")
    });
    let bulk = m.header_values("Precedence").any(|v| {
        let v = v.trim();
        ["bulk", "list", "junk"].iter().any(|w| v.eq_ignore_ascii_case(w))
    });
    let from = m.from.email.trim().to_ascii_lowercase();
    let list_address = m.header_values("List-Post").any(|v| list_post_address(v).is_some_and(|a| a == from));
    auto || bulk || list_address || no_reply(&from)
}

/// The address a `List-Post` line names (`<mailto:list@example.org>`), lower case.
fn list_post_address(value: &str) -> Option<String> {
    let rest = value.to_ascii_lowercase();
    let at = rest.find("mailto:")?;
    let address = rest.get(at + "mailto:".len()..)?;
    let end = address.find(['>', '?', ' ', ',']).unwrap_or(address.len());
    address.get(..end).map(|a| a.trim().to_string()).filter(|a| !a.is_empty())
}

/// Whether the part of `address` before the `@` is one of the names programs send from.
fn no_reply(address: &str) -> bool {
    let local = address.rsplit_once('@').map_or(address, |(local, _)| local);
    let squeezed: String = local.chars().filter(|c| !matches!(c, '-' | '_' | '.')).collect();
    squeezed.starts_with("noreply") || squeezed.starts_with("donotreply") || matches!(squeezed.as_str(), "mailerdaemon" | "postmaster")
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
    match refusal(s, &original) {
        None => {}
        Some(Why::Own) => return Err(fail(OWN)),
        Some(Why::Junk) => return Err(fail(JUNK)),
        Some(Why::NoSender) => return Err(fail(NO_SENDER)),
        Some(Why::Automated) => return Err(fail(AUTOMATED)),
        Some(Why::NoMessageId) => return Err(fail(NO_MESSAGE_ID)),
        Some(Why::Account) => return Err(fail(ACCOUNT)),
        Some(Why::Warned) => return Err(fail(WARNED)),
    }
    let to = recipient(s, &original).ok_or_else(|| fail(NO_SENDER))?;
    // The same reaction twice (a double click) is one mail: refused while the first is in this
    // computer's Outbox or Sent Items. The copy a server gives back to Sent Items has lost the
    // `reaction` mark, so it is known by what it is: an answer to this message whose text is the emoji.
    let again = s.mailbox.messages.values().any(|m| {
        m.account == original.account
            && m.in_reply_to == original.message_id
            && canonical(&m.body.text) == Some(emoji)
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
    let salt = emoji.bytes().fold(0u64, |acc, b| acc.wrapping_mul(0x0100_0000_01B3).wrapping_add(u64::from(b)));
    let message_id = mailcraft_mime::new_message_id(domain, now, s.rev().wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ id.rotate_left(32) ^ salt);
    // The wait of Undo Send applies as it does to any message sent (`compose::send_now`): the
    // reaction waits in the Outbox that long and the Undo bar offers to take it back.
    let waits = s.prefs.undo_send();
    let by_wait = queued && waits > 0;
    let send_at = by_wait.then(|| now.saturating_add(i64::from(waits)));
    let at = send_at.unwrap_or(now);
    let m = Message {
        account: account.id,
        folder,
        message_id,
        in_reply_to: original.message_id.clone(),
        references: super::compose::references_of(&original),
        from: account.address(),
        to: vec![to],
        subject: super::compose::prefixed("RE", &original.subject),
        date: at,
        received: at,
        send_at,
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
    s.undo_send = by_wait.then_some(sent);
    // (This command's own step is pushed once it returns.)
    s.undo_send_step = by_wait.then(|| s.undo_steps_moved().wrapping_add(1));
    Ok(json!({"id": sent, "queued": queued, "emoji": emoji, "sendAt": send_at, "wait": by_wait}))
}
