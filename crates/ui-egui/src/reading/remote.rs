// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pictures from the web: whether a message may show them, asking the host for them, and what
//! the owner is told and can do about it.
//!
//! A picture on the web tells whoever serves it that the message was opened, when, and from
//! what address. So none is asked for unless the owner said so, in one of four ways: "Download
//! pictures" for the message at hand (it holds until the program closes), "Always download
//! pictures from" its sender or from everybody at its sender's domain (remembered in the
//! settings), or the switch in File › Options that downloads them for all mail (off unless the
//! owner turns it on). The last three do not hold for what is in Junk E-mail (or under it). A
//! remembered sender or domain holds only for a message the receiving server vouches for
//! ([`vouched`]): a `From:` line is anybody's to write.
//!
//! This crate opens no connections and keeps no files. It hands an address to the host
//! ([`Services::pictures`]) and takes back bytes or a refusal; the host is the one that checks
//! where an address leads before it goes there, and the one that may keep what it brought on
//! disk so as not to ask the same server again at the next start ([`Services::picture_cache`]).
//! Where there is no such host (the browser, the tests) nothing is offered.
//!
//! What is asked of the host is bounded here: a few pictures at a time, so many for one message
//! and so many bytes, each address once, and nothing for a message that is not on screen. A
//! picture that failed is not tried again unless the owner says "Try again"; why it failed is
//! kept with it and shown where it would have been. The bytes that come are kept in memory with
//! the other pictures ([`Pictures`]) and are looked at as warily as an attachment's, whether the
//! host downloaded them just now or kept them from before.
//!
//! [`Services::pictures`]: crate::Services::pictures
//! [`Services::picture_cache`]: crate::Services::picture_cache

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Color32, Sense, Stroke, Ui, vec2};
use mailcraft_model::{Account, AccountKind, Id, Message, provider};
use serde_json::{Value, json};

use super::Place;
use super::images::{self, Pictures, Remote};
use super::view::Doc;
use crate::theme::{Tokens, regular, semibold};
use crate::{MailApp, UiState, icons};

/// Most pictures asked for on behalf of one message while the program runs; the rest stay
/// placeholders.
pub const MAX_PER_MESSAGE: usize = 60;
/// Most bytes taken for one message's pictures; once that many have come, no more are asked for.
pub const MAX_MESSAGE_BYTES: usize = 24 * 1024 * 1024;
/// Largest picture taken from the host, in bytes (the host has the same limit of its own).
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
/// How many pictures are on their way at a time.
pub const AT_A_TIME: usize = 4;
/// How long the host is waited on for one picture, in milliseconds (it gives up sooner by itself).
const PATIENCE_MS: f64 = 60_000.0;
/// Most senders whose pictures are always downloaded, and most domains.
pub const MAX_SENDERS: usize = 500;
pub const MAX_DOMAINS: usize = 500;
/// Longest address of a sender that is remembered, and the longest domain.
const MAX_SENDER_LEN: usize = 320;
const MAX_DOMAIN_LEN: usize = 253;
/// How many of a message's headers are looked through for the server's verdict, and how many
/// words of it.
const MAX_HEADERS_READ: usize = 2_000;
const MAX_TOKENS: usize = 200;
const MAX_HEADER_CHARS: usize = 4_000;
/// Most messages the owner asked pictures for that are kept track of.
const MAX_ASKED: usize = 500;
/// Most messages whose share of the downloading is kept track of.
const MAX_SPENT: usize = 200;
/// Most addresses waiting for a turn.
const MAX_WAITING: usize = 4 * MAX_PER_MESSAGE;
/// Most messages waiting to have their failed pictures tried again.
const MAX_RETRIES: usize = 32;

/// Whether the pictures a message refers to on the web may be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// This build has no way to download anything.
    Unavailable,
    /// It could, and the owner has not asked.
    Blocked,
    /// The owner asked: for this message, for its sender or its sender's domain, or for all mail.
    Allowed,
}

/// A message, told from any other that might come to have its number (another mailbox opened).
type Mark = (Id, i64, u64);

fn mark(m: &Message) -> Mark {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (&m.message_id, &m.from.email, &m.subject).hash(&mut hasher);
    (m.id, m.date, hasher.finish())
}

/// A picture the host is downloading.
struct Flight {
    message: Id,
    url: Arc<str>,
    answer: Receiver<Result<Vec<u8>, String>>,
    /// When it was asked for (milliseconds).
    since: f64,
}

/// What downloading for one message has come to.
#[derive(Clone, Copy, Debug, Default)]
struct Spent {
    asked: usize,
    bytes: usize,
}

impl Spent {
    fn out(self) -> bool {
        self.asked >= MAX_PER_MESSAGE || self.bytes >= MAX_MESSAGE_BYTES
    }
}

