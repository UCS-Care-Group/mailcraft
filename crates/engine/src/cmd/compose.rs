// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writing mail: new messages, replies, forwards, drafts, sending.

use mailcraft_model::rich::RichText;
use mailcraft_model::signature::Picture;
use mailcraft_model::sync::{DraftKind, DraftOf, DraftState, ServerDraft};
use mailcraft_model::{
    AccountKind, Address, Answers, Attachment, Body, FlagStatus, FolderKind, Id, Importance, Message, NOT_DOWNLOADED, NOT_DOWNLOADED_NOTE,
    NOT_SENDING_YET, RichBody, Signature, format_list,
};
use serde_json::{Value, json};

use super::{bad, base64_decode, done, fail, needs_compose, needs_message, source};
use crate::{BodyFormat, CmdError, CmdResult, CommandSpec, Compose, ComposeMode, InsertedSignature, Quote, Session, p};

/// Largest single attachment and most attachments on one message.
const MAX_ATTACHMENT: usize = 64 << 20;
const MAX_ATTACHMENTS: usize = 200;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mail.new", "New Email", "Home › New", new)
            .pure()
            .key("Mod+N")
            .params(
                r#"{"to"?: string, "cc"?: string, "bcc"?: string, "subject"?: string, "body"?: string, "account"?: number, "mailto"?: string (a mailto: link: its recipients, subject and body start the message, and nothing else in it is used)}"#,
            ),
        CommandSpec::new("mail.reply", "Reply", "Home › Respond", reply).pure().key("Mod+R").params(SOURCE).selection(needs_message),
        CommandSpec::new("mail.replyAll", "Reply All", "Home › Respond", reply_all).pure().key("Mod+Shift+R").params(SOURCE).selection(needs_message),
        CommandSpec::new("mail.forward", "Forward", "Home › Respond", forward).pure().key("Mod+F").params(SOURCE).selection(needs_message),
        CommandSpec::new("mail.forwardAsAttachment", "Forward as Attachment", "Home › Respond", forward_as_attachment)
            .pure()
            .key("Mod+Alt+F")
            .params(SOURCE)
            .selection(needs_one_message),
        CommandSpec::new("mail.editDraft", "Open Draft", "Mail", edit_draft).pure().params(SOURCE).selection(needs_message),
        CommandSpec::new("compose.set", "Edit Message", "Message", set)
            .pure()
            .params(r#"{"to"?: string, "cc"?: string, "bcc"?: string, "subject"?: string, "body"?: string, "importance"?: "low"|"normal"|"high", "account"?: number}"#)
            .when(needs_compose),
        CommandSpec::new("compose.attach", "Attach File", "Message › Include", attach)
            .pure()
            .params(r#"{"path": string} | {"name": string, "data": base64}, "mime"?: string"#)
            .when(needs_compose),
        CommandSpec::new("compose.removeAttachment", "Remove Attachment", "Message › Include", remove_attachment).pure().params(r#"{"index": number}"#).when(needs_compose),
        CommandSpec::new("compose.signature", "Signature", "Message › Include", signature).pure().params(r#"{"name"?: string} (default: the account's signature)"#).when(needs_compose),
        CommandSpec::new("compose.checkNames", "Check Names", "Message › Names", check_names).pure().key("Mod+K").when(needs_compose),
        CommandSpec::new("compose.suggest", "Suggest Recipients", "Message › Names", suggest_command)
            .pure()
            .params(r#"{"query": string, "limit"?: 1-20} — answers who the start of a name or address could be: contacts first, then people mail was exchanged with, the latest first"#),
        CommandSpec::new("compose.highImportance", "High Importance", "Message › Tags", high_importance).pure().when(needs_compose),
        CommandSpec::new("compose.lowImportance", "Low Importance", "Message › Tags", low_importance).pure().when(needs_compose),
        CommandSpec::new("compose.send", "Send", "Message", send).key("Mod+Enter / Alt+S").when(needs_compose),
        CommandSpec::new("compose.saveDraft", "Save Draft", "Message", save_draft).key("Mod+S").when(needs_compose),
        CommandSpec::new("compose.close", "Close Message", "Message", close)
            .pure()
            .key("Escape")
            .params(r#"{"discard"?: true} (a message with changes that aren't saved is saved to Drafts first, unless `discard`)"#)
            .when(needs_compose),
        CommandSpec::new("compose.discard", "Discard", "Message", discard).when(needs_compose),
    ]
}

const SOURCE: &str =
    r#"{"id"?: number} (default: the selected message; a message being written with changes that aren't saved is saved to Drafts first)"#;

/// The commands that close the message being written or put another in its place (#442).
pub const LEAVING: [&str; 7] =
    ["compose.close", "mail.new", "mail.reply", "mail.replyAll", "mail.forward", "mail.forwardAsAttachment", "mail.editDraft"];

/// Whether running `id` saves the message being written to Drafts first (#442): it leaves the
/// message, which has changes that aren't saved, and the caller didn't let them go (`discard`).
/// Nobody who calls a command (an agent, a script) is asked, so [`Session::run`] runs Save Draft
/// first, as a step of its own: undoing what the command did never takes the draft away. A save
/// that fails fails the command, and nothing changes. (The window asks the owner first.)
pub fn saves_first(s: &Session, id: &str, params: &Value) -> bool {
    LEAVING.contains(&id) && p::bool(params, "discard") != Some(true) && s.compose.as_ref().is_some_and(|c| c.dirty)
}

fn compose_mut(s: &mut Session) -> Result<&mut Compose, CmdError> {
    s.compose.as_mut().ok_or_else(|| fail("no message is being written"))
}

/// The signature `name` names, else the account's for new messages, or for replies and forwards
/// when `reply`; `None` when that is none, or one with nothing in it.
fn chosen_signature(s: &Session, account: Id, name: Option<&str>, reply: bool) -> Option<Signature> {
    let name = name
        .map(str::to_string)
        .or_else(|| s.mailbox.account(account).map(|a| if reply { a.reply_signature().to_string() } else { a.signature.clone() }))?;
    s.mailbox.signature(&name).filter(|sig| !sig.plain().trim().is_empty() || !sig.pictures().is_empty() || sig.html().is_some()).cloned()
}

/// Put `sig` in the message as its signature (in place of the one there, else at `at`), or take
/// it out; and remember what went in, to tell later whether the owner has edited it.
fn put_signature(c: &mut Compose, sig: Option<&Signature>, at: usize) -> Result<(), CmdError> {
    c.body.put_signature(sig.map(Signature::body), at).map_err(|e| fail(e.to_string()))?;
    c.signature = match (sig, c.body.text().signature()) {
        (Some(sig), Some(text)) => Some(InsertedSignature {
            name: sig.name.clone(),
            layout: sig.layout,
            pictures: sig.pictures().to_vec(),
            text,
            html: sig.html().map(str::to_string),
        }),
        _ => None,
    };
    Ok(())
}

/// A message starting with `sig` at `at`: how it starts is not a step to undo.
fn start_with(c: &mut Compose, sig: &Signature, at: usize) -> Result<(), CmdError> {
    put_signature(c, Some(sig), at)?;
    c.body = RichBody::new(c.body.text().clone());
    Ok(())
}

/// Whether the signature in the message is still the one that went in.
fn signature_unedited(c: &Compose) -> bool {
    c.signature.as_ref().is_some_and(|inserted| c.body.text().signature().as_ref() == Some(&inserted.text))
}

fn is_response(mode: ComposeMode) -> bool {
    matches!(mode, ComposeMode::Reply | ComposeMode::ReplyAll | ComposeMode::Forward)
}

fn new(s: &mut Session, v: &Value) -> CmdResult {
    let account =
        p::id(v, "account").filter(|a| s.mailbox.account(*a).is_some()).or_else(|| s.mailbox.folder(s.view.folder).map(|f| f.account)).unwrap_or(0);
    // A link clicked somewhere else says who the message is for and how it starts; what the
    // caller names itself comes first.
    let link = match p::str(v, "mailto") {
        Some(link) => mailcraft_mime::parse_mailto(link).ok_or_else(|| bad("`mailto` is a mailto: link"))?,
        None => mailcraft_mime::Mailto::default(),
    };
    let text = |key: &str, linked: String| p::str(v, key).map_or(linked, str::to_string);
    let body = text("body", link.body);
    let sig = chosen_signature(s, account, None, false);
    let mut c = Compose {
        mode: ComposeMode::New,
        account,
        to: text("to", link.to.join("; ")),
        cc: text("cc", link.cc.join("; ")),
        bcc: text("bcc", link.bcc.join("; ")),
        subject: text("subject", link.subject),
        // A blank line, then the signature.
        body: RichBody::from_plain(&if sig.is_some() { format!("{body}\n") } else { body }),
        ..Default::default()
    };
    if let Some(sig) = &sig {
        let end = c.body.text().len();
        start_with(&mut c, sig, end)?;
    }
    s.compose = Some(c);
    s.view.module = crate::Module::Mail;
    Ok(json!({"composing": true}))
}

/// The message a command answers, forwards or opens again, whole: its body and its files in
/// memory, fetched from where the mailbox keeps them when they weren't (`Session::content`).
fn source_message(s: &Session, v: &Value) -> Result<Message, CmdError> {
    let id = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    if s.mailbox.message(id).is_none() {
        return Err(failed!("no message with id {id}", id));
    }
    s.loaded_message(id).map(|m| Message::clone(&m))
}

/// The message a reply answers, with what the reply quotes of it in memory: its body and the
/// pictures its HTML shows (`Session::loaded_for_reply`); its other files stay where they are.
fn reply_source(s: &Session, v: &Value) -> Result<Message, CmdError> {
    let id = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    if s.mailbox.message(id).is_none() {
        return Err(failed!("no message with id {id}", id));
    }
    s.loaded_for_reply(id).map(|m| Message::clone(&m))
}

/// `RE: subject`, unless it already starts that way.
fn prefixed(prefix: &str, subject: &str) -> String {
    let lower = subject.trim_start().to_ascii_lowercase();
    if lower.starts_with(&format!("{}:", prefix.to_ascii_lowercase())) { subject.trim().to_string() } else { format!("{prefix}: {}", subject.trim()) }
}

/// The line that opens the quoted original in plain text, in English; a language has its own
/// (`-----Message d'origine-----`), which a draft reopening knows too.
const ORIGINAL: &str = "-----Original Message-----";

/// The names of the header lines above a quoted original, with their colons, in the interface
/// language: the recipient reads them as the owner's program wrote them (French puts a narrow
/// no-break space before the colon: `De\u{202f}:`).
fn header_names(s: &Session) -> [&'static str; 5] {
    let lang = s.interface_language;
    [lang.t("From:"), lang.t("Sent:"), lang.t("To:"), lang.t("Cc:"), lang.t("Subject:")]
}

/// When the original was sent, as its header line says it.
fn sent_on(s: &Session, m: &Message) -> String {
    let at = s.local(m.date);
    s.date_time_said(at)
}

/// Most names a quoted `To:` or `Cc:` line carries; a message can list thousands.
const QUOTED_NAMES: usize = 100;

/// A recipient line for the quote under a reply or forward, cut after [`QUOTED_NAMES`] with a count.
fn quoted_list(list: &[Address]) -> String {
    let (mut line, more) = mailcraft_model::format_list_capped(list, QUOTED_NAMES);
    if more > 0 {
        line.push_str(&format!("; … (+{more})"));
    }
    line
}

/// The original message under a reply or forward, as plain text.
fn quoted(s: &Session, m: &Message) -> String {
    let [from, sent, to, cc, subject] = header_names(s);
    let mut out = format!("\n\n{}\n", s.interface_language.t(ORIGINAL));
    out.push_str(&format!("{from} {}\n", m.from.full()));
    out.push_str(&format!("{sent} {}\n", sent_on(s, m)));
    if !m.to.is_empty() {
        out.push_str(&format!("{to} {}\n", quoted_list(&m.to)));
    }
    if !m.cc.is_empty() {
        out.push_str(&format!("{cc} {}\n", quoted_list(&m.cc)));
    }
    out.push_str(&format!("{subject} {}\n\n", m.subject));
    out.push_str(m.body.text.trim_end());
    out.push('\n');
    out
}

/// The quoted original as plain text for the text part of a message that carries it in HTML: what
/// its HTML reads as, under the opening line. Never the text alternative its sender wrote beside
/// it (the owner never saw that one, and it can say something else).
fn quote_text(marker: &str, quoted_html: &str) -> String {
    let text = mailcraft_html::to_text(quoted_html);
    // (The rule above the header lines is for the eye.)
    let text = text.trim_start_matches('-').trim();
    format!("\n\n{marker}\n{text}\n")
}

/// Where the quoted original starts in the HTML MailCraft writes; what is above it is the
/// owner's (a draft reopens on it).
pub(crate) const QUOTE_MARK: &str = "<div id=\"mailcraft-quote\">";

/// The original under a reply or forward, in HTML: Outlook's header lines (written here, every
/// value escaped) and the original's look, without anything in it that runs, embeds or tracks
/// (`mailcraft_html::quotable`): it goes to other people, whose programs might run what ours never
/// would.
fn quoted_html(s: &Session, m: &Message, html: &str) -> String {
    let e = mailcraft_html::escape;
    let [from, sent, to, cc, subject] = header_names(s).map(e);
    let mut out = String::from(QUOTE_MARK);
    out.push_str("<hr style=\"border:none;border-top:solid #E1E1E1 1.0pt\"><div>");
    out.push_str(&format!("<b>{from}</b> {}<br>", e(&m.from.full())));
    out.push_str(&format!("<b>{sent}</b> {}<br>", e(&sent_on(s, m))));
    if !m.to.is_empty() {
        out.push_str(&format!("<b>{to}</b> {}<br>", e(&quoted_list(&m.to))));
    }
    if !m.cc.is_empty() {
        out.push_str(&format!("<b>{cc}</b> {}<br>", e(&quoted_list(&m.cc))));
    }
    out.push_str(&format!("<b>{subject}</b> {}</div><br>\r\n", e(&m.subject)));
    out.push_str(&mailcraft_html::quotable(html));
    out.push_str("\r\n</div>");
    out
}

/// The original's pictures that its HTML shows in place.
fn shown_pictures(m: &Message, html: &str) -> Vec<Attachment> {
    m.attachments.iter().filter(|a| !a.content_id.is_empty() && html.contains(&format!("cid:{}", a.content_id))).cloned().collect()
}

fn respond(s: &mut Session, v: &Value, mode: ComposeMode) -> CmdResult {
    // A forward carries every file; a reply only quotes the text and the pictures it shows.
    let m = match mode {
        ComposeMode::Forward => source_message(s, v)?,
        _ => reply_source(s, v)?,
    };
    let mine: Vec<String> = s.mailbox.accounts.iter().map(|a| a.email.to_lowercase()).filter(|e| !e.is_empty()).collect();
    let is_mine = |a: &Address| mine.contains(&a.email.to_lowercase());
    let (to, cc) = match mode {
        ComposeMode::Forward => (Vec::new(), Vec::new()),
        _ => {
            // A reply to a message I sent goes to the people I sent it to.
            let mut to: Vec<Address> = if is_mine(&m.from) && !m.to.is_empty() {
                m.to.clone()
            } else if m.reply_to.is_empty() {
                vec![m.from.clone()]
            } else {
                m.reply_to.clone()
            };
            let mut cc = Vec::new();
            if mode == ComposeMode::ReplyAll {
                for a in m.to.iter().filter(|a| !is_mine(a)) {
                    if !to.iter().any(|x| x.same_mailbox(a)) {
                        to.push(a.clone());
                    }
                }
                for a in m.cc.iter().filter(|a| !is_mine(a)) {
                    if !to.iter().chain(&cc).any(|x| x.same_mailbox(a)) {
                        cc.push(a.clone());
                    }
                }
            }
            (to, cc)
        }
    };
    let mut references = m.references.clone();
    if !m.message_id.is_empty() {
        references.push(m.message_id.clone());
    }
    // A long thread's References stay bounded: the root and the latest ones.
    if references.len() > 20 {
        let tail = references.split_off(references.len() - 19);
        references.truncate(1);
        references.extend(tail);
    }
    let quoting = s.prefs.quote_on_reply || mode == ComposeMode::Forward;
    // An original in HTML is carried below with its look; one in plain text is quoted in the body.
    let quote = match &m.body.html {
        Some(html) if quoting => {
            let html = quoted_html(s, &m, html);
            let text = quote_text(s.interface_language.t(ORIGINAL), &html);
            // The pictures the quote still shows: not those of what was left out of it.
            let pictures = if mode == ComposeMode::Forward { Vec::new() } else { shown_pictures(&m, &html) };
            Some(Quote { text, html, pictures })
        }
        _ => None,
    };
    let body = if quoting && quote.is_none() { quoted(s, &m) } else { String::new() };
    let sig = chosen_signature(s, m.account, None, true);
    // A line to write on, a blank one, then the signature above the original.
    let body = if sig.is_some() { format!("\n{}", body.strip_prefix('\n').unwrap_or(&body)) } else { body };
    let mut c = Compose {
        mode,
        source: Some(m.id),
        account: m.account,
        to: format_list(&to),
        cc: format_list(&cc),
        subject: prefixed(if mode == ComposeMode::Forward { "FW" } else { "RE" }, &m.subject),
        body: RichBody::from_plain(&body),
        // A reply is written the way the original was: HTML to HTML, plain text to plain text.
        format: if m.body.html.is_some() { BodyFormat::Html } else { BodyFormat::Plain },
        quote,
        attachments: if mode == ComposeMode::Forward { m.attachments.clone() } else { Vec::new() },
        in_reply_to: if mode == ComposeMode::Forward { String::new() } else { m.message_id.clone() },
        references: if mode == ComposeMode::Forward { Vec::new() } else { references },
        ..Default::default()
    };
    if let Some(sig) = &sig {
        start_with(&mut c, sig, 1)?;
    }
    s.compose = Some(c);
    Ok(json!({"composing": true, "mode": mode}))
}

fn reply(s: &mut Session, v: &Value) -> CmdResult {
    respond(s, v, ComposeMode::Reply)
}

fn reply_all(s: &mut Session, v: &Value) -> CmdResult {
    respond(s, v, ComposeMode::ReplyAll)
}

fn forward(s: &mut Session, v: &Value) -> CmdResult {
    respond(s, v, ComposeMode::Forward)
}

/// Forward a message as an attachment (#315): a new message from its account, with nothing
/// quoted and the message attached whole as an `.eml`, its headers and all, as Save Message As
/// writes it. A forward all the same: its original is marked forwarded once it has gone.
fn forward_as_attachment(s: &mut Session, v: &Value) -> CmdResult {
    let id = one_message(s, v)?;
    let m = s.mailbox.message(id).ok_or_else(|| failed!("no message with id {id}", id))?;
    // What goes is the message itself, never what stands in for it here.
    if m.incomplete {
        return Err(fail("only the headers of this message are here, so it can't be forwarded as an attachment"));
    }
    if m.unfetched {
        return Err(fail("this message isn't downloaded yet: open it once you're online, then forward it as an attachment"));
    }
    // Too large already as it is kept, before any of it is read in: written out it is no smaller.
    let name = super::file::eml_name(m);
    let too_large = || failed!("{name} is larger than {mb} MB", name = name.clone(), mb = MAX_ATTACHMENT >> 20);
    if m.size() > u64::try_from(MAX_ATTACHMENT).unwrap_or(u64::MAX) {
        return Err(too_large());
    }
    let m = s.loaded_message(id)?;
    if m.kept.as_deref().is_some_and(|k| k.failed) {
        return Err(fail("the text of this message can't be read on this computer, so it can't be forwarded as an attachment"));
    }
    if not_downloaded(&m) {
        return Err(fail("this message was too large to download: only its header is here, so it can't be forwarded as an attachment"));
    }
    // What is attached goes to other people: a draft's Bcc, the owner's own marks on the message
    // (read, answered, flagged, categorised), and every header kept with it but those known to
    // come from the sender or the servers on the way, stay here.
    let opts = mailcraft_mime::WriteOptions { include_bcc: false, offset_minutes: s.offset_at(m.date) };
    let unmarked = Message {
        read: false,
        answered: false,
        flag: FlagStatus::None,
        categories: Vec::new(),
        headers: m.headers.iter().filter(|(name, _)| goes_along(name)).cloned().collect(),
        ..Message::clone(&m)
    };
    let bytes = mailcraft_mime::write(&unmarked, &opts);
    if bytes.len() > MAX_ATTACHMENT {
        return Err(too_large());
    }
    let sig = chosen_signature(s, m.account, None, true);
    let mut c = Compose {
        mode: ComposeMode::Forward,
        source: Some(m.id),
        as_attachment: true,
        account: m.account,
        subject: prefixed("FW", &m.subject),
        // A line to write on, a blank one, then the signature.
        body: RichBody::from_plain(if sig.is_some() { "\n" } else { "" }),
        attachments: vec![Attachment::new(name, "message/rfc822", bytes)],
        ..Default::default()
    };
    if let Some(sig) = &sig {
        start_with(&mut c, sig, 1)?;
    }
    s.compose = Some(c);
    Ok(json!({"composing": true, "mode": ComposeMode::Forward}))
}

/// The message Forward as Attachment takes: the one `id` names, else the one selected. A caller
/// that names messages is let past the selection ([`Session::run`]), so anything else named
/// (`ids`, `message`, an `id` that is no id) is refused, never taken to mean the selection.
fn one_message(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let named = |k: &str| v.get(k).is_some_and(|x| !x.is_null());
    if named("ids") || named("message") {
        return Err(bad("name the message to forward as an attachment by `id`: one message at a time"));
    }
    if named("id") {
        return p::req_id(v, "id");
    }
    s.sel.focus().ok_or_else(|| fail("select a message first"))
}

/// Enabled with one message selected: several are not forwarded as attachments at once yet.
fn needs_one_message(s: &Session) -> Option<&'static str> {
    match s.sel.messages.len() {
        0 => Some("select a message first"),
        1 => None,
        _ => Some("select only one message to forward as an attachment"),
    }
}

/// Whether `m` is what is kept of an IMAP message too large to download: its header and, in
/// place of its text, a note that says so, with the mark `mailcraft-net` gives it. A sender can
/// write that mark into a message of their own; only plain text that opens as the note does,
/// with no file, is taken for it.
fn not_downloaded(m: &Message) -> bool {
    m.body.html.is_none()
        && m.attachments.is_empty()
        && m.body.text.starts_with(NOT_DOWNLOADED_NOTE)
        && m.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case(NOT_DOWNLOADED))
}

/// Whether a header kept with a message goes with it when it is forwarded as an attachment. Its
/// own fields (From, To, Cc, Reply-To, Subject, Date, Message-ID, In-Reply-To, References,
/// Importance, the MIME fields) are written from what it is, never a Bcc; of the other headers
/// only those known to come from the sender or the servers on the way go:
/// - the rest of RFC 5322: `Sender`, `Comments`, the trace (`Return-Path`, `Received`) and a
///   resend's `Resent-…` fields, but never `Resent-Bcc`;
/// - the checks of where it came from: `Authentication-Results`, `DKIM-Signature`, `ARC-…` and a
///   spam filter's `X-Spam-…`;
/// - a mailing list's `List-…`.
///
/// Everything else stays here: what a mail program or the owner's server wrote for the owner
/// (folders, labels, flags, every other `X-…`), the headers that name the address it was
/// delivered to (`Delivered-To`, `X-Original-To`, `Envelope-To`, `Received-SPF`; a `Received`
/// line's `for <…>` can name it too, and goes as it is), a request for a receipt
/// (`Disposition-Notification-To`, `Return-Receipt-To`), which the recipient's program could
/// answer to the sender, and the sender's organisation and mail program (`Organization`,
/// `User-Agent`, `X-Mailer`), which aren't needed to tell where it came from.
fn goes_along(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("list-")
        || name.starts_with("x-spam-")
        || matches!(
            name.as_str(),
            "sender"
                | "comments"
                | "return-path"
                | "received"
                | "resent-date"
                | "resent-from"
                | "resent-sender"
                | "resent-to"
                | "resent-cc"
                | "resent-message-id"
                | "authentication-results"
                | "dkim-signature"
                | "arc-seal"
                | "arc-message-signature"
                | "arc-authentication-results"
        )
}

fn edit_draft(s: &mut Session, v: &Value) -> CmdResult {
    let m = source_message(s, v)?;
    if !m.draft {
        return Err(fail("that message isn't a draft"));
    }
    // Only its headers are here: written over, the server's whole draft would be lost.
    if m.unfetched {
        return Err(fail("this draft isn't downloaded yet: open it once you're online, then edit it"));
    }
    // A draft written here with an original quoted below opens as it was: the owner's text in
    // rich text, the original as it was carried. A draft is a file anybody could have written (or
    // an older MailCraft, which carried the original as it came), so what is below the mark is
    // made fit to quote again: for what this program wrote, that changes nothing.
    let (own_html, quote) = match m.body.html.as_deref().and_then(|h| h.find(QUOTE_MARK).map(|at| (h, at))) {
        Some((html, at)) => {
            let end = html.rfind("</body>").filter(|end| *end >= at).unwrap_or(html.len());
            let quoted = mailcraft_html::quotable(html.get(at..end).unwrap_or("")).trim_end().to_string();
            // (Its text part is what the HTML reads as: the text part the draft file carries is
            // only asked which language the opening line is in.)
            let marker = mailcraft_i18n::Lang::ALL
                .iter()
                .map(|l| l.t(ORIGINAL))
                .find(|marker| m.body.text.contains(&format!("\n\n{marker}")))
                .unwrap_or_else(|| s.interface_language.t(ORIGINAL));
            let text = quote_text(marker, &quoted);
            let pictures = shown_pictures(&m, &quoted);
            (Some(html.get(..at).unwrap_or("").to_string()), Some(Quote { text, html: quoted, pictures }))
        }
        None => (m.body.html.clone(), None),
    };
    let attachments: Vec<Attachment> = match &quote {
        Some(q) => m.attachments.iter().filter(|a| !q.pictures.contains(a)).cloned().collect(),
        None => m.attachments.clone(),
    };
    s.compose = Some(Compose {
        mode: ComposeMode::Draft,
        draft: Some(m.id),
        account: m.account,
        to: format_list(&m.to),
        cc: format_list(&m.cc),
        bcc: format_list(&m.bcc),
        subject: m.subject.clone(),
        // A draft written in HTML opens in rich text, reduced to what can be written here.
        body: match &own_html {
            Some(html) => RichBody::new(mailcraft_mime::rich::from_html(html)),
            None => RichBody::from_plain(&m.body.text),
        },
        format: if m.body.html.is_some() { BodyFormat::Html } else { BodyFormat::Plain },
        quote,
        attachments,
        importance: m.importance,
        in_reply_to: m.in_reply_to.clone(),
        references: m.references.clone(),
        ..Default::default()
    });
    Ok(json!({"composing": true, "draft": m.id}))
}

fn set(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::id(v, "account");
    if let Some(a) = account
        && s.mailbox.account(a).is_none()
    {
        return Err(failed!("no account with id {id}", id = a));
    }
    let importance = match p::str(v, "importance") {
        Some(i) => Some(Importance::parse(i).ok_or_else(|| bad("`importance` is low, normal or high"))?),
        None => None,
    };
    // Sent from another account, the message takes that account's signature, unless the owner
    // edited the one in it.
    // ponytail: a message that started with no signature gets none; add one when owners ask.
    let swap = match (account, s.compose.as_ref()) {
        (Some(a), Some(c)) if a != c.account && signature_unedited(c) => Some(chosen_signature(s, a, None, is_response(c.mode))),
        _ => None,
    };
    let c = compose_mut(s)?;
    let before = c.clone();
    if let Some(sig) = swap {
        put_signature(c, sig.as_ref(), 0)?;
    }
    for (key, field) in [("to", &mut c.to), ("cc", &mut c.cc), ("bcc", &mut c.bcc), ("subject", &mut c.subject)] {
        if let Some(text) = p::str(v, key) {
            *field = text.replace(['\r', '\n'], " ");
        }
    }
    // The body as plain text: what didn't change keeps its formatting.
    if let Some(text) = p::str(v, "body") {
        c.body.set_plain(text).map_err(|e| fail(e.to_string()))?;
    }
    if let Some(i) = importance {
        c.importance = i;
    }
    if let Some(a) = account {
        c.account = a;
    }
    if *c != before {
        c.dirty = true;
    }
    done()
}

fn attach(s: &mut Session, v: &Value) -> CmdResult {
    let (name, bytes) = match p::str(v, "data") {
        Some(data) => (p::req_str(v, "name")?.to_string(), base64_decode(data)?),
        None => source(v)?,
    };
    if bytes.len() > MAX_ATTACHMENT {
        return Err(failed!("{name} is larger than {mb} MB", name, mb = MAX_ATTACHMENT >> 20));
    }
    // Only the file's own name travels, never the folders it was in.
    let name = name.rsplit(['/', '\\']).next().unwrap_or("").trim().to_string();
    let name = if name.is_empty() { "attachment".to_string() } else { name };
    let mime = p::str(v, "mime").map(str::to_string).unwrap_or_else(|| mailcraft_mime::media_type_for(&name).to_string());
    let c = compose_mut(s)?;
    if c.attachments.len() >= MAX_ATTACHMENTS {
        return Err(fail("this message already has as many attachments as it can carry"));
    }
    let size = bytes.len();
    c.attachments.push(Attachment::new(name.clone(), mime, bytes));
    c.dirty = true;
    Ok(json!({"name": name, "size": size, "attachments": c.attachments.len()}))
}

fn remove_attachment(s: &mut Session, v: &Value) -> CmdResult {
    let index = p::u64(v, "index").ok_or_else(|| bad("`index` (number) is required"))? as usize;
    let c = compose_mut(s)?;
    if index >= c.attachments.len() {
        return Err(failed!("the message has no attachment {index}", index));
    }
    let removed = c.attachments.remove(index);
    c.dirty = true;
    Ok(json!({"removed": removed.name}))
}

fn signature(s: &mut Session, v: &Value) -> CmdResult {
    let account = s.compose.as_ref().map_or(0, |c| c.account);
    if let Some(name) = p::str(v, "name")
        && s.mailbox.signature(name).is_none()
    {
        return Err(failed!("no signature named \"{name}\"", name));
    }
    let reply = s.compose.as_ref().is_some_and(|c| is_response(c.mode));
    let sig =
        chosen_signature(s, account, p::str(v, "name"), reply).ok_or_else(|| fail("this account has no signature; add one in File › Options"))?;
    let c = compose_mut(s)?;
    // In place of the one there, else where the caret is.
    let at = c.body.selection().head;
    put_signature(c, Some(&sig), at)?;
    c.dirty = true;
    done()
}

/// Replace names typed on the recipient lines with the contact they stand for, when exactly one
/// contact matches. Answers the names nobody matched.
fn check_names(s: &mut Session, _: &Value) -> CmdResult {
    let c = s.compose.clone().ok_or_else(|| fail("no message is being written"))?;
    let mut unresolved = Vec::new();
    let mut resolve = |line: &str| -> String {
        let list: Vec<Address> = Address::parse_list(line)
            .into_iter()
            .map(|a| {
                if a.is_valid() {
                    return a;
                }
                let typed = a.email.clone();
                let hits: Vec<Address> = s.mailbox.contacts.values().filter(|k| k.matches(&typed)).filter_map(|k| k.address()).collect();
                match hits.as_slice() {
                    [one] => one.clone(),
                    _ => {
                        unresolved.push(typed);
                        a
                    }
                }
            })
            .collect();
        format_list(&list)
    };
    let (to, cc, bcc) = (resolve(&c.to), resolve(&c.cc), resolve(&c.bcc));
    let now = compose_mut(s)?;
    if (&to, &cc, &bcc) != (&now.to, &now.cc, &now.bcc) {
        (now.to, now.cc, now.bcc) = (to, cc, bcc);
        now.dirty = true;
    }
    Ok(json!({"unresolved": unresolved}))
}

fn toggle_importance(s: &mut Session, level: Importance) -> CmdResult {
    let c = compose_mut(s)?;
    c.importance = if c.importance == level { Importance::Normal } else { level };
    c.dirty = true;
    Ok(json!({"importance": c.importance}))
}

fn high_importance(s: &mut Session, _: &Value) -> CmdResult {
    toggle_importance(s, Importance::High)
}

fn low_importance(s: &mut Session, _: &Value) -> CmdResult {
    toggle_importance(s, Importance::Low)
}

/// The message as written so far. `strict`: every recipient must be a deliverable address. Under
/// the Message-ID `message_id` when it is given (a draft saved again keeps its own, and so do the
/// Content-IDs of its pictures).
fn build_as(s: &Session, c: &Compose, strict: bool, message_id: Option<&str>) -> Result<Message, CmdError> {
    // Mail written in a shared mailbox is from its address, kept in its Drafts and sent through
    // it (#70): never from its owner's own account instead.
    let account = s.mailbox.account(c.account).or(s.mailbox.default_account()).ok_or_else(|| fail("add an account first (File › Add Account)"))?;
    let parse = |line: &str, what: &'static str| -> Result<Vec<Address>, CmdError> {
        let list = Address::parse_list(line);
        if strict && let Some(bad_one) = list.iter().find(|a| !a.is_valid()) {
            return Err(failed!(
                "\"{address}\" on the {what} line isn't an e-mail address",
                address = bad_one.email,
                what = mailcraft_i18n::Arg::words(what)
            ));
        }
        Ok(list)
    };
    let (to, cc, bcc) = (parse(&c.to, "To")?, parse(&c.cc, "Cc")?, parse(&c.bcc, "Bcc")?);
    if strict && to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return Err(fail("add at least one recipient"));
    }
    let now = s.now();
    let domain = account.email.rsplit_once('@').map_or("", |(_, d)| d);
    let message_id = match message_id {
        Some(id) => id.to_string(),
        None => mailcraft_mime::new_message_id(
            domain,
            now,
            s.rev().wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ c.body.text().len() as u64 ^ (c.subject.len() as u64) << 32,
        ),
    };
    Ok(Message {
        account: account.id,
        in_reply_to: c.in_reply_to.clone(),
        references: c.references.clone(),
        from: account.address(),
        to,
        cc,
        bcc,
        subject: c.subject.trim().to_string(),
        date: now,
        received: now,
        body: body_of(c, &message_id),
        attachments: attachments_of(c, &message_id),
        read: true,
        importance: c.importance,
        message_id,
        ..Default::default()
    })
}

