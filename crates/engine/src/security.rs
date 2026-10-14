//! Signed and encrypted mail in the session: what the reading pane says about a message
//! ([`SecurityView`]), protecting a message that goes out ([`protect`]), and the owner's keys.
//!
//! Public keys and certificates live in the mailbox ([`mailcraft_model::security::StoredKey`]).
//! Private keys live here, in memory, for the session ([`SecurityState`]): the host puts them in
//! the operating system's credential store when the owner wants them remembered
//! ([`Session::take_secrets_to_remember`]) and hands them back at the next start
//! ([`Session::restore_secret`]). They are never in the mailbox, the settings, a command's
//! params or answer, or a log line. Neither is a passphrase: the window sets one for the next
//! import only ([`Session::set_passphrase`]), and it is dropped once used.
//!
//! What an encrypted message says is worked out when it is shown and kept in memory only; the
//! mailbox keeps it encrypted.

use std::collections::HashMap;
use std::sync::Arc;

use mailcraft_crypto as crypto;
use mailcraft_model::security::{KeyScheme, StoredKey};
use mailcraft_model::{Attachment, Body, Id, Message, Protect, Protection, Secured};
use mailcraft_time::Timestamp;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::{CmdError, Session};

/// Most opened messages kept in memory.
const MAX_VIEWS: usize = 256;

/// Private keys and what was worked out about messages, for this session only.
#[derive(Default)]
pub struct SecurityState {
    secrets: Vec<crypto::SecretKey>,
    views: HashMap<Id, (u64, Arc<SecurityView>)>,
    passphrase: Option<Zeroizing<String>>,
    remember: Vec<crypto::SecretKey>,
    forget: Vec<String>,
    /// Moves on whenever a key comes or goes, so that what was worked out is worked out again.
    generation: u64,
}

impl std::fmt::Debug for SecurityState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityState").field("secrets", &self.secrets.len()).field("views", &self.views.len()).finish()
    }
}

impl SecurityState {
    fn changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.views.clear();
    }

    fn add_secret(&mut self, secret: crypto::SecretKey) {
        self.secrets.retain(|s| !(s.scheme == secret.scheme && s.fingerprint == secret.fingerprint));
        self.secrets.push(secret);
        self.changed();
    }

    pub(crate) fn has_secret(&self, scheme: KeyScheme, fingerprint: &str) -> bool {
        self.secrets.iter().any(|s| s.scheme == to_scheme(scheme) && s.fingerprint == fingerprint)
    }
}

fn to_scheme(s: KeyScheme) -> crypto::Scheme {
    match s {
        KeyScheme::Smime => crypto::Scheme::Smime,
        KeyScheme::OpenPgp => crypto::Scheme::OpenPgp,
    }
}

fn from_scheme(s: crypto::Scheme) -> KeyScheme {
    match s {
        crypto::Scheme::Smime => KeyScheme::Smime,
        crypto::Scheme::OpenPgp => KeyScheme::OpenPgp,
    }
}

/// The name a private key goes under in the credential store.
fn store_name(scheme: crypto::Scheme, fingerprint: &str) -> String {
    format!("key:{}:{fingerprint}", scheme.code())
}

impl Session {
    /// The passphrase for the next key import (a `.p12` file, a protected OpenPGP key). Kept in
    /// memory until that import, then dropped.
    pub fn set_passphrase(&mut self, passphrase: &str) {
        self.security.passphrase = Some(Zeroizing::new(passphrase.to_string()));
    }

    /// Private keys imported since the last call, for the host to keep in the operating system's
    /// credential store: the name each goes under and the text to keep. Nothing else may keep it.
    pub fn take_secrets_to_remember(&mut self) -> Vec<(String, Zeroizing<String>)> {
        std::mem::take(&mut self.security.remember)
            .into_iter()
            .map(|s| {
                let text = Zeroizing::new(format!("{}:{}:{}", s.scheme.code(), s.fingerprint, crate::cmd::base64_encode(&s.data)));
                (store_name(s.scheme, &s.fingerprint), text)
            })
            .collect()
    }

    /// The credential-store names of private keys the owner removed since the last call.
    pub fn take_secrets_to_forget(&mut self) -> Vec<String> {
        std::mem::take(&mut self.security.forget)
    }

