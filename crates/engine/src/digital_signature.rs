// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Digital signatures checked where a message is read (#30): from the bytes a Digitally signed
//! message came as ([`mailcraft_model::Message::original`]), against the certificates the host
//! trusts ([`Session::set_trust_anchors`]: the system's certification authorities on the
//! desktop, none in the browser, whose signers are then "not verified"). The result is worked
//! out when it is first asked for and kept in memory for the session, never written: a
//! certificate that runs out, or trust that changes, is seen at the next start. Nothing is sent
//! anywhere for it, and revocation isn't checked (#30 R7).
//!
//! A signer's certificate that rests on a trusted authority, is valid now and is for the sender
//! is kept for the sender's address ([`mailcraft_model::Mailbox::public_keys`], #30 R9), at the
//! next command: what the check learns is no step of the history (and an undo leaves it).

use std::collections::HashMap;
use std::sync::Arc;

use mailcraft_model::digital_signature::{Coverage, DigitalSignature, SignatureFormat, Verdict, Why};
use mailcraft_model::public_keys::PublicKey;
use mailcraft_model::{Id, Message};

use crate::{Part, Session};

/// The most results kept: past it they are all worked out again as they are asked for.
const MAX_KEPT: usize = 8_192;
/// The most certificates waiting to be kept for their senders.
const MAX_TO_KEEP: usize = 256;

/// What the session knows of Digital signatures. Never saved.
#[derive(Default)]
pub(crate) struct SignatureChecks {
    /// The certificates the host trusts.
    anchors: Arc<mailcraft_smime::Anchors>,
    /// Each message's result, with what it was worked out from ([`Basis`]); `None`: the message
    /// isn't Digitally signed in a format checked here.
    kept: HashMap<Id, (Basis, Option<Arc<DigitalSignature>>)>,
    /// Each opaque signed message as it is shown: its body the content that was signed.
    views: HashMap<Id, (Basis, Option<Arc<Message>>)>,
    /// Whether the host has handed in its authorities yet (`None`: not yet).
    anchors_read: Option<bool>,
    /// Signers' certificates to keep for their addresses at the next command.
    to_keep: Vec<(String, PublicKey)>,
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
    use std::hash::{Hash, Hasher};
    let mut keys = std::collections::hash_map::DefaultHasher::new();
    for k in session.mailbox.public_keys.of(&m.from.email) {
        k.fingerprint.hash(&mut keys);
        k.data.hash(&mut keys);
    }
    Basis {
        original: m.original.as_ref().filter(|o| o.is_there()).map(|o| (o.size(), o.kept.map(|k| k.sha256))),
        from: m.from.email.trim().to_lowercase(),
        known: keys.finish(),
        day: session.now().div_euclid(86_400),
    }
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
        self.views.clear();
        self.to_keep.clear();
    }
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
        let m = self.mailbox.messages.get(&id)?.clone();
        let basis = basis(self, &m);
        if let Some((kept, result)) = self.signatures.kept.get(&id)
            && *kept == basis
        {
            return result.clone();
        }
        let result = self.check_signature(&m).map(Arc::new);
        if self.signatures.kept.len() >= MAX_KEPT {
            self.signatures.kept.clear();
        }
        self.signatures.kept.insert(id, (basis, result.clone()));
        result
    }

    /// Message `id` as it is shown when it is an opaque signed message (`application/pkcs7-mime`
    /// signed data, #30): its body the content that was signed, read from the bytes it came as,
    /// under what the check found. `None` for any other message (shown as it is).
    pub fn signed_view(&mut self, id: Id) -> Option<Arc<Message>> {
        let m = self.mailbox.messages.get(&id)?.clone();
        if !m.has_original() {
            return None;
        }
        let basis = basis(self, &m);
        if let Some((kept, view)) = self.signatures.views.get(&id)
            && *kept == basis
        {
            return view.clone();
        }
        let view = self.opaque_view(&m).map(Arc::new);
        if self.signatures.views.len() >= MAX_KEPT {
            self.signatures.views.clear();
        }
        self.signatures.views.insert(id, (basis, view.clone()));
        view
    }

    fn opaque_view(&mut self, m: &Message) -> Option<Message> {
        // (Only a Digital signature: an Encrypted message's CMS is no signed data.)
        self.digital_signature(m.id)?;
        let bytes = self.content(m.id, Part::Original).ok()?;
        let parts = mailcraft_mime::signed_parts(&bytes)?;
        if parts.kind != mailcraft_mime::SignedKind::SmimeOpaque {
            return None;
        }
        let content = mailcraft_smime::opaque_content(&parts.signature)?;
        let inner = mailcraft_mime::parse(&content);
        let mut view = m.clone();
        view.body = inner.body;
        view.hidden_text = inner.hidden_text;
        Some(view)
    }

    /// Whether message `id` was looked at already (signed or not): the list asks no more of it.
    pub fn digital_signature_checked(&self, id: Id) -> bool {
        self.signatures.kept.contains_key(&id)
    }

    /// The result for message `id` worked out already, if it was: for the list, which never
    /// reads a message's bytes to draw a row.
    pub fn digital_signature_known(&self, id: Id) -> Option<Arc<DigitalSignature>> {
        self.signatures.kept.get(&id).and_then(|(_, r)| r.clone())
    }

    fn check_signature(&mut self, m: &Message) -> Option<DigitalSignature> {
        if m.draft {
            return None;
        }
        if !m.has_original() {
            return looks_smime_signed(m).then(|| DigitalSignature::bare(SignatureFormat::Smime, Verdict::Unchecked, Why::NoOriginal));
        }
        let Ok(bytes) = self.content(m.id, Part::Original) else {
            return looks_smime_signed(m).then(|| DigitalSignature::bare(SignatureFormat::Smime, Verdict::Unchecked, Why::NoOriginal));
        };
        let parts = mailcraft_mime::signed_parts(&bytes)?;
        let signed = match parts.kind {
            mailcraft_mime::SignedKind::SmimeDetached => mailcraft_smime::Signed::Detached { content: &parts.content, signature: &parts.signature },
            mailcraft_mime::SignedKind::SmimeOpaque => mailcraft_smime::Signed::Opaque(&parts.signature),
            // (OpenPGP is checked elsewhere.)
            mailcraft_mime::SignedKind::PgpDetached => return None,
        };
        let known: Vec<Arc<Vec<u8>>> =
            self.mailbox.public_keys.of(&m.from.email).iter().filter(|k| k.format == SignatureFormat::Smime).map(|k| k.data.clone()).collect();
        let known: Vec<&[u8]> = known.iter().map(|k| k.as_slice()).collect();
        let now = self.now();
        // (A message from several people is nobody's alone: its signer's address never matches.)
        let from = if parts.one_from { m.from.email.trim() } else { "" };
        let coverage = if parts.whole { Coverage::Whole } else { Coverage::Part };
        let context = mailcraft_smime::Context { anchors: &self.signatures.anchors, from, now, known: &known, coverage };
        let mut checked = mailcraft_smime::check(signed, &context)?;
        // No authority to trust here at all (none could be read, or not yet): said as that, not as
        // an issuer this computer doesn't trust.
        if checked.signature.why == Some(Why::UnknownIssuer) && self.signatures.anchors.is_empty() {
            checked.signature.why = Some(Why::NoAuthorities);
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
        Some(checked.signature)
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