/// How a message's pictures are coming along.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// How many pictures on the web the message may ask for.
    pub total: usize,
    /// Downloaded, and pictures.
    pub ready: usize,
    /// Asked for or about to be.
    pub waiting: usize,
    pub failed: usize,
    /// Not asked for: the message has more pictures, or heavier ones, than are downloaded for one.
    pub left_out: usize,
}

/// The state of downloading pictures, for as long as the program runs.
#[derive(Default)]
pub struct RemotePictures {
    /// Messages the owner said "Download pictures" for.
    asked: VecDeque<Mark>,
    /// Addresses that messages on screen are waiting for. Gathered while they are laid out and
    /// taken up at the next frame: what is no longer on screen is no longer asked for.
    waiting: Vec<(Id, Arc<str>)>,
    flying: Vec<Flight>,
    spent: VecDeque<(Id, Spent)>,
    /// The frame arrivals were last looked for in.
    ticked: Option<u64>,
    /// Messages whose failed pictures the owner wants tried again: seen to when they are next
    /// laid out.
    retry: Vec<Id>,
    /// The owner cleared what is kept of pictures: what is in memory goes at the next frame.
    forget_all: bool,
}

impl RemotePictures {
    fn spent(&self, message: Id) -> Spent {
        self.spent.iter().find(|(id, _)| *id == message).map(|(_, spent)| *spent).unwrap_or_default()
    }

    fn spend(&mut self, message: Id, asked: usize, bytes: usize) {
        match self.spent.iter_mut().find(|(id, _)| *id == message) {
            Some((_, spent)) => {
                spent.asked = spent.asked.saturating_add(asked);
                spent.bytes = spent.bytes.saturating_add(bytes);
            }
            None => {
                if self.spent.len() >= MAX_SPENT {
                    self.spent.pop_front();
                }
                self.spent.push_back((message, Spent { asked, bytes }));
            }
        }
    }

    /// Give `message` back what `asked` of its pictures cost: they are to be asked for again.
    fn refund(&mut self, message: Id, asked: usize) {
        if let Some((_, spent)) = self.spent.iter_mut().find(|(id, _)| *id == message) {
            spent.asked = spent.asked.saturating_sub(asked);
        }
    }

    /// How many pictures are on their way.
    pub fn flying(&self) -> usize {
        self.flying.len()
    }

    /// Pretend the pictures on their way were asked for `ms` milliseconds earlier.
    #[cfg(test)]
    pub fn age(&mut self, ms: f64) {
        for flight in &mut self.flying {
            flight.since -= ms;
        }
    }
}

/// A sender's address as it is remembered; `None` for what is no address.
fn sender(text: &str) -> Option<String> {
    let text = text.trim();
    // ASCII only, folded as ASCII: Unicode lower-casing maps some letters (the Kelvin sign) onto
    // ASCII ones, which would let a lookalike address stand for a remembered one.
    let plain = text.is_ascii() && text.len() <= MAX_SENDER_LEN && text.contains('@') && !text.chars().any(|c| c.is_whitespace() || c.is_control());
    (plain && text.len() >= 3).then(|| text.to_ascii_lowercase())
}

/// A domain as it is remembered: what follows the `@` of an address (or the whole of `text`,
/// with or without an `@` in front), in ASCII lower case; `None` for what is no domain (a name
/// with a letter outside ASCII is written the way it goes over the wire, `xn--…`, to be one).
fn domain(text: &str) -> Option<String> {
    let text = text.trim();
    let name = text.rsplit_once('@').map_or(text, |(_, name)| name).trim_end_matches('.');
    let plain = !name.is_empty()
        && name.len() <= MAX_DOMAIN_LEN
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
        && !name.starts_with(['.', '-'])
        && !name.contains("..");
    plain.then(|| name.to_ascii_lowercase())
}

/// Whether pictures from `address` are always downloaded.
pub fn sender_remembered(ui: &UiState, address: &str) -> bool {
    sender(address).is_some_and(|address| ui.picture_senders.iter().take(MAX_SENDERS).any(|kept| kept.trim().eq_ignore_ascii_case(&address)))
}

/// Whether pictures from everybody at the domain of `address` are always downloaded: that
/// domain itself, not the ones under it or above it.
pub fn domain_remembered(ui: &UiState, address: &str) -> bool {
    let Some(of_sender) = sender(address).and_then(|address| domain(&address)) else { return false };
    ui.picture_domains.iter().take(MAX_DOMAINS).any(|kept| domain(kept).is_some_and(|kept| kept == of_sender))
}

/// Whether `m` is in an account's Junk E-mail folder or under it.
fn in_junk(app: &MailApp, m: &Message) -> bool {
    app.session.mailbox.in_junk(m.folder)
}