    /// Whether the private key of a kept key is here, in memory.
    pub fn security_has_secret(&self, scheme: KeyScheme, fingerprint: &str) -> bool {
        self.security.has_secret(scheme, fingerprint)
    }

    /// The credential-store names of the private keys the mailbox says the owner has.
    pub fn secret_names(&self) -> Vec<String> {
        self.mailbox.keys.iter().filter(|k| k.own).map(|k| store_name(to_scheme(k.scheme), &k.fingerprint)).collect()
    }

    /// Take back a private key the host kept ([`Session::take_secrets_to_remember`]). Says
    /// whether it was one.
    pub fn restore_secret(&mut self, text: &str) -> bool {
        let mut it = text.splitn(3, ':');
        let (Some(scheme), Some(fingerprint), Some(data)) = (it.next(), it.next(), it.next()) else { return false };
        let Some(scheme) = crypto::Scheme::parse(scheme) else { return false };
        let Ok(data) = crate::cmd::base64_decode(data) else { return false };
        if fingerprint.is_empty() || fingerprint.len() > 128 || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
        self.security.add_secret(crypto::SecretKey { scheme, fingerprint: fingerprint.to_string(), data: Zeroizing::new(data) });
        true
    }

    /// [`Session::security_view`] as far as it was worked out already (nothing is worked out).
    pub fn cached_security_view(&self, id: Id) -> Option<Arc<SecurityView>> {
        self.security.views.get(&id).map(|(_, view)| view.clone())
    }

    /// What the reading pane says about a message's signature and encryption, and what an
    /// encrypted one says once opened; `None` for a message that is neither signed nor
    /// encrypted. Worked out once, then kept in memory.
    pub fn security_view(&mut self, id: Id) -> Option<Arc<SecurityView>> {
        let m = self.mailbox.message(id)?;
        let inline = m.secured.is_none() && mailcraft_mime::secure::inline_pgp(&m.body.text).is_some();
        if m.secured.is_none() && !inline {
            return None;
        }
        let stamp = self.security.generation ^ (m.secured.as_ref().map_or(0, |s| s.raw.len() as u64) << 20) ^ m.body.text.len() as u64;
        if let Some((at, view)) = self.security.views.get(&id)
            && *at == stamp
        {
            return Some(view.clone());
        }
        let view = Arc::new(compute(self, m));
        if self.security.views.len() >= MAX_VIEWS {
            self.security.views.clear();
        }
        self.security.views.insert(id, (stamp, view.clone()));
        Some(view)
    }
}

/// How the reading pane shows a message's protection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Signed by the sender, and somebody the owner trusts vouches for them.
    Good,
    /// Signed and intact, but nobody the owner trusts vouches for the signer, or the signer isn't
    /// the sender the message names; or encrypted and opened without a signature.
    Notice,
    /// The signature doesn't hold.
    Bad,
    /// Encrypted, and not opened.
    Locked,
}

/// What checking a signature found, for people.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureView {
    pub scheme: KeyScheme,
    pub validity: SigState,
    /// Why it isn't valid, when it isn't.
    pub detail: String,
    pub signer: String,
    pub signer_emails: Vec<String>,
    pub fingerprint: String,
    /// Somebody the owner trusts vouches for the signer (a certificate authority, or the owner
    /// for an OpenPGP key).
    pub verified: bool,
    /// Why not, when not.
    pub trust_note: String,
    /// The signer's key is for the address the message says it is from.
    pub from_matches: bool,
    pub signed_at: Option<Timestamp>,
    /// Inline (`-----BEGIN PGP SIGNED MESSAGE-----`): only the text between the markers is signed.
    pub inline: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigState {
    Valid,
    Invalid,
    UnknownSigner,
    Unsupported,
}

impl SigState {
    pub fn code(self) -> &'static str {
        match self {
            SigState::Valid => "valid",
            SigState::Invalid => "invalid",
            SigState::UnknownSigner => "unknownSigner",
            SigState::Unsupported => "unsupported",
        }
    }
}

/// A message's signature and encryption as the reading pane shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecurityView {
    pub signature: Option<SignatureView>,
    /// It was encrypted (with this scheme).
    pub encrypted: Option<KeyScheme>,
    /// Why it couldn't be opened, when it couldn't.
    pub locked: Option<String>,
    /// What it says, once opened (decrypted, or unwrapped from its signature).
    pub opened: Option<(Body, Vec<Attachment>)>,
    /// Keys the signature carries that the mailbox doesn't have yet.
    pub collectable: Vec<StoredKey>,
}

