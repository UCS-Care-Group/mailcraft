// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writing a message. Everything goes through the engine's compose commands (`Session::run`, so a
//! command that fails leaves the message as it was and the guard still stands): the binding turns the
//! message being written into a typed, bounded view, takes edits as ranges of the text the app shows
//! (counted in UTF-16 units, as Kotlin and Swift count), and maps them to the engine's own positions
//! (characters). The body is the engine's own rich text (bold, italic, underline, links, lists and
//! pictures): the app draws it from the spans below and never builds HTML. Nothing here reads a file by
//! a path: bytes come from the app (the document picker), with a size cap, and a name from outside is
//! made safe by the engine before it is written anywhere.

use mailcraft_engine::opening;
use mailcraft_engine::{BodyFormat, Compose, ComposeMode, Session};
use mailcraft_model::rich::{ListKind, numbers};
use mailcraft_model::{Address, format_list};
use serde_json::{Value, json};

use crate::message::sniff;
use crate::records::{AddressRecord, PictureBytes};
use crate::text::{self, MAX_ADDRESS, MAX_NAME, MAX_PEOPLE_IN_ITEM, MAX_SUBJECT};
use crate::{Core, CoreError};

/// The most characters of body the app edits (more is refused, so that a position is a small number
/// and a view is a bounded size).
pub const MAX_BODY_CHARS: usize = 1_000_000;
/// The most characters one edit brings in (one paste).
const MAX_EDIT_CHARS: usize = 200_000;
/// The most paragraphs, style spans and pictures a view lists.
const MAX_PARAGRAPHS: usize = 20_000;
const MAX_SPANS: usize = 40_000;
const MAX_PICTURES: usize = 200;
/// The most people on one line of recipients, and the most characters of the quoted original shown.
const MAX_RECIPIENTS: usize = 500;
const MAX_QUOTE_CHARS: usize = 8_000;
/// The most bytes of one attached file, and of all of a message's files together, on a phone.
pub const MAX_COMPOSE_FILE: usize = 25 * 1024 * 1024;
pub const MAX_COMPOSE_FILES: usize = 50 * 1024 * 1024;
/// The most bytes of a picture put in the text.
const MAX_COMPOSE_PICTURE: usize = 20 * 1024 * 1024;
/// The most bytes of a `mailto:` link that is read.
const MAX_MAILTO: usize = 8 * 1024;
const MAX_SUGGEST_QUERY: usize = 200;
const MAX_ALT: usize = 500;
/// The most characters of a signature written on a phone.
pub const MAX_SIGNATURE_CHARS: usize = 2_000;

/// What the message being written is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ComposeKind {
    New,
    Reply,
    ReplyAll,
    Forward,
    /// A saved draft, continued.
    Draft,
}

/// How a paragraph is marked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ListMark {
    None,
    Bullet,
    Number,
}

/// A stretch of the body that is not plain text. `start` and `end` count UTF-16 units of `text`.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ComposeSpan {
    pub start: u32,
    pub end: u32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Where the text links to (a web address, `mailto:` or `tel:`), if it does.
    pub link: Option<String>,
}

/// A paragraph of the body: its place in `text` (UTF-16 units, without the line break after it) and how it is marked.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ComposeParagraph {
    pub start: u32,
    pub end: u32,
    pub list: ListMark,
    /// How deep in its list, from 0.
    pub level: u8,
    /// The number a numbered item carries (1 for the first), 0 otherwise.
    pub number: u32,
    /// It is part of the signature that was put in.
    pub signature: bool,
}

