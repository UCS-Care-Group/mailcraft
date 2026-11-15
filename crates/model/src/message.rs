// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mail message.

use std::sync::Arc;

use mailcraft_time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::headers::HeaderBlock;
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

/// The most bytes of a Digitally signed or Encrypted message's original that are kept
/// ([`Message::original`], #30): a larger one is read as any other message, without them.
pub const MAX_ORIGINAL: usize = 64 << 20;

/// The most bytes of originals a mailbox keeps in all (#30): past it, a message that comes in is
/// kept without its original, as one read before they were kept, and Check again
/// (`mail.checkAgain`) can fetch it once there is room.
pub const MAX_ORIGINALS: u64 = 1 << 30;

/// The bytes a Digitally signed or Encrypted message came as (#30): its MIME as its server or
/// file gave it, in CRLF lines, which a Digital signature is checked against and whose body a
/// copy of it elsewhere (another account's server, a saved `.eml`) carries, so that the
/// signature still checks there. Kept as an attachment is: in memory (`data`) or in the content folder beside
/// the mailbox file (`kept`), hashed and checked against its length when read back.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Original {
    /// The bytes, when they are in memory. Empty while they are kept outside (`kept`).
    #[serde(with = "crate::b64")]
    pub data: Arc<Vec<u8>>,
    /// Where the bytes are when `data` doesn't hold them ([`Attachment::kept`]). Not written.
    #[serde(skip)]
    pub kept: Option<ContentRef>,
}

impl std::fmt::Debug for Original {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Original({} bytes{})", self.size(), if self.is_loaded() { "" } else { ", kept" })
    }
}

impl Original {
    /// `bytes` as a message's original; `None` when there are none, or more than [`MAX_ORIGINAL`].
    pub fn new(bytes: Vec<u8>) -> Option<Original> {
        (!bytes.is_empty() && bytes.len() <= MAX_ORIGINAL).then(|| Original { data: Arc::new(bytes), kept: None })
    }

    /// How many bytes it is, in memory or where it is kept.
    pub fn size(&self) -> u64 {
        match self.kept {
            Some(k) if self.data.is_empty() => k.len,
            _ => self.data.len() as u64,
        }
    }

    /// Whether its bytes are in memory, or there is nothing kept outside to bring in.
    pub fn is_loaded(&self) -> bool {
        self.kept.is_none() || !self.data.is_empty()
    }

    /// Whether there is anything of it: bytes in memory or kept outside.
    pub fn is_there(&self) -> bool {
        !self.data.is_empty() || self.kept.is_some()
    }
}

/// [`Message::original`] as a file or a synchronisation result holds it: past [`MAX_ORIGINAL`]
/// it is read as none.
fn capped_original<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Original>, D::Error> {
    let original = Option::<Original>::deserialize(d)?;
    Ok(original.filter(|o| o.data.len() <= MAX_ORIGINAL))
}

/// Most characters of its text a message to be fetched again keeps as its preview
/// ([`Message::to_fetch_again`]): as many as Microsoft Graph gives as one.
pub const PREVIEW_CHARS: usize = 255;

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