impl SecurityView {
    pub fn level(&self) -> Level {
        if self.locked.is_some() {
            return Level::Locked;
        }
        match &self.signature {
            Some(sig) if sig.validity == SigState::Invalid => Level::Bad,
            Some(sig) if sig.validity == SigState::Valid && sig.verified && sig.from_matches => Level::Good,
            Some(_) => Level::Notice,
            None if self.encrypted.is_some() => Level::Good,
            None => Level::Notice,
        }
    }

    /// One line for the reading pane's bar.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(scheme) = self.encrypted {
            parts.push(match &self.locked {
                Some(why) => format!("Encrypted ({}). It can't be opened: {why}.", scheme.label()),
                None => format!("Encrypted ({}).", scheme.label()),
            });
        }
        if let Some(sig) = &self.signature {
            let who = if sig.signer.is_empty() { sig.signer_emails.first().cloned().unwrap_or_default() } else { sig.signer.clone() };
            parts.push(match sig.validity {
                SigState::Valid if sig.verified && sig.from_matches => format!("Signed by {who}; the signature is valid."),
                SigState::Valid if !sig.from_matches => {
                    format!("Signed by {who} ({}), who is not the sender this message names.", sig.signer_emails.join(", "))
                }
                SigState::Valid => {
                    format!("Signed by {who}; the signature is mathematically valid, but the signer is not verified: {}.", sig.trust_note)
                }
                SigState::Invalid => format!("The signature is not valid: {}. The message may have been changed.", sig.detail),
                SigState::UnknownSigner => "Signed with a key that isn't here, so the signature can't be checked.".to_string(),
                SigState::Unsupported => format!("Signed in a way this program doesn't check: {}.", sig.detail),
            });
            if sig.inline {
                parts.push("Only the text between the PGP markers is signed.".into());
            }
        }
        parts.join(" ")
    }

    /// For agents (`security.check`). Says nothing of what the message says.
    pub fn to_json(&self) -> Value {
        let level = match self.level() {
            Level::Good => "good",
            Level::Notice => "notice",
            Level::Bad => "bad",
            Level::Locked => "locked",
        };
        json!({
            "level": level,
            "summary": self.summary(),
            "encrypted": self.encrypted.map(KeyScheme::code),
            "opened": self.opened.is_some(),
            "locked": self.locked,
            "signature": self.signature.as_ref().map(|s| json!({
                "scheme": s.scheme.code(),
                "validity": s.validity.code(),
                "detail": s.detail,
                "signer": s.signer,
                "signerEmails": s.signer_emails,
                "fingerprint": s.fingerprint,
                "verified": s.verified,
                "trustNote": s.trust_note,
                "fromMatches": s.from_matches,
                "signedAt": s.signed_at.map(mailcraft_time::format_iso_utc),
                "inline": s.inline,
            })),
            "collectable": self.collectable.iter().map(|k| json!({"scheme": k.scheme.code(), "fingerprint": k.fingerprint, "name": k.name, "emails": k.emails})).collect::<Vec<_>>(),
        })
    }

    /// The message as the reading pane shows it: what it says once opened.
    pub fn apply(&self, m: &Message) -> Message {
        let mut shown = m.clone();
        if let Some((body, attachments)) = &self.opened {
            shown.body = body.clone();
            shown.attachments = attachments.clone();
        }
        shown
    }
}

/// A kept key as the cryptography takes it.
pub(crate) fn to_crypto(k: &StoredKey) -> crypto::PublicKey {
    crypto::PublicKey {
        scheme: to_scheme(k.scheme),
        fingerprint: k.fingerprint.clone(),
        name: k.name.clone(),
        emails: k.emails.clone(),
        issuer: k.issuer.clone(),
        not_before: k.not_before,
        not_after: k.not_after,
        is_ca: k.is_ca,
        self_signed: false,
        can_sign: true,
        can_encrypt: true,
        data: k.data.to_vec(),
    }
}

