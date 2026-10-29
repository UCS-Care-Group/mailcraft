// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mail message.

use std::sync::Arc;

use mailcraft_time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::sync::{MovingOut, RemoteFlags, ServerMessage};
use crate::{Address, Id};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Importance {
    Low,
    #[default]
    Normal,
    High,
}

impl Importance {
    pub fn parse(s: &str) -> Option<Importance> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" | "5" | "non-urgent" => Some(Importance::Low),
            "normal" | "3" | "medium" => Some(Importance::Normal),
            "high" | "1" | "urgent" => Some(Importance::High),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Importance::Low => "low",
            Importance::Normal => "normal",
            Importance::High => "high",
        }
    }
}

/// Follow-up flag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlagStatus {
    #[default]
    None,
    Flagged,
    Complete,
}

/// Where the bytes of a part of a message are while they aren't in memory: a content file beside
/// the mailbox file, named by the SHA-256 of its bytes and checked against its length (the
/// engine's `store`). A mailbox a program works on keeps attachments, and the bodies of older
/// mail, this way, and brings them in when something needs them (`Session::content`). Never
/// written to a file: the mailbox file names the content in its own way.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentRef {
    pub sha256: [u8; 32],
    pub len: u64,
}

impl std::fmt::Debug for ContentRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ContentRef({} bytes)", self.len)
    }
}

/// A file carried by a message.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Attachment {
    pub name: String,
    /// Media type (`application/pdf`).
    pub mime: String,
    /// `Content-ID` without angle brackets, for pictures the HTML body shows (`cid:`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content_id: String,
    /// Shown inside the body rather than in the attachment list.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub inline: bool,
    /// The bytes, when they are in memory. Empty for a file that is kept outside (`kept`) until
    /// something asks for it.
    #[serde(with = "crate::b64")]
    pub data: Arc<Vec<u8>>,
    /// Where the bytes are when `data` doesn't hold them. `None` for a file that is whole in
    /// memory (one that was just made, or brought in). Not written: the mailbox file has its
    /// own way of naming content, and a self-contained file carries the bytes.
    #[serde(skip)]
    pub kept: Option<ContentRef>,
}

impl Attachment {
    pub fn new(name: impl Into<String>, mime: impl Into<String>, data: Vec<u8>) -> Attachment {
        Attachment { name: name.into(), mime: mime.into(), content_id: String::new(), inline: false, data: Arc::new(data), kept: None }
    }

    /// How many bytes the file is, in memory or where it is kept.
    pub fn size(&self) -> usize {
        match self.kept {
            Some(k) if self.data.is_empty() => usize::try_from(k.len).unwrap_or(usize::MAX),
            _ => self.data.len(),
        }
    }

    /// Whether its bytes are in memory (`data` is the file). `false` for one kept outside and not
    /// brought in yet, whose bytes `Session::content` fetches.
    pub fn is_loaded(&self) -> bool {
        self.kept.is_none() || !self.data.is_empty()
    }

    /// `12 KB`, `3.4 MB`.
    pub fn size_label(&self) -> String {
        size_label(self.size() as u64)
    }
}

/// A hash of a body's text, to know it again within one run ([`KeptBody::text_hash`]).
pub fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// The header `mailcraft-net` gives a message too large to download, with its size on the server.
pub const NOT_DOWNLOADED: &str = "X-MailCraft-Not-Downloaded";
/// How the note `mailcraft-net` keeps in place of such a message's text begins.
pub const NOT_DOWNLOADED_NOTE: &str = "This message is too large to download:";

/// A byte count for people.
pub fn size_label(bytes: u64) -> String {
    size_label_in(mailcraft_i18n::Lang::English, bytes)
}

/// [`size_label`] in `lang` (its units, and its decimal mark: the template has it).
pub fn size_label_in(lang: mailcraft_i18n::Lang, bytes: u64) -> String {
    const KB: u64 = 1024;
    // Tenths of `unit`, rounded.
    let tenths = |unit: u64| (u128::from(bytes) * 10 + u128::from(unit / 2)) / u128::from(unit);
    let (template, n, tenth) = if bytes < KB {
        (lang.tn(bytes, "{n} byte", "{n} bytes"), u128::from(bytes), 0)
    } else if bytes < KB * KB {
        (lang.t("{n} KB"), u128::from(bytes.div_ceil(KB)), 0)
    } else if bytes < KB * KB * KB {
        let t = tenths(KB * KB);
        (lang.t("{n}.{tenth} MB"), t / 10, t % 10)
    } else {
        let t = tenths(KB * KB * KB);
        (lang.t("{n}.{tenth} GB"), t / 10, t % 10)
    };
    mailcraft_i18n::fmt(template, &[("n", &n.to_string()), ("tenth", &tenth.to_string())])
}

