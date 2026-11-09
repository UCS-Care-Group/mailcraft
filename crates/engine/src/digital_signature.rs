// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Digital signatures checked where a message is read (#30): from the bytes a Digitally signed
//! message came as ([`mailcraft_model::Message::original`]), against the certificates the host
//! trusts ([`Session::set_trust_anchors`]: the system's certification authorities on the
//! desktop, none in the browser, whose signers are then "not verified"). The result is worked
//! out when it is first asked for and kept in memory, never written, and worked out again when
//! what it rests on changes (the bytes, the sender, the certificates kept for them, the day, the
//! authorities handed in). Nothing is sent anywhere for it, and revocation isn't checked (#30 R7).
//!
//! What an opaque signed message holds is its text and files already, read with it
//! (`mailcraft_mime::parse`); the file it came as is the Digital signature, told here by its bytes
//! ([`Session::digital_signature_part`]).
//!
//! A signer's certificate that rests on a trusted authority, is valid now and is for the sender
//! is kept for the sender's address ([`mailcraft_model::Mailbox::public_keys`], #30 R9), at the
//! next command: what the check learns is no step of the history (and an undo leaves it).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use mailcraft_model::digital_signature::{Coverage, DigitalSignature, SignatureFormat, Verdict, Why};
use mailcraft_model::public_keys::PublicKey;
use mailcraft_model::{Id, Message};

use crate::{Part, Session};

/// The most results kept: past it they are all worked out again as they are asked for.
const MAX_KEPT: usize = 8_192;
/// The most certificates waiting to be kept for their senders.
const MAX_TO_KEEP: usize = 256;
/// The most opaque signed messages of an older build waiting to be read again.
const MAX_TO_READ_AGAIN: usize = 100_000;
/// How many of them are read again before one command.
const READ_AGAIN_AT_ONCE: usize = 32;
/// How many bytes they came as are read again before one command (at least one message's).
const READ_AGAIN_BYTES: u64 = 64 * 1024 * 1024;

/// What the session knows of Digital signatures. Never saved.
#[derive(Default)]
pub(crate) struct SignatureChecks {
    /// The certificates the host trusts.
    anchors: Arc<mailcraft_smime::Anchors>,
    /// What was found of each message, with what it was worked out from ([`Basis`]).
    kept: HashMap<Id, (Basis, Found)>,
    /// Whether the host has handed in its authorities yet (`None`: not yet).
    anchors_read: Option<bool>,
    /// Signers' certificates to keep for their addresses at the next command.
    to_keep: Vec<(String, PublicKey)>,
    /// Opaque signed messages read by a build before #30's, to be read again from the bytes
    /// they came as ([`Session::read_opaque_again`]).
    pub(crate) to_read_again: ToReadAgain,
    /// [`READ_AGAIN_BYTES`] but where a test sets another.
    pub(crate) read_again_bytes: Option<u64>,
    /// The hash of the keys kept for each address, with the revision of the keys it was worked
    /// out at: a row's result is asked of at every frame, and the keys change seldom.
    key_hashes: Mutex<HashMap<String, (u64, u64)>>,
    /// How many times keys were hashed, for the tests.
    #[cfg(test)]
    pub(crate) hashed: std::sync::atomic::AtomicUsize,
}

/// The messages to read again, in order, with a set beside them so that whether one is there
/// already is answered at once (a command may change a hundred thousand messages).
#[derive(Default)]
pub(crate) struct ToReadAgain {
    order: Vec<Id>,
    there: HashSet<Id>,
    /// How many times the queue was asked whether it holds a message, for the tests.
    #[cfg(test)]
    pub(crate) asked: usize,
}

impl ToReadAgain {
    /// Note `id`, unless it is noted already or [`MAX_TO_READ_AGAIN`] are.
    fn push(&mut self, id: Id) {
        #[cfg(test)]
        {
            self.asked += 1;
        }
        if self.order.len() < MAX_TO_READ_AGAIN && self.there.insert(id) {
            self.order.push(id);
        }
    }

    fn last(&self) -> Option<Id> {
        self.order.last().copied()
    }