/// A picture standing in the text at `at` (UTF-16 units; the text has one object character there).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ComposePicture {
    pub id: u32,
    pub at: u32,
    pub name: String,
    pub alt: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ComposeFile {
    pub index: u32,
    pub name: String,
    pub mime: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecipientRecord {
    pub name: String,
    pub address: String,
    /// Looks deliverable (`local@domain`).
    pub valid: bool,
}

/// The message being written, as the app draws it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ComposeView {
    pub rev: u64,
    /// Its number among the messages being written: what every call about it names.
    pub compose: u64,
    pub kind: ComposeKind,
    /// The message replied to or forwarded (0 when none).
    pub source: u64,
    /// The account it is sent from (a shared mailbox is an account).
    pub account: u64,
    pub to: Vec<RecipientRecord>,
    pub cc: Vec<RecipientRecord>,
    pub bcc: Vec<RecipientRecord>,
    pub subject: String,
    /// HTML (bold, italic, underline, lists, links and pictures can be used) or plain text only.
    pub html: bool,
    /// The body's text: one line break between paragraphs, a picture one object character.
    pub text: String,
    pub spans: Vec<ComposeSpan>,
    pub paragraphs: Vec<ComposeParagraph>,
    pub pictures: Vec<ComposePicture>,
    /// The caret or selection the engine holds, in UTF-16 units of `text`.
    pub selection_start: u32,
    pub selection_end: u32,
    pub files: Vec<ComposeFile>,
    /// The name of the signature that went in; empty when none.
    pub signature: String,
    /// The original of a reply or forward in HTML, below what the owner writes, as text.
    pub quote: String,
    /// Changed since it was opened or saved.
    pub dirty: bool,
}

/// Where a message being written starts.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ComposeStart {
    /// From `account` (0 for the one the list is in), starting from a `mailto:` link when there is one.
    New {
        account: u64,
        mailto: Option<String>,
    },
    Reply {
        id: u64,
    },
    ReplyAll {
        id: u64,
    },
    Forward {
        id: u64,
    },
    /// A saved draft, continued.
    Draft {
        id: u64,
    },
}

/// What can be done to the text. The range it is done to is the one given, in UTF-16 units.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ComposeFormat {
    Bold,
    Italic,
    Underline,
    Bullets,
    Numbering,
    Indent,
    Outdent,
    ClearFormatting,
    /// Make the range a link to `url`; none takes the link away.
    Link {
        url: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RecipientLine {
    To,
    Cc,
    Bcc,
}

/// Somebody the owner may mean by what they typed.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SuggestionRecord {
    pub name: String,
    pub address: String,
    /// A contact (else somebody mail was exchanged with).
    pub contact: bool,
}

/// An account (or shared mailbox) a message can be sent from.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FromChoice {
    pub account: u64,
    pub name: String,
    pub address: String,
    pub shared: bool,
}

/// What a send came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SentRecord {
    /// It waits in the Outbox and goes at the next synchronisation (else it was filed as sent).
    pub queued: bool,
}

fn bad(reason: &str) -> CoreError {
    CoreError::BadRequest { reason: reason.to_string() }
}

fn failed(s: &Session, reason: &str) -> CoreError {
    CoreError::Failed { reason: s.interface_language.t(reason).to_string() }
}

/// Where each character of a text starts in UTF-16 units.
struct Units(Vec<u32>);

impl Units {
    fn of(text: &str) -> Units {
        let mut at = Vec::with_capacity(text.len().min(MAX_BODY_CHARS) + 1);
        let mut unit = 0u32;
        for c in text.chars() {
            at.push(unit);
            unit = unit.saturating_add(u32::try_from(c.len_utf16()).unwrap_or(2));
        }
        at.push(unit);
        Units(at)
    }

    /// UTF-16 units of the whole text.
    fn total(&self) -> u32 {
        self.0.last().copied().unwrap_or(0)
    }

    /// The UTF-16 offset of character `n` (the end for one past the last).
    fn unit(&self, n: usize) -> u32 {
        self.0.get(n).copied().unwrap_or_else(|| self.total())
    }

    /// The character an offset is at or just before (an offset inside a pair belongs to the character it is in).
    fn floor(&self, unit: u32) -> usize {
        self.0.partition_point(|u| *u <= unit).saturating_sub(1).min(self.0.len().saturating_sub(1))
    }

    /// The character an offset is at or just after.
    fn ceil(&self, unit: u32) -> usize {
        self.0.partition_point(|u| *u < unit).min(self.0.len().saturating_sub(1))
    }
}

