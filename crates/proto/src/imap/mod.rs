//! An IMAP4rev1 client (RFC 3501) with the commands a two-way synchronisation needs: reading
//! mailboxes, and changing flags and keywords, moving, copying, removing and filing messages (UID
//! MOVE of RFC 6851; UID EXPUNGE and the `COPYUID` and `APPENDUID` answers of RFC 4315), finding
//! a message by its Message-ID (UID SEARCH), and waiting for news (IDLE, RFC 2177). Special-use mailbox flags
//! (RFC 6154) come through in [`Mailbox::flags`] as the server wrote them. Signing in is LOGIN
//! with a password, or AUTHENTICATE XOAUTH2 with an OAuth access token.
//!
//! There is no plain EXPUNGE and no CLOSE here, on purpose: both remove every message marked
//! `\Deleted` in the mailbox, also those another program marked.

mod parse;
mod utf7;

use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};

use parse::{Piece, Status, StatusLine, Value};
pub use utf7::{decode_utf7, encode_utf7};

use crate::line::Wire;
use crate::{MAX_LINE, MAX_LITERAL, ProtoError, Secrets, tidy, xoauth2};

/// Most responses the server may send to one command.
const MAX_RESPONSES: usize = 1_000_000;
/// Most literals one response may carry.
const MAX_LITERALS: usize = 32;
/// Most text one response may carry outside its literals.
const MAX_TEXT: usize = 4 * MAX_LINE;
/// Most flags kept for one message or mailbox, and most sent in one command.
const MAX_FLAGS: usize = 64;
const MAX_CAPABILITIES: usize = 256;
/// Longest string sent in quotes; a longer one goes as a literal.
const MAX_QUOTED: usize = 1_000;
/// Most messages named in one set by [`uid_sets`], and most a `COPYUID` answer may speak of.
const MAX_SET_UIDS: usize = 500;
/// Longest set [`uid_sets`] writes, in characters.
const MAX_SET_TEXT: usize = 800;
/// Most UIDs a SEARCH answer is read for.
const MAX_FOUND: usize = 1_000;

/// An IMAP session on a stream.
pub struct Imap<S> {
    wire: Wire<S>,
    /// The number in the next command's tag.
    next_tag: u32,
    secrets: Secrets,
    /// What the server said it can do, upper-cased; `None` until it has.
    capabilities: Option<Vec<String>>,
    authenticated: bool,
    /// What the mailbox being opened has said about itself so far.
    selected: Selected,
    /// Every `COPYUID` answer since the last copy or move was sent (`None` for one that couldn't
    /// be read); two are as many as it takes to know there was more than one.
    copied: Vec<Option<Copied>>,
    /// LOGOUT was sent, so BYE is the answer we asked for.
    leaving: bool,
    /// Where the last APPEND put its message, when the server said (`APPENDUID`).
    appended: Option<(u32, u32)>,
    /// The tag of the IDLE under way, between [`Imap::idle`] and [`Imap::idle_done`].
    idling: Option<String>,
    /// The server has greeted: a line from it that is no IMAP response is now the end of the
    /// session, not the wrong kind of server.
    greeted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mailbox {
    /// Decoded from modified UTF-7.
    pub name: String,
    /// The name exactly as the server wrote it (use this in commands).
    pub raw: String,
    pub delimiter: Option<char>,
    /// Flags as written, e.g. `\\Noselect`, `\\Sent`, `\\HasChildren`.
    pub flags: Vec<String>,
}

impl Mailbox {
    /// Whether the mailbox has `flag` (`\Noselect`), whatever the letter case.
    pub fn has_flag(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f.eq_ignore_ascii_case(flag))
    }
}

/// How a mailbox stands, asked without opening it ([`Imap::status`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MailboxStatus {
    /// How many messages it has (`MESSAGES`).
    pub exists: u32,
    pub uid_next: u32,
    pub uid_validity: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selected {
    pub exists: u32,
    pub uid_validity: u32,
    pub uid_next: u32,
    /// The flags the server keeps on the mailbox's messages for good (`PERMANENTFLAGS`), as
    /// written; `\*` among them means any keyword can be made. `None` when the server didn't say,
    /// which RFC 3501 asks to be taken as every flag.
    pub permanent_flags: Option<Vec<String>>,
}

impl Selected {
    /// Whether the server keeps keywords of the owner's choosing on the mailbox's messages: it
    /// said it keeps any (`\*`), or it said nothing about what it keeps.
    pub fn keeps_keywords(&self) -> bool {
        self.permanent_flags.as_ref().is_none_or(|flags| flags.iter().any(|f| f == "\\*"))
    }
}

/// What came while the server was waiting with news (IDLE).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleNews {
    /// Nothing, for as long as the stream waits for a read.
    Quiet,
    /// The mailbox has new or fewer messages: one came (`EXISTS`) or went (`EXPUNGE`). A flag
    /// that changed (`FETCH`) or `RECENT` is [`IdleNews::Other`]: it is not what a watch on the
    /// Inbox waits for, and the watcher's own changes come back that way.
    Changed,
    /// Something else: a line of status, news about something else.
    Other,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fetched {
    pub uid: u32,
    pub flags: Vec<String>,
    /// INTERNALDATE as written by the server, when asked for.
    pub internal_date: Option<String>,
    /// The whole message (BODY[]), when asked for.
    pub body: Option<Vec<u8>>,
}

/// What became of the messages a UID COPY or UID MOVE named, when the server says (the `COPYUID`
/// response code of RFC 4315).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Copied {
    /// `UIDVALIDITY` of the mailbox the messages went to.
    pub uid_validity: u32,
    /// Each message's UID where it was, and its UID where it is now.
    pub uids: Vec<(u32, u32)>,
}

/// Neither the stream nor anything secret is shown.
impl<S> std::fmt::Debug for Imap<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Imap").field("next_tag", &self.next_tag).field("authenticated", &self.authenticated).finish_non_exhaustive()
    }
}

/// One response from the server.
enum Response {
    /// `+ …`: the server is ready for the rest of the command.
    GoAhead,
    Status {
        tag: Option<String>,
        line: StatusLine,
    },
    /// Untagged data: `* 3 EXISTS`, `* LIST …`, `* 3 FETCH (…)`.
    Data(Vec<Value>),
}

enum Part<'a> {
    Text(Vec<u8>),
    Literal(&'a [u8]),
}

/// A command being put together (without its tag).
struct Command<'a> {
    parts: Vec<Part<'a>>,
    /// What the log shows in place of the command.
    shown: Option<&'static str>,
    /// An argument has a NUL, which no IMAP string can carry.
    unsendable: bool,
}

