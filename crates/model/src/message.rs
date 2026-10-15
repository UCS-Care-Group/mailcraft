//! A mail message.

use std::sync::Arc;

use mailcraft_time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::sync::RemoteFlags;
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

/// A byte count for people.
pub fn size_label(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} bytes")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// How a message is signed or encrypted (S/MIME, RFC 8551; OpenPGP, RFC 3156), as it came or as
/// it goes out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Protection {
    /// `multipart/signed` with `application/pkcs7-signature`: readable, and signed.
    SmimeSigned,
    /// `application/pkcs7-mime; smime-type=signed-data`: signed, the content inside the signature.
    SmimeSignedData,
    /// `application/pkcs7-mime; smime-type=enveloped-data`: encrypted.
    SmimeEncrypted,
    /// `multipart/signed` with `application/pgp-signature`.
    PgpSigned,
    /// `multipart/encrypted` with `application/pgp-encrypted`.
    PgpEncrypted,
}

impl Protection {
    /// Whether the content can only be read once it is opened with a key (or the signature
    /// unwrapped).
    pub fn is_opaque(self) -> bool {
        matches!(self, Protection::SmimeSignedData | Protection::SmimeEncrypted | Protection::PgpEncrypted)
    }

    pub fn is_encrypted(self) -> bool {
        matches!(self, Protection::SmimeEncrypted | Protection::PgpEncrypted)
    }

    pub fn is_smime(self) -> bool {
        matches!(self, Protection::SmimeSigned | Protection::SmimeSignedData | Protection::SmimeEncrypted)
    }
}

/// The signed or encrypted body of a message exactly as it came (or as it was made to go out):
/// its content fields and its content, so that a signature can be checked and an encrypted
/// message opened at any time, and the message written out again unchanged. What is encrypted
/// stays encrypted here; only the window, while it shows the message, holds what it says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Secured {
    pub kind: Protection,
    /// The MIME entity: `Content-Type` (and `Content-Transfer-Encoding`) fields, an empty line,
    /// the content; CRLF line ends.
    #[serde(with = "crate::b64")]
    pub raw: Arc<Vec<u8>>,
}

/// What the writer of a message asked for: sign it, encrypt it (kept with a draft).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Protect {
    pub sign: bool,
    pub encrypt: bool,
}

impl Protect {
    pub fn is_none(&self) -> bool {
        !self.sign && !self.encrypt
    }
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

/// Where a message's text and HTML are while they aren't in memory ([`Message::kept`]): the body
/// of a message older than the memory a mailbox is given at opening, brought in when the message
/// is shown, searched, answered, printed or exported. While it is kept, `body.text` is empty (or
/// says the text is unavailable, once bringing it in failed) and `body.html` is `None`.
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
    /// For an account whose mail is read through Microsoft Graph: the id Graph knows the message
    /// by (an immutable one, which a move doesn't change). The `uid` it has here is this
    /// program's own number for it in its folder. Empty for any other message.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote_id: String,
    /// A message in the Outbox that is not to go before this time (Delay Delivery): a
    /// synchronisation leaves it there until then.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub deliver_after: Option<Timestamp>,
    /// What was done about the sender's request for a read receipt, when the message has one
    /// ([`Message::receipt_to`]).
    #[serde(skip_serializing_if = "crate::ReceiptAnswer::is_pending", deserialize_with = "crate::sync::lenient_or_default")]
    pub receipt: crate::ReceiptAnswer,
    /// The categories the server last had as keywords on the message under `uid`
    /// ([`crate::sync::category_keyword`]): as last seen there, or as last pushed. A category
    /// that differs was given or taken here and is still to be pushed; one that doesn't follows
    /// the server. Only categories that can be keywords are ever in it.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub synced_categories: Vec<String>,
    /// The body as it came signed or encrypted ([`Secured`]); `None` for an ordinary message.
    /// For a signed message `body` and `attachments` are what it says; for an encrypted one they
    /// are empty until it is opened, and opening it changes nothing here.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub secured: Option<Secured>,
    /// A draft's Sign and Encrypt choices.
    #[serde(skip_serializing_if = "Protect::is_none", deserialize_with = "crate::sync::lenient_or_default")]
    pub protect: Protect,
    /// Why a message in the Outbox is not sent until the owner changes it or sends it again (the
    /// server refused to send it as the account: the Send As right on a shared mailbox, which an
    /// administrator gives); `None` for a message that goes with the next Send/Receive.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub hold: Option<String>,
    /// Where the body is while it isn't in memory ([`KeptBody`]); `None` once it is (or when it
    /// always was). Not written: see [`Attachment::kept`].
    #[serde(skip)]
    pub kept: Option<Box<KeptBody>>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Message {
    /// Every recipient: To, Cc, Bcc.
    pub fn recipients(&self) -> impl Iterator<Item = &Address> {
        self.to.iter().chain(&self.cc).chain(&self.bcc)
    }

    /// Whether the attachment list has something.
    pub fn has_attachments(&self) -> bool {
        self.listed_attachments().next().is_some()
    }

    /// The attachments the attachment list shows, with their index: everything except pictures
    /// the HTML body shows in place (`cid:` references). A file merely marked "inline" by the
    /// sending program, which nothing in the body refers to, is still listed.
    pub fn listed_attachments(&self) -> impl Iterator<Item = (usize, &Attachment)> {
        let html = self.body.html.as_deref().unwrap_or("");
        self.attachments
            .iter()
            .enumerate()
            .filter(move |(_, a)| !(a.inline && !a.content_id.is_empty() && html.contains(&format!("cid:{}", a.content_id))))
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

    /// A header field MailCraft doesn't model, by name (whatever its case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Where the sender asked a read receipt to go (`Disposition-Notification-To`, RFC 8098):
    /// the first address of that field, when it is one.
    pub fn receipt_to(&self) -> Option<Address> {
        let field = self.header("Disposition-Notification-To")?;
        Address::parse_list(field).into_iter().next().filter(Address::is_valid)
    }

    /// Whether it is still waiting in the Outbox for its time at `now` (Delay Delivery).
    pub fn is_deferred(&self, now: Timestamp) -> bool {
        self.deliver_after.is_some_and(|at| at > now)
    }
}

/// Strip reply and forward prefixes (`Re:`, `RE:`, `Fw:`, `Fwd:`, `AW:`, `WG:`, `Re[2]:`), however
/// many there are.
pub fn normalize_subject(subject: &str) -> String {
    let mut s = subject.trim();
    while let Some((prefix, rest)) = s.split_once(':') {
        let p = prefix.trim().to_ascii_lowercase();
        let p = p.split('[').next().unwrap_or("").trim();
        if matches!(p, "re" | "fw" | "fwd" | "aw" | "wg" | "sv" | "vs" | "tr" | "rv") {
            s = rest.trim();
        } else {
            break;
        }
    }
    s.to_string()
}

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
}