    fn pop(&mut self) {
        if let Some(id) = self.order.pop() {
            self.there.remove(&id);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.order.clear();
        self.there.clear();
    }
}

/// What a check found of one message.
#[derive(Clone, Default)]
struct Found {
    /// The result; `None`: the message isn't Digitally signed in a format checked here.
    result: Option<Arc<DigitalSignature>>,
    /// Which of its attachments is the Digital signature itself (`smime.p7s`, or the
    /// `smime.p7m` an opaque signed message is): the very part that was checked, by its bytes.
    part: Option<usize>,
}

/// What a result rests on: the bytes the message came as (their size and where they are kept),
/// its From address, the Public keys kept for that address (every one of them, not how many: one
/// put in the place of another changes it), and the day (a certificate runs out on one).
#[derive(Clone, PartialEq, Eq)]
struct Basis {
    original: Option<(u64, Option<[u8; 32]>)>,
    from: String,
    known: u64,
    day: i64,
}

fn basis(session: &Session, m: &Message) -> Basis {
    let from = m.from.email.trim().to_lowercase();
    Basis {
        original: m.original.as_ref().filter(|o| o.is_there()).map(|o| (o.size(), o.kept.map(|k| k.sha256))),
        known: keys_hash(session, &from),
        from,
        day: session.now().div_euclid(86_400),
    }
}

/// The hash of the keys kept for `address`: worked out once for each revision of the keys.
fn keys_hash(session: &Session, address: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let keys = &session.mailbox.public_keys;
    let revision = keys.revision();
    let mut cache = session.signatures.key_hashes.lock().ok();
    if let Some((at, hash)) = cache.as_ref().and_then(|c| c.get(address))
        && *at == revision
    {
        return *hash;
    }
    #[cfg(test)]
    session.signatures.hashed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for k in keys.of(address) {
        k.fingerprint.hash(&mut hasher);
        k.data.hash(&mut hasher);
    }
    let hash = hasher.finish();
    if let Some(cache) = cache.as_mut() {
        if cache.len() >= MAX_KEPT {
            cache.clear();
        }
        cache.insert(address.to_string(), (revision, hash));
    }
    hash
}

/// Whether what is here of `m` (its parts, without the bytes it came as) carries an S/MIME
/// signature: its `smime.p7s`, or S/MIME's signed data in one part.
fn looks_smime_signed(m: &Message) -> bool {
    m.attachments.iter().any(|a| {
        let mime = a.mime.to_ascii_lowercase();
        mime.contains("pkcs7-signature") || mime.contains("signed-data") || a.name.eq_ignore_ascii_case("smime.p7s")
    })
}

impl SignatureChecks {
    /// Another mailbox took the place of the one these were worked out for.
    pub(crate) fn forget_messages(&mut self) {
        self.kept.clear();
        self.to_keep.clear();
        self.to_read_again.clear();
    }
}

/// Whether `m` is an opaque signed message as a build before #30's read it: the file it came
/// as and nothing else (no text, no other file), with the bytes it came as to read it again from.
fn read_bare(m: &Message) -> bool {
    !m.draft
        && m.has_original()
        && !m.body_is_kept()
        && m.body.text.trim().is_empty()
        && m.body.html.is_none()
        && matches!(m.attachments.as_slice(), [only] if only.mime.to_ascii_lowercase().contains("pkcs7-mime"))
}

impl Session {
    /// Hand in the certificates this computer trusts, in DER (the host reads them from the
    /// system; one that isn't a certificate is left out). Results worked out before are worked
    /// out again.
    pub fn set_trust_anchors<I, B>(&mut self, ders: I)
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        self.signatures.anchors = Arc::new(mailcraft_smime::Anchors::new(ders));
        self.signatures.anchors_read = Some(!self.signatures.anchors.is_empty());
        self.signatures.kept.clear();
    }

    /// Whether the host has handed in its authorities yet, and whether any could be read.
    pub fn trust_anchors_read(&self) -> Option<bool> {
        self.signatures.anchors_read
    }

    /// How many trust anchors the host handed in.
    pub fn trust_anchor_count(&self) -> usize {
        self.signatures.anchors.len()
    }

    /// What checking message `id`'s Digital signature finds, when it is Digitally signed in a
    /// format checked here (S/MIME): worked out from the bytes it came as the first time it is
    /// asked, then kept for the session. A message that looks signed but was read before those
    /// bytes were kept can't be checked ([`Why::NoOriginal`]: Check again downloads them).
    /// `None` for any other message.
    pub fn digital_signature(&mut self, id: Id) -> Option<Arc<DigitalSignature>> {
        self.found(id).result
    }