/// A key the cryptography read, as the mailbox keeps it.
pub(crate) fn to_stored(k: &crypto::PublicKey) -> StoredKey {
    StoredKey {
        scheme: from_scheme(k.scheme),
        fingerprint: k.fingerprint.clone(),
        name: k.name.clone(),
        emails: k.emails.clone(),
        issuer: k.issuer.clone(),
        not_before: k.not_before,
        not_after: k.not_after,
        is_ca: k.is_ca,
        data: Arc::new(k.data.clone()),
        ..Default::default()
    }
}

struct Keys {
    known: Vec<crypto::PublicKey>,
    anchors: Vec<crypto::PublicKey>,
    /// The owner's certificates that have their private key here.
    identities: Vec<(crypto::PublicKey, crypto::SecretKey)>,
    secrets: Vec<crypto::SecretKey>,
}

fn keys(s: &Session) -> Keys {
    let known: Vec<crypto::PublicKey> = s.mailbox.keys.iter().map(to_crypto).collect();
    let anchors = s.mailbox.keys.iter().filter(|k| k.trusted && k.scheme == KeyScheme::Smime).map(to_crypto).collect();
    let identities = known
        .iter()
        .filter_map(|k| {
            s.security.secrets.iter().find(|sec| sec.scheme == k.scheme && sec.fingerprint == k.fingerprint).map(|sec| (k.clone(), sec.clone()))
        })
        .collect();
    Keys { known, anchors, identities, secrets: s.security.secrets.clone() }
}

fn signature_view(s: &Session, m: &Message, check: &crypto::SignatureCheck, inline: bool) -> SignatureView {
    let (validity, detail) = match &check.validity {
        crypto::Validity::Valid => (SigState::Valid, String::new()),
        crypto::Validity::Invalid(why) => (SigState::Invalid, why.clone()),
        crypto::Validity::UnknownSigner => (SigState::UnknownSigner, String::new()),
        crypto::Validity::Unsupported(why) => (SigState::Unsupported, why.clone()),
    };
    let signer = check.signer.as_ref();
    let fingerprint = signer.map(|k| k.fingerprint.clone()).unwrap_or_default();
    let (mut verified, mut trust_note) = match &check.trust {
        crypto::Trust::Verified(by) => (true, format!("vouched for by {by}")),
        crypto::Trust::NotVerified(why) => (false, why.clone()),
    };
    // An OpenPGP key is vouched for by the owner.
    if check.scheme == crypto::Scheme::OpenPgp
        && s.mailbox.keys.iter().any(|k| k.scheme == KeyScheme::OpenPgp && k.fingerprint == fingerprint && (k.trusted || k.own))
    {
        verified = true;
        trust_note = "you vouched for this key".into();
    }
    let emails = check.signer_emails().to_vec();
    let from = m.from.email.trim().to_ascii_lowercase();
    SignatureView {
        scheme: from_scheme(check.scheme),
        validity,
        detail,
        signer: signer.map(|k| k.name.clone()).unwrap_or_default(),
        from_matches: !from.is_empty() && emails.contains(&from),
        signer_emails: emails,
        fingerprint,
        verified: verified && validity == SigState::Valid,
        trust_note,
        signed_at: check.signed_at,
        inline,
    }
}

fn collectable(s: &Session, check: &crypto::SignatureCheck) -> Vec<StoredKey> {
    let mut out: Vec<StoredKey> = Vec::new();
    let candidates = check.carried.iter().chain(check.signer.iter());
    for k in candidates {
        let kept = s.mailbox.keys.iter().any(|x| x.scheme == from_scheme(k.scheme) && x.fingerprint == k.fingerprint);
        if !kept && !out.iter().any(|o| o.fingerprint == k.fingerprint) {
            let mut stored = to_stored(k);
            stored.collected = true;
            out.push(stored);
        }
    }
    out
}