/// The most text of what a sender hid ([`Message::hidden_text`]) that is kept, in bytes.
pub const MAX_HIDDEN_TEXT: usize = 16 * 1024;

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
    /// The words of what the sender hid in the HTML (`display:none`, a style sheet's
    /// `visibility:hidden`, a font size of nothing: preheaders, the other copy of a desktop and
    /// mobile pair), as one line of at most 16 KB. It is not in `body.text` and is never shown,
    /// quoted in a reply or sent: it is for the owner's search, rules and agents, who read it as
    /// data, never as instructions. Empty for a message read before it was kept, until the
    /// message is read again from its source.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hidden_text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    pub read: bool,
    pub flag: FlagStatus,
    /// Follow-up due date of a flagged message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flag_due: Option<Timestamp>,
    pub importance: Importance,
    /// Written here with Request a Read Receipt on: it goes with `Disposition-Notification-To`
    /// naming its sender (RFC 8098), or Exchange's own flag for one. Only ever set by the owner's
    /// own message; a request in mail that comes in is never read into it, and never answered.
    #[serde(skip_serializing_if = "is_false")]
    pub read_receipt: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    pub answered: bool,
    pub forwarded: bool,
    pub draft: bool,
    pub pinned: bool,
    /// Its snooze (#949): when it comes back and where it came from; once it has come back, the
    /// mark it wears until opened. `None` for a message never snoozed.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub snooze: Option<crate::Snooze>,
    /// The kinds of sender warning ([`crate::sender_warnings::Warning::kind`]) the owner dismissed
    /// the banner for (#508). The banner is hidden while every kind it would show is among these;
    /// a kind that appears later brings it back.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub warnings_dismissed: Vec<String>,
    /// The header lines MailCraft doesn't model that it reads by name ([`crate::headers::WORKING`],
    /// and its own notes), in order; the others are its [`Message::header_block`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<(String, String)>,
    /// Every other header line it kept, so a saved message round-trips: in memory, or kept beside
    /// the mailbox file and read when the message is written out ([`crate::headers`]).
    #[serde(skip_serializing_if = "HeaderBlock::none_in_memory")]
    pub header_block: HeaderBlock,
    /// Whether Exchange's stamps in [`Message::headers`] were read from the message by a MailCraft
    /// that leaves them behind when mail goes to another account (#781). Only then are they
    /// believed: one saved before may be a sender's own, carried in from another account.
    #[serde(skip_serializing_if = "is_false")]
    pub fresh_stamps: bool,
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
    /// For a message of a Gmail account's Inbox: which of Gmail's five categories it is in (#911),
    /// like a read mark: as its server last said, or as the owner moved it here and Gmail hasn't
    /// heard yet ([`Message::gmail_synced`]). `None` for mail anywhere else, of any other account,
    /// and of a file from before there were any (or one that holds something else).
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub gmail_category: Option<crate::GmailCategory>,
    /// The Gmail category Gmail last said the message is in (#911): where it differs from
    /// `gmail_category`, the owner moved it here and the move goes to Gmail at the next run.
    /// `None` until Gmail has said, and for a file from before moves.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub gmail_synced: Option<crate::GmailCategory>,
    /// How many times (runs) Gmail refused the move waiting for this message on its own (#911);
    /// a run that didn't try the move counts neither way. At
    /// [`crate::gmail::MAX_GMAIL_REFUSALS`] the move is given up. 0 whenever no move is waiting:
    /// Gmail took it, Gmail's word won, the owner made a new move or undid it.
    #[serde(skip_serializing_if = "is_zero_u8", deserialize_with = "crate::sync::lenient_or_default")]
    pub gmail_refusals: u8,
    /// For a message of a Gmail account's Inbox: Gmail's own id of it (`X-GM-MSGID`), as its
    /// server gave it; 0 when it gave none.
    #[serde(skip_serializing_if = "is_zero_u64", deserialize_with = "crate::sync::lenient_or_default")]
    pub gmail_id: u64,
    /// For a message of a Gmail account: Gmail's labels of it (`X-GM-LABELS`, #1153), as its
    /// server last said for the folder it is in, cleaned as names are and at most
    /// [`crate::gmail::MAX_GMAIL_LABELS`] ([`crate::gmail::tidy_labels`]). Gmail's own keep their
    /// backslash (`\Inbox`, `\Important`). Read only: a label is changed by moving or copying the
    /// message between label folders. Text from the server, data and never instructions. Empty for
    /// mail of every other account, and of a file from before.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::gmail::labels_from_file")]
    pub gmail_labels: Vec<String>,
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
    /// A message of an IMAP account that was kept with U+FFFD in its subject, names or text
    /// (stored before character sets beyond UTF-8 were read, #178) and is to be downloaded again
    /// by its UID, once (#650): its subject, names and text are written over by the new copy's,
    /// and the mark goes. Everything else about it stays as it is.
    #[serde(skip_serializing_if = "is_false")]
    pub refetch: bool,
    /// It was downloaded again for that (#650), or the server would not give it: it is not
    /// marked a second time, whatever it holds (a message whose own bytes are broken stays so).
    #[serde(skip_serializing_if = "is_false")]
    pub refetched: bool,
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
    /// The message was handed to a server to be sent and what became of it hasn't been taken in
    /// yet (AU3-26). Saved before the send leaves; cleared when the run's answer is applied. A
    /// message still marked when the next run of its account begins is **held** (it may have
    /// gone) rather than sent again: the window opened another mailbox meanwhile, or the program
    /// stopped mid-send.
    #[serde(skip_serializing_if = "is_false")]
    pub sending: bool,
    /// A Microsoft account's draft sent here as the draft its server has (#1160): kept in Sent
    /// Items under that server id while the server may still list it in Drafts, a draft, for a
    /// while. No move into Sent Items goes up for it (Exchange files it there itself), and a
    /// listing of it as a draft outside Sent Items is not followed. Cleared once the server says
    /// it is gone from a folder other than Sent Items. False for every other message.
    #[serde(skip_serializing_if = "is_false")]
    pub sent_here: bool,
    /// For a reply or forward written here and not sent yet: the message it answers or forwards,
    /// which is marked so once this has gone (#154), not when Send is pressed. `None` for every
    /// other message, and once it is sent.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub answers: Option<Answers>,
    /// Where the body is while it isn't in memory ([`KeptBody`]); `None` once it is (or when it
    /// always was). Not written: see [`Attachment::kept`].
    #[serde(skip)]
    pub kept: Option<Box<KeptBody>>,
    /// For a Digitally signed or Encrypted message (#30): the bytes it came as ([`Original`]).
    /// `None` for every other message, for one read before they were kept (Check again downloads
    /// them, `mail.checkAgain`), and once something the bytes hold was taken out of it here (an
    /// attachment removed). Wherever the message is written whole (another account's server, a
    /// saved `.eml`, a forward as an attachment) its body is these bytes' body, under their own
    /// content fields, and its header is written from the message, with the filters every
    /// message's header goes through.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "capped_original")]
    pub original: Option<Original>,
    /// A part of it was taken out here (Remove Attachment): the bytes it came as no longer are
    /// this message, and Check again never brings them back (#30).
    #[serde(skip_serializing_if = "is_false")]
    pub parts_removed: bool,
    /// Check again asked for the bytes it came as (#30): what its server gives next brings those
    /// and nothing else; its text, files, headers and marks stay as they are here.
    #[serde(skip_serializing_if = "is_false")]
    pub original_wanted: bool,
    /// The bytes it came as were asked for again (#30), and what came didn't bring them: the
    /// server's copy isn't protected either (one put there from its parts). Not asked again, and
    /// a copy made of its parts may stand for it where it goes.
    #[serde(skip_serializing_if = "is_false")]
    pub original_asked: bool,
    /// How many times the server refused the bytes it came as when they were asked for again
    /// (#30), or [`ORIGINAL_TRIES`] once it is given up on (refused that often over at least
    /// [`ORIGINAL_PATIENCE`], or too large to take): not asked again then until Check again, and
    /// what waits for them goes on waiting.
    #[serde(skip_serializing_if = "is_zero_u8")]
    pub original_tries: u8,
    /// When the server first refused them ([`Message::original_tries`]); 0: it hasn't.
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub original_refused: Timestamp,
    /// Not asked for again before this time, after a refusal (#30); 0: whenever.
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub original_retry: Timestamp,
    /// The size of the bytes it came as when there was no room for them here (#30), the bytes
    /// having come: not asked for again until there is room for that much. 0: none of that.
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub original_no_room: u64,
    /// The IMAP account whose server took a copy of it, moved there, written from the bytes it
    /// came as (#30): what it was moved out of may go then. 0: no such copy. (A copy Graph makes
    /// is made of its parts, and never holds them.)
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub original_placed: Id,
}