/// The authserv-id (RFC 8601) an `Authentication-Results` value starts with: the first word before
/// the first `;`, after comments, ignoring an optional version number. `None` when the value
/// starts with a result instead (Exchange Online writes no id), or the id is no host name.
fn authserv_id(value: &str) -> Option<String> {
    let mut plain = String::new();
    let mut depth = 0usize;
    for c in value.chars().take(MAX_HEADER_CHARS) {
        match c {
            '(' => depth = depth.saturating_add(1),
            ')' => depth = depth.saturating_sub(1),
            ';' if depth == 0 => break,
            c if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let first = plain.split_whitespace().next()?;
    if first.contains('=') { None } else { domain(first) }
}

/// Whether the topmost `Authentication-Results` header of `m` was written by `account`'s own
/// server: any header for a Microsoft account (Exchange Online puts its own on top, with no id),
/// and for another account one whose authserv-id is a known id of its IMAP host's provider
/// (`provider::authserv_ids`), or the IMAP host's domain one label up, or a name under that.
/// Anything else is not known to be the account's server's.
fn from_own_server(account: Option<&Account>, value: &str) -> bool {
    let Some(account) = account else { return false };
    let host = account.incoming.as_ref().map(|server| server.host.trim().trim_end_matches('.').to_ascii_lowercase());
    if account.kind == AccountKind::Microsoft || host.as_deref().is_some_and(provider::is_microsoft_imap_host) {
        return true;
    }
    let (Some(host), Some(id)) = (host, authserv_id(value)) else { return false };
    if provider::authserv_ids(&host).contains(&id.as_str()) {
        return true;
    }
    let Some((_, rest)) = host.split_once('.') else { return false };
    rest.split('.').count() >= 2 && !provider::is_public_suffix(rest) && (id == rest || id.strip_suffix(rest).is_some_and(|head| head.ends_with('.')))
}

/// The clauses of an `Authentication-Results` value (RFC 8601), each as its words: split at the
/// `;` that is neither in a comment (nested, with `\` quoting) nor in a quoted string, a comment
/// being a gap between words and a quoted string staying whole inside its word. What a sender
/// can put into a value (a `reason="…"`, a `(comment)`) therefore stays inside its own word. A
/// last clause cut off at [`MAX_HEADER_CHARS`], or left inside a quoted string or a comment, is
/// dropped: its last word may be the start of a longer one (`header.d=shop.example` of
/// `shop.example.evil.example`). With the clauses comes whether the whole value was read: not
/// when it was cut off, left open, or longer than [`MAX_TOKENS`] clauses.
fn clauses(value: &str) -> (Vec<Vec<String>>, bool) {
    let (mut out, mut words, mut word) = (Vec::new(), Vec::new(), String::new());
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    let mut chars = value.chars();
    for c in chars.by_ref().take(MAX_HEADER_CHARS) {
        if escaped {
            escaped = false;
            if quoted {
                word.push(c);
            }
            continue;
        }
        match c {
            '\\' if depth > 0 || quoted => escaped = true,
            _ if depth > 0 => match c {
                '(' => depth = depth.saturating_add(1),
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            },
            '"' => {
                quoted = !quoted;
                word.push(c);
            }
            _ if quoted => word.push(c),
            '(' => {
                depth = 1;
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            ';' | ' ' | '\t' | '\r' | '\n' => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                if c == ';' {
                    out.push(std::mem::take(&mut words));
                    if out.len() >= MAX_TOKENS {
                        return (out, false);
                    }
                }
            }
            _ => word.push(c),
        }
    }
    if chars.next().is_some() || quoted || depth > 0 {
        return (out, false);
    }
    if !word.is_empty() {
        words.push(word);
    }
    out.push(words);
    (out, true)
}

/// Whether the server that received `m` vouches that it came from the domain it says it did:
/// `m` is on the server (it has a UID there, or the server's own record of it: mail brought in
/// from a file has the file's word and nothing else); the topmost `Authentication-Results`
/// header (the receiving server puts its own on top; the ones below were written before it),
/// written by the server of the account `m` is in ([`from_own_server`]), has a clause whose
/// result is `dmarc=pass` (or Exchange Online's `compauth=pass`) and that clause names the
/// `header.from` it judged, which is the sender's domain or one above it ([`judged_for`]). When
/// the server gave no DMARC verdict ([`dmarc_verdict`]) in that header or in another with the
/// same authserv-id (a DMARC filter may write its own beside a DKIM filter's), a clause whose
/// result is `dkim=pass` counts instead, by the signer's `header.d` (or, when the clause names
/// none, the domain of its `header.i`, which the verifier checks is `header.d` or under it: RFC
/// 6376, 6.1.1) under the same rule: a signature of the sender's domain that passes is what makes
/// DMARC pass (RFC 7489, 3.1.1 and 4.2). A verdict the server gave, `fail` included, is never
/// overruled by a signature. Only the clause's own words are read, not what a comment or quoted
/// string in the header says. A header the sender could have written is not read.
pub fn vouched(account: Option<&Account>, m: &Message) -> bool {
    if m.uid == 0 && m.server.is_none() {
        return false;
    }
    let mut results = m.headers.iter().take(MAX_HEADERS_READ).filter(|(name, _)| name.eq_ignore_ascii_case("Authentication-Results"));
    let Some((_, value)) = results.next() else {
        return false;
    };
    if !from_own_server(account, value) {
        return false;
    }
    let Some(sender_domain) = sender(&m.from.email).and_then(|address| domain(&address)) else { return false };
    let id = authserv_id(value);
    // A header below with the server's id may be forged, but it can only take the signature away.
    let verdict_given = dmarc_verdict(value) || results.any(|(_, other)| authserv_id(other) == id && dmarc_verdict(other));
    let (parsed, _) = clauses(value);
    parsed.iter().any(|words| {
        let Some(result) = words.first() else { return false };
        let names = |wanted: &str| words.iter().any(|word| word.split_once('=').is_some_and(|(key, _)| key.eq_ignore_ascii_case(wanted)));
        let property = if result.eq_ignore_ascii_case("dmarc=pass") || result.eq_ignore_ascii_case("compauth=pass") {
            "header.from"
        } else if !verdict_given && result.eq_ignore_ascii_case("dkim=pass") {
            if names("header.d") { "header.d" } else { "header.i" }
        } else {
            return false;
        };
        words.iter().skip(1).any(|word| {
            word.split_once('=').is_some_and(|(key, name)| {
                key.eq_ignore_ascii_case(property) && domain(name.trim_matches('"')).is_some_and(|judged| judged_for(&judged, &sender_domain))
            })
        })
    })
}

/// Whether the `Authentication-Results` value `value` gives a DMARC verdict: a `dmarc` (or
/// `compauth`) clause of any result, or a part not read ([`clauses`]) where one may stand.
fn dmarc_verdict(value: &str) -> bool {
    let (parsed, whole) = clauses(value);
    !whole
        || parsed.iter().any(|words| {
            words.first().is_some_and(|result| {
                let method = result.split(['/', '=']).next().unwrap_or_default();
                method.eq_ignore_ascii_case("dmarc") || method.eq_ignore_ascii_case("compauth")
            })
        })
}

/// Whether a verdict on the domain `judged` stands for mail from `sender_domain`: the same name,
/// or a name under it, as Gmail names the organisation's domain (`shop.example` for mail from
/// `news.shop.example`) and as an organisation signs for its names; never a bare suffix such as
/// `co.uk`, which is nobody's organisation.
fn judged_for(judged: &str, sender_domain: &str) -> bool {
    judged == sender_domain
        || (judged.contains('.') && !provider::is_public_suffix(judged) && sender_domain.strip_suffix(judged).is_some_and(|head| head.ends_with('.')))
}

/// Whether a remembered sender or domain stands for `m`'s sender, and the server vouches for it.
fn remembered_and_vouched(app: &MailApp, m: &Message) -> bool {
    (sender_remembered(&app.ui, &m.from.email) || domain_remembered(&app.ui, &m.from.email)) && vouched(app.session.mailbox.account(m.account), m)
}

/// A remembered sender or domain stands for `m`'s sender but nothing vouches for the address.
fn remembered_unvouched(app: &MailApp, m: &Message) -> bool {
    !in_junk(app, m)
        && !app.ui.download_pictures
        && (sender_remembered(&app.ui, &m.from.email) || domain_remembered(&app.ui, &m.from.email))
        && !vouched(app.session.mailbox.account(m.account), m)
}

/// Whether this build can download pictures at all.
pub fn available(app: &MailApp) -> bool {
    app.services.pictures.is_some()
}

/// Whether `m` may show the pictures it refers to on the web.
pub fn access(app: &MailApp, m: &Message) -> Access {
    if !available(app) {
        return Access::Unavailable;
    }
    // What lands in Junk E-mail is trusted by nobody's say-so but the owner's, message by message.
    let trusted = !in_junk(app, m) && (app.ui.download_pictures || remembered_and_vouched(app, m));
    if trusted || app.remote_pictures.asked.contains(&mark(m)) { Access::Allowed } else { Access::Blocked }
}

/// Take in the pictures the host has brought, and hand it the next ones that messages on
/// screen wait for. Once a frame, wherever it is called from.
pub fn tick(app: &mut MailApp, ctx: &egui::Context) {
    let frame = images::frame(ctx);
    let state = &mut app.remote_pictures;
    if state.ticked == Some(frame) {
        return;
    }
    state.ticked = Some(frame);
    if std::mem::take(&mut state.forget_all) {
        // What is on its way was asked for before: it is not waited for, and not kept.
        state.flying.clear();
        state.waiting.clear();
        state.spent.clear();
        Pictures::of(ctx).lock().unwrap_or_else(|e| e.into_inner()).forget_remote();
        ctx.request_repaint();
    }
    let now = crate::now_ms();
    let mut landed: Vec<(Id, Arc<str>, Result<Vec<u8>, Arc<str>>)> = Vec::new();
    state.flying.retain(|flight| {
        let answer = match flight.answer.try_recv() {
            Ok(Ok(bytes)) => Ok(bytes),
            Ok(Err(why)) => {
                log::debug!("a picture was not downloaded: {}", why.chars().take(200).collect::<String>());
                Err(images::reason(&why))
            }
            // The host went away, or is taking longer than anything should.
            Err(TryRecvError::Disconnected) => Err(Arc::from(tl!("the download stopped before it was done"))),
            Err(TryRecvError::Empty) if now - flight.since > PATIENCE_MS => Err(Arc::from(tl!("it took too long"))),
            Err(TryRecvError::Empty) => return true,
        };
        landed.push((flight.message, flight.url.clone(), answer));
        false
    });
    if !landed.is_empty() {
        let pictures = Pictures::of(ctx);
        let mut pictures = pictures.lock().unwrap_or_else(|e| e.into_inner());
        for (message, url, answer) in landed {
            state.spend(message, 0, answer.as_ref().map_or(0, Vec::len));
            // Whatever came is a stranger's bytes: a picture within the limits, or nothing.
            let outcome = match answer {
                Ok(bytes) if bytes.len() > MAX_BYTES => Err(Arc::from(tl!("it is too large to show"))),
                Ok(bytes) if images::probe(&bytes).is_some() => Ok(bytes),
                Ok(bytes) => Err(Arc::from(images::not_a_picture(&bytes))),
                Err(why) => Err(why),
            };
            if let (Err(_), Some(kept)) = (&outcome, &app.services.picture_cache) {
                // Whatever the host may have kept from there is no use: it is not kept on.
                kept.forget(&url);
            }
            pictures.set_remote(url, outcome.map_or_else(Remote::Failed, |bytes| Remote::Ready(Arc::new(bytes))), frame);
        }
        ctx.request_repaint();
    }
    // What was wanted a frame ago is asked for now, if its message may still show it: a say-so
    // that was taken back in between is taken back for these too.
    let waiting = std::mem::take(&mut state.waiting);
    let allowed = |message: &Id| app.session.mailbox.messages.get(message).is_some_and(|m| access(app, m) == Access::Allowed);
    let waiting: Vec<(Id, Arc<str>)> = waiting.into_iter().filter(|(message, _)| allowed(message)).collect();
    let state = &mut app.remote_pictures;
    if let Some(download) = &app.services.pictures
        && !waiting.is_empty()
    {
        let pictures = Pictures::of(ctx);
        let pictures = pictures.lock().unwrap_or_else(|e| e.into_inner());
        for (message, url) in waiting {
            if state.flying.len() >= AT_A_TIME {
                // The rest is asked for again by its message, for as long as that is on screen.
                break;
            }
            // Each address once: not one that is on its way, nor one whose fate is known.
            if state.flying.iter().any(|flight| flight.url == url) || pictures.remote_peek(&url).is_some() || state.spent(message).out() {
                continue;
            }
            state.spend(message, 1, 0);
            let answer = download(url.to_string());
            state.flying.push(Flight { message, url, answer, since: now });
        }
    }
    if !state.flying.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// `m` is on screen and may show its pictures: note which of them are still to come, so that
/// they are asked for, and say how they are coming along.
pub fn want(app: &mut MailApp, ctx: &egui::Context, m: &Message, doc: &Doc) -> Progress {
    let mut progress = Progress { total: doc.remote.len().saturating_add(doc.remote_left_out), left_out: doc.remote_left_out, ..Progress::default() };
    if doc.remote.is_empty() {
        return progress;
    }
    let frame = images::frame(ctx);
    let pictures = Pictures::of(ctx);
    let mut pictures = pictures.lock().unwrap_or_else(|e| e.into_inner());
    // "Try again": what failed of this message's pictures is forgotten, here and by the host,
    // and so asked for once more, below.
    if let Some(at) = app.remote_pictures.retry.iter().position(|id| *id == m.id) {
        app.remote_pictures.retry.swap_remove(at);
        let mut again = 0;
        for url in &doc.remote {
            if pictures.forget_failure(url) {
                again += 1;
                if let Some(kept) = &app.services.picture_cache {
                    kept.forget(url);
                }
            }
        }
        app.remote_pictures.refund(m.id, again);
    }
    let state = &mut app.remote_pictures;
    let out = state.spent(m.id).out();
    for url in &doc.remote {
        match pictures.remote(url, frame) {
            Some(Remote::Ready(_)) => progress.ready += 1,
            Some(Remote::Failed(_)) => progress.failed += 1,
            None if state.flying.iter().any(|flight| flight.url == *url) => progress.waiting += 1,
            None if out => progress.left_out += 1,
            None => {
                progress.waiting += 1;
                if state.waiting.len() < MAX_WAITING && !state.waiting.iter().any(|(_, waiting)| waiting == url) {
                    state.waiting.push((m.id, url.clone()));
                }
            }
        }
    }
    if progress.waiting > 0 {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    progress
}

/// `text`, cut to `max` characters with an ellipsis when it is longer.
fn short(text: &str, max: usize) -> String {
    if text.chars().count() > max { text.chars().take(max.saturating_sub(1)).chain(['…']).collect() } else { text.to_string() }
}

/// The strip a notice above a message is drawn in: a wash of `hue` over whatever the header is
/// painted on, with an edge of it, in light and dark alike.
pub(crate) fn notice(hue: Color32, t: &Tokens) -> egui::Frame {
    let wash = Color32::from_rgba_unmultiplied(hue.r(), hue.g(), hue.b(), if t.dark { 46 } else { 30 });
    let edge = Color32::from_rgba_unmultiplied(hue.r(), hue.g(), hue.b(), if t.dark { 110 } else { 90 });
    egui::Frame::NONE.fill(wash).stroke(Stroke::new(1.0, edge)).corner_radius(4.0).inner_margin(egui::Margin { left: 8, right: 8, top: 4, bottom: 4 })
}

/// The bar above a message that has pictures on the web: that they are not shown and what the
/// owner can do about it, how downloading them is going, or how many could not be had. One
/// glance says which: a tinted strip the width of the header, a mark, a few words in bold, the
/// rest after them.
pub fn line(app: &mut MailApp, ui: &mut Ui, m: &Message, doc: &Doc, access: Access, progress: Progress, place: Place, t: &Tokens) {
    if doc.remote.is_empty() {
        return;
    }
    let missing = progress.failed + progress.left_out;
    let count = |template: &str, n: usize| crate::i18n::fmt(template, &[("n", &n.to_string()), ("total", &progress.total.to_string())]);
    let (lead, rest, trouble) = match access {
        Access::Unavailable => {
            (tl!("Pictures not shown.").to_string(), tl!("This message has pictures on the web. This build can't download them.").to_string(), false)
        }
        Access::Blocked if remembered_unvouched(app, m) => (
            tl!("Pictures not downloaded.").to_string(),
            tl!("You always download this sender's pictures, but the mail server didn't confirm that this message came from them, so it is asked first.").to_string(),
            false,
        ),
        Access::Blocked => (
            tl!("Pictures not downloaded.").to_string(),
            tl!("They are on the web, and fetching them would tell the sender that you opened this message.").to_string(),
            false,
        ),
        Access::Allowed if progress.waiting > 0 => {
            (tl!("Downloading pictures…").to_string(), count(tl!("{n} of {total} so far."), progress.ready), false)
        }
        Access::Allowed => match missing {
            0 => return,
            1 => (tl!("A picture couldn't be downloaded.").to_string(), tl!("Point at it to see why.").to_string(), true),
            n => (count(tl!("{n} pictures couldn't be downloaded."), n), tl!("Point at one to see why.").to_string(), true),
        },
    };
    let hue = if trouble { t.orange } else { t.blue };
    notice(hue, t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            icons::paint(ui.painter(), r.shrink(1.0), if trouble { "highImportance" } else { "info" }, t.text, hue);
            ui.add(egui::Label::new(egui::RichText::new(lead).font(semibold(11.5)).color(t.text)).wrap());
            ui.add(egui::Label::new(egui::RichText::new(rest).font(regular(11.5)).color(t.text_dim)).wrap());
            // What can be done about it: each a few words that stay together, on the next line
            // when this one is full.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            let action =
                |ui: &mut Ui, label: String, tip: &str| ui.add(egui::Link::new(egui::RichText::new(label).font(semibold(11.5)))).on_hover_text(tip);
            match access {
                Access::Blocked => {}
                // Only what failed can be tried again; what was left out for its number or its weight can't.
                Access::Allowed if trouble && progress.failed > 0 => {
                    let again = action(ui, tl!("Try again").to_string(), tl!("Ask once more for the pictures that couldn't be downloaded."));
                    if place == Place::Pane {
                        app.hit.add("retryPictures", m.id, again.rect);
                    }
                    if again.clicked() {
                        let _ = app.run("ui.retryPictures", json!({"message": m.id}));
                    }
                    return;
                }
                _ => return,
            }
            let download = action(
                ui,
                tl!("Download pictures").to_string(),
                tl!("Show this message's pictures, this once. Whoever sent it can then tell that you opened it."),
            );
            if place == Place::Pane {
                app.hit.add("downloadPictures", m.id, download.rect);
            }
            if download.clicked() {
                let _ = app.run("ui.downloadPictures", json!({"message": m.id}));
            }
            // Remembering a sender does nothing for what is in Junk E-mail, nor for a message the
            // server doesn't vouch for, so it is not offered there.
            let Some(from) = sender(&m.from.email).filter(|_| !in_junk(app, m) && vouched(app.session.mailbox.account(m.account), m)) else { return };
            let label = crate::i18n::fmt(tl!("Always download pictures from {sender}"), &[("sender", &short(&from, 60))]);
            let always = action(ui, label, tl!("Remembered in the settings; File › Options lists the senders and takes them off again."));
            if place == Place::Pane {
                app.hit.add("alwaysDownloadPictures", m.id, always.rect);
            }
            if always.clicked() {
                let _ = app.run("ui.alwaysDownloadPictures", json!({"sender": from}));
            }
            let Some(of_sender) = domain(&from) else { return };
            let label = crate::i18n::fmt(tl!("Always from @{domain}"), &[("domain", &short(&of_sender, 48))]);
            let everybody = action(
                ui,
                label,
                tl!("Always download pictures from everybody whose address ends this way. Remembered in the settings; File › Options lists the domains and takes them off again."),
            );
            if place == Place::Pane {
                app.hit.add("alwaysDownloadPicturesDomain", m.id, everybody.rect);
            }
            if everybody.clicked() {
                let _ = app.run("ui.alwaysDownloadPictures", json!({"domain": of_sender}));
            }
        });
    });
}

/// A number of bytes for the owner to read: `340 KB`, `12.4 MB`.
fn size_label(bytes: u64) -> String {
    // (In KB at least.)
    mailcraft_model::size_label_in(crate::i18n::current(), bytes.max(1024))
}

/// The rows of File › Options: the switch for all mail, the senders and the domains that are
/// remembered, and what is kept of downloaded pictures on this computer.
pub fn options(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let mut all = app.ui.download_pictures;
    let tip = tl!(
        "A picture on the web tells its sender that you opened the message, and when. Left off, pictures are downloaded only for the messages, senders and domains you say."
    );
    if ui.checkbox(&mut all, tl!("Download pictures in mail automatically")).on_hover_text(tip).changed() {
        let _ = app.run("ui.downloadPicturesAutomatically", json!({"value": all}));
    }
    if !available(app) {
        ui.label(egui::RichText::new(tl!("This build can't download pictures: they stay placeholders.")).color(t.text_dim));
    }
    let senders: Vec<String> = app.ui.picture_senders.iter().take(MAX_SENDERS).map(|s| s.chars().take(MAX_SENDER_LEN).collect()).collect();
    let domains: Vec<String> = app.ui.picture_domains.iter().take(MAX_DOMAINS).filter_map(|d| domain(d)).collect();
    if senders.is_empty() && domains.is_empty() {
        let none = tl!("No senders are remembered. \"Always download pictures from\" above a message adds its sender or its sender's domain here.");
        ui.label(egui::RichText::new(none).color(t.text_dim));
    } else {
        ui.label(tl!("Pictures are always downloaded from:"));
    }
    for address in senders {
        ui.horizontal(|ui| {
            if ui.small_button(tl!("Remove")).on_hover_text(tl!("Ask before downloading this sender's pictures again")).clicked() {
                let _ = app.run("ui.alwaysDownloadPictures", json!({"sender": address, "value": false}));
            }
            ui.add(egui::Label::new(address.as_str()).truncate());
        });
    }
    for name in domains {
        ui.horizontal(|ui| {
            if ui.small_button(tl!("Remove")).on_hover_text(tl!("Ask before downloading pictures from this domain's senders again")).clicked() {
                let _ = app.run("ui.alwaysDownloadPictures", json!({"domain": name, "value": false}));
            }
            let everybody = crate::i18n::fmt(tl!("everybody at @{domain}"), &[("domain", &name)]);
            ui.add(egui::Label::new(everybody).truncate());
        });
    }
    // What the host keeps of them between runs, when it keeps anything.
    let Some((count, bytes)) = app.services.picture_cache.as_ref().map(|kept| kept.usage()) else { return };
    ui.horizontal(|ui| {
        let kept = match count {
            0 => tl!("No downloaded pictures are kept on this computer.").to_string(),
            n => crate::i18n::fmt(
                &tln!(n, "{n} downloaded picture is kept on this computer ({size}).", "{n} downloaded pictures are kept on this computer ({size})."),
                &[("size", &size_label(bytes))],
            ),
        };
        let tip = tl!(
            "A picture you asked for is kept for a month, so that reading the message again doesn't ask its sender's server again. Nothing kept is ever run, and it is checked like a new download before it is shown."
        );
        ui.label(kept).on_hover_text(tip);
        if ui.add_enabled(count > 0, egui::Button::new(tl!("Clear")).small()).on_hover_text(tl!("Forget the pictures that are kept; they are downloaded again when you ask.")).clicked() {
            let _ = app.run("ui.clearPictureCache", json!({}));
        }
    });
}

/// The message a command is about: the one it names, else the one being read.
fn subject(app: &MailApp, p: &Value) -> Result<Arc<Message>, String> {
    let id = match p.get("message") {
        None | Some(Value::Null) => app.session.sel.focus().ok_or_else(|| tl!("Select a message first").to_string())?,
        Some(id) => id.as_u64().ok_or_else(|| "`message` is a message's id".to_string())?,
    };
    app.session.mailbox.messages.get(&id).cloned().ok_or_else(|| format!("no message with id {id}"))
}

/// The window commands that are about pictures (`UI_COMMANDS` lists them).
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "ui.downloadPictures" => download(app, p),
        "ui.alwaysDownloadPictures" => always(app, p),
        "ui.downloadPicturesAutomatically" => {
            let on = p.get("value").and_then(Value::as_bool).unwrap_or(!app.ui.download_pictures);
            app.ui.download_pictures = on;
            Ok(json!({"value": on}))
        }
        "ui.retryPictures" => retry(app, p),
        "ui.clearPictureCache" => clear(app),
        // The other commands of a message that is being read: its selected text, and the
        // meeting it carries.
        _ => return super::select::ui_command(app, id, p).or_else(|| super::invitation::ui_command(app, id, p)),
    })
}

