// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encrypted messages opened where they are read (#30 part 6, R8, R15, R22, R23).
//!
//! **What an Encrypted message opens to exists in memory only.** The mailbox keeps the message
//! as it came (its bytes, [`mailcraft_model::Message::original`], and the parts read from them:
//! an Encrypted message's own parts are the encrypted file); the text and files it opens to are
//! held here, for the session, in [`Decryptions`]: never in the mailbox, never saved, never in
//! the undo history, never in a search, a rule, a list's preview or a notification, and never
//! given to an agent. Only [`Session::decrypted_message`], which the reading pane draws and no
//! command reads, hands them out. A few messages are kept at once, within a byte budget, and the
//! oldest is let go of first; the plaintext's own buffer is overwritten when it is dropped (the
//! text of the message read from it, which is made of ordinary strings, is not).
//!
//! **The engine does no I/O and holds no keys.** [`Session::begin_decrypt`] says what is to be
//! opened ([`DecryptJob`]: the bytes and their format), the host does it on a thread of its own
//! with the keys it can reach (the system's certificate store, a smartcard, gpg-agent) and gives
//! back [`Plain`] or a [`Failure`], which [`Session::finish_decrypt`] takes in. Anything that is
//! signed inside the encryption is checked against the same keys and trust as any other Digital
//! signature ([`Session::digital_signature`] then gives that result).
//!
//! What comes back is as hostile as the message: it is read with the caps every message is read
//! with, and an Encrypted message inside the encryption isn't opened again (nothing says its
//! key is the owner's): only a Digital signature inside it is read, once.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use mailcraft_model::digital_signature::{DigitalSignature, SignatureFormat};
use mailcraft_model::{Id, Message};
use zeroize::Zeroizing;

use crate::{Part, Session};

/// How many opened messages are kept at once.
pub const MAX_KEPT: usize = 16;
/// How many bytes of plaintext are kept in all (a message larger than this is not opened).
pub const MAX_KEPT_BYTES: usize = 256 << 20;
/// The most messages whose kind is remembered.
const MAX_KINDS: usize = 8_192;

/// Why an Encrypted message wasn't opened. Texts for the owner are the window's; none of these
/// says anything about what was wrong with the message beyond what is the same for every one
/// (a sender can't tell a bad padding from a wrong key from a changed byte).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// None of the message's recipients is one of the owner's keys.
    NoKey,
    /// The message is encrypted in a way MailCraft doesn't read (a password, key agreement on an
    /// S/MIME certificate, an algorithm it doesn't know).
    Unsupported,
    /// The place the owner's keys are (GnuPG, the system's certificate store, a smartcard)
    /// can't be reached, or has none to ask: what the host can say of that.
    Unavailable,
    /// A smartcard needs its PIN (its name): the window asks, and the message is then tried again.
    PinNeeded(String),
    /// The owner closed the prompt for a PIN or a passphrase.
    Cancelled,
    /// A wrong passphrase or PIN was typed for the key: the owner's to fix, nothing wrong with
    /// the message.
    WrongPassphrase,
    /// What it opens to is more than a message is kept at (it is not opened).
    TooLarge,
    /// Anything else, all alike: a key that doesn't open it, a message changed or damaged.
    Failed,
}

/// What the host gives back for an Encrypted message it opened. Overwritten when dropped.
pub struct Plain {
    /// What the message holds: a MIME entity (S/MIME and PGP/MIME), or the text of an inline
    /// OpenPGP message ([`Plain::inline`]).
    pub data: Zeroizing<Vec<u8>>,
    /// An OpenPGP message in the text of the mail, whose plaintext is text, not MIME.
    pub inline: bool,
    /// OpenPGP Digital signature packets that were inside the encryption, over [`Plain::data`].
    pub signatures: Vec<u8>,
    /// Whether the encryption detects changes (AES-GCM, OpenPGP's integrity-protected data). One
    /// that doesn't (AES-CBC, 3DES) can be changed by someone who knows part of the text, so what
    /// it opens to is never allowed to fetch anything ([`Session::decrypted_authenticated`]).
    pub authenticated: bool,
}