/// The least time, in seconds, over which a server's refusals of the bytes a message came as are
/// counted before they are asked for no more ([`Message::original_tries`]): a day, so that an
/// outage doesn't hold a move for good.
pub const ORIGINAL_PATIENCE: Timestamp = 24 * 60 * 60;

fn is_zero_i64(n: &i64) -> bool {
    *n == 0
}

/// How many refusals of the bytes a message came as are taken before they are asked for no more
/// ([`Message::original_tries`]).
pub const ORIGINAL_TRIES: u8 = 5;

fn is_zero_u8(n: &u8) -> bool {
    *n == 0
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
    /// The words the sender hid ([`Message::hidden_text`]) when they are over the size kept in
    /// the mailbox file (#599): `hidden_text` is empty until they are brought in with the body.
    pub hidden: Option<ContentRef>,
    /// Bringing it in was tried and failed (the file is missing or damaged): the text says so,
    /// and nothing tries again until the mailbox is opened anew.
    pub failed: bool,
    /// [`text_hash`] of `body.text` as the mailbox was opened (a short text is in the mailbox
    /// file itself, with its HTML kept outside): the kept HTML goes with that text and no other.
    pub text_hash: u64,
}

impl KeptBody {
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.html.is_none() && self.hidden.is_none()
    }

    /// Whether `text` is the text the mailbox was opened with.
    pub fn same_text(&self, text: &str) -> bool {
        self.text_hash == text_hash(text)
    }

    /// How many bytes the kept text and HTML come to.
    pub fn len(&self) -> u64 {
        self.text.map_or(0, |r| r.len).saturating_add(self.html.map_or(0, |r| r.len)).saturating_add(self.hidden.map_or(0, |r| r.len))
    }
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}