/// The characters a range of UTF-16 units stands for: a range takes in every character it touches (half
/// an emoji is a whole one), and a caret in the middle of one is just after it.
fn range_of(units: &Units, from: u32, to: u32) -> (usize, usize) {
    let (from, to) = if from <= to { (from, to) } else { (to, from) };
    if from == to {
        let at = units.ceil(from);
        return (at, at);
    }
    (units.floor(from), units.ceil(to))
}

fn people(line: &str) -> Vec<RecipientRecord> {
    text::first(Address::parse_list(line), MAX_RECIPIENTS, |a| RecipientRecord {
        valid: a.is_valid(),
        name: text::clip(&a.name, MAX_NAME),
        address: text::clip(&a.email, MAX_ADDRESS),
    })
}

fn kind_of(mode: ComposeMode) -> ComposeKind {
    match mode {
        ComposeMode::New => ComposeKind::New,
        ComposeMode::Reply => ComposeKind::Reply,
        ComposeMode::ReplyAll => ComposeKind::ReplyAll,
        ComposeMode::Forward => ComposeKind::Forward,
        ComposeMode::Draft => ComposeKind::Draft,
    }
}

fn find(s: &Session, id: u64) -> Result<&Compose, CoreError> {
    let found = if id == 0 { s.compose.as_ref() } else { s.composing_by_id(id) };
    found.ok_or_else(|| failed(s, "no message is being written"))
}

fn view_of(s: &Session, id: u64) -> Result<ComposeView, CoreError> {
    let c = find(s, id)?;
    let body = c.body.text();
    if body.len() > MAX_BODY_CHARS || body.paragraphs().len() > MAX_PARAGRAPHS {
        return Err(failed(s, "this message is too long to be edited on a phone"));
    }
    let text = body.text();
    let units = Units::of(&text);
    let numbered = numbers(body.paragraphs());
    let (mut spans, mut paragraphs, mut pictures) = (Vec::new(), Vec::new(), Vec::new());
    let mut at = 0usize;
    for (n, p) in body.paragraphs().iter().enumerate() {
        let start = units.unit(at);
        for run in &p.runs {
            let len = run.text.chars().count();
            let (from, to) = (units.unit(at), units.unit(at + len));
            let style = &run.style;
            if (style.bold || style.italic || style.underline || style.link.is_some()) && spans.len() < MAX_SPANS {
                spans.push(ComposeSpan {
                    start: from,
                    end: to,
                    bold: style.bold,
                    italic: style.italic,
                    underline: style.underline,
                    link: style.link.as_deref().map(|l| text::clip(l, text::MAX_URL)),
                });
            }
            if let Some(id) = run.picture.filter(|_| pictures.len() < MAX_PICTURES) {
                let shown = c.pictures.iter().find(|p| p.id == id);
                pictures.push(ComposePicture {
                    id,
                    at: from,
                    name: shown.map(|p| text::clip(&p.name, MAX_NAME)).unwrap_or_default(),
                    alt: shown.map(|p| text::clip(&p.alt, MAX_ALT)).unwrap_or_default(),
                    bytes: shown.map_or(0, |p| u64::try_from(p.data.len()).unwrap_or(u64::MAX)),
                });
            }
            at += len;
        }
        let end = units.unit(at);
        let list = match p.list {
            ListKind::None => ListMark::None,
            ListKind::Bullet => ListMark::Bullet,
            ListKind::Numbered => ListMark::Number,
        };
        paragraphs.push(ComposeParagraph {
            start,
            end,
            list,
            level: p.level,
            number: if list == ListMark::Number { numbered.get(n).copied().unwrap_or(0) } else { 0 },
            signature: p.signature,
        });
        // The line break after it.
        at += 1;
    }
    let selection = c.body.selection();
    let (a, b) = (units.unit(selection.anchor), units.unit(selection.head));
    Ok(ComposeView {
        rev: s.rev(),
        compose: c.id,
        kind: kind_of(c.mode),
        source: c.source.unwrap_or(0),
        account: c.account,
        to: people(&c.to),
        cc: people(&c.cc),
        bcc: people(&c.bcc),
        subject: text::clip(&c.subject, MAX_SUBJECT),
        html: c.format == BodyFormat::Html,
        text,
        spans,
        paragraphs,
        pictures,
        selection_start: a.min(b),
        selection_end: a.max(b),
        files: text::first(c.attachments.iter().enumerate(), text::MAX_LISTED, |(i, f)| ComposeFile {
            index: text::count_of(i),
            name: text::clip(&f.name, MAX_NAME),
            mime: text::clip(&f.mime, MAX_NAME),
            size: u64::try_from(f.size()).unwrap_or(u64::MAX),
        }),
        signature: c.signature.as_ref().map(|i| text::clip(&i.name, MAX_NAME)).unwrap_or_default(),
        quote: c.quote.as_ref().map(|q| text::clip(&q.text, MAX_QUOTE_CHARS)).unwrap_or_default(),
        dirty: c.dirty,
    })
}