impl std::fmt::Debug for Plain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Plain(<{} bytes>)", self.data.len())
    }
}

/// An Encrypted message the host is to open: its bytes as they came, and their format.
#[derive(Clone)]
pub struct DecryptJob {
    pub id: Id,
    pub format: SignatureFormat,
    pub original: Arc<Vec<u8>>,
}

impl std::fmt::Debug for DecryptJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DecryptJob(message {}, {:?}, {} bytes)", self.id, self.format, self.original.len())
    }
}

/// How far an Encrypted message has got, for the bar over it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decryption {
    /// Not an Encrypted message (or one this program doesn't read).
    NotEncrypted,
    /// An Encrypted message read before the bytes it came as were kept: Check Again downloads them.
    NoOriginal,
    /// The host is opening it.
    Opening(SignatureFormat),
    /// It was opened; what it holds is [`Session::decrypted_message`].
    Opened(SignatureFormat),
    /// It wasn't, and why.
    Failed(SignatureFormat, Failure),
    /// It hasn't been asked for yet (the host begins it with [`Session::begin_decrypt`]).
    Waiting(SignatureFormat),
}

/// What an Encrypted message had besides what was encrypted: text or files in the clear, not
/// shown with it (nothing says who wrote them, nor that they belong to what was encrypted).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outside {
    /// There was text around the encrypted block (inline OpenPGP), or beside it.
    pub text: bool,
    /// How many files were beside it, unencrypted.
    pub files: usize,
}

impl Outside {
    /// Whether there was anything.
    pub fn any(self) -> bool {
        self.text || self.files > 0
    }
}

/// An Encrypted message opened: what it holds. The message to read is made from the stored one
/// each time ([`Session::decrypted_message`]), so its flag, categories and the rest stay as the
/// mailbox's are.
pub struct Decrypted {
    body: mailcraft_model::Body,
    attachments: Vec<mailcraft_model::Attachment>,
    hidden_text: String,
    /// The Digital signature that was inside it, checked.
    pub signature: Option<Arc<DigitalSignature>>,
    pub format: SignatureFormat,
    /// See [`Plain::authenticated`].
    pub authenticated: bool,
    /// What was beside it, not shown.
    pub outside: Outside,
    bytes: usize,
    /// The view last made: the stored message it was made from, and it.
    view: Mutex<Option<(Arc<Message>, Arc<Message>)>>,
}

/// What an opened message rests on: the bytes it came as.
type Basis = (u64, Option<[u8; 32]>);

enum Entry {
    /// Asked of the host, for the bytes with this basis.
    Pending(SignatureFormat, Basis),
    /// It wasn't opened, for the bytes with this basis.
    Failed(SignatureFormat, Failure, Basis),
    Opened(Arc<Decrypted>),
}

/// What the session knows of Encrypted messages. Never saved.
#[derive(Default)]
pub(crate) struct Decryptions {
    entries: HashMap<Id, Entry>,
    /// The opened ones, oldest first.
    order: VecDeque<Id>,
    bytes: usize,
    /// What each message looked at is, with the basis it was told from.
    kinds: HashMap<Id, (Basis, Option<SignatureFormat>)>,
    /// How many opened messages were let go of: the window drops what it kept of them when it changes.
    gone: u64,
}

impl Decryptions {
    pub(crate) fn forget_messages(&mut self) {
        self.gone = self.gone.wrapping_add(1);
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
        self.kinds.clear();
    }

    fn drop_entry(&mut self, id: Id) {
        if let Some(Entry::Opened(d)) = self.entries.remove(&id) {
            self.bytes = self.bytes.saturating_sub(d.bytes);
            self.gone = self.gone.wrapping_add(1);
        }
        self.order.retain(|o| *o != id);
    }