fn compute(s: &Session, m: &Message) -> SecurityView {
    let keys = keys(s);
    let now = s.now();
    let mut view = SecurityView::default();
    let mut current: Option<Secured> = m.secured.clone();
    let mut seen_signature = false;
    let mut note = |view: &mut SecurityView, check: &crypto::SignatureCheck, inline: bool| {
        // The outermost signature is the one shown; the keys of every one are offered.
        if !seen_signature {
            view.signature = Some(signature_view(s, m, check, inline));
            seen_signature = true;
        }
        view.collectable.extend(collectable(s, check));
    };
    // Signed inside encrypted (and the like): a few layers at most.
    for _ in 0..3 {
        let Some(sec) = current.take() else { break };
        match sec.kind {
            Protection::SmimeSigned | Protection::PgpSigned => {
                let Some(parts) = mailcraft_mime::secure::signed_parts(&sec.raw) else {
                    let check = crypto::SignatureCheck {
                        scheme: if sec.kind == Protection::SmimeSigned { crypto::Scheme::Smime } else { crypto::Scheme::OpenPgp },
                        validity: crypto::Validity::Invalid("the signed message is broken".into()),
                        trust: crypto::Trust::NotVerified(String::new()),
                        signer: None,
                        signed_at: None,
                        carried: Vec::new(),
                    };
                    note(&mut view, &check, false);
                    break;
                };
                let check = if sec.kind == Protection::SmimeSigned {
                    crypto::smime::verify_detached(&parts.content, &parts.signature, &keys.known, &keys.anchors, now)
                } else {
                    crypto::openpgp::verify_detached(&parts.content, &parts.signature, &keys.known)
                };
                note(&mut view, &check, false);
                if view.opened.is_some() {
                    // Inside an encryption: what was signed is what the message says.
                    let inner = mailcraft_mime::secure::read_entity(&parts.content);
                    view.opened = Some((inner.body, inner.attachments));
                }
            }
            Protection::SmimeSignedData | Protection::SmimeEncrypted => {
                let Some(payload) = mailcraft_mime::secure::encrypted_payload(&sec.raw) else {
                    view.encrypted = Some(KeyScheme::Smime);
                    view.locked = Some("the encrypted part is missing".into());
                    break;
                };
                let identities: Vec<crypto::smime::Identity<'_>> = keys
                    .identities
                    .iter()
                    .filter(|(k, _)| k.scheme == crypto::Scheme::Smime)
                    .map(|(k, sec)| crypto::smime::Identity { certificate: k, secret: sec })
                    .collect();
                match crypto::smime::open(&payload, &identities, &keys.known, &keys.anchors, now) {
                    Ok(opened) => {
                        if opened.encrypted {
                            view.encrypted = Some(KeyScheme::Smime);
                        }
                        if let Some(check) = &opened.signature {
                            note(&mut view, check, false);
                        }
                        let inner = mailcraft_mime::secure::read_entity(&opened.content);
                        view.opened = Some((inner.body.clone(), inner.attachments.clone()));
                        current = inner.secured;
                    }
                    Err(e) => {
                        view.encrypted = Some(KeyScheme::Smime);
                        view.locked = Some(e.to_string());
                    }
                }
            }
            Protection::PgpEncrypted => {
                view.encrypted = Some(KeyScheme::OpenPgp);
                let Some(payload) = mailcraft_mime::secure::encrypted_payload(&sec.raw) else {
                    view.locked = Some("the encrypted part is missing".into());
                    break;
                };
                match crypto::openpgp::decrypt(&payload, &keys.secrets, &keys.known) {
                    Ok(d) => {
                        if let Some(check) = &d.signature {
                            note(&mut view, check, false);
                        }
                        let inner = mailcraft_mime::secure::read_entity(&d.content);
                        view.opened = Some((inner.body.clone(), inner.attachments.clone()));
                        current = inner.secured;
                    }
                    Err(e) => view.locked = Some(e.to_string()),
                }
            }
        }
    }
    if m.secured.is_none()
        && let Some((kind, block)) = mailcraft_mime::secure::inline_pgp(&m.body.text)
    {
        match kind {
            mailcraft_mime::secure::InlinePgp::Signed => match crypto::openpgp::verify_cleartext(block, &keys.known) {
                Ok((check, _)) => note(&mut view, &check, true),
                Err(e) => {
                    let check = crypto::SignatureCheck {
                        scheme: crypto::Scheme::OpenPgp,
                        validity: crypto::Validity::Invalid(e.to_string()),
                        trust: crypto::Trust::NotVerified(String::new()),
                        signer: None,
                        signed_at: None,
                        carried: Vec::new(),
                    };
                    note(&mut view, &check, true);
                }
            },
            mailcraft_mime::secure::InlinePgp::Encrypted => {
                view.encrypted = Some(KeyScheme::OpenPgp);
                match crypto::openpgp::decrypt(block.as_bytes(), &keys.secrets, &keys.known) {
                    Ok(d) => {
                        if let Some(check) = &d.signature {
                            note(&mut view, check, true);
                        }
                        let text = m.body.text.replacen(block, &mailcraft_mime::secure::inline_text(&d.content), 1);
                        view.opened = Some((Body { text, html: None }, m.attachments.clone()));
                    }
                    Err(e) => view.locked = Some(e.to_string()),
                }
            }
        }
    }
    view
}