/// What is sent of the owner's text: HTML with a plain-text part, or plain text only.
/// The original quoted below follows it, in both forms.
fn body_of(c: &Compose, message_id: &str) -> Body {
    let text = c.body.text();
    let quote_text = c.quote.as_ref().map_or("", |q| q.text.as_str());
    let quote_html = c.quote.as_ref().map(|q| q.html.as_str());
    let Some((before, sig, after)) = sent_signature(c) else {
        let plain = format!("{}{quote_text}", text.to_plain());
        return match c.format {
            BodyFormat::Html => Body { text: plain, html: Some(mailcraft_mime::rich::to_html_document(text, quote_html)) },
            BodyFormat::Plain => Body { text: plain, html: None },
        };
    };
    // The signature laid out as it was made, its pictures the message's own parts.
    let parts = [before.as_ref().map(RichText::to_plain), Some(sig.plain()), after.as_ref().map(RichText::to_plain)];
    let plain = format!("{}{quote_text}", parts.into_iter().flatten().collect::<Vec<_>>().join("\n"));
    match c.format {
        BodyFormat::Html => {
            let cid = |p: &Picture| signature_cid(p, message_id);
            // An HTML signature goes out as the owner wrote it, cleaned, unless they edited it here.
            let own = c.signature.as_ref().and_then(|i| i.html.as_deref()).filter(|_| signature_unedited(c));
            let html = [
                before.as_ref().map(mailcraft_mime::rich::to_html),
                Some(own.map_or_else(|| mailcraft_mime::rich::signature_html(&sig, &cid), mailcraft_html::quotable)),
                after.as_ref().map(mailcraft_mime::rich::to_html),
            ];
            Body { text: plain, html: Some(mailcraft_mime::rich::document(&html.into_iter().flatten().collect::<String>(), quote_html)) }
        }
        BodyFormat::Plain => Body { text: plain, html: None },
    }
}