    fn keep(&mut self, id: Id, decrypted: Decrypted) {
        self.drop_entry(id);
        while self.order.len() >= MAX_KEPT || self.bytes.saturating_add(decrypted.bytes) > MAX_KEPT_BYTES {
            let Some(oldest) = self.order.pop_front() else { break };
            if let Some(Entry::Opened(d)) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(d.bytes);
                self.gone = self.gone.wrapping_add(1);
            }
        }
        self.bytes = self.bytes.saturating_add(decrypted.bytes);
        self.order.push_back(id);
        self.entries.insert(id, Entry::Opened(Arc::new(decrypted)));
    }
}

fn basis_of(m: &Message) -> Basis {
    m.original.as_ref().filter(|o| o.is_there()).map_or((0, None), |o| (o.size(), o.kept.map(|k| k.sha256)))
}

/// The format of the Encrypted message `bytes`, when it is one.
pub(crate) fn kind_of(bytes: &[u8]) -> Option<SignatureFormat> {
    use mailcraft_mime::Protection;
    match mailcraft_mime::protection(bytes)? {
        Protection::Encrypted => Some(SignatureFormat::OpenPgp),
        Protection::InlinePgp => mailcraft_openpgp::encrypted_message(bytes).map(|_| SignatureFormat::OpenPgp),
        Protection::Pkcs7 => {
            let parts = mailcraft_mime::signed_parts(bytes)?;
            (parts.kind == mailcraft_mime::SignedKind::SmimeOpaque && mailcraft_smime::is_enveloped(&parts.signature))
                .then_some(SignatureFormat::Smime)
        }
        Protection::Signed => None,
    }
}

/// Whether what is here of `m` (its parts, without the bytes it came as) looks like an Encrypted
/// message: S/MIME's file with no text, or PGP/MIME's encrypted file.
fn looks_encrypted(m: &Message) -> bool {
    let pgp = m.attachments.iter().any(|a| a.mime.to_ascii_lowercase().contains("pgp-encrypted") || a.name.eq_ignore_ascii_case("encrypted.asc"));
    let smime = m.body.text.trim().is_empty()
        && m.body.html.is_none()
        && m.attachments.iter().any(|a| a.mime.to_ascii_lowercase().contains("pkcs7-mime") && !a.mime.to_ascii_lowercase().contains("signed"));
    pgp || smime
}

impl Session {
    /// What kind of Encrypted message message `id` is, if it is one: found from the bytes it came
    /// as the first time it is asked, then remembered while they stay the same. `None` for any
    /// other message, a draft, and one that has no bytes here.
    pub fn encryption_of(&mut self, id: Id) -> Option<SignatureFormat> {
        let m = self.mailbox.messages.get(&id)?;
        if m.draft || !m.has_original() {
            return None;
        }
        let basis = basis_of(m);
        if let Some((kept, kind)) = self.decryptions.kinds.get(&id)
            && *kept == basis
        {
            return *kind;
        }
        let kind = self.content(id, Part::Original).ok().and_then(|bytes| kind_of(&bytes));
        if self.decryptions.kinds.len() >= MAX_KINDS {
            self.decryptions.kinds.clear();
        }
        self.decryptions.kinds.insert(id, (basis, kind));
        kind
    }

    /// How far message `id`, an Encrypted message, has got ([`Decryption`]).
    pub fn decryption(&mut self, id: Id) -> Decryption {
        let Some(m) = self.mailbox.messages.get(&id) else { return Decryption::NotEncrypted };
        if m.draft {
            return Decryption::NotEncrypted;
        }
        if !m.has_original() {
            return if looks_encrypted(m) { Decryption::NoOriginal } else { Decryption::NotEncrypted };
        }
        let basis = basis_of(m);
        let Some(format) = self.encryption_of(id) else { return Decryption::NotEncrypted };
        match self.decryptions.entries.get(&id) {
            Some(Entry::Pending(f, b)) if *b == basis => Decryption::Opening(*f),
            // (A failure for other bytes than these isn't one for these: asked again.)
            Some(Entry::Failed(f, why, b)) if *b == basis => Decryption::Failed(*f, why.clone()),
            Some(Entry::Opened(d)) => Decryption::Opened(d.format),
            // (A job for other bytes than these, or none: to be asked again.)
            _ => Decryption::Waiting(format),
        }
    }