/// Sign and/or encrypt a message that is about to go out, in place: its body and attachments go
/// into [`Message::secured`] as S/MIME or PGP/MIME. An encrypted message goes to every
/// recipient's key and to the owner's own (so that Sent Items can be read), and its body here is
/// emptied (what it said stays encrypted). Refused, naming who, when a recipient has no key.
pub(crate) fn protect(s: &Session, m: &mut Message, want: Protect, prefer: Option<KeyScheme>) -> Result<(), CmdError> {
    if want.is_none() {
        return Ok(());
    }
    let now = s.now();
    let from = m.from.email.trim().to_ascii_lowercase();
    let keys = keys(s);
    let own = |scheme: KeyScheme| keys.identities.iter().find(|(k, _)| k.scheme == to_scheme(scheme) && k.is_for(&from) && k.in_force(now));
    let recipients: Vec<String> = m.recipients().map(|a| a.email.trim().to_ascii_lowercase()).filter(|e| !e.is_empty()).collect();
    let key_for = |scheme: KeyScheme, email: &str| -> Option<&crypto::PublicKey> {
        // The newest key in force for the address.
        keys.known
            .iter()
            .filter(|k| k.scheme == to_scheme(scheme) && k.is_for(email) && k.in_force(now) && !k.is_ca)
            .max_by_key(|k| k.not_before.unwrap_or(0))
    };
    let usable = |scheme: KeyScheme| -> bool {
        (!want.sign || own(scheme).is_some()) && (!want.encrypt || (own(scheme).is_some() && recipients.iter().all(|r| key_for(scheme, r).is_some())))
    };
    let scheme = match prefer {
        Some(p) => p,
        None => [KeyScheme::Smime, KeyScheme::OpenPgp].into_iter().find(|s| usable(*s)).unwrap_or_else(|| {
            // Say what's missing for the kind the owner has an identity for, if any.
            if own(KeyScheme::Smime).is_none() && own(KeyScheme::OpenPgp).is_some() { KeyScheme::OpenPgp } else { KeyScheme::Smime }
        }),
    };
    let Some((my_key, my_secret)) = own(scheme) else {
        return Err(crate::cmd::fail(format!(
            "to {} you need your own {} certificate or key for {from} with its private key: import it in File › Options › Trust Center",
            if want.sign { "sign" } else { "encrypt" },
            scheme.label()
        )));
    };
    let fail = |e: crypto::CryptoError| crate::cmd::fail(e.to_string());
    let mut entity = mailcraft_mime::secure::content_entity(m);
    let mut secured: Option<Secured> = None;
    if want.sign {
        let signed = match scheme {
            KeyScheme::Smime => {
                let chain: Vec<crypto::PublicKey> = keys.known.iter().filter(|k| k.is_ca && k.scheme == crypto::Scheme::Smime).cloned().collect();
                let sig = crypto::smime::sign_detached(&entity, &crypto::smime::Identity { certificate: my_key, secret: my_secret }, &chain, now)
                    .map_err(fail)?;
                mailcraft_mime::secure::wrap_smime_signed(&entity, &sig)
            }
            KeyScheme::OpenPgp => {
                let sig = crypto::openpgp::sign_detached(&entity, my_secret).map_err(fail)?;
                mailcraft_mime::secure::wrap_pgp_signed(&entity, &sig)
            }
        };
        entity = signed.raw.to_vec();
        secured = Some(signed);
    }
    if want.encrypt {
        let missing: Vec<&str> = recipients.iter().filter(|r| key_for(scheme, r).is_none()).map(String::as_str).collect();
        if !missing.is_empty() {
            return Err(crate::cmd::fail(format!(
                "the message can't be encrypted: there is no {} {} here for {}. Import theirs (a signed message from them brings it), take them off, or send it unencrypted",
                scheme.label(),
                if scheme == KeyScheme::Smime { "certificate" } else { "key" },
                missing.join(", ")
            )));
        }
        let mut to: Vec<crypto::PublicKey> = recipients.iter().filter_map(|r| key_for(scheme, r).cloned()).collect();
        to.push(my_key.clone());
        to.dedup_by(|a, b| a.fingerprint == b.fingerprint);
        let enc = match scheme {
            KeyScheme::Smime => mailcraft_mime::secure::wrap_smime_encrypted(&crypto::smime::encrypt(&entity, &to).map_err(fail)?),
            KeyScheme::OpenPgp => mailcraft_mime::secure::wrap_pgp_encrypted(&crypto::openpgp::encrypt(&entity, &to).map_err(fail)?),
        };
        secured = Some(enc);
        // What it says stays encrypted here too.
        m.body = Body::default();
        m.attachments.clear();
    }
    m.secured = secured;
    m.protect = Protect::default();
    Ok(())
}