/// Runs compose command `command` on message `id` with `params` (its `compose` is set here).
fn run(s: &mut Session, id: u64, command: &str, params: Value) -> Result<Value, CoreError> {
    let mut params = params;
    if let Some(map) = params.as_object_mut() {
        map.insert("compose".to_string(), json!(id));
    }
    Ok(s.run(command, &params)?)
}

/// Makes the message `id` the one commands without a name act on, for the commands that name none.
fn current(s: &mut Session, id: u64) -> Result<u64, CoreError> {
    let id = if id == 0 { s.compose_id().ok_or_else(|| failed(s, "no message is being written"))? } else { id };
    find(s, id)?;
    Ok(id)
}

fn replace_range(s: &mut Session, id: u64, from: u32, to: u32, with: &str) -> Result<(), CoreError> {
    let with = with.replace("\r\n", "\n").replace('\r', "\n");
    if with.chars().count() > MAX_EDIT_CHARS {
        return Err(failed(s, "that is too much to put in at once"));
    }
    let (units, current_len, selection) = {
        let c = find(s, id)?;
        (Units::of(&c.body.text().text()), c.body.text().len(), c.body.selection())
    };
    let (a, b) = range_of(&units, from, to);
    if current_len.saturating_sub(b - a).saturating_add(with.chars().count()) > MAX_BODY_CHARS {
        return Err(failed(s, "this message is too long to be edited on a phone"));
    }
    if a < b && with.is_empty() {
        run(s, id, "compose.delete", json!({"from": a, "to": b}))?;
    } else if !with.is_empty() {
        // The caret is put where the edit is only when the engine's is elsewhere: what it holds of the
        // look the next character takes (a bold chosen with nothing selected) stays.
        if (selection.anchor, selection.head) != (a, b) {
            run(s, id, "compose.select", json!({"from": a, "to": b}))?;
        }
        run(s, id, "compose.type", json!({"text": with, "autoList": with == " "}))?;
    }
    Ok(())
}

fn select(s: &mut Session, id: u64, from: u32, to: u32) -> Result<(usize, usize), CoreError> {
    let units = Units::of(&find(s, id)?.body.text().text());
    let (a, b) = range_of(&units, from, to);
    let sel = find(s, id)?.body.selection();
    if (sel.anchor, sel.head) != (a, b) {
        run(s, id, "compose.select", json!({"from": a, "to": b}))?;
    }
    Ok((a, b))
}

fn cap_line(people: &[AddressRecord]) -> String {
    let list: Vec<Address> =
        people.iter().take(MAX_RECIPIENTS).map(|p| Address::new(text::clip(&p.name, MAX_NAME), text::clip(&p.address, MAX_ADDRESS))).collect();
    format_list(&list)
}