    /// The Encrypted message `id` to open, when it hasn't been asked for: marked as being opened.
    /// The host opens it and gives the result to [`Session::finish_decrypt`]. `None` for a
    /// message that isn't one, or has been asked for already (a failed one is asked for again
    /// only after [`Session::forget_decryption`]).
    pub fn begin_decrypt(&mut self, id: Id) -> Option<DecryptJob> {
        let Decryption::Waiting(format) = self.decryption(id) else { return None };
        let m = self.mailbox.messages.get(&id)?;
        let basis = basis_of(m);
        let original = self.content(id, Part::Original).ok()?;
        self.decryptions.drop_entry(id);
        self.decryptions.entries.insert(id, Entry::Pending(format, basis));
        Some(DecryptJob { id, format, original })
    }

    /// Take in what the host made of the job for message `id`. A result for bytes the message no
    /// longer has, or for a message that is gone, is let go of.
    pub fn finish_decrypt(&mut self, id: Id, result: Result<Plain, Failure>) {
        let (format, basis) = match self.decryptions.entries.get(&id) {
            Some(Entry::Pending(format, basis)) => (*format, *basis),
            _ => return,
        };
        let Some(stored) = self.mailbox.messages.get(&id).cloned() else {
            self.decryptions.drop_entry(id);
            return;
        };
        if basis_of(&stored) != basis {
            self.decryptions.drop_entry(id);
            return;
        }
        let entry = match result {
            Err(why) => Entry::Failed(format, why, basis),
            Ok(plain) => match self.open_plain(&stored, format, &plain) {
                Some(decrypted) if decrypted.bytes <= MAX_KEPT_BYTES => {
                    self.decryptions.keep(id, decrypted);
                    self.list_due();
                    return;
                }
                Some(_) => Entry::Failed(format, Failure::TooLarge, basis),
                None => Entry::Failed(format, Failure::Failed, basis),
            },
        };
        self.decryptions.entries.insert(id, entry);
    }

    /// Message `id` is to be opened again (the owner asked, or the PIN it waited for was given).
    pub fn forget_decryption(&mut self, id: Id) {
        self.decryptions.drop_entry(id);
    }

    /// Every message that waited for a PIN is to be opened again: the PIN was given.
    pub fn retry_decryptions_waiting_for_pin(&mut self) {
        let waiting: Vec<Id> =
            self.decryptions.entries.iter().filter(|(_, e)| matches!(e, Entry::Failed(_, Failure::PinNeeded(_), _))).map(|(id, _)| *id).collect();
        for id in waiting {
            self.decryptions.drop_entry(id);
        }
    }

    /// The message `id` opened to, for the reading pane to draw: the stored message as it is now
    /// (its flag, categories, answered marks) with the text and files from the encryption in the
    /// place of the encrypted ones. No command, search, rule or agent reads it (#30 R22, R23).
    pub fn decrypted_message(&self, id: Id) -> Option<Arc<Message>> {
        let Some(Entry::Opened(d)) = self.decryptions.entries.get(&id) else { return None };
        let stored = self.mailbox.messages.get(&id)?;
        let mut cached = d.view.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((from, view)) = cached.as_ref()
            && Arc::ptr_eq(from, stored)
        {
            return Some(view.clone());
        }
        let mut view = Message::clone(stored);
        view.original = None;
        view.kept = None;
        view.unfetched = false;
        view.hidden_text = d.hidden_text.clone();
        view.attachments = d.attachments.clone();
        view.body = d.body.clone();
        let view = Arc::new(view);
        *cached = Some((stored.clone(), view.clone()));
        Some(view)
    }