impl Message {
    /// It lies in `folder` of `account` from now on: the message `uid` of that folder's server
    /// mailbox (0: none yet), of which the server last said `synced`. A copy of it that was
    /// expected where it lay is no longer what it waits for ([`Message::placed`], #451). Out of the
    /// folder it was in, it is in none of Gmail's categories, which are of a Gmail Inbox (#911).
    pub fn lies_in(&mut self, folder: Id, account: Id, uid: u32, synced: Option<RemoteFlags>) {
        if folder != self.folder {
            self.left_gmail_inbox();
        }
        (self.folder, self.account, self.uid, self.synced) = (folder, account, uid, synced);
        self.placed.clear();
    }

    /// It is in none of Gmail's categories, and Gmail's id of it, a move between them waiting
    /// for Gmail and Gmail's labels of it are forgotten: what the server says of the folder it is
    /// in now says them again, if anything (#911, #1153). A copy into another folder, a local one
    /// or another account's, takes none of them.
    pub fn left_gmail_inbox(&mut self) {
        (self.gmail_category, self.gmail_synced, self.gmail_refusals, self.gmail_id) = (None, None, 0, 0);
        self.gmail_labels.clear();
    }

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
    /// the HTML body shows in place ([`crate::shown`]: an `<img>` asks for it by `cid:`, and it is
    /// a PNG, JPEG, GIF or WebP by its bytes). A file merely marked "inline" by the sending
    /// program, which no `<img>` of the body asks for, is still listed, and so is a file of any
    /// other kind, whatever the body says of it. While the HTML is kept outside memory
    /// ([`Message::kept`]), an inline picture with a `Content-ID` is taken to be one it shows,
    /// until the HTML is read and says; a file that is no picture is listed all the same.
    pub fn listed_attachments(&self) -> impl Iterator<Item = (usize, &Attachment)> {
        let html_kept = self.body.html.is_none() && self.kept.as_ref().is_some_and(|k| k.html.is_some() && !k.failed);
        // Read only when some part may be one it shows.
        let may_show = self.attachments.iter().any(|a| a.inline && !a.content_id.is_empty());
        let ids = if may_show && !html_kept { crate::shown::shown_ids(self.body.html.as_deref().unwrap_or("")) } else { Default::default() };
        self.attachments.iter().enumerate().filter(move |(_, a)| {
            let shown = a.inline
                && !a.content_id.is_empty()
                && if html_kept { crate::shown::is_picture_part(a) } else { crate::shown::shown_in_place(&ids, a) };
            !shown
        })
    }