#[uniffi::export]
impl Core {
    /// Starts a message (new, a reply, a reply to all, a forward, or a saved draft continued) and
    /// answers it. The engine does the quoting and puts in the account's signature.
    pub async fn compose_start(&self, start: ComposeStart) -> Result<ComposeView, CoreError> {
        self.call(move |s| {
            let answered = match start {
                ComposeStart::New { account, mailto } => {
                    let mut params = serde_json::Map::new();
                    if account != 0 {
                        params.insert("account".to_string(), json!(account));
                    }
                    if let Some(link) = mailto {
                        if link.len() > MAX_MAILTO {
                            return Err(bad("the link is too long"));
                        }
                        params.insert("mailto".to_string(), json!(link));
                    }
                    s.run("mail.new", &Value::Object(params))?
                }
                ComposeStart::Reply { id } => s.run("mail.reply", &json!({"id": id}))?,
                ComposeStart::ReplyAll { id } => s.run("mail.replyAll", &json!({"id": id}))?,
                ComposeStart::Forward { id } => s.run("mail.forward", &json!({"id": id}))?,
                ComposeStart::Draft { id } => s.run("mail.editDraft", &json!({"id": id}))?,
            };
            let id = answered.get("compose").and_then(Value::as_u64).or_else(|| s.compose_id()).ok_or(CoreError::Internal)?;
            view_of(s, id)
        })
        .await
    }

    /// The message being written `compose` (0: the current one).
    pub async fn compose_view(&self, compose: u64) -> Result<ComposeView, CoreError> {
        self.call(move |s| view_of(s, compose)).await
    }

    /// Replaces `from..to` of the body's text (UTF-16 units) by `text`: typing, deleting and pasting.
    pub async fn compose_edit(&self, compose: u64, from: u32, to: u32, text: String) -> Result<ComposeView, CoreError> {
        self.call(move |s| {
            let id = current(s, compose)?;
            replace_range(s, id, from, to, &text)?;
            view_of(s, id)
        })
        .await
    }

    /// Puts the caret (or the selection) of the message at `from..to`, in UTF-16 units.
    pub async fn compose_select(&self, compose: u64, from: u32, to: u32) -> Result<ComposeView, CoreError> {
        self.call(move |s| {
            let id = current(s, compose)?;
            select(s, id, from, to)?;
            view_of(s, id)
        })
        .await
    }