    /// Whether what message `id` opened to may fetch anything from the web: only an encryption
    /// that detects changes allows that (and then only on the owner's word, never by a switch or
    /// a rule: what is opened is the owner's own mail). `None` when it isn't opened.
    pub fn decrypted_authenticated(&self, id: Id) -> Option<bool> {
        match self.decryptions.entries.get(&id) {
            Some(Entry::Opened(d)) => Some(d.authenticated),
            _ => None,
        }
    }

    /// What message `id` had beside what was encrypted, which isn't shown.
    pub fn decrypted_outside(&self, id: Id) -> Outside {
        match self.decryptions.entries.get(&id) {
            Some(Entry::Opened(d)) => d.outside,
            _ => Outside::default(),
        }
    }

    /// How many opened messages were let go of so far: the window drops what it kept of them
    /// (a layout of the text) when this changes, so that nothing outlives its message here.
    pub fn decryptions_let_go(&self) -> u64 {
        self.decryptions.gone
    }

    /// The messages that are opened now.
    pub fn opened_decryptions(&self) -> Vec<Id> {
        self.decryptions.order.iter().copied().collect()
    }

    /// Whether `m` is the view of a message that was opened and has since been let go of.
    pub fn decrypted_view_gone(&self, m: &Message) -> bool {
        m.original.is_none()
            && self.decryptions.kinds.get(&m.id).is_some_and(|(_, kind)| kind.is_some())
            && !matches!(self.decryptions.entries.get(&m.id), Some(Entry::Opened(_)))
    }

    /// The Digital signature that was inside message `id`'s encryption, when it was opened and
    /// carried one.
    pub fn decrypted_signature(&self, id: Id) -> Option<Arc<DigitalSignature>> {
        match self.decryptions.entries.get(&id) {
            Some(Entry::Opened(d)) => d.signature.clone(),
            _ => None,
        }
    }

    /// Whether `m` is a message made by [`Session::decrypted_message`] (the reading pane draws its
    /// files differently: their numbers are not the stored message's). Told by what the stored
    /// message of an Encrypted message always has and the view never has: the bytes it came as.
    pub fn is_decrypted_view(&self, m: &Message) -> bool {
        m.original.is_none() && matches!(self.decryptions.entries.get(&m.id), Some(Entry::Opened(_)))
    }

    /// Read what the host opened `stored` to, check what is signed inside, and keep what is to be
    /// read. What is beside what was encrypted is not kept, only said.
    fn open_plain(&mut self, stored: &Message, format: SignatureFormat, plain: &Plain) -> Option<Decrypted> {
        // (A message that is MIME is read from the bytes as they are: no second copy of them.)
        let inline_entity: Option<Zeroizing<Vec<u8>>> = plain.inline.then(|| {
            let charset = self.content(stored.id, Part::Original).ok().and_then(|o| outer_charset(&o)).unwrap_or_else(|| "utf-8".to_string());
            let mut entity = format!("Content-Type: text/plain; charset={charset}\r\n\r\n").into_bytes();
            entity.extend_from_slice(&plain.data);
            Zeroizing::new(entity)
        });
        let entity: &[u8] = inline_entity.as_ref().map_or(plain.data.as_slice(), |e| e.as_slice());
        let read = mailcraft_mime::parse(entity);
        let mut outside = outside_of(stored, plain.inline);
        let body = read.body;
        // What is signed inside the encryption. (With text around an inline block, which nobody
        // signed, the signature is never all of the message.)
        let signature = self.signature_inside(stored, format, entity, plain, !outside.text);
        // The signature file isn't one of the files, as in a message that was not encrypted.
        let mut attachments = read.attachments;
        attachments.retain(|a| {
            let mime = a.mime.to_ascii_lowercase();
            !(mime.contains("pkcs7-signature") || mime.contains("pgp-signature"))
        });
        outside.files = outside.files.min(MAX_KINDS);
        // (What is kept: the text, the files and the words that were hidden; not the entity.)
        let bytes = body
            .text
            .len()
            .saturating_add(body.html.as_ref().map_or(0, String::len))
            .saturating_add(read.hidden_text.len())
            .saturating_add(attachments.iter().map(|a| a.data.len()).sum::<usize>());
        Some(Decrypted {
            body,
            attachments,
            hidden_text: read.hidden_text,
            signature,
            format,
            authenticated: plain.authenticated,
            outside,
            bytes,
            view: Mutex::new(None),
        })
    }