    /// The subject without `Re:` / `Fw:` prefixes: what a conversation is about.
    pub fn topic(&self) -> String {
        normalize_subject(&self.subject)
    }

    /// The topic, case-folded: what the messages of one conversation have the same of (empty
    /// for a message without a subject, which is a conversation of its own). Two messages with
    /// the same topic are of one conversation only when they are linked too
    /// ([`crate::ThreadIndex`], #795).
    pub fn topic_key(&self) -> String {
        self.topic().to_lowercase()
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
        if self.has_header(NOT_DOWNLOADED) {
            return None;
        }
        // The bytes it came as are a second copy of what its parts hold (#30).
        Some(self.size().saturating_sub(self.original.as_ref().map_or(0, Original::size)))
    }

    /// Size in bytes, roughly: text, HTML, attachments and the bytes a Digitally signed or
    /// Encrypted message came as (#30), in memory or where they are kept.
    pub fn size(&self) -> u64 {
        let kept = self.kept.as_deref();
        let text = if self.body.text.is_empty() { kept.and_then(|k| k.text).map_or(0, |r| r.len) } else { self.body.text.len() as u64 };
        let html = match &self.body.html {
            Some(h) => h.len() as u64,
            None => kept.and_then(|k| k.html).map_or(0, |r| r.len),
        };
        let hidden = if self.hidden_text.is_empty() { kept.and_then(|k| k.hidden).map_or(0, |r| r.len) } else { self.hidden_text.len() as u64 };
        let original = self.original.as_ref().map_or(0, Original::size);
        self.attachments
            .iter()
            .map(|a| a.size() as u64)
            .fold(text.saturating_add(html).saturating_add(hidden).saturating_add(original), u64::saturating_add)
    }

    /// To be fetched again, whole, from the server that has it (#793): its headers, marks and
    /// server state stay, with the first [`PREVIEW_CHARS`] characters of its text as its preview
    /// when that text is in memory (its HTML may be on disk); its body and files go, and it
    /// wears the paperclip when it had files. What a message deleted for good keeps of itself, unless it was whole in memory,
    /// while its server is asked to delete it: the content kept on disk for it is named by
    /// nothing once the mailbox file no longer has the message, and goes with the next sweep.
    pub fn to_fetch_again(&mut self) {
        let had_files = self.has_attachments();
        // (The text in memory, when there is one: a short text stays in the mailbox file while
        // its HTML is kept on disk. Not the words that say the text couldn't be read.)
        let failed = self.kept.as_deref().is_some_and(|k| k.failed);
        let preview = if failed { String::new() } else { self.body.text.chars().take(PREVIEW_CHARS).collect() };
        self.body = Body { text: preview, html: None };
        self.hidden_text = String::new();
        self.attachments.clear();
        self.kept = None;
        (self.original, self.original_wanted) = (None, false);
        self.attachments_on_server = had_files;
        self.unfetched = true;
    }

    /// Whether the message looks as one put back by 0.5.0 up to #793 after a refused delete for
    /// good: a file of no bytes that names nothing kept outside, or no text at all with nothing
    /// kept outside. (A message that really is so is fetched again once, for nothing.)
    pub fn looks_hollow(&self) -> bool {
        let empty_file = self.attachments.iter().any(|a| a.data.is_empty() && a.kept.is_none());
        let no_text = self.body.text.is_empty() && self.body.html.is_none() && self.kept.as_deref().is_none_or(|k| k.is_empty() && !k.failed);
        empty_file || no_text
    }

    /// Whether some of the body is kept outside memory and not brought in yet ([`Message::kept`]):
    /// its text is empty, or its HTML missing, until it is. Not so once bringing it in failed
    /// (the text says so then).
    pub fn body_is_kept(&self) -> bool {
        self.kept.as_ref().is_some_and(|k| {
            !k.failed
                && ((k.text.is_some() && self.body.text.is_empty())
                    || (k.html.is_some() && self.body.html.is_none())
                    || (k.hidden.is_some() && self.hidden_text.is_empty()))
        })
    }