    /// Formats `from..to` of the body (UTF-16 units). A message in plain text has no formatting.
    pub async fn compose_format(&self, compose: u64, format: ComposeFormat, from: u32, to: u32) -> Result<ComposeView, CoreError> {
        self.call(move |s| {
            let id = current(s, compose)?;
            select(s, id, from, to)?;
            let (command, params) = match format {
                ComposeFormat::Bold => ("format.bold", json!({})),
                ComposeFormat::Italic => ("format.italic", json!({})),
                ComposeFormat::Underline => ("format.underline", json!({})),
                ComposeFormat::Bullets => ("format.bullets", json!({})),
                ComposeFormat::Numbering => ("format.numbering", json!({})),
                ComposeFormat::Indent => ("format.indent", json!({})),
                ComposeFormat::Outdent => ("format.outdent", json!({})),
                ComposeFormat::ClearFormatting => ("format.clear", json!({})),
                ComposeFormat::Link { url } => match url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
                    Some(u) if u.len() > text::MAX_URL => return Err(bad("the link is too long")),
                    Some(u) => ("compose.link", json!({"url": u})),
                    None => ("compose.link", json!({})),
                },
            };
            run(s, id, command, params)?;
            view_of(s, id)
        })
        .await
    }

    /// Sets the people on a line of recipients (the whole line), the subject, or the account it is sent from.
    /// An account the owner has not got is refused.
    pub async fn compose_recipients(&self, compose: u64, line: RecipientLine, people: Vec<AddressRecord>) -> Result<ComposeView, CoreError> {
        self.call(move |s| {
            let id = current(s, compose)?;
            let key = match line {
                RecipientLine::To => "to",
                RecipientLine::Cc => "cc",
                RecipientLine::Bcc => "bcc",
            };
            run(s, id, "compose.set", json!({ key: cap_line(&people) }))?;
            view_of(s, id)
        })
        .await
    }

    pub async fn compose_subject(&self, compose: u64, subject: String) -> Result<ComposeView, CoreError> {
        self.call(move |s| {
            let id = current(s, compose)?;
            run(s, id, "compose.set", json!({"subject": text::clip(&subject, MAX_SUBJECT)}))?;
            view_of(s, id)
        })
        .await
    }

    /// Sends the message from `account`: the engine puts that account's signature in when the owner has not edited the one there.
    pub async fn compose_from(&self, compose: u64, account: u64) -> Result<ComposeView, CoreError> {
        self.call(move |s| {
            let id = current(s, compose)?;
            run(s, id, "compose.set", json!({"account": account}))?;
            view_of(s, id)
        })
        .await
    }

    /// Attaches a file whose bytes the app read (the document picker). The name is made safe, the kind is
    /// told from the bytes (a picture is one by its own bytes, and a name that says picture over bytes that are
    /// not is no picture), and a file or a total over the caps is refused.
    pub async fn compose_attach(&self, compose: u64, name: String, bytes: Vec<u8>) -> Result<ComposeView, CoreError> {
        self.call(move |s| {
            let id = current(s, compose)?;
            if bytes.len() > MAX_COMPOSE_FILE {
                return Err(failed(s, "that file is too large to attach on a phone"));
            }
            let kept: usize = find(s, id)?.attachments.iter().map(mailcraft_model::Attachment::size).sum();
            if kept.saturating_add(bytes.len()) > MAX_COMPOSE_FILES {
                return Err(failed(s, "the files on this message are as large together as they can be"));
            }
            let safe = opening::file_name(&text::clip(&name, 1_024));
            let mime = match sniff(&bytes) {
                Some(picture) => picture,
                None => match mailcraft_mime::media_type_for(&safe) {
                    claimed if claimed.starts_with("image/") => "application/octet-stream",
                    other => other,
                },
            };
            run(s, id, "compose.attach", json!({"name": safe, "data": mailcraft_engine::cmd::base64_encode(&bytes), "mime": mime}))?;
            view_of(s, id)
        })
        .await
    }

    pub async fn compose_remove_file(&self, compose: u64, index: u32) -> Result<ComposeView, CoreError> {
        self.call(move |s| {
            let id = current(s, compose)?;
            run(s, id, "compose.removeAttachment", json!({"index": index}))?;
            view_of(s, id)
        })
        .await
    }

    /// Puts a picture (a PNG, JPEG, GIF or WebP, by its own bytes) in the text at `at` (UTF-16 units).
    pub async fn compose_picture(&self, compose: u64, bytes: Vec<u8>, alt: String, at: u32) -> Result<ComposeView, CoreError> {
        self.call(move |s| {
            let id = current(s, compose)?;
            if bytes.len() > MAX_COMPOSE_PICTURE {
                return Err(failed(s, "that picture is too large"));
            }
            let mime = sniff(&bytes).ok_or_else(|| failed(s, "that is not a picture that can be put in a message"))?;
            let at = Units::of(&find(s, id)?.body.text().text()).floor(at);
            let file = match mime {
                "image/png" => "picture.png",
                "image/jpeg" => "picture.jpg",
                "image/gif" => "picture.gif",
                _ => "picture.webp",
            };
            run(
                s,
                id,
                "insert.picture",
                json!({"data": mailcraft_engine::cmd::base64_encode(&bytes), "file": file, "alt": text::clip(&alt, MAX_ALT), "at": at}),
            )?;
            view_of(s, id)
        })
        .await
    }

    /// The bytes of a picture of the message (the ids `ComposeView.pictures` names).
    pub async fn compose_picture_bytes(&self, compose: u64, picture: u32) -> Result<PictureBytes, CoreError> {
        self.call(move |s| {
            let c = find(s, compose)?;
            let p = c.pictures.iter().find(|p| p.id == picture).ok_or_else(|| bad("the message has no such picture"))?;
            Ok(PictureBytes { rev: s.rev(), mime: text::clip(&p.mime, MAX_NAME), bytes: p.data.to_vec() })
        })
        .await
    }

    /// Who the start of a name or an address could be: contacts first, then people mail went to or came from.
    pub async fn suggest_recipients(&self, query: String, limit: u32) -> Result<Vec<SuggestionRecord>, CoreError> {
        let query = text::clip(&query, MAX_SUGGEST_QUERY);
        self.call(move |s| {
            let limit = limit.clamp(1, 20);
            let found = s.run("compose.suggest", &json!({"query": query, "limit": limit}))?;
            let list = found.get("suggestions").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            Ok(text::first(list, 20, |x| SuggestionRecord {
                name: text::clip(x.get("name").and_then(Value::as_str).unwrap_or_default(), MAX_NAME),
                address: text::clip(x.get("email").and_then(Value::as_str).unwrap_or_default(), MAX_ADDRESS),
                contact: x.get("contact").and_then(Value::as_bool).unwrap_or(false),
            }))
        })
        .await
    }

    /// The accounts and shared mailboxes a message can be sent from.
    pub async fn compose_accounts(&self) -> Result<Vec<FromChoice>, CoreError> {
        self.call(|s| {
            Ok(text::first(
                s.mailbox.accounts.iter().filter(|a| a.kind == mailcraft_model::AccountKind::Local || a.kind.sends_mail()),
                MAX_PEOPLE_IN_ITEM,
                |a| FromChoice {
                    account: a.id,
                    name: text::clip(if a.display_name.is_empty() { &a.name } else { &a.display_name }, MAX_NAME),
                    address: text::clip(&a.email, MAX_ADDRESS),
                    shared: a.shared_of.is_some(),
                },
            ))
        })
        .await
    }

    /// Keeps the message in Drafts as the timer does (no step to undo): the five-minute save of the desktop, as often as the app asks.
    pub async fn compose_save(&self, compose: u64) -> Result<(), CoreError> {
        self.call(move |s| {
            let id = current(s, compose)?;
            s.save_in_background(id)?;
            Ok(())
        })
        .await
    }

    /// Puts the message in the Outbox for the next synchronisation (`queued`), or files it as sent for an account that sends nothing.
    pub async fn compose_send(&self, compose: u64) -> Result<SentRecord, CoreError> {
        self.call(move |s| {
            let id = current(s, compose)?;
            let done = run(s, id, "compose.send", json!({}))?;
            Ok(SentRecord { queued: done.get("queued").and_then(Value::as_bool).unwrap_or(false) })
        })
        .await
    }

    /// Closes the message: saved to Drafts first when it has changes, unless `discard`.
    pub async fn compose_close(&self, compose: u64, discard: bool) -> Result<(), CoreError> {
        self.call(move |s| {
            let id = current(s, compose)?;
            run(s, id, "compose.close", json!({"discard": discard.then_some(true)}))?;
            Ok(())
        })
        .await
    }

    /// The text of the signature new messages from `account` start with (empty when it has none).
    pub async fn account_signature(&self, account: u64) -> Result<String, CoreError> {
        self.call(move |s| {
            let a = s.mailbox.account(account).ok_or_else(|| failed(s, "no such account"))?;
            let text = s.mailbox.signature(&a.signature).map(mailcraft_model::Signature::plain).unwrap_or_default();
            Ok(text::clip(&text, MAX_SIGNATURE_CHARS))
        })
        .await
    }

    /// Makes `text` the signature of `account`, for new messages and for replies and forwards; an empty text
    /// is none. A signature of the owner's with the same name is replaced, so the desktop's own are never changed.
    pub async fn set_account_signature(&self, account: u64, text: String) -> Result<(), CoreError> {
        self.call(move |s| {
            let email = s.mailbox.account(account).ok_or_else(|| failed(s, "no such account"))?.email.clone();
            let text = text::clip(text.replace("\r\n", "\n").trim_end(), MAX_SIGNATURE_CHARS);
            if text.trim().is_empty() {
                s.run("signature.defaults", &json!({"account": account, "new": "", "reply": ""}))?;
            } else {
                let name = format!("Phone: {}", text::clip(&email, MAX_ADDRESS));
                s.run("signature.set", &json!({"name": name, "text": text, "default": true, "account": account}))?;
            }
            Ok(())
        })
        .await
    }
}