/// What an import brought.
pub(crate) struct Imported {
    pub keys: Vec<StoredKey>,
    pub secrets: usize,
}

/// Read a key file into the mailbox (public halves) and the session (private keys). The
/// passphrase is the one the window set ([`Session::set_passphrase`]), used once.
pub(crate) fn import(s: &mut Session, bytes: &[u8], trust: bool) -> Result<Imported, CmdError> {
    let passphrase = s.security.passphrase.take();
    let got = crypto::import(bytes, passphrase.as_ref().map(|p| p.as_str())).map_err(|e| match e {
        crypto::CryptoError::Passphrase => crate::cmd::fail("the file needs its passphrase, or the one given is wrong"),
        other => crate::cmd::fail(other.to_string()),
    })?;
    drop(passphrase);
    let mut secrets = got.secret;
    // A private key that came without its certificate goes with one the mailbox has.
    for secret in secrets.iter_mut().filter(|x| x.fingerprint.is_empty()) {
        if let Some(k) = s.mailbox.keys.iter().find(|k| crypto::secret_belongs_to(secret, &to_crypto(k))) {
            secret.fingerprint = k.fingerprint.clone();
        }
    }
    secrets.retain(|x| !x.fingerprint.is_empty());
    if got.public.is_empty() && secrets.is_empty() {
        return Err(crate::cmd::fail("the private key belongs to no certificate here: import the certificate with it"));
    }
    let mut keys = Vec::new();
    for public in &got.public {
        let mut stored = to_stored(public);
        let own = secrets.iter().any(|x| x.scheme == public.scheme && x.fingerprint == public.fingerprint);
        match s.mailbox.keys.iter_mut().find(|k| k.scheme == stored.scheme && k.fingerprint == stored.fingerprint) {
            Some(existing) => {
                existing.collected = false;
                existing.own |= own;
                existing.trusted |= trust;
                stored = existing.clone();
            }
            None => {
                if s.mailbox.keys.len() >= mailcraft_model::security::MAX_KEYS {
                    return Err(crate::cmd::fail("the mailbox keeps no more keys: remove some first"));
                }
                stored.own = own;
                stored.trusted = trust;
                s.mailbox.keys.push(stored.clone());
            }
        }
        keys.push(stored);
    }
    for secret in &secrets {
        if let Some(k) = s.mailbox.keys.iter_mut().find(|k| to_scheme(k.scheme) == secret.scheme && k.fingerprint == secret.fingerprint) {
            k.own = true;
        }
    }
    let count = secrets.len();
    for secret in secrets {
        s.security.remember.push(secret.clone());
        s.security.add_secret(secret);
    }
    s.security.changed();
    Ok(Imported { keys, secrets: count })
}

/// Remove a key from the mailbox, and its private key from the session (the host is told to
/// forget it too).
pub(crate) fn remove(s: &mut Session, fingerprint: &str) -> Result<StoredKey, CmdError> {
    let at = s.mailbox.keys.iter().position(|k| k.fingerprint.eq_ignore_ascii_case(fingerprint)).ok_or_else(|| crate::cmd::fail("no such key"))?;
    let key = s.mailbox.keys.remove(at);
    let scheme = to_scheme(key.scheme);
    s.security.secrets.retain(|x| !(x.scheme == scheme && x.fingerprint == key.fingerprint));
    if key.own {
        s.security.forget.push(store_name(scheme, &key.fingerprint));
    }
    s.security.changed();
    Ok(key)
}

/// Keys changed in the mailbox (trust, collected keys): work things out again.
pub(crate) fn keys_changed(s: &mut Session) {
    s.security.changed();
}