    fn signature_inside(
        &mut self,
        stored: &Message,
        format: SignatureFormat,
        entity: &[u8],
        plain: &Plain,
        whole: bool,
    ) -> Option<Arc<DigitalSignature>> {
        match format {
            SignatureFormat::Smime => self.check_signed_bytes(stored, entity).result,
            SignatureFormat::OpenPgp => {
                let outer = self.content(stored.id, Part::Original).ok()?;
                if !plain.signatures.is_empty() {
                    self.check_openpgp(stored, &outer, None, Some((&plain.data, &plain.signatures, whole))).result
                } else {
                    self.check_signed_bytes(stored, entity).result
                }
            }
        }
    }
}

/// What `stored` had beside what was encrypted. Inline: any text but white space outside the
/// armoured block. PGP/MIME or S/MIME: text, or files that aren't the encrypted file, that the
/// message had besides it (a list's footer, files sent in the clear).
pub(crate) fn outside_of(stored: &Message, inline: bool) -> Outside {
    if inline {
        const BEGIN: &str = "-----BEGIN PGP MESSAGE-----";
        const END: &str = "-----END PGP MESSAGE-----";
        let text = &stored.body.text;
        let around = text.find(BEGIN).and_then(|start| {
            let end = text.get(start..)?.find(END)?.checked_add(start)?.checked_add(END.len())?;
            Some((text.get(..start)?.trim().is_empty(), text.get(end..)?.trim().is_empty()))
        });
        return Outside { text: around.is_none_or(|(before, after)| !(before && after)), files: 0 };
    }
    let encrypted_file = |a: &mailcraft_model::Attachment| {
        let mime = a.mime.to_ascii_lowercase();
        mime.contains("pgp-encrypted") || mime.contains("pkcs7-mime") || a.name.eq_ignore_ascii_case("encrypted.asc")
    };
    Outside {
        text: !stored.body.text.trim().is_empty() || stored.body.html.as_ref().is_some_and(|h| !h.trim().is_empty()),
        files: stored.attachments.iter().filter(|a| !encrypted_file(a)).count(),
    }
}

/// The charset the message's own header names for its text (an inline OpenPGP message's text is
/// in that), made of letters, digits and `-_:.` alone; `None` when it names none.
pub(crate) fn outer_charset(original: &[u8]) -> Option<String> {
    let head = original.get(..original.len().min(64 << 10))?;
    let end = head.windows(4).position(|w| w == b"\r\n\r\n").or_else(|| head.windows(2).position(|w| w == b"\n\n")).unwrap_or(head.len());
    let text = String::from_utf8_lossy(head.get(..end)?).replace("\r\n", "\n").replace("\n ", " ").replace("\n\t", " ");
    let line = text.lines().find(|l| l.get(..13).is_some_and(|p| p.eq_ignore_ascii_case("content-type:")))?;
    let lower = line.to_ascii_lowercase();
    let at = lower.find("charset=")?.checked_add(8)?;
    let rest = line.get(at..)?.trim_start().trim_start_matches('"');
    let value: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || "-_:.".contains(*c)).collect();
    // (A name that goes on with anything but the end of the parameter isn't one.)
    let ends = rest.get(value.len()..).and_then(|r| r.chars().next()).is_none_or(|c| c == '"' || c == ';' || c.is_whitespace());
    (ends && !value.is_empty() && value.len() <= 40).then_some(value)
}