/// "Download pictures": this message's, from now until the program closes.
fn download(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let m = subject(app, p)?;
    if !available(app) {
        return Err(tl!("This build can't download pictures").to_string());
    }
    let mark = mark(&m);
    let asked = &mut app.remote_pictures.asked;
    if !asked.contains(&mark) {
        if asked.len() >= MAX_ASKED {
            asked.pop_front();
        }
        asked.push_back(mark);
    }
    Ok(json!({"message": m.id}))
}

/// "Try again": the pictures of this message that couldn't be downloaded are asked for once more.
fn retry(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let m = subject(app, p)?;
    if !available(app) {
        return Err(tl!("This build can't download pictures").to_string());
    }
    let waiting = &mut app.remote_pictures.retry;
    if !waiting.contains(&m.id) {
        if waiting.len() >= MAX_RETRIES {
            waiting.remove(0);
        }
        waiting.push(m.id);
    }
    Ok(json!({"message": m.id}))
}

/// "Clear": what is kept of downloaded pictures goes, on disk and in memory.
fn clear(app: &mut MailApp) -> Result<Value, String> {
    let on_disk = match &app.services.picture_cache {
        Some(kept) => {
            let (count, _) = kept.usage();
            kept.clear()?;
            count
        }
        None => 0,
    };
    app.remote_pictures.forget_all = true;
    app.sender_images = Default::default();
    Ok(json!({"cleared": on_disk}))
}