/// A message body. `text` is always there (readers derive it from the HTML when a message has no
/// plain part); `html` is the original HTML when the message has one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Body {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Message {
    pub id: Id,
    pub account: Id,
    pub folder: Id,
    /// `Message-ID` without angle brackets.
    pub message_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub in_reply_to: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<String>,
    pub from: Address,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reply_to: Vec<Address>,
    pub to: Vec<Address>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<Address>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<Address>,
    pub subject: String,
    /// When it was sent (`Date:`).
    pub date: Timestamp,
    /// When it arrived here.
    pub received: Timestamp,
    pub body: Body,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    pub read: bool,
    pub flag: FlagStatus,
    /// Follow-up due date of a flagged message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flag_due: Option<Timestamp>,
    pub importance: Importance,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    pub answered: bool,
    pub forwarded: bool,
    pub draft: bool,
    pub pinned: bool,
    /// The kinds of sender warning ([`crate::sender_warnings::Warning::kind`]) the owner dismissed
    /// the banner for (#508). The banner is hidden while every kind it would show is among these;
    /// a kind that appears later brings it back.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub warnings_dismissed: Vec<String>,
    /// Headers MailCraft doesn't model, kept so a saved message round-trips.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<(String, String)>,
    /// The message's UID in the server mailbox of the folder it is in (IMAP); 0 for a message
    /// that isn't on the server there (yet).
    #[serde(skip_serializing_if = "is_zero")]
    pub uid: u32,
    /// The message's flags as the server last had them under `uid`: as last seen there, or as
    /// last pushed. A flag that differs was changed here and is still to be pushed; one that
    /// doesn't follows the server. `None`: the server hasn't been asked about this UID yet.
    ///
    /// With `uid` 0 it marks a message that is in its folder's server mailbox without its UID
    /// there being known (it was just sent, or moved by a server that didn't say where to): the
    /// copy that comes down is matched to it by its Message-ID instead of being added.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub synced: Option<RemoteFlags>,
    /// For an IMAP message too large to download, of which only the header is here: its size on
    /// the server in bytes, as the server gave it (`RFC822.SIZE`); 0 for every other message.
    /// A synchronisation sets it from what the server says; unlike the header [`NOT_DOWNLOADED`],
    /// which any sender can write into a message, nothing in a message's MIME can. A mailbox file
    /// (one opened from elsewhere too) or a synchronisation result given to `sync.apply` can hold
    /// any size there, so it is a size to show, not a sign of where the message came from. A file
    /// from before has none, and one that holds something other than a size there reads as 0.
    #[serde(skip_serializing_if = "is_zero_u64", deserialize_with = "crate::sync::lenient_or_default")]
    pub not_downloaded: u64,
    /// For a message of a Microsoft account that the server has: its server id and what the
    /// server last said of it ([`ServerMessage`], ADR 0003). `None` for a message the server
    /// hasn't got (or hasn't said so yet), and for every message of any other account; `uid` and
    /// `synced` are IMAP's and stay as they are.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub server: Option<ServerMessage>,
    /// Only its headers and a preview are here: it is not **fetched** yet. Every
    /// message of a file from before this was kept is whole.
    #[serde(skip_serializing_if = "is_false")]
    pub unfetched: bool,
    /// While it is not fetched: the server said it has attachments (Graph's `hasAttachments`),
    /// which its headers and preview don't show. It wears the paperclip and `has:attachment`
    /// finds it; once it is fetched its own attachments say.
    #[serde(skip_serializing_if = "is_false")]
    pub attachments_on_server: bool,
    /// It was moved here, out of a Microsoft account (into a local folder or another account),
    /// from the server's copy this names, and that copy is still on the server: it is **being
    /// moved** (R3 spec S19). The server's copy is removed only once this message is fetched
    /// whole and the mailbox file holding it is saved. `None` for every other message.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub moving_out: Option<MovingOut>,
    /// Put back as its server's message when a copy of it in another account was refused (#451):
    /// how many more times that server is to list its folder with it still there before it is
    /// the server's message as any other ([`RETURNED_LISTINGS`] at first, since the first listing
    /// may have been asked for before it was put back). Should the server say meanwhile that it
    /// is gone (deleted elsewhere), it is kept on this computer instead of going with it.
    #[serde(skip_serializing_if = "none_left")]
    pub returned: u8,
    /// The marker of the copy of it put on its account's IMAP server from elsewhere, while that
    /// server hasn't said where the copy is (#451): only a message there that carries it is
    /// taken for that copy. Empty for every other message.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub placed: String,
    /// The server's copy was gone before it could be fetched: only the headers and preview it
    /// had are here, kept as they are, and nothing will fetch the rest (R3 spec S19).
    #[serde(skip_serializing_if = "is_false")]
    pub incomplete: bool,
    /// For a draft of a Microsoft account: its marker on the server, what the server last had
    /// of it, and whether it waits to go up (R3.4.1). `None` for every other message.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub server_draft: Option<crate::sync::ServerDraft>,
    /// For a message of a Microsoft account's Outbox that is **held** (R3.4.2): what happened and
    /// what to check. It isn't sent again until the owner says so. Empty for every other message.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub held: String,
    /// For a reply or forward written here and not sent yet: the message it answers or forwards,
    /// which is marked so once this has gone (#154), not when Send is pressed. `None` for every
    /// other message, and once it is sent.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub answers: Option<Answers>,
    /// Where the body is while it isn't in memory ([`KeptBody`]); `None` once it is (or when it
    /// always was). Not written: see [`Attachment::kept`].
    #[serde(skip)]
    pub kept: Option<Box<KeptBody>>,
}