    /// Whether everything of the message is in memory: its body and every attachment.
    pub fn is_loaded(&self) -> bool {
        !self.body_is_kept() && self.attachments.iter().all(Attachment::is_loaded) && self.original.as_ref().is_none_or(Original::is_loaded)
    }

    /// Whether the copy it was moved out of may go, as far as the mailbox alone can tell (#30):
    /// the bytes it came as can't be lost with it. It has none named, a download from a server
    /// brought it without them ([`Message::original_asked`]), or the copy where it is now, on
    /// an IMAP server of the account it is in, was written from them
    /// ([`Message::original_placed`]). Otherwise only bytes read whole here at the release let
    /// it go (the engine's gate); a placement gone wrong is a copy too many, never a loss.
    pub fn may_release(&self) -> bool {
        self.original.is_none() || self.original_asked || (self.original_placed != 0 && self.original_placed == self.account)
    }

    /// Whether the bytes it came as are here ([`Message::original`]): in memory or kept outside.
    pub fn has_original(&self) -> bool {
        self.original.as_ref().is_some_and(Original::is_there)
    }

    /// The bytes it came as, when they are in memory ([`Message::original`]).
    pub fn original_bytes(&self) -> Option<&[u8]> {
        self.original.as_ref().map(|o| o.data.as_slice()).filter(|d| !d.is_empty())
    }

    /// Whether it looks Digitally signed or Encrypted by what was read of it (#30), for a message
    /// without the bytes it came as: a part that carries a signature or encrypted content
    /// (`smime.p7s`, `smime.p7m`, `signature.asc`, PGP/MIME's control part), or OpenPGP armour
    /// at the start of a line in the first 64 KB of its text in memory
    /// ([`crate::protected::has_armour_within`]): cheap enough to ask as a menu is drawn.
    pub fn looks_protected(&self) -> bool {
        self.attachments.iter().any(|a| crate::protected::is_protection_part(&a.mime, &a.name))
            || crate::protected::has_armour_within(self.body.text.as_bytes(), 64 * 1024)
    }

    pub fn has_category(&self, name: &str) -> bool {
        self.categories.iter().any(|c| c.eq_ignore_ascii_case(name))
    }
}

/// Strip reply and forward prefixes (`Re:`, `RE:`, `Fw:`, `Fwd:`, `Re[2]:`; German `AW:`, `WG:`;
/// Spanish `RV:`; Italian `R:`, `I:`; French `TR :`; Dutch `Antw:`, `Doorst:`), however many there are.
pub fn normalize_subject(subject: &str) -> String {
    topic_of(subject).to_string()
}

/// [`normalize_subject`] as a part of `subject`, without a copy.
pub(crate) fn topic_of(subject: &str) -> &str {
    const PREFIXES: [&str; 13] = ["re", "fw", "fwd", "aw", "wg", "sv", "vs", "tr", "rv", "r", "i", "antw", "doorst"];
    let mut s = subject.trim();
    while let Some((prefix, rest)) = s.split_once(':') {
        let p = prefix.trim().split('[').next().unwrap_or("").trim();
        if PREFIXES.iter().any(|x| p.eq_ignore_ascii_case(x)) {
            s = rest.trim();
        } else {
            break;
        }
    }
    s
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
        assert_eq!(normalize_subject("Antw: Doorst: Lunch"), "Lunch");
        assert_eq!(normalize_subject("Ristorante: Pranzo"), "Ristorante: Pranzo");
    }

    #[test]
    fn threads_and_sizes() {
        let mut a = Message { id: 1, subject: "Budget".into(), ..Default::default() };
        let b = Message { id: 2, subject: "RE: budget".into(), ..Default::default() };
        assert_eq!(a.topic_key(), b.topic_key());
        a.subject.clear();
        assert!(a.topic_key().is_empty());
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
        let picture =
            Attachment { content_id: "logo@x".into(), inline: true, ..Attachment::new("logo.png", "image/png", b"\x89PNG\r\n\x1a\n".to_vec()) };
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
        let logo =
            Attachment { content_id: "logo@x.example".into(), mime: "image/png".into(), inline: true, kept: Some(r(900)), ..Attachment::default() };
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