/// The text before the signature, the signature as it goes out (only the pictures its text
/// still shows), and the text after it; `None` when the message has no signature.
fn sent_signature(c: &Compose) -> Option<(Option<RichText>, Signature, Option<RichText>)> {
    let inserted = c.signature.as_ref()?;
    let (before, sig, after) = c.body.text().around_signature()?;
    Some((before, Signature::assemble(&inserted.name, sig, &inserted.pictures, inserted.layout), after))
}

/// A signature picture's Content-ID: unique to the message, and naming nothing of the owner.
fn signature_cid(p: &Picture, message_id: &str) -> String {
    let local = message_id.split('@').next().unwrap_or("");
    format!("sig-{}.{local}@mailcraft.invalid", p.id)
}

/// What goes with the message: the attachments, and in HTML the pictures of the signature and of
/// the quoted original.
fn attachments_of(c: &Compose, message_id: &str) -> Vec<Attachment> {
    let mut all = c.attachments.clone();
    if c.format == BodyFormat::Html
        && let Some((_, sig, _)) = sent_signature(c)
    {
        all.extend(sig.pictures().iter().map(|p| Attachment {
            name: p.name.clone(),
            mime: p.mime.clone(),
            content_id: signature_cid(p, message_id),
            inline: true,
            data: p.data.clone(),
            kept: None,
        }));
    }
    if c.format == BodyFormat::Html
        && let Some(q) = &c.quote
    {
        all.extend(q.pictures.iter().map(|p| Attachment { inline: true, ..p.clone() }));
    }
    all
}