/// The message a reply or forward not sent yet answers or forwards ([`Message::answers`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Answers {
    /// The original's id in this mailbox.
    pub message: Id,
    /// The original's `Message-ID`, when it has one: a message under that id that isn't it (the
    /// mailbox was put together otherwise since) isn't marked.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message_id: String,
    /// It is a forward (the original is marked forwarded); otherwise a reply (marked replied).
    #[serde(skip_serializing_if = "is_false")]
    pub forward: bool,
    /// A forward of several messages, each attached whole (#360): the others it forwards, each
    /// marked as its own entry says (their own `also` is not looked at). Empty for every other
    /// reply or forward; a build from before reads the first original alone.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub also: Vec<Answers>,
}

/// How many listings of its folder a message put back after a refused copy waits for
/// ([`Message::returned`]).
pub const RETURNED_LISTINGS: u8 = 2;

fn none_left(n: &u8) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Where a message's text and HTML are while they aren't in memory ([`Message::kept`]): the body
/// of a message older than the memory a mailbox is given at opening, brought in when the message
/// is shown, searched, answered or exported. While it is kept, `body.text` is empty (or says the
/// text is unavailable, once bringing it in failed) and `body.html` is `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeptBody {
    pub text: Option<ContentRef>,
    pub html: Option<ContentRef>,
    /// Bringing it in was tried and failed (the file is missing or damaged): the text says so,
    /// and nothing tries again until the mailbox is opened anew.
    pub failed: bool,
    /// [`text_hash`] of `body.text` as the mailbox was opened (a short text is in the mailbox
    /// file itself, with its HTML kept outside): the kept HTML goes with that text and no other.
    pub text_hash: u64,
}

impl KeptBody {
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.html.is_none()
    }

    /// Whether `text` is the text the mailbox was opened with.
    pub fn same_text(&self, text: &str) -> bool {
        self.text_hash == text_hash(text)
    }

    /// How many bytes the kept text and HTML come to.
    pub fn len(&self) -> u64 {
        self.text.map_or(0, |r| r.len).saturating_add(self.html.map_or(0, |r| r.len))
    }
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}

impl Message {
    /// Every recipient: To, Cc, Bcc.
    pub fn recipients(&self) -> impl Iterator<Item = &Address> {
        self.to.iter().chain(&self.cc).chain(&self.bcc)
    }

    /// Whether the attachment list has something, or, for a message not fetched yet, the server
    /// said it will ([`Message::attachments_on_server`]).
    pub fn has_attachments(&self) -> bool {
        (self.unfetched && self.attachments_on_server) || self.listed_attachments().next().is_some()
    }

    /// The attachments the attachment list shows, with their index: everything except pictures
    /// the HTML body shows in place (`cid:` references). A file merely marked "inline" by the
    /// sending program, which nothing in the body refers to, is still listed. While the HTML is
    /// kept outside memory ([`Message::kept`]), an inline picture with a `Content-ID` is taken to
    /// be one it shows, until the HTML is read and says.
    pub fn listed_attachments(&self) -> impl Iterator<Item = (usize, &Attachment)> {
        let html = self.body.html.as_deref().unwrap_or("");
        let html_kept = self.body.html.is_none() && self.kept.as_ref().is_some_and(|k| k.html.is_some() && !k.failed);
        self.attachments
            .iter()
            .enumerate()
            .filter(move |(_, a)| !(a.inline && !a.content_id.is_empty() && (html_kept || html.contains(&format!("cid:{}", a.content_id)))))
    }