    /// Which attachment of message `id` is its Digital signature (the part its result was worked
    /// out from, found by its bytes): shown in the result's Details rather than among the files.
    /// `None` when there is none, or none could be told.
    pub fn digital_signature_part(&mut self, id: Id) -> Option<usize> {
        self.found(id).part
    }

    fn found(&mut self, id: Id) -> Found {
        let Some(m) = self.mailbox.messages.get(&id).cloned() else { return Found::default() };
        let basis = basis(self, &m);
        if let Some((kept, found)) = self.signatures.kept.get(&id)
            && *kept == basis
        {
            return found.clone();
        }
        let found = self.check_signature(&m);
        if self.signatures.kept.len() >= MAX_KEPT {
            self.signatures.kept.clear();
        }
        self.signatures.kept.insert(id, (basis, found.clone()));
        found
    }

    /// What was found of message `id` already, when it still holds (nothing it rests on changed).
    fn found_already(&self, id: Id) -> Option<&Found> {
        let m = self.mailbox.messages.get(&id)?;
        self.signatures.kept.get(&id).filter(|(kept, _)| *kept == basis(self, m)).map(|(_, found)| found)
    }

    /// Whether message `id` was looked at already (signed or not), and nothing it rests on has
    /// changed since: the list asks no more of it.
    pub fn digital_signature_checked(&self, id: Id) -> bool {
        self.found_already(id).is_some()
    }

    /// The result for message `id` worked out already, if it was and still holds (its keys, its
    /// sender and the day are as they were): for the list, which never reads a message's bytes to
    /// draw a row.
    pub fn digital_signature_known(&self, id: Id) -> Option<Arc<DigitalSignature>> {
        self.found_already(id).and_then(|f| f.result.clone())
    }

    fn check_signature(&mut self, m: &Message) -> Found {
        let bare = |why| Found { result: Some(Arc::new(DigitalSignature::bare(SignatureFormat::Smime, Verdict::Unchecked, why))), part: None };
        if m.draft {
            return Found::default();
        }
        let Some(bytes) = m.has_original().then(|| self.content(m.id, Part::Original).ok()).flatten() else {
            return if looks_smime_signed(m) { bare(Why::NoOriginal) } else { Found::default() };
        };
        let Some(parts) = mailcraft_mime::signed_parts(&bytes) else { return Found::default() };
        let signed = match parts.kind {
            mailcraft_mime::SignedKind::SmimeDetached => mailcraft_smime::Signed::Detached { content: &parts.content, signature: &parts.signature },
            mailcraft_mime::SignedKind::SmimeOpaque => mailcraft_smime::Signed::Opaque(&parts.signature),
            // (OpenPGP is checked elsewhere.)
            mailcraft_mime::SignedKind::PgpDetached => return Found::default(),
        };
        let known: Vec<Arc<Vec<u8>>> =
            self.mailbox.public_keys.of(&m.from.email).iter().filter(|k| k.format == SignatureFormat::Smime).map(|k| k.data.clone()).collect();
        let known: Vec<&[u8]> = known.iter().map(|k| k.as_slice()).collect();
        let now = self.now();
        // (A message from several people is nobody's alone: its signer's address never matches.)
        let from = if parts.one_from { m.from.email.trim() } else { "" };
        let coverage = if parts.whole { Coverage::Whole } else { Coverage::Part };
        let context = mailcraft_smime::Context { anchors: &self.signatures.anchors, from, now, known: &known, coverage };
        let Some(mut checked) = mailcraft_smime::check(signed, &context) else { return Found::default() };
        // No authority to trust here at all: said as that (none could be read, or they haven't
        // been yet), not as an issuer this computer doesn't trust.
        if checked.signature.why == Some(Why::UnknownIssuer) && self.signatures.anchors.is_empty() {
            checked.signature.why = Some(if self.signatures.anchors_read.is_none() { Why::AuthoritiesNotRead } else { Why::NoAuthorities });
        }
        if let (Some(der), Some(signer)) = (checked.certificate, checked.signature.signer.as_ref())
            && self.signatures.to_keep.len() < MAX_TO_KEEP
        {
            let key = PublicKey {
                format: SignatureFormat::Smime,
                data: Arc::new(der),
                fingerprint: signer.fingerprint.clone(),
                not_after: signer.not_after,
                seen: m.received.max(m.date),
            };
            self.signatures.to_keep.push((m.from.email.trim().to_string(), key));
        }
        Found { result: Some(Arc::new(checked.signature)), part: self.signature_part(m, &parts.signature) }
    }