/// The original a reply or forward being written answers or forwards.
fn answers_of(s: &Session, c: &Compose) -> Option<Answers> {
    let forward = match c.mode {
        ComposeMode::Forward => true,
        ComposeMode::Reply | ComposeMode::ReplyAll => false,
        _ => return None,
    };
    let original = s.mailbox.message(c.source?)?;
    Some(Answers { message: original.id, message_id: original.message_id.clone(), forward })
}

fn send(s: &mut Session, _: &Value) -> CmdResult {
    let c = s.compose.clone().ok_or_else(|| fail("no message is being written"))?;
    // A draft of a Microsoft account goes as the draft its server has (R3.4.2): the same
    // Message-ID, marker and server id, brought up to date and sent there.
    let saved = c.draft.and_then(|d| s.mailbox.message(d).cloned()).filter(|d| d.server_draft.is_some() && s.mailbox.is_graph(d.account));
    let mut m = build_as(s, &c, true, saved.as_ref().map(|d| d.message_id.as_str()).filter(|id| !id.is_empty()))?;
    if let Some(d) = saved.as_ref().filter(|d| d.account == m.account) {
        (m.server, m.server_draft) = (d.server.clone(), d.server_draft.clone());
        if let Some(sd) = m.server_draft.as_mut() {
            (sd.pending, sd.saves) = (true, sd.saves.saturating_add(1));
        }
    }
    let as_draft = m.server_draft.is_some();
    let account = s.mailbox.account(m.account).cloned().ok_or_else(|| fail("the account is gone"))?;
    // Nothing here sends its mail, and it isn't a local account either: the message stays as it
    // is being written, neither queued nor filed as sent.
    if account.kind == AccountKind::Unknown {
        return Err(super::account::added_by_newer(
            &account,
            "which sends its mail: send this message from another account, or save it as a draft to send it from that version",
        ));
    }
    // An account kept in step with a server that doesn't send its mail yet (JMAP) has nowhere
    // for the message: not its Outbox, where it would wait for ever, nor Sent Items, as if it had
    // gone. It stays as it is being written.
    if account.kind.has_server() && !account.kind.sends_mail() {
        return Err(failed!(
            "{not_sending_yet}: send this message from another account, or save it as a draft",
            not_sending_yet = mailcraft_i18n::Arg::words(NOT_SENDING_YET)
        ));
    }
    // An account that sends its mail queues the message for the next Send/Receive; a local one
    // has nowhere to send it, so it is filed as sent (and delivered to local accounts it names).
    let queued = account.kind.sends_mail();
    let kind = if queued { FolderKind::Outbox } else { FolderKind::Sent };
    m.folder = s
        .mailbox
        .folder_of_kind(account.id, kind)
        .ok_or_else(|| failed!("this account has no {folder} folder", folder = mailcraft_i18n::Arg::folder(kind.default_name())))?;
    // The original is marked replied or forwarded once this has gone (#154): from the Outbox when
    // the server has taken it, at once for a local account, which files it as sent.
    m.answers = answers_of(s, &c).or_else(|| c.draft.and_then(|d| s.mailbox.message(d)).and_then(|d| d.answers.clone()));
    let id = s.mailbox.add_message(m.clone())?;
    if !queued {
        s.mailbox.mark_answered(id);
    }
    if let Some(d) = c.draft {
        // The draft became this message: one the server has goes from there by being sent, not
        // by being deleted.
        let _ = if as_draft { s.mailbox.forget_message(d) } else { s.mailbox.remove_message(d) };
    }
    let mut delivered = 0;
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
            let copy = Message { id: 0, folder: inbox, read: false, bcc: Vec::new(), answers: None, ..m.clone() };
            let cid = s.mailbox.add_message(copy)?;
            s.mailbox.apply_rules(cid);
            delivered += 1;
        }
    }
    s.compose = None;
    Ok(json!({"id": id, "queued": queued, "delivered": delivered}))
}