    /// The subject without `Re:` / `Fw:` prefixes: what a conversation is about.
    pub fn topic(&self) -> String {
        normalize_subject(&self.subject)
    }

    /// The key messages of one conversation share (the topic, case-folded); a message without a
    /// subject is its own conversation.
    pub fn thread_key(&self) -> String {
        let t = self.topic().to_lowercase();
        if t.is_empty() { format!("\u{0}{}", self.id) } else { t }
    }

    /// Its size as the message list shows and sorts it (#273, #300): an IMAP message too large to
    /// download has its size on the server ([`Message::not_downloaded`]); a Microsoft account's
    /// message of which only the headers and a preview are here has the size its server said, or
    /// that of the whole message when it was fetched; otherwise [`Message::size`], of what is here
    /// (a whole message marked to be fetched again keeps its body until then). `None` for one
    /// with only the header of an IMAP message too large to download, as a file from before
    /// `not_downloaded` has it: the size that header gives is not believed, since any sender can
    /// write a header of that name into a message.
    pub fn list_size(&self) -> Option<u64> {
        if self.not_downloaded > 0 {
            return Some(self.not_downloaded);
        }
        let said = self.server.as_ref().map(|s| s.size).filter(|size| *size > 0);
        if let Some(size) = said.filter(|_| self.unfetched || self.incomplete) {
            return Some(size);
        }
        if self.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case(NOT_DOWNLOADED)) {
            return None;
        }
        Some(self.size())
    }

    /// Size in bytes, roughly: text, HTML and attachments, in memory or where they are kept.
    pub fn size(&self) -> u64 {
        let kept = self.kept.as_deref();
        let text = if self.body.text.is_empty() { kept.and_then(|k| k.text).map_or(0, |r| r.len) } else { self.body.text.len() as u64 };
        let html = match &self.body.html {
            Some(h) => h.len() as u64,
            None => kept.and_then(|k| k.html).map_or(0, |r| r.len),
        };
        self.attachments.iter().map(|a| a.size() as u64).fold(text.saturating_add(html), u64::saturating_add)
    }

    /// Whether some of the body is kept outside memory and not brought in yet ([`Message::kept`]):
    /// its text is empty, or its HTML missing, until it is. Not so once bringing it in failed
    /// (the text says so then).
    pub fn body_is_kept(&self) -> bool {
        self.kept
            .as_ref()
            .is_some_and(|k| !k.failed && ((k.text.is_some() && self.body.text.is_empty()) || (k.html.is_some() && self.body.html.is_none())))
    }

    /// Whether everything of the message is in memory: its body and every attachment.
    pub fn is_loaded(&self) -> bool {
        !self.body_is_kept() && self.attachments.iter().all(Attachment::is_loaded)
    }

    pub fn has_category(&self, name: &str) -> bool {
        self.categories.iter().any(|c| c.eq_ignore_ascii_case(name))
    }
}

/// Strip reply and forward prefixes (`Re:`, `RE:`, `Fw:`, `Fwd:`, `Re[2]:`; German `AW:`, `WG:`;
/// Spanish `RV:`; Italian `R:`, `I:`; French `TR :`), however many there are.
pub fn normalize_subject(subject: &str) -> String {
    let mut s = subject.trim();
    while let Some((prefix, rest)) = s.split_once(':') {
        let p = prefix.trim().to_ascii_lowercase();
        let p = p.split('[').next().unwrap_or("").trim();
        if matches!(p, "re" | "fw" | "fwd" | "aw" | "wg" | "sv" | "vs" | "tr" | "rv" | "r" | "i") {
            s = rest.trim();
        } else {
            break;
        }
    }
    s.to_string()
}