    /// Note the opaque signed messages a build before #30's read (nothing but the file they came
    /// as): a mailbox was just opened. Nothing is read here; [`Session::read_opaque_again`] does.
    pub(crate) fn find_opaque_to_read_again(&mut self) {
        let found: Vec<Id> = self.mailbox.messages.values().filter(|m| read_bare(m)).map(|m| m.id).take(MAX_TO_READ_AGAIN).collect();
        self.signatures.to_read_again.clear();
        found.into_iter().for_each(|id| self.signatures.to_read_again.push(id));
    }

    /// Note again the messages among `ids` that are as an older build read them (an undo put
    /// one back as it was, or the bytes it came as arrived for it): the next command reads them.
    /// (Most messages a command changes aren't: they are let go of before the queue is looked at.)
    pub(crate) fn note_to_read_again(&mut self, ids: impl IntoIterator<Item = Id>) {
        for id in ids {
            if self.mailbox.message(id).is_some_and(read_bare) {
                self.signatures.to_read_again.push(id);
            }
        }
    }

    /// Read the text and files of up to [`READ_AGAIN_AT_ONCE`] of the messages noted
    /// ([`Session::find_opaque_to_read_again`], [`Session::note_to_read_again`]), and of at most
    /// [`READ_AGAIN_BYTES`] of the bytes they came as (always one, however big), from those
    /// bytes (with the caps every message is read with), once each: before a command, never
    /// inside one and never while a message is only looked at. No step of the history; the
    /// mailbox is changed (saved, and what is worked out from it is due again) when one was read.
    pub(crate) fn read_opaque_again(&mut self) {
        let budget = self.signatures.read_again_bytes.unwrap_or(READ_AGAIN_BYTES);
        let mut spent: u64 = 0;
        let mut changed = false;
        for _ in 0..READ_AGAIN_AT_ONCE {
            let Some(id) = self.signatures.to_read_again.last() else { break };
            let Some(m) = self.mailbox.message(id).filter(|m| read_bare(m)) else {
                self.signatures.to_read_again.pop();
                continue;
            };
            let size = m.original.as_ref().map_or(0, mailcraft_model::Original::size);
            if spent > 0 && spent.saturating_add(size) > budget {
                break;
            }
            spent = spent.saturating_add(size);
            self.signatures.to_read_again.pop();
            let Ok(bytes) = self.content(id, Part::Original) else { continue };
            if !mailcraft_mime::signed_parts(&bytes).is_some_and(|p| p.kind == mailcraft_mime::SignedKind::SmimeOpaque) {
                continue;
            }
            let read = mailcraft_mime::parse(&bytes);
            let files: Vec<mailcraft_model::Attachment> = read.attachments.into_iter().skip(1).collect();
            if read.body.text.trim().is_empty() && read.body.html.is_none() && files.is_empty() {
                continue;
            }
            if let Ok(stored) = self.mailbox.message_mut(id) {
                stored.body = read.body;
                stored.hidden_text = read.hidden_text;
                stored.attachments.extend(files);
                changed = true;
            }
        }
        if changed {
            self.list_due();
            self.dirty = true;
        }
    }

    /// The attachment of `m` that is the Digital signature `signature`: of S/MIME's type, and
    /// those very bytes (a file of the same name or type sent along is another part).
    pub(crate) fn signature_part(&self, m: &Message, signature: &[u8]) -> Option<usize> {
        m.attachments.iter().enumerate().find_map(|(i, a)| {
            let fits = a.mime.to_ascii_lowercase().contains("pkcs7") && a.size() == signature.len();
            (fits && self.content(m.id, Part::Attachment(i)).is_ok_and(|bytes| bytes.as_slice() == signature)).then_some(i)
        })
    }

    /// Keep the signers' certificates the checks since the last command found (#30 R9): no step
    /// of the history, and saved with the mailbox.
    pub(crate) fn keep_found_certificates(&mut self) {
        if self.signatures.to_keep.is_empty() {
            return;
        }
        let mut changed = false;
        for (address, key) in std::mem::take(&mut self.signatures.to_keep) {
            changed |= self.mailbox.public_keys.add(&address, key);
        }
        if changed {
            self.dirty = true;
        }
    }
}