fn save_draft(s: &mut Session, _: &Value) -> CmdResult {
    let c = s.compose.clone().ok_or_else(|| fail("no message is being written"))?;
    // Whole, so that what the server had of it is worked out from its real content (R6.1); as it
    // is when that can't be read.
    let old = c.draft.and_then(|d| s.loaded_message(d).ok().map(|m| Message::clone(&m)).or_else(|| s.mailbox.message(d).cloned()));
    let mut m = build_as(s, &c, false, old.as_ref().map(|o| o.message_id.as_str()).filter(|id| !id.is_empty()))?;
    m.draft = true;
    // A reply or forward kept as a draft still knows its original, to be marked once it is sent.
    m.answers = answers_of(s, &c).or_else(|| old.as_ref().and_then(|o| o.answers.clone()));
    m.folder = s.mailbox.folder_of_kind(m.account, FolderKind::Drafts).ok_or_else(|| fail("this account has no Drafts folder"))?;
    // Saved again in the same account: what the mailbox keeps of it beside what is written (where
    // its server has it, and its marks) stays, so that the server's copy is changed in place
    // rather than made a second time (R3.4.1).
    if let Some(old) = old.as_ref().filter(|o| o.account == m.account) {
        (m.server, m.server_draft, m.uid, m.synced) = (old.server.clone(), old.server_draft.clone(), old.uid, old.synced);
        (m.read, m.flag, m.flag_due, m.categories, m.pinned) = (old.read, old.flag, old.flag_due, old.categories.clone(), old.pinned);
    }
    let graph = s.mailbox.account(m.account).is_some_and(|a| a.kind == AccountKind::Microsoft);
    if graph {
        // Begun as an answer to, or a forward of, a message its server has: made there as one.
        // Not a forward as an attachment: made as a forward, it would have the source's files
        // beside the attached message, and a meeting request would be forwarded as itself (#315).
        let of =
            c.source.filter(|_| !c.as_attachment).and_then(|src| s.mailbox.message(src)).filter(|src| src.account == m.account).and_then(|src| {
                let id = src.server.as_ref().filter(|server| !server.gone)?.id.clone();
                let kind = match c.mode {
                    ComposeMode::Reply => DraftKind::Reply,
                    ComposeMode::ReplyAll => DraftKind::ReplyAll,
                    ComposeMode::Forward => DraftKind::Forward,
                    _ => return None,
                };
                Some(DraftOf { id, kind })
            });
        let mut sd = m.server_draft.take().unwrap_or_else(|| ServerDraft { of, ..Default::default() });
        // A draft the server had from elsewhere, edited here for the first time: what the server
        // had is what it was before.
        if sd.synced.is_none()
            && let Some(old) = old.as_ref().filter(|o| o.account == m.account && o.server.is_some() && !o.unfetched)
        {
            sd.synced = Some(DraftState::of(old));
        }
        sd.pending = true;
        sd.saves = sd.saves.saturating_add(1);
        m.server_draft = Some(sd);
    }
    let id = match old.as_ref().map(|o| o.id) {
        Some(d) => {
            m.id = d;
            *s.mailbox.message_mut(d)? = m;
            d
        }
        None => s.mailbox.add_message(m)?,
    };
    let now = u64::try_from(s.now()).unwrap_or_default();
    if let Ok(m) = s.mailbox.message_mut(id)
        && let Some(sd) = m.server_draft.as_mut().filter(|sd| sd.marker.is_empty())
    {
        sd.marker = super::graph_drafts::marker(id, now);
    }
    let c = compose_mut(s)?;
    c.draft = Some(id);
    c.dirty = false;
    Ok(json!({"draft": id}))
}