impl<'a> Command<'a> {
    fn new(verb: &str) -> Self {
        Command { parts: vec![Part::Text(verb.as_bytes().to_vec())], shown: None, unsendable: false }
    }

    fn text(&mut self, bytes: &[u8]) {
        match self.parts.last_mut() {
            Some(Part::Text(text)) => text.extend_from_slice(bytes),
            _ => self.parts.push(Part::Text(bytes.to_vec())),
        }
    }

    /// A bare argument: our own words, or text that was checked.
    fn arg(mut self, word: &str) -> Self {
        self.text(b" ");
        self.text(word.as_bytes());
        self
    }

    /// An argument as a quoted string, or as a literal when quotes can't carry it.
    fn string(mut self, bytes: &'a [u8]) -> Self {
        self.text(b" ");
        if bytes.len() <= MAX_QUOTED && bytes.iter().all(|b| matches!(b, b' '..=b'~')) {
            let mut quoted = Vec::with_capacity(bytes.len() + 2);
            quoted.push(b'"');
            for b in bytes {
                if matches!(b, b'"' | b'\\') {
                    quoted.push(b'\\');
                }
                quoted.push(*b);
            }
            quoted.push(b'"');
            self.text(&quoted);
        } else {
            self.unsendable |= bytes.contains(&0);
            self.parts.push(Part::Literal(bytes));
        }
        self
    }

    /// An argument that always goes as a literal (a message).
    fn literal(mut self, bytes: &'a [u8]) -> Self {
        self.text(b" ");
        self.parts.push(Part::Literal(bytes));
        self
    }

    fn shown_as(mut self, shown: &'static str) -> Self {
        self.shown = Some(shown);
        self
    }

    /// The command for the log: literals by size only.
    fn for_log(&self) -> String {
        if let Some(shown) = self.shown {
            return shown.to_string();
        }
        self.parts
            .iter()
            .map(|part| match part {
                Part::Text(text) => String::from_utf8_lossy(text).into_owned(),
                Part::Literal(bytes) => format!("({} bytes)", bytes.len()),
            })
            .collect()
    }
}

/// What one FETCH response says about a message.
#[derive(Default)]
struct Item {
    uid: u32,
    flags: Option<Vec<String>>,
    internal_date: Option<String>,
    body: Option<Vec<u8>>,
    /// The header alone (`BODY[HEADER]`).
    header: Option<Vec<u8>>,
    size: Option<u64>,
}

impl Item {
    /// From `n FETCH (name value name value …)`, the names in any order.
    fn from(values: Vec<Value>) -> Option<Item> {
        let mut values = values.into_iter();
        values.next()?.number()?;
        if !values.next()?.atom()?.eq_ignore_ascii_case("FETCH") {
            return None;
        }
        let Value::List(pairs) = values.next()? else { return None };
        let mut item = Item::default();
        let mut pairs = pairs.into_iter();
        while let Some(name) = pairs.next() {
            // Something that isn't a name is skipped, in the hope that a name follows.
            let Some(name) = name.atom().map(str::to_ascii_uppercase) else { continue };
            let Some(value) = pairs.next() else { break };
            match (name.as_str(), value) {
                ("UID", value) => item.uid = value.number().unwrap_or(0),
                ("FLAGS", Value::List(flags)) => item.flags = Some(flags.into_iter().filter_map(Value::into_text).take(MAX_FLAGS).collect()),
                ("INTERNALDATE", Value::Str(date)) => item.internal_date = Some(String::from_utf8_lossy(&date).into_owned()),
                ("RFC822.SIZE", value) => item.size = value.atom().and_then(|n| n.parse().ok()),
                ("BODY[]" | "RFC822", Value::Str(body)) => item.body = Some(body),
                ("BODY[HEADER]" | "RFC822.HEADER", Value::Str(header)) => item.header = Some(header),
                _ => {}
            }
        }
        Some(item)
    }
}

/// From `LIST (flags) "delimiter" name`.
fn mailbox_from(values: Vec<Value>) -> Option<Mailbox> {
    let mut values = values.into_iter();
    if !values.next()?.atom()?.eq_ignore_ascii_case("LIST") {
        return None;
    }
    let Value::List(flags) = values.next()? else { return None };
    let delimiter = match values.next()? {
        Value::Str(d) => String::from_utf8_lossy(&d).chars().next(),
        _ => None,
    };
    let raw = values.next()?.into_text()?;
    Some(Mailbox { name: decode_utf7(&raw), raw, delimiter, flags: flags.into_iter().filter_map(Value::into_text).take(MAX_FLAGS).collect() })
}

/// `* STATUS <name> (MESSAGES 4 UIDNEXT 9 UIDVALIDITY 11)`: the numbers, in whatever order the
/// server lists them; one it leaves out is 0. `None` for a line that isn't a STATUS.
fn status_from(values: Vec<Value>) -> Option<MailboxStatus> {
    let mut values = values.into_iter();
    if !values.next()?.atom()?.eq_ignore_ascii_case("STATUS") {
        return None;
    }
    let _name = values.next()?;
    let Value::List(items) = values.next()? else { return None };
    let mut status = MailboxStatus::default();
    let mut items = items.into_iter();
    while let Some(item) = items.next() {
        let Some(name) = item.atom() else { continue };
        let value = items.next().and_then(|v| v.number()).unwrap_or(0);
        match name.to_ascii_uppercase().as_str() {
            "MESSAGES" => status.exists = value,
            "UIDNEXT" => status.uid_next = value,
            "UIDVALIDITY" => status.uid_validity = value,
            _ => {}
        }
    }
    Some(status)
}

/// A message set: `1:*`, `5,9,12:20`.
fn is_set(set: &str) -> bool {
    !set.is_empty() && set.len() <= MAX_QUOTED && set.bytes().all(|b| b.is_ascii_digit() || matches!(b, b':' | b',' | b'*'))
}

/// A flag or keyword: `\Seen`, `$Forwarded`.
fn is_flag(flag: &str) -> bool {
    let word = flag.strip_prefix('\\').unwrap_or(flag);
    !word.is_empty()
        && word.len() <= 100
        && word.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'(' | b')' | b'{' | b'%' | b'*' | b'"' | b'\\' | b']'))
}

/// `(\Seen \Flagged)`, once every flag is known to be one.
fn flag_list(flags: &[&str]) -> Result<String, ProtoError> {
    if flags.len() > MAX_FLAGS {
        return Err(ProtoError::Protocol("too many flags for one command".into()));
    }
    match flags.iter().find(|f| !is_flag(f)) {
        Some(bad) => Err(ProtoError::Protocol(format!("\"{}\" can't be used as a flag", tidy(bad)))),
        None => Ok(format!("({})", flags.join(" "))),
    }
}