/// "Always download pictures from": remember a sender or a domain, or forget one.
fn always(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let on = p.get("value").and_then(Value::as_bool).unwrap_or(true);
    if let Some(named) = p.get("domain").filter(|named| !named.is_null()) {
        let named = named.as_str().ok_or_else(|| "`domain` is what follows the @ of an address".to_string())?;
        let Some(name) = domain(named) else { return Err(tl!("That is not a domain").to_string()) };
        let kept = &mut app.ui.picture_domains;
        kept.retain(|other| domain(other).is_some_and(|other| other != name));
        kept.truncate(MAX_DOMAINS);
        if on {
            if kept.len() >= MAX_DOMAINS {
                return Err(tl!("The list of domains is full: remove some in File › Options").to_string());
            }
            kept.push(name.clone());
        }
        return Ok(json!({"domain": name, "value": on, "domains": kept.len()}));
    }
    let named = match p.get("sender") {
        None | Some(Value::Null) => subject(app, p)?.from.email.clone(),
        Some(text) => text.as_str().ok_or_else(|| "`sender` is an address".to_string())?.to_string(),
    };
    let Some(address) = sender(&named) else {
        // An entry from before addresses were kept in ASCII only can still be taken off.
        let old = named.trim();
        if !on && !old.is_empty() && old.len() <= MAX_SENDER_LEN {
            app.ui.picture_senders.retain(|other| other.trim().to_lowercase() != old.to_lowercase());
            return Ok(json!({"sender": old, "value": false, "senders": app.ui.picture_senders.len()}));
        }
        return Err(tl!("That is not a sender's address").to_string());
    };
    let kept = &mut app.ui.picture_senders;
    kept.retain(|other| !other.trim().eq_ignore_ascii_case(&address));
    kept.truncate(MAX_SENDERS);
    if on {
        if kept.len() >= MAX_SENDERS {
            return Err(tl!("The list of senders is full: remove some in File › Options").to_string());
        }
        kept.push(address.clone());
    }
    Ok(json!({"sender": address, "value": on, "senders": kept.len()}))
}