fn close(s: &mut Session, _: &Value) -> CmdResult {
    // (Saved to Drafts first unless `discard`, by `saves_first`; what was last saved stays there.)
    s.compose = None;
    done()
}

fn discard(s: &mut Session, _: &Value) -> CmdResult {
    let draft = s.compose.take().and_then(|c| c.draft);
    // Already gone is fine; one a JMAP account's server has stays (M6), and the answer says so.
    let deleted = draft.is_some_and(|d| s.mailbox.remove_message(d).is_ok() || s.mailbox.message(d).is_none());
    // Why it stays in Drafts, for the window to say: the reason the Move menu greys for it.
    if let Some(d) = draft.filter(|d| !deleted && s.mailbox.stays_on_server(*d)) {
        return Ok(json!({"draftDeleted": false, "draft": d, "stays": mailcraft_model::NOT_MOVING_YET}));
    }
    Ok(json!({"draftDeleted": deleted}))
}

/// Someone a name or address being typed could be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub name: String,
    pub email: String,
    /// From the contacts, rather than from mail that was exchanged.
    pub contact: bool,
}

/// What was typed for a person, as [`suggest`] compares a name and an address with it.
pub(crate) struct Typed {
    q: String,
    wanted: Vec<String>,
}

impl Typed {
    /// `None` for nothing, or for more than 200 characters. With an `@` in it, it is an address
    /// being typed and is not taken apart into words: `ada@` is not Ada Smith at another address.
    pub(crate) fn new(query: &str) -> Option<Typed> {
        let q = query.trim().to_lowercase();
        if q.is_empty() || q.chars().count() > 200 {
            return None;
        }
        let wanted = if q.contains('@') { Vec::new() } else { mailcraft_model::words::words(&q) };
        Some(Typed { wanted, q })
    }