#[cfg(test)]
#[path = "message_size_tests.rs"]
mod size_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_normalize() {
        assert_eq!(normalize_subject("Re: RE: Fwd: Lunch on Friday"), "Lunch on Friday");
        assert_eq!(normalize_subject("Re[2]: AW: Plan"), "Plan");
        assert_eq!(normalize_subject("Reminder: dentist"), "Reminder: dentist");
        assert_eq!(normalize_subject("  re:  "), "");
        assert_eq!(normalize_subject("Agenda 10:30"), "Agenda 10:30");
        // French puts a space before the colon, often a narrow or plain no-break one.
        assert_eq!(normalize_subject("TR : RE\u{202f}: Fwd\u{a0}: tr: Déjeuner"), "Déjeuner");
        assert_eq!(normalize_subject("AW: WG: Mittagessen"), "Mittagessen");
        assert_eq!(normalize_subject("RE: RV: Almuerzo"), "Almuerzo");
        assert_eq!(normalize_subject("R: I: Pranzo"), "Pranzo");
        assert_eq!(normalize_subject("Ristorante: Pranzo"), "Ristorante: Pranzo");
    }

    #[test]
    fn threads_and_sizes() {
        let mut a = Message { id: 1, subject: "Budget".into(), ..Default::default() };
        let b = Message { id: 2, subject: "RE: budget".into(), ..Default::default() };
        assert_eq!(a.thread_key(), b.thread_key());
        a.subject.clear();
        assert_ne!(a.thread_key(), Message { id: 3, ..Default::default() }.thread_key());
        assert_eq!(size_label(900), "900 bytes");
        assert_eq!(size_label(1025), "2 KB");
        assert_eq!(size_label(5 * 1024 * 1024 / 2), "2.5 MB");
        assert_eq!(
            (size_label(1), size_label(1024 * 1024 * 1024 * 3 / 2), size_label(u64::MAX)),
            ("1 byte".into(), "1.5 GB".into(), "17179869184.0 GB".into())
        );
        let fr = mailcraft_i18n::test_support::po(&[("{n}.{tenth} MB", "{n},{tenth} Mo")]);
        mailcraft_i18n::test_support::with_catalogue(mailcraft_i18n::Lang::French, &fr, || {
            assert_eq!(size_label_in(mailcraft_i18n::Lang::French, 5 * 1024 * 1024 / 2), "2,5 Mo");
        });
        assert_eq!(Importance::parse("HIGH"), Some(Importance::High));
    }

    #[test]
    fn inline_files_nothing_refers_to_are_still_listed() {
        let picture = Attachment { content_id: "logo@x".into(), inline: true, ..Attachment::new("logo.png", "image/png", vec![1]) };
        let report = Attachment { inline: true, ..Attachment::new("report.pdf", "application/pdf", vec![2]) };
        let mut m = Message { attachments: vec![picture, report], ..Default::default() };
        m.body.html = Some("<p>Hello <img src=\"cid:logo@x\"></p>".into());
        assert_eq!(m.listed_attachments().map(|(i, a)| (i, a.name.as_str())).collect::<Vec<_>>(), [(1, "report.pdf")]);
        // Without a body that shows it, the picture is a file like any other.
        m.body.html = None;
        assert_eq!(m.listed_attachments().count(), 2);
        assert!(m.has_attachments() && !Message::default().has_attachments());
    }

    #[test]
    fn attachments_round_trip_as_base64() {
        let m = Message { attachments: vec![Attachment::new("a.bin", "application/octet-stream", vec![0, 255, 7])], ..Default::default() };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"data\":\"AP8H\""), "{json}");
        assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), m);
        assert!(serde_json::from_str::<Attachment>(r#"{"name":"x","data":"***"}"#).is_err());
    }

    #[test]
    fn what_is_kept_outside_memory_counts_and_is_never_written() {
        let r = |len: u64| ContentRef { sha256: [7; 32], len };
        let logo = Attachment { content_id: "logo@x.example".into(), inline: true, kept: Some(r(900)), ..Attachment::default() };
        let file = Attachment { name: "a.pdf".into(), kept: Some(r(5_000)), ..Attachment::default() };
        let mut m = Message {
            body: Body { text: "Short text.".into(), html: None },
            attachments: vec![logo, file],
            kept: Some(Box::new(KeptBody { html: Some(r(2_000)), text_hash: text_hash("Short text."), ..KeptBody::default() })),
            ..Message::default()
        };
        // Sizes are what they are on disk; nothing of it is in memory.
        assert_eq!((m.size(), m.attachments[0].size()), (11 + 2_000 + 900 + 5_000, 900));
        assert!(m.body_is_kept() && !m.is_loaded() && !m.attachments[1].is_loaded());
        // While the HTML waits, an inline picture with a Content-ID is taken for one it shows.
        assert_eq!(m.listed_attachments().map(|(i, _)| i).collect::<Vec<_>>(), vec![1]);
        // Written to a file, none of it is said (the mailbox file names content its own way).
        let json = serde_json::to_string(&m).unwrap();
        assert!(!json.contains("kept") && !json.contains("textHash"), "{json}");
        // Brought in, the HTML says what is shown; a read that failed is no longer waiting.
        m.body.html = Some("<p>No picture here.</p>".into());
        assert!(!m.body_is_kept() && m.listed_attachments().count() == 2);
        m.body.html = None;
        if let Some(k) = m.kept.as_deref_mut() {
            k.failed = true;
        }
        assert!(!m.body_is_kept() && m.listed_attachments().count() == 2);
    }
}