/// A number as IMAP writes one: digits and nothing else.
fn number(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// The UIDs a set names, in the order written (`4,7:9` is 4, 7, 8, 9). `None` for a set that
/// doesn't name its messages one by one beyond doubt: `*`, zero, a range written backwards, or
/// more than [`MAX_SET_UIDS`] numbers.
fn uids_of(set: &str) -> Option<Vec<u32>> {
    let mut out: Vec<u32> = Vec::new();
    for part in set.split(',') {
        let (first, last) = match part.split_once(':') {
            Some((first, last)) => (number(first)?, number(last)?),
            None => {
                let one = number(part)?;
                (one, one)
            }
        };
        if first == 0 || first > last || (last - first) as usize >= MAX_SET_UIDS.saturating_sub(out.len()) {
            return None;
        }
        out.extend(first..=last);
    }
    Some(out)
}

/// A set for a command that changes things: it names its messages number by number. `1:*`
/// would take in messages nobody looked at.
fn named(set: &str) -> Result<&str, ProtoError> {
    if set.len() <= MAX_QUOTED && uids_of(set).is_some() {
        Ok(set)
    } else {
        Err(ProtoError::Protocol(format!("\"{}\" doesn't name the messages to change", tidy(set))))
    }
}

/// From the text of a `COPYUID` code: `<uidvalidity> <where they were> <where they are>`.
/// `None` unless it pairs every message beyond doubt (the first set strictly ascending, the
/// second as long and without repeats): a wrong pairing would tie a message here to another one
/// on the server.
fn copied_from(arg: &str) -> Option<Copied> {
    let mut words = arg.split_whitespace();
    let uid_validity = number(words.next()?).filter(|validity| *validity != 0)?;
    let (from, to) = (uids_of(words.next()?)?, uids_of(words.next()?)?);
    let mut distinct = to.clone();
    distinct.sort_unstable();
    distinct.dedup();
    if words.next().is_some() || from.len() != to.len() || distinct.len() != to.len() || !from.is_sorted_by(|a, b| a < b) {
        return None;
    }
    Some(Copied { uid_validity, uids: from.into_iter().zip(to).collect() })
}

/// From the text of an `APPENDUID` code: `<uidvalidity> <uid>`. `None` unless both are numbers
/// other than zero and nothing else is there (a server that appended many names a set).
fn appended_from(arg: &str) -> Option<(u32, u32)> {
    let mut words = arg.split_whitespace();
    let uid_validity = number(words.next()?).filter(|n| *n != 0)?;
    let uid = number(words.next()?).filter(|n| *n != 0)?;
    words.next().is_none().then_some((uid_validity, uid))
}

/// UIDs as message sets for commands (`4,7:9,12`), each with the UIDs it names: sorted, each UID
/// once, consecutive numbers as ranges, and no set longer than one command should carry. A
/// range only ever spans numbers that were asked for.
pub fn uid_sets(uids: &[u32]) -> Vec<(String, Vec<u32>)> {
    let mut sorted: Vec<u32> = uids.iter().copied().filter(|uid| *uid != 0).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let mut sets = Vec::new();
    let (mut text, mut uids_named) = (String::new(), Vec::new());
    let mut at = 0;
    // Every round takes at least one UID off the list.
    while let Some(&first) = sorted.get(at) {
        let (mut last, mut next) = (first, at + 1);
        while uids_named.len() + (next - at) < MAX_SET_UIDS && sorted.get(next).is_some_and(|uid| last.checked_add(1) == Some(*uid)) {
            last = last.saturating_add(1);
            next += 1;
        }
        let range = if last == first { first.to_string() } else { format!("{first}:{last}") };
        if !text.is_empty() && text.len() + 1 + range.len() > MAX_SET_TEXT {
            sets.push((std::mem::take(&mut text), std::mem::take(&mut uids_named)));
        }
        if !text.is_empty() {
            text.push(',');
        }
        text.push_str(&range);
        uids_named.extend_from_slice(sorted.get(at..next).unwrap_or(&[]));
        at = next;
        if uids_named.len() >= MAX_SET_UIDS {
            sets.push((std::mem::take(&mut text), std::mem::take(&mut uids_named)));
        }
    }
    if !text.is_empty() {
        sets.push((text, uids_named));
    }
    sets
}

fn ignore(_: Vec<Value>) {}

impl<S: Read + Write> Imap<S> {
    /// Read the greeting (`* OK` or `* PREAUTH`; `* BYE` is an error).
    pub fn connect(stream: S) -> Result<Self, ProtoError> {
        let mut imap = Self::resume(stream);
        imap.greeted = false;
        match imap.read_response()? {
            Response::Status { tag: None, line } => match line.status {
                Status::Ok | Status::Preauth => {
                    imap.authenticated = line.status == Status::Preauth;
                    imap.greeted = true;
                    imap.note_status(&line);
                    Ok(imap)
                }
                Status::Bye => Err(ProtoError::Rejected(imap.refusal(&line))),
                Status::No | Status::Bad => Err(ProtoError::Protocol(format!("the server greeted with a refusal: {}", imap.refusal(&line)))),
            },
            _ => Err(ProtoError::Protocol("the server didn't greet like an IMAP server".into())),
        }
    }

    /// Continue on a stream that already greeted (after the caller wrapped it in TLS).
    pub fn resume(stream: S) -> Self {
        Imap {
            wire: Wire::new(stream),
            next_tag: 1,
            secrets: Secrets::default(),
            capabilities: None,
            authenticated: false,
            selected: Selected::default(),
            copied: Vec::new(),
            leaving: false,
            appended: None,
            idling: None,
            greeted: true,
        }
    }

    /// Whether the session is signed in: after [`login`](Self::login) or
    /// [`authenticate_xoauth2`](Self::authenticate_xoauth2), or from the start when the server greeted with PREAUTH.
    pub fn is_authenticated(&self) -> bool {
        self.authenticated
    }

    /// The stream the session runs on, for its owner to set up between commands (how long a
    /// socket waits, say). Reading from it or writing to it would take the session apart.
    pub fn get_mut(&mut self) -> &mut S {
        self.wire.get_mut()
    }

    /// Whether the server has said it can do `capability` (upper-cased); `false` while it hasn't said anything.
    pub fn offers(&self, capability: &str) -> bool {
        self.capabilities.as_ref().is_some_and(|caps| caps.iter().any(|c| c == capability))
    }

    /// Upper-cased capability atoms (`IMAP4REV1`, `STARTTLS`, `SPECIAL-USE`…).
    pub fn capabilities(&mut self) -> Result<Vec<String>, ProtoError> {
        self.capabilities = None;
        self.run(Command::new("CAPABILITY"), &mut ignore)?;
        Ok(self.capabilities.clone().unwrap_or_default())
    }

    /// Send STARTTLS and hand the stream back for the caller to wrap; call `resume` afterwards.
    pub fn starttls(mut self) -> Result<S, ProtoError> {
        self.run(Command::new("STARTTLS"), &mut ignore)?;
        // Anything after the go-ahead was slipped in before encryption started.
        if self.wire.has_pending() {
            return Err(ProtoError::Protocol("the server sent data before the secure connection started".into()));
        }
        Ok(self.wire.into_inner())
    }

    /// LOGIN with properly quoted (or literal) strings. NO → `ProtoError::Auth`.
    pub fn login(&mut self, user: &str, password: &str) -> Result<(), ProtoError> {
        if user.contains('\0') || password.contains('\0') {
            return Err(ProtoError::Auth("the user name or password has a character that can't be sent".into()));
        }
        // RFC 3501 §6.2.3: a server that says LOGINDISABLED must not be sent a password.
        if self.offers("LOGINDISABLED") {
            return Err(ProtoError::Auth("the server doesn't take a password on this connection".into()));
        }
        self.secrets.add(password);
        let command = Command::new("LOGIN").string(user.as_bytes()).string(password.as_bytes()).shown_as("LOGIN ***");
        let line = self.exchange(command, &mut ignore)?;
        match line.status {
            Status::Ok => {
                self.authenticated = true;
                Ok(())
            }
            Status::No => Err(ProtoError::Auth(self.refusal(&line))),
            _ => Err(ProtoError::Rejected(self.refusal(&line))),
        }
    }

    /// AUTHENTICATE XOAUTH2: an OAuth access token in place of a password. NO → `ProtoError::Auth`.
    ///
    /// The token goes out with the command (RFC 4959) only to a server that listed both
    /// `SASL-IR` and `AUTH=XOAUTH2`. Any other server, one that listed neither or hasn't said
    /// what it can do, is tried all the same, but asked first: it gets the token after its
    /// go-ahead, so one that doesn't know the mechanism refuses without having seen it.
    pub fn authenticate_xoauth2(&mut self, user: &str, token: &str) -> Result<(), ProtoError> {
        let payload = xoauth2(user, token)?;
        self.secrets.add(token);
        self.secrets.add(&payload);
        let inline = self.offers("SASL-IR") && self.offers("AUTH=XOAUTH2");
        let tag = self.tag();
        log::trace!("C: {tag} AUTHENTICATE XOAUTH2{}", if inline { " ***" } else { "" });
        let mut out = format!("{tag} AUTHENTICATE XOAUTH2");
        if inline {
            out.push(' ');
            out.push_str(&payload);
        }
        out.push_str("\r\n");
        self.wire.write(out.as_bytes())?;
        self.wire.flush()?;
        // What each `+` from the server is answered with: the token (unless it went with the
        // command), an empty line for the reasons a refusal starts with (JSON in base64), and
        // for a question after that, the end of the exchange.
        let answers = [(payload.as_str(), "***"), ("", ""), ("*", "*")];
        let mut asked = usize::from(inline);
        let line = loop {
            if let Some(line) = self.read_until(&tag, true, &mut ignore)? {
                break line;
            }
            let Some((answer, shown)) = answers.get(asked) else {
                return Err(ProtoError::Protocol("the server asked for more than a sign-in has".into()));
            };
            asked += 1;
            log::trace!("C: {shown}");
            self.wire.write(format!("{answer}\r\n").as_bytes())?;
            self.wire.flush()?;
        };
        match line.status {
            // The exchange was called off, whatever the server makes of that.
            _ if asked == answers.len() => Err(ProtoError::Auth("the server asked for more than a user name and access token".into())),
            // Nobody is signed in by a token that was never sent.
            Status::Ok if asked == 0 => Err(ProtoError::Protocol("the server ended the command early".into())),
            Status::Ok => {
                self.authenticated = true;
                Ok(())
            }
            Status::No => Err(ProtoError::Auth(self.refusal(&line))),
            _ => Err(ProtoError::Rejected(self.refusal(&line))),
        }
    }

    /// LIST "" "*".
    pub fn list(&mut self) -> Result<Vec<Mailbox>, ProtoError> {
        let mut out: Vec<Mailbox> = Vec::new();
        let mut seen = HashSet::new();
        self.run(Command::new("LIST").arg("\"\"").arg("\"*\""), &mut |values| {
            // A mailbox the server names twice is listed once.
            if let Some(mailbox) = mailbox_from(values)
                && seen.insert(mailbox.raw.clone())
            {
                out.push(mailbox);
            }
        })?;
        Ok(out)
    }

    /// STATUS <mailbox> (MESSAGES UIDNEXT UIDVALIDITY): how the mailbox stands without opening
    /// it, which costs the server far less than SELECT or EXAMINE, and leaves the open mailbox
    /// as it is (RFC 3501 §6.3.10). A server that answers without the numbers gives zeros for
    /// them; one that answers with no `STATUS` line at all is a protocol error.
    pub fn status(&mut self, raw_name: &str) -> Result<MailboxStatus, ProtoError> {
        let mut found: Option<MailboxStatus> = None;
        let command = Command::new("STATUS").string(raw_name.as_bytes()).arg("(MESSAGES UIDNEXT UIDVALIDITY)");
        self.run(command, &mut |values| {
            if let Some(status) = status_from(values) {
                found = Some(status);
            }
        })?;
        found.ok_or_else(|| ProtoError::Protocol("the server answered STATUS without saying how the mailbox stands".into()))
    }

    /// EXAMINE (read-only) or SELECT.
    pub fn select(&mut self, raw_name: &str, read_only: bool) -> Result<Selected, ProtoError> {
        self.selected = Selected::default();
        let command = Command::new(if read_only { "EXAMINE" } else { "SELECT" }).string(raw_name.as_bytes());
        self.run(command, &mut ignore)?;
        Ok(self.selected.clone())
    }

    /// UID FETCH <set> (UID FLAGS): every message in the set with its flags. `set` like "1:*" or "5,9,12:20".
    ///
    /// The messages come back in UID order, each once.
    pub fn uid_fetch_flags(&mut self, set: &str) -> Result<Vec<Fetched>, ProtoError> {
        let mut found: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        self.run(Self::fetch(set, "(UID FLAGS)")?, &mut |values| {
            // A response with a UID shows the message is there, even one that forgot the flags.
            if let Some(item) = Item::from(values)
                && item.uid != 0
            {
                let flags = found.entry(item.uid).or_default();
                // When the server speaks of a message twice, its later word counts.
                if let Some(later) = item.flags {
                    *flags = later;
                }
            }
        })?;
        Ok(found.into_iter().map(|(uid, flags)| Fetched { uid, flags, internal_date: None, body: None }).collect())
    }

    /// UID FETCH <set> (UID RFC822.SIZE): the size in bytes of every message in the set, in UID order.
    pub fn uid_fetch_sizes(&mut self, set: &str) -> Result<Vec<(u32, u64)>, ProtoError> {
        let mut out: Vec<(u32, u64)> = Vec::new();
        self.run(Self::fetch(set, "(UID RFC822.SIZE)")?, &mut |values| {
            if let Some(Item { uid, size: Some(size), .. }) = Item::from(values)
                && uid != 0
            {
                out.push((uid, size));
            }
        })?;
        out.sort_by_key(|(uid, _)| *uid);
        out.dedup_by_key(|(uid, _)| *uid);
        Ok(out)
    }

    /// UID FETCH <uid> (UID FLAGS INTERNALDATE BODY.PEEK[]); `None` when the UID is gone.
    pub fn uid_fetch_message(&mut self, uid: u32) -> Result<Option<Fetched>, ProtoError> {
        self.fetch_one(uid, "(UID FLAGS INTERNALDATE BODY.PEEK[])", false)
    }

    /// UID FETCH <uid> (UID FLAGS INTERNALDATE BODY.PEEK[HEADER]): the header alone of a message
    /// that is not taken whole (one that is too large), in [`Fetched::body`], up to and with the
    /// empty line that ends it. `None` when the UID is gone.
    pub fn uid_fetch_header(&mut self, uid: u32) -> Result<Option<Fetched>, ProtoError> {
        self.fetch_one(uid, "(UID FLAGS INTERNALDATE BODY.PEEK[HEADER])", true)
    }

    /// UID FETCH <set> (UID FLAGS INTERNALDATE BODY.PEEK[]), or with `header_only`
    /// BODY.PEEK[HEADER]: many messages in one command, each handed to `on_message` as soon as
    /// it has arrived, in the order the server sends them (as a rule, by UID upwards). The server
    /// answers them one after the other without waiting to be asked again, which is what makes a
    /// first download fast: one round trip for the lot instead of one per message.
    ///
    /// Only messages of the set are handed over, each once, and only with what was asked for (a
    /// whole message in answer to a question about its header is not taken in, nor the other
    /// way round). Flags or a date the server says of a message in a response of its own before
    /// the one with the message go along with it. A message that doesn't come back is gone, or
    /// kept back by the server: the caller tells which. When the command fails part way (it is
    /// refused, the connection goes), what arrived before has been handed over all the same.
    pub fn uid_fetch_messages(&mut self, set: &str, header_only: bool, on_message: &mut dyn FnMut(Fetched)) -> Result<(), ProtoError> {
        let items = if header_only { "(UID FLAGS INTERNALDATE BODY.PEEK[HEADER])" } else { "(UID FLAGS INTERNALDATE BODY.PEEK[])" };
        let command = Self::fetch(set, items)?;
        // The set's messages one by one, when it names them so (`1:*` doesn't).
        let wanted: Option<HashSet<u32>> = uids_of(set).map(|uids| uids.into_iter().collect());
        let mut handed: HashSet<u32> = HashSet::new();
        // What responses without the message said of it, for the one with the message.
        let mut said: BTreeMap<u32, (Option<Vec<String>>, Option<String>)> = BTreeMap::new();
        self.run(command, &mut |values| {
            let Some(item) = Item::from(values) else { return };
            let uid = item.uid;
            if uid == 0 || wanted.as_ref().is_some_and(|wanted| !wanted.contains(&uid)) || handed.contains(&uid) {
                return;
            }
            let asked_for = if header_only { item.header } else { item.body };
            match asked_for {
                Some(body) => {
                    let (flags, date) = said.remove(&uid).unwrap_or_default();
                    handed.insert(uid);
                    on_message(Fetched {
                        uid,
                        flags: item.flags.or(flags).unwrap_or_default(),
                        internal_date: item.internal_date.or(date),
                        body: Some(body),
                    });
                }
                // Kept for so many messages only: a server can't make this grow without end.
                None if said.len() < MAX_SET_UIDS || said.contains_key(&uid) => {
                    let entry = said.entry(uid).or_default();
                    if item.flags.is_some() {
                        entry.0 = item.flags;
                    }
                    if item.internal_date.is_some() {
                        entry.1 = item.internal_date;
                    }
                }
                None => {}
            }
        })?;
        Ok(())
    }

    /// One message with its flags and date, and either all of it or (`header_only`) its header.
    fn fetch_one(&mut self, uid: u32, items: &str, header_only: bool) -> Result<Option<Fetched>, ProtoError> {
        if uid == 0 {
            return Ok(None);
        }
        let mut found: Option<Fetched> = None;
        self.run(Self::fetch(&uid.to_string(), items)?, &mut |values| {
            // Other messages may be mentioned in passing; this one may come in more than one response.
            if let Some(item) = Item::from(values)
                && item.uid == uid
            {
                let message = found.get_or_insert_with(|| Fetched { uid, ..Default::default() });
                if let Some(flags) = item.flags {
                    message.flags = flags;
                }
                message.internal_date = item.internal_date.or(message.internal_date.take());
                // Only what was asked for: a whole message in answer to a question about its
                // header is not taken in.
                let asked_for = if header_only { item.header } else { item.body };
                message.body = asked_for.or(message.body.take());
            }
        })?;
        Ok(found.filter(|message| message.body.is_some()))
    }

    /// UID STORE <uid> +FLAGS.SILENT / -FLAGS.SILENT (flags).
    pub fn uid_store(&mut self, uid: u32, add: bool, flags: &[&str]) -> Result<(), ProtoError> {
        let flags = flag_list(flags)?;
        if uid == 0 {
            return Err(ProtoError::Protocol("a message that isn't on the server has no flags there".into()));
        }
        self.store(&uid.to_string(), add, &flags)
    }

    /// UID STORE <set> +FLAGS.SILENT / -FLAGS.SILENT (flags): the same change for every message
    /// the set names (number by number, as [`uid_sets`] writes it; `1:*` is refused). A UID that
    /// is no longer there is passed over by the server without a word.
    pub fn uid_store_set(&mut self, set: &str, add: bool, flags: &[&str]) -> Result<(), ProtoError> {
        let flags = flag_list(flags)?;
        self.store(named(set)?, add, &flags)
    }

    fn store(&mut self, set: &str, add: bool, flags: &str) -> Result<(), ProtoError> {
        // No flags: nothing to ask of the server.
        if flags == "()" {
            return Ok(());
        }
        let command = Command::new("UID STORE").arg(set).arg(if add { "+FLAGS.SILENT" } else { "-FLAGS.SILENT" }).arg(flags);
        self.run(command, &mut ignore).map(|_| ())
    }

    /// UID MOVE <set> <mailbox> (RFC 6851, for a server that offers `MOVE`): the messages leave
    /// the open mailbox for the named one in one step. Answers where they are now when the
    /// server says so beyond doubt (`COPYUID`), `None` otherwise.
    pub fn uid_move(&mut self, set: &str, raw_name: &str) -> Result<Option<Copied>, ProtoError> {
        self.transfer("UID MOVE", set, raw_name)
    }

    /// UID COPY <set> <mailbox>: copies of the messages are added to the named mailbox. Answers
    /// like [`uid_move`](Self::uid_move).
    pub fn uid_copy(&mut self, set: &str, raw_name: &str) -> Result<Option<Copied>, ProtoError> {
        self.transfer("UID COPY", set, raw_name)
    }

    fn transfer(&mut self, verb: &str, set: &str, raw_name: &str) -> Result<Option<Copied>, ProtoError> {
        let command = Command::new(verb).arg(named(set)?).string(raw_name.as_bytes());
        self.copied.clear();
        let done = self.run(command, &mut ignore);
        // One answer that could be read, or nothing is known: two that might disagree are none.
        let copied = match std::mem::take(&mut self.copied).as_mut_slice() {
            [only] => only.take(),
            _ => None,
        };
        done.map(|_| copied)
    }

    /// UID EXPUNGE <set> (RFC 4315): the messages of the set that are marked `\Deleted` go for
    /// good, and no others. Refused unsent unless the server has said `UIDPLUS`: a server
    /// without it only knows the EXPUNGE that takes every marked message of the mailbox.
    pub fn uid_expunge(&mut self, set: &str) -> Result<(), ProtoError> {
        let set = named(set)?;
        if !self.offers("UIDPLUS") {
            return Err(ProtoError::Protocol("the server can't remove single messages (it doesn't offer UIDPLUS)".into()));
        }
        self.run(Command::new("UID EXPUNGE").arg(set), &mut ignore).map(|_| ())
    }

    /// CREATE <mailbox>. `raw_name` is the name as the server wants it: in modified UTF-7
    /// ([`encode_utf7`]), its levels joined with the server's own delimiter.
    pub fn create(&mut self, raw_name: &str) -> Result<(), ProtoError> {
        self.run(Command::new("CREATE").string(raw_name.as_bytes()), &mut ignore).map(|_| ())
    }

    /// SUBSCRIBE <mailbox>: other mail programs list subscribed mailboxes only.
    pub fn subscribe(&mut self, raw_name: &str) -> Result<(), ProtoError> {
        self.run(Command::new("SUBSCRIBE").string(raw_name.as_bytes()), &mut ignore).map(|_| ())
    }

    /// UNSUBSCRIBE <mailbox>: for a mailbox that is gone or has another name now.
    pub fn unsubscribe(&mut self, raw_name: &str) -> Result<(), ProtoError> {
        self.run(Command::new("UNSUBSCRIBE").string(raw_name.as_bytes()), &mut ignore).map(|_| ())
    }

    /// RENAME <mailbox> <new name> (RFC 3501 6.3.5): both names as the server wants them, as for
    /// [`Imap::create`]. The mailboxes inside it go with it.
    pub fn rename(&mut self, raw_from: &str, raw_to: &str) -> Result<(), ProtoError> {
        self.run(Command::new("RENAME").string(raw_from.as_bytes()).string(raw_to.as_bytes()), &mut ignore).map(|_| ())
    }

    /// DELETE <mailbox> (RFC 3501 6.3.4): the mailbox and every message in it go for good.
    pub fn delete(&mut self, raw_name: &str) -> Result<(), ProtoError> {
        self.run(Command::new("DELETE").string(raw_name.as_bytes()), &mut ignore).map(|_| ())
    }

    /// APPEND with a literal (handles the `+` continuation).
    pub fn append(&mut self, raw_name: &str, flags: &[&str], message: &[u8]) -> Result<(), ProtoError> {
        self.append_uid(raw_name, flags, message).map(|_| ())
    }

    /// [`append`](Self::append), answering where the message went when the server says so
    /// (`APPENDUID`, RFC 4315): the mailbox's `UIDVALIDITY` and the message's UID there.
    pub fn append_uid(&mut self, raw_name: &str, flags: &[&str], message: &[u8]) -> Result<Option<(u32, u32)>, ProtoError> {
        let mut command = Command::new("APPEND").string(raw_name.as_bytes());
        if !flags.is_empty() {
            command = command.arg(&flag_list(flags)?);
        }
        self.appended = None;
        self.run(command.literal(message), &mut ignore)?;
        Ok(self.appended.take())
    }

    /// UID SEARCH HEADER Message-ID <id>: the UIDs of the open mailbox's messages whose
    /// Message-ID header has `message_id` in it (given with or without its angle brackets), at
    /// most [`MAX_FOUND`] of them, in UID order. An id that can't be sent is refused unsent.
    pub fn uid_search_message_id(&mut self, message_id: &str) -> Result<Vec<u32>, ProtoError> {
        let id = message_id.trim().trim_start_matches('<').trim_end_matches('>').trim();
        if id.is_empty() || id.len() > MAX_QUOTED || !id.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(ProtoError::Protocol("that Message-ID can't be searched for".into()));
        }
        let mut found: Vec<u32> = Vec::new();
        self.run(Command::new("UID SEARCH HEADER Message-ID").string(id.as_bytes()), &mut |values| {
            let mut values = values.into_iter();
            if !values.next().is_some_and(|v| v.atom().is_some_and(|a| a.eq_ignore_ascii_case("SEARCH"))) {
                return;
            }
            for value in values {
                if found.len() >= MAX_FOUND {
                    break;
                }
                if let Some(uid) = value.atom().and_then(number).filter(|uid| *uid != 0) {
                    found.push(uid);
                }
            }
        })?;
        found.sort_unstable();
        found.dedup();
        Ok(found)
    }

    /// IDLE (RFC 2177): ask the server to say when the open mailbox changes, and wait for its
    /// go-ahead. What it says then comes from [`idle_wait`](Self::idle_wait), and
    /// [`idle_done`](Self::idle_done) ends it. Refused unsent unless the server has said `IDLE`.
    pub fn idle(&mut self) -> Result<(), ProtoError> {
        if !self.offers("IDLE") {
            return Err(ProtoError::Protocol("the server can't wait with news (it doesn't offer IDLE)".into()));
        }
        if self.idling.is_some() {
            return Err(ProtoError::Protocol("already waiting for news".into()));
        }
        let tag = self.tag();
        log::trace!("C: {tag} IDLE");
        self.wire.write(format!("{tag} IDLE\r\n").as_bytes())?;
        self.wire.flush()?;
        match self.read_until(&tag, true, &mut ignore)? {
            None => {
                self.idling = Some(tag);
                Ok(())
            }
            // Ended before it began.
            Some(line) if line.status == Status::Ok => Err(ProtoError::Protocol("the server ended the command early".into())),
            Some(line) => Err(ProtoError::Rejected(self.refusal(&line))),
        }
    }

    /// The next thing the server says while it waits with news: [`IdleNews::Quiet`] when the
    /// stream's wait for a read ran out first (the caller sets how long that is), which leaves
    /// the session as it was. A server that ends the session, or the IDLE, is an error.
    pub fn idle_wait(&mut self) -> Result<IdleNews, ProtoError> {
        let Some(tag) = self.idling.clone() else { return Err(ProtoError::Protocol("not waiting for news".into())) };
        let Some(line) = self.wire.poll_line()? else { return Ok(IdleNews::Quiet) };
        match self.response_from(line)? {
            Response::Data(values) => {
                self.note_data(&values);
                let word = values.get(1).and_then(Value::atom).map(str::to_ascii_uppercase);
                Ok(match word.as_deref() {
                    Some("EXISTS" | "EXPUNGE") => IdleNews::Changed,
                    _ => IdleNews::Other,
                })
            }
            Response::Status { tag: None, line } if line.status == Status::Bye => {
                self.idling = None;
                Err(ProtoError::Io(format!("the server ended the session: {}", self.refusal(&line))))
            }
            Response::Status { tag: None, line } => {
                self.note_status(&line);
                Ok(IdleNews::Other)
            }
            Response::Status { tag: Some(answered), line } => {
                self.idling = None;
                if answered != tag {
                    return Err(ProtoError::Protocol("the server answered a command that wasn't sent".into()));
                }
                Err(ProtoError::Protocol(format!("the server stopped waiting with news: {}", self.refusal(&line))))
            }
            Response::GoAhead => Err(ProtoError::Protocol("the server asked for more than the command had".into())),
        }
    }

    /// DONE: end the IDLE under way, and read up to its answer. Nothing to do when none is.
    pub fn idle_done(&mut self) -> Result<(), ProtoError> {
        let Some(tag) = self.idling.take() else { return Ok(()) };
        log::trace!("C: DONE");
        self.wire.write(b"DONE\r\n")?;
        self.wire.flush()?;
        match self.read_until(&tag, false, &mut ignore)? {
            Some(line) if line.status == Status::Ok => Ok(()),
            Some(line) => Err(ProtoError::Rejected(self.refusal(&line))),
            None => Err(ProtoError::Protocol("the server asked for more than the command had".into())),
        }
    }

    /// LOGOUT. A server that hangs up without answering has logged us out all the same.
    pub fn logout(mut self) -> Result<(), ProtoError> {
        self.leaving = true;
        match self.run(Command::new("LOGOUT"), &mut ignore) {
            Ok(_) | Err(ProtoError::Io(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn fetch(set: &str, items: &str) -> Result<Command<'static>, ProtoError> {
        if !is_set(set) {
            return Err(ProtoError::Protocol(format!("\"{}\" is not a set of messages", tidy(set))));
        }
        Ok(Command::new("UID FETCH").arg(set).arg(items))
    }

    /// The tag of the next command.
    fn tag(&mut self) -> String {
        let tag = format!("A{:04}", self.next_tag);
        self.next_tag = self.next_tag.wrapping_add(1);
        tag
    }

    /// Run a command that must end in OK; untagged data goes to `on_data` as it arrives.
    fn run(&mut self, command: Command<'_>, on_data: &mut dyn FnMut(Vec<Value>)) -> Result<StatusLine, ProtoError> {
        let line = self.exchange(command, on_data)?;
        match line.status {
            Status::Ok => Ok(line),
            _ => Err(ProtoError::Rejected(self.refusal(&line))),
        }
    }

    /// Send a command and read up to its tagged answer: OK, NO or BAD.
    fn exchange(&mut self, command: Command<'_>, on_data: &mut dyn FnMut(Vec<Value>)) -> Result<StatusLine, ProtoError> {
        if command.unsendable {
            return Err(ProtoError::Protocol("the text has a character that can't be sent".into()));
        }
        let tag = self.tag();
        if log::log_enabled!(log::Level::Trace) {
            log::trace!("C: {tag} {}", self.secrets.hide(&command.for_log()));
        }
        let mut out = Vec::with_capacity(128);
        out.extend_from_slice(tag.as_bytes());
        out.push(b' ');
        for part in &command.parts {
            match part {
                Part::Text(text) => out.extend_from_slice(text),
                Part::Literal(bytes) => {
                    // A literal is announced, and only sent once the server says `+`.
                    out.extend_from_slice(format!("{{{}}}\r\n", bytes.len()).as_bytes());
                    self.wire.write(&out)?;
                    self.wire.flush()?;
                    out.clear();
                    match self.read_until(&tag, true, on_data)? {
                        None => {}
                        // The command can't have succeeded without the rest of it.
                        Some(line) if line.status == Status::Ok => return Err(ProtoError::Protocol("the server ended the command early".into())),
                        Some(refused) => return Ok(refused),
                    }
                    self.wire.write(bytes)?;
                }
            }
        }
        out.extend_from_slice(b"\r\n");
        self.wire.write(&out)?;
        self.wire.flush()?;
        match self.read_until(&tag, false, on_data)? {
            Some(line) => Ok(line),
            None => Err(ProtoError::Protocol("the server asked for more than the command had".into())),
        }
    }

    /// Read responses up to the command's tagged answer (`Some`) or, when a literal is waiting to
    /// be sent, the server's go-ahead (`None`).
    fn read_until(&mut self, tag: &str, literal_waiting: bool, on_data: &mut dyn FnMut(Vec<Value>)) -> Result<Option<StatusLine>, ProtoError> {
        for _ in 0..MAX_RESPONSES {
            match self.read_response()? {
                Response::GoAhead if literal_waiting => return Ok(None),
                Response::GoAhead => return Err(ProtoError::Protocol("the server asked for more than the command had".into())),
                Response::Status { tag: Some(answered), line } => {
                    if answered != tag || !matches!(line.status, Status::Ok | Status::No | Status::Bad) {
                        return Err(ProtoError::Protocol("the server answered a command that wasn't sent".into()));
                    }
                    self.note_status(&line);
                    return Ok(Some(line));
                }
                Response::Status { tag: None, line } => {
                    if line.status == Status::Bye && !self.leaving {
                        return Err(ProtoError::Io(format!("the server ended the session: {}", self.refusal(&line))));
                    }
                    self.note_status(&line);
                }
                Response::Data(values) => {
                    self.note_data(&values);
                    on_data(values);
                }
            }
        }
        Err(ProtoError::Protocol("the server sent too many responses to one command".into()))
    }

    fn read_response(&mut self) -> Result<Response, ProtoError> {
        let line = self.wire.read_line()?;
        self.response_from(line)
    }

    /// The response that begins with `line` (read already), with the literals that follow it.
    fn response_from(&mut self, line: Vec<u8>) -> Result<Response, ProtoError> {
        if log::log_enabled!(log::Level::Trace) {
            log::trace!("S: {}", self.secrets.hide(&String::from_utf8_lossy(&line)));
        }
        if line.first() == Some(&b'+') {
            return Ok(Response::GoAhead);
        }
        let Some(rest) = line.strip_prefix(b"* ") else {
            // Tagged: `A0001 OK done`.
            let text = String::from_utf8_lossy(&line);
            let mut words = text.splitn(3, ' ');
            let (tag, word, rest) = (words.next().unwrap_or(""), words.next().unwrap_or(""), words.next().unwrap_or(""));
            return match Status::parse(word) {
                Some(status) if !tag.is_empty() && tag != "*" => {
                    Ok(Response::Status { tag: Some(tag.to_string()), line: StatusLine::new(status, rest) })
                }
                // Before the greeting it is the wrong kind of server; after it, a server that has
                // stopped speaking IMAP (Exchange Online writes a bare `Server Unavailable. 15`
                // when it throttles, and hangs up): the session is over, and the connection is
                // as good as lost, to be tried again later rather than command by command.
                _ if !self.greeted => Err(ProtoError::Protocol(format!("\"{}\" is not an IMAP response", tidy(&self.secrets.hide(&text))))),
                _ => Err(ProtoError::Io(format!("the server closed the connection ({})", tidy(&self.secrets.hide(&text))))),
            };
        };
        // `* OK …`, `* BYE …`: the rest is text for people, where `{5}` is no literal.
        let word_end = rest.iter().position(|b| *b == b' ').unwrap_or(rest.len());
        let word = String::from_utf8_lossy(rest.get(..word_end).unwrap_or(&[]));
        if let Some(status) = Status::parse(&word) {
            let text = String::from_utf8_lossy(rest.get(word_end..).unwrap_or(&[]));
            return Ok(Response::Status { tag: None, line: StatusLine::new(status, &text) });
        }
        // Data: a line, and for as long as a line ends in `{n}`, n bytes and another line.
        let mut pieces = Vec::new();
        let mut text = rest.to_vec();
        let (mut text_bytes, mut literal_bytes) = (text.len(), 0usize);
        for _ in 0..=MAX_LITERALS {
            let Some((_, claimed)) = parse::literal_marker(&text) else {
                pieces.push(Piece::Text(text));
                return Ok(Response::Data(parse::values(pieces)?));
            };
            let size = usize::try_from(claimed).ok().filter(|n| literal_bytes.saturating_add(*n) <= MAX_LITERAL).ok_or_else(|| {
                ProtoError::Protocol(format!(
                    "the server sent {claimed} bytes in one piece, more than the {} MB MailCraft accepts",
                    MAX_LITERAL >> 20
                ))
            })?;
            let literal = self.wire.read_block(size)?;
            literal_bytes = literal_bytes.saturating_add(size);
            log::trace!("S: ({size} bytes)");
            pieces.push(Piece::Text(text));
            pieces.push(Piece::Literal(literal));
            text = self.wire.read_line()?;
            text_bytes = text_bytes.saturating_add(text.len());
            if text_bytes > MAX_TEXT {
                return Err(ProtoError::Protocol("the server's response is too long".into()));
            }
        }
        Err(ProtoError::Protocol("the server's response has too many parts".into()))
    }

    /// Keep what a status line says in its response code.
    fn note_status(&mut self, line: &StatusLine) {
        match line.code.as_str() {
            "UIDVALIDITY" => self.selected.uid_validity = line.arg.parse().unwrap_or(0),
            "UIDNEXT" => self.selected.uid_next = line.arg.parse().unwrap_or(0),
            "COPYUID" if self.copied.len() < 2 => self.copied.push(copied_from(&line.arg)),
            "APPENDUID" => self.appended = appended_from(&line.arg),
            "PERMANENTFLAGS" => {
                let inner = line.arg.trim().trim_start_matches('(').trim_end_matches(')');
                self.selected.permanent_flags = Some(inner.split_whitespace().take(MAX_FLAGS).map(str::to_string).collect());
            }
            "CAPABILITY" => self.capabilities = Some(line.arg.split_whitespace().take(MAX_CAPABILITIES).map(str::to_ascii_uppercase).collect()),
            _ => {}
        }
    }

    /// Keep what untagged data says about the session, whichever command it arrives during.
    fn note_data(&mut self, values: &[Value]) {
        match (values.first().and_then(Value::atom), values.get(1).and_then(Value::atom)) {
            (Some(count), Some(word)) if word.eq_ignore_ascii_case("EXISTS") => self.selected.exists = count.parse().unwrap_or(0),
            (Some(word), _) if word.eq_ignore_ascii_case("CAPABILITY") => {
                self.capabilities = Some(values.iter().skip(1).filter_map(Value::atom).take(MAX_CAPABILITIES).map(str::to_ascii_uppercase).collect());
            }
            _ => {}
        }
    }

    /// The server's own words for why it said no, fit to show.
    fn refusal(&self, line: &StatusLine) -> String {
        // Hidden before it is shortened: a secret cut in half would no longer be recognised.
        let text = tidy(&self.secrets.hide(&line.text));
        if text.is_empty() { "the command was refused".into() } else { text }
    }
}

#[cfg(test)]
mod tests;