    /// Whether it is the start of the address, of the name, or of words of the name one after
    /// another, compared as the search compares them, ignoring case and accents (`gvoz` is
    /// Gvožđar, #308).
    pub(crate) fn fits(&self, name: &str, email: &str) -> bool {
        email.to_lowercase().starts_with(&self.q)
            || name.to_lowercase().starts_with(&self.q)
            || (!self.wanted.is_empty() && mailcraft_model::words::starts_in(name, &self.wanted))
    }
}

/// Who `query` (the start of a name, of a word in it, or of an address) could be: contacts first,
/// then the people mail was exchanged with, the latest first, matched as [`Typed::fits`] says. The
/// owner's own addresses and addresses that don't look like one are left out. One pass over the
/// mailbox, so it can run on every key press.
pub fn suggest(s: &Session, query: &str, limit: usize) -> Vec<Suggestion> {
    let Some(typed) = Typed::new(query) else { return Vec::new() };
    let limit = limit.clamp(1, 20);
    let fits = |name: &str, email: &str| typed.fits(name, email);
    let own: Vec<String> = s.mailbox.accounts.iter().map(|a| a.email.trim().to_lowercase()).filter(|e| !e.is_empty()).collect();
    let mut seen: std::collections::HashSet<String> = own.into_iter().collect();
    let mut out: Vec<Suggestion> = Vec::new();
    let mut people: Vec<&std::sync::Arc<mailcraft_model::Contact>> = s.mailbox.contacts.values().collect();
    people.sort_by_cached_key(|c| (!c.favorite, c.name().to_lowercase()));
    for c in people {
        let name = c.name();
        for e in &c.emails {
            let email = e.value.trim();
            if mailcraft_model::Address::new("", email).is_valid() && fits(&name, email) && seen.insert(email.to_lowercase()) {
                out.push(Suggestion { name: name.clone(), email: email.to_string(), contact: true });
            }
        }
    }
    if out.len() < limit {
        // Whoever wrote, or was written to: each address once, as it was last seen.
        // A sender writes the date, the From line and the folder rules put mail in Junk E-mail:
        // mail there is not a source, a date after now counts as now, and an address the owner
        // has written to (Sent Items) ranks above one that only wrote.
        let (junk, sent): (Vec<Id>, Vec<Id>) = {
            let ids = |keep: &dyn Fn(&mailcraft_model::Folder) -> bool| s.mailbox.folders.iter().filter(|f| keep(f)).map(|f| f.id).collect();
            (ids(&|f| s.mailbox.in_junk(f.id)), ids(&|f| f.kind == FolderKind::Sent))
        };
        let now = s.now();
        let mut recent: std::collections::HashMap<String, (bool, i64, &mailcraft_model::Address)> = std::collections::HashMap::new();
        // A name and an address are matched once, however many messages carry them.
        let mut matched: std::collections::HashMap<(&str, &str), bool> = std::collections::HashMap::new();
        for m in s.mailbox.messages.values().filter(|m| !junk.contains(&m.folder)) {
            let written = sent.contains(&m.folder);
            let date = m.date.min(now);
            let people: Vec<(&mailcraft_model::Address, bool)> = if written {
                m.to.iter().chain(&m.cc).chain(&m.bcc).map(|a| (a, true)).collect()
            } else {
                std::iter::once(&m.from).chain(&m.to).chain(&m.cc).map(|a| (a, false)).collect()
            };
            for (a, to) in people {
                if !*matched.entry((a.name.as_str(), a.email.as_str())).or_insert_with(|| fits(&a.name, &a.email)) {
                    continue;
                }
                let key = a.email.trim().to_lowercase();
                if !seen.contains(&key) && a.is_valid() {
                    let known = recent.entry(key).or_insert((to, date, a));
                    if (to, date) >= (known.0, known.1) {
                        *known = (to || known.0, date, a);
                    }
                }
            }
        }
        let mut recent: Vec<(bool, i64, &mailcraft_model::Address)> = recent.into_values().collect();
        recent.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)).then_with(|| a.2.email.cmp(&b.2.email)));
        out.extend(recent.into_iter().map(|(_, _, a)| Suggestion {
            name: a.name.trim().to_string(),
            email: a.email.trim().to_string(),
            contact: false,
        }));
    }
    out.truncate(limit);
    out
}

fn suggest_command(s: &mut Session, v: &Value) -> CmdResult {
    let query = p::req_str(v, "query")?;
    let limit = p::u64(v, "limit").unwrap_or(6).clamp(1, 20) as usize;
    let found: Vec<Value> = suggest(s, query, limit).into_iter().map(|x| json!({"name": x.name, "email": x.email, "contact": x.contact})).collect();
    Ok(json!({"suggestions": found}))
}
