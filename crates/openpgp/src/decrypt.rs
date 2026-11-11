// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening an OpenPGP Encrypted message (#30 part 6): PGP/MIME (RFC 3156 §4) and inline
//! (RFC 9580 §6.2, an armoured `PGP MESSAGE` in the text).
//!
//! **No RSA private-key operation runs in this process** (decision 6). What is done here is
//! everything around the one step that needs a private key:
//!
//! - [`encrypted_message`] finds the message's encrypted data and reads which keys its session
//!   key was encrypted to ([`Recipient`]).
//! - [`decryption_keys`] reads the owner's own keys (as `gpg --export` writes them) for the
//!   ones that can be a recipient, so that the program can tell which of gpg-agent's keys
//!   to ask ([`DecryptionKey::matches`], [`DecryptionKey::is_recipient`]).
//! - The program asks gpg-agent to decrypt: for RSA, the wrapped session key (`PKDECRYPT`); for
//!   ECDH (NIST curves and Curve25519) and X25519, the agent does the key agreement and gives
//!   the shared secret back ([`agent_request`] says what to ask). [`session_key`] makes
//!   the session key of what the agent answered: unpadding RSA's frame, deriving the key
//!   encryption key (RFC 9580 §11.5, §5.1.6) and unwrapping the key (RFC 3394).
//! - [`EncryptedMessage::decrypt`] decrypts the data with the session key, in memory, inflates
//!   what is compressed (to at most [`MAX_PLAINTEXT`]), and hands back the literal data and any
//!   Digital signature packets that were inside ([`Plaintext`]).
//!
//! A key MailCraft holds itself (imported in the window) is used in-process only when it is an
//! X25519-class key ([`decrypt_with_secret_key`]); an RSA or NIST-curve key never is.
//!
//! The message is written by whoever sent it and is hostile. Sizes, counts and nesting are
//! capped, rPGP's panics are caught, nothing here panics on any bytes, and every secret (the
//! session key, the shared secret, the plaintext) is overwritten when dropped.
//!
//! **One failure.** A wrong key, a changed ciphertext, a bad checksum and anything else wrong
//! with what a sender made are all [`DecryptError::Failed`] (and a fault in rPGP's reading is
//! the same): nothing here tells a sender which, so that no padding or integrity oracle is
//! reachable through what MailCraft shows.

use std::io::Read;

use aes_kw::{KekAes128, KekAes192, KekAes256};
use hkdf::Hkdf;
use pgp::composed::PlainSessionKey;
use pgp::composed::{Deserializable, Message, SignedSecretKey};
use pgp::crypto::public_key::PublicKeyAlgorithm;
use pgp::crypto::sym::SymmetricKeyAlgorithm;
use pgp::packet::PublicKeyEncryptedSessionKey as Pkesk;
use pgp::ser::Serialize;
use pgp::types::{KeyDetails, KeyVersion, Password, PkeskBytes};
use sha2::Digest;
use zeroize::Zeroizing;

use crate::armour::{self, Kind};
use crate::keys::{Fingerprint, KeyId, packets};
use crate::sign::{AgentKey, Reader, split_certificates, unpadded};
use crate::{MAX_MESSAGE, mime};

/// The most bytes of plaintext (inflated if compressed) a message may open to: the message
/// size cap. An inflated size past it is a failure, however small the compressed data was.
pub const MAX_PLAINTEXT: usize = MAX_MESSAGE;
/// The most recipients of one message looked at.
pub const MAX_RECIPIENTS: usize = 256;
/// The most keys of the owner's read.
pub const MAX_DECRYPTION_KEYS: usize = 512;
/// How many layers of compression and signature a message is opened through.
const MAX_LAYERS: usize = 4;
/// The most bytes of signature packets taken out of a message.
const MAX_SIGNATURE_BYTES: usize = 64 * 1024;
/// How many certificates of an export are looked at.
const MAX_CERTIFICATES: usize = 1_024;
/// The most bytes of an export read.
const MAX_EXPORT: usize = 32 << 20;
/// The most packets of one certificate read.
const MAX_PACKETS_PER_CERTIFICATE: usize = 4_096;

/// Why a message wasn't opened: always the same, whatever the reason ([`crate`] docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecryptError {
    #[error("the message can't be decrypted")]
    Failed,
}

/// How an encrypted session key is made for one recipient.
#[derive(Clone, PartialEq, Eq)]
pub enum Esk {
    /// RSA: the encrypted value, without leading zeros.
    Rsa { m: Vec<u8> },
    /// ECDH (RFC 9580 §5.1.4): the ephemeral point (as written) and the wrapped session key.
    Ecdh { point: Vec<u8>, wrapped: Vec<u8> },
    /// X25519 (RFC 9580 §5.1.6): the ephemeral public key and the wrapped session key, and the
    /// symmetric algorithm the packet names (a version 3 packet).
    X25519 { ephemeral: [u8; 32], wrapped: Vec<u8>, sym_alg: Option<u8> },
    /// Any other (ElGamal, X448, post-quantum, an unknown algorithm): not opened here.
    Other,
}

impl std::fmt::Debug for Esk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Esk::Rsa { .. } => "Esk::Rsa",
            Esk::Ecdh { .. } => "Esk::Ecdh",
            Esk::X25519 { .. } => "Esk::X25519",
            Esk::Other => "Esk::Other",
        })
    }
}

/// One key a message's session key was encrypted to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recipient {
    /// A version 6 packet, which has no symmetric algorithm octet and no checksum for X25519.
    pub v6: bool,
    /// The key id a version 3 packet names; all zero for an anonymous recipient.
    pub key_id: Option<KeyId>,
    /// The fingerprint a version 6 packet names; none for an anonymous recipient.
    pub fingerprint: Option<Fingerprint>,
    pub esk: Esk,
}

impl Recipient {
    /// Whether the packet doesn't say which key it is for (the receiver tries its keys).
    pub fn anonymous(&self) -> bool {
        self.key_id.is_none_or(|id| id.0 == [0u8; 8]) && self.fingerprint.is_none()
    }
}

/// What gpg-agent is asked for one recipient.
#[derive(Clone, PartialEq, Eq)]
pub enum AgentRequest {
    /// `(enc-val (rsa (a <value>)))`.
    Rsa { a: Vec<u8> },
    /// `(enc-val (ecdh (s <wrapped key>) (e <point>)))`.
    Ecdh { s: Vec<u8>, e: Vec<u8> },
}

impl std::fmt::Debug for AgentRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AgentRequest::Rsa { .. } => "AgentRequest::Rsa",
            AgentRequest::Ecdh { .. } => "AgentRequest::Ecdh",
        })
    }
}

/// The agent's request for `recipient`; `None` for one that isn't opened here.
pub fn agent_request(recipient: &Recipient) -> Option<AgentRequest> {
    match &recipient.esk {
        Esk::Rsa { m } => Some(AgentRequest::Rsa { a: m.clone() }),
        Esk::Ecdh { point, wrapped } => Some(AgentRequest::Ecdh { s: wrapped.clone(), e: point.clone() }),
        Esk::X25519 { ephemeral, wrapped, .. } => {
            // As gpg-agent does Curve25519 point: the 0x40 prefix, then the 32 bytes.
            let mut e = vec![0x40];
            e.extend_from_slice(ephemeral);
            Some(AgentRequest::Ecdh { s: wrapped.clone(), e })
        }
        Esk::Other => None,
    }
}

/// A message whose data is encrypted.
pub struct EncryptedMessage {
    /// The binary packets: ESK packets, then the encrypted data.
    packets: Vec<u8>,
    recipients: Vec<Recipient>,
    /// Whether the message was in the text of the mail (inline) and not a PGP/MIME part.
    pub inline: bool,
}

impl std::fmt::Debug for EncryptedMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EncryptedMessage({} bytes, {} recipients)", self.packets.len(), self.recipients.len())
    }
}

impl EncryptedMessage {
    /// The keys its session key was encrypted to, in the order the message has them.
    pub fn recipients(&self) -> &[Recipient] {
        &self.recipients
    }
}

/// The session key of a message: overwritten when dropped.
pub struct SessionKey {
    /// The symmetric algorithm, from a version 3 packet's plaintext or clear octet.
    sym_alg: Option<u8>,
    key: Zeroizing<Vec<u8>>,
    v6: bool,
}

#[cfg(test)]
impl SessionKey {
    /// A session key of any bytes, for the tests of what is done with a wrong one.
    pub(crate) fn for_test(key: Vec<u8>) -> SessionKey {
        SessionKey { sym_alg: Some(7), key: Zeroizing::new(key), v6: false }
    }
}

impl std::fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SessionKey(<{} bytes>)", self.key.len())
    }
}

/// What a message opened to.
pub struct Plaintext {
    /// The literal data: for PGP/MIME, the MIME entity; inline, the text.
    pub data: Zeroizing<Vec<u8>>,
    /// Digital signature packets inside the encrypted data (binary), over [`Plaintext::data`].
    pub signatures: Vec<u8>,
}

impl std::fmt::Debug for Plaintext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Plaintext(<{} bytes>, {} signature bytes)", self.data.len(), self.signatures.len())
    }
}

// ---- finding the encrypted data in a message ----

/// The encrypted OpenPGP message in `original` (a message as it came): a PGP/MIME one
/// (`multipart/encrypted; protocol="application/pgp-encrypted"`, at the top or as the first
/// part of wrapping `multipart/mixed` parts), or an armoured `PGP MESSAGE` in a plain text
/// body. `None` when there is none, or it can't be read as one.
pub fn encrypted_message(original: &[u8]) -> Option<EncryptedMessage> {
    if original.len() > MAX_MESSAGE {
        return None;
    }
    let (headers, body) = mime::split(original);
    find(&headers, body, 0)
}

/// How deep into wrapping parts an encrypted message is looked for.
const MAX_DEPTH: usize = 4;

fn find(headers: &mime::Headers, body: &[u8], depth: usize) -> Option<EncryptedMessage> {
    let content_type = headers.content_type();
    match content_type.value.as_str() {
        "multipart/encrypted" => {
            let protocol = content_type.get("protocol").unwrap_or_default().trim().to_ascii_lowercase();
            if protocol != "application/pgp-encrypted" {
                return None;
            }
            let boundary = content_type.get("boundary").filter(|b| !b.is_empty())?;
            let parts = mime::parts(body, boundary, 3);
            // RFC 3156 §4: the control part, then the encrypted data.
            let [_, data] = parts.as_slice() else { return None };
            let (data_headers, data_body) = mime::split(data);
            let bytes = mime::decode(&data_headers.encoding(), data_body);
            let packets = armoured_or_binary(&bytes)?;
            read(packets, false)
        }
        "text/plain" | "" => {
            let text = mime::decode(&headers.encoding(), body);
            let block = armour::blocks(&text, Kind::Message, 1, MAX_MESSAGE).into_iter().next()?;
            read(block, true)
        }
        "multipart/mixed" if depth < MAX_DEPTH => {
            let boundary = content_type.get("boundary").filter(|b| !b.is_empty())?;
            let first = *mime::parts(body, boundary, 1).first()?;
            let (part_headers, part_body) = mime::split(first);
            find(&part_headers, part_body, depth.saturating_add(1))
        }
        _ => None,
    }
}

/// The packets in a PGP/MIME data part: armoured, or binary from a writer that didn't armour.
fn armoured_or_binary(bytes: &[u8]) -> Option<Vec<u8>> {
    if let Some(block) = armour::blocks(bytes, Kind::Message, 1, MAX_MESSAGE).into_iter().next() {
        return Some(block);
    }
    (bytes.first().is_some_and(|b| b & 0x80 != 0) && bytes.len() <= MAX_MESSAGE).then(|| bytes.to_vec())
}

/// Reads the ESK packets of the binary message `packets`.
pub(crate) fn read(packets: Vec<u8>, inline: bool) -> Option<EncryptedMessage> {
    let esks = crate::guard(|| {
        let message = Message::from_bytes(packets.as_slice()).ok()?;
        match message {
            Message::Encrypted { esk, .. } => Some(esk),
            _ => None,
        }
    })??;
    let mut recipients = Vec::new();
    for esk in esks.iter().take(MAX_RECIPIENTS) {
        if let pgp::composed::Esk::PublicKeyEncryptedSessionKey(pkesk) = esk {
            recipients.push(recipient(pkesk));
        }
        // (A password-encrypted message, SKESK, isn't opened here: no key of the owner's.)
    }
    Some(EncryptedMessage { packets, recipients, inline })
}

fn recipient(pkesk: &Pkesk) -> Recipient {
    let (v6, key_id, fingerprint, values) = match pkesk {
        Pkesk::V3 { id, values, .. } => (false, Some(KeyId::of(id)), None, Some(values)),
        Pkesk::V6 { fingerprint, values, .. } => (true, None, fingerprint.as_ref().map(Fingerprint::of), Some(values)),
        Pkesk::Other { .. } => (false, None, None, None),
    };
    let esk = match values {
        Some(PkeskBytes::Rsa { mpi }) => Esk::Rsa { m: unpadded(mpi.as_ref()).to_vec() },
        Some(PkeskBytes::Ecdh { public_point, encrypted_session_key }) => {
            Esk::Ecdh { point: public_point.as_ref().to_vec(), wrapped: encrypted_session_key.to_vec() }
        }
        Some(PkeskBytes::X25519 { ephemeral, session_key, sym_alg }) => {
            Esk::X25519 { ephemeral: *ephemeral, wrapped: session_key.to_vec(), sym_alg: sym_alg.map(u8::from) }
        }
        _ => Esk::Other,
    };
    Recipient { v6, key_id, fingerprint, esk }
}

// ---- the owner's keys ----

/// The curves a decryption key may be on, by what its key packet and gpg-agent call them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Curve {
    P256,
    P384,
    P521,
    Cv25519,
}

impl Curve {
    fn from_oid(oid: &[u8]) -> Option<Curve> {
        Some(match oid {
            [0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07] => Curve::P256,
            [0x2B, 0x81, 0x04, 0x00, 0x22] => Curve::P384,
            [0x2B, 0x81, 0x04, 0x00, 0x23] => Curve::P521,
            [0x2B, 0x06, 0x01, 0x04, 0x01, 0x97, 0x55, 0x01, 0x05, 0x01] => Curve::Cv25519,
            _ => return None,
        })
    }

    fn from_name(name: &str) -> Option<Curve> {
        let name = name.trim().trim_matches('"').trim().to_ascii_lowercase();
        Some(match name.as_str() {
            "nist p-256" | "nistp256" | "p-256" | "secp256r1" | "prime256v1" | "1.2.840.10045.3.1.7" => Curve::P256,
            "nist p-384" | "nistp384" | "p-384" | "secp384r1" | "1.3.132.0.34" => Curve::P384,
            "nist p-521" | "nistp521" | "p-521" | "secp521r1" | "1.3.132.0.35" => Curve::P521,
            "curve25519" | "cv25519" | "x25519" | "1.3.6.1.4.1.3029.1.5.1" => Curve::Cv25519,
            _ => return None,
        })
    }

    /// The bytes of a coordinate.
    fn field_len(self) -> usize {
        match self {
            Curve::P256 => 32,
            Curve::P384 => 48,
            Curve::P521 => 66,
            Curve::Cv25519 => 32,
        }
    }
}

/// What a decryption key is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Material {
    Rsa {
        n: Vec<u8>,
        e: Vec<u8>,
    },
    /// ECDH (18): the curve, its OID as the packet has it, the point, and the KDF's hash and
    /// key-encryption algorithm (RFC 9580 §11.5).
    Ecdh {
        curve: Curve,
        oid: Vec<u8>,
        q: Vec<u8>,
        hash: u8,
        kek: u8,
    },
    /// X25519 (25): the 32-byte public key.
    X25519 {
        q: [u8; 32],
    },
}

/// One of the owner's keys, primary or subkey, that a session key may have been encrypted to.
#[derive(Clone, Debug)]
pub struct DecryptionKey {
    fingerprint: Fingerprint,
    key_id: KeyId,
    primary: Fingerprint,
    addresses: Vec<String>,
    version: KeyVersion,
    material: Material,
}

impl DecryptionKey {
    /// This key's fingerprint.
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// The fingerprint of the certificate's primary key.
    pub fn primary(&self) -> &Fingerprint {
        &self.primary
    }

    /// The addresses of the certificate (lower case).
    pub fn addresses(&self) -> &[String] {
        &self.addresses
    }

    /// Whether the key is RSA (whose private-key operation is never in this process).
    pub fn is_rsa(&self) -> bool {
        matches!(self.material, Material::Rsa { .. })
    }

    /// Whether the key is an X25519-class key (X25519, or ECDH on Curve25519): one MailCraft may
    /// use itself when the owner imported it.
    pub fn is_x25519_class(&self) -> bool {
        matches!(self.material, Material::X25519 { .. } | Material::Ecdh { curve: Curve::Cv25519, .. })
    }

    /// Whether `agent` (a key the holder has, as `READKEY` gives it) is this key: the same
    /// public material, leading zero bytes and a Curve25519 point's 0x40 aside.
    pub fn matches(&self, agent: &AgentKey<'_>) -> bool {
        match (&self.material, agent) {
            (Material::Rsa { n, e }, AgentKey::Rsa { n: an, e: ae }) => !n.is_empty() && unpadded(an) == n.as_slice() && unpadded(ae) == e.as_slice(),
            (Material::Ecdh { curve, q, .. }, AgentKey::Ecc { curve: name, q: aq }) => {
                Curve::from_name(name) == Some(*curve) && !q.is_empty() && point(*curve, aq) == point(*curve, q)
            }
            (Material::X25519 { q }, AgentKey::Ecc { curve: name, q: aq }) => {
                Curve::from_name(name) == Some(Curve::Cv25519) && point(Curve::Cv25519, aq) == q.as_slice()
            }
            _ => false,
        }
    }

    /// Whether `recipient`'s packet is for this key: by the key id or fingerprint it names; an
    /// anonymous one may be for any key of the same kind.
    pub fn is_recipient(&self, recipient: &Recipient) -> bool {
        let kind = matches!(
            (&recipient.esk, &self.material),
            (Esk::Rsa { .. }, Material::Rsa { .. }) | (Esk::Ecdh { .. }, Material::Ecdh { .. }) | (Esk::X25519 { .. }, Material::X25519 { .. })
        );
        if !kind {
            return false;
        }
        if recipient.anonymous() {
            return true;
        }
        match (&recipient.fingerprint, &recipient.key_id) {
            (Some(fp), _) => fp == &self.fingerprint,
            (None, Some(id)) => *id == self.key_id,
            (None, None) => false,
        }
    }
}

/// A point without the 0x40 a Curve25519 point is written with (and the leading zeros an MPI
/// has none of): the bytes to compare.
fn point(curve: Curve, bytes: &[u8]) -> &[u8] {
    if curve == Curve::Cv25519 {
        return match bytes {
            [0x40, rest @ ..] if rest.len() == 32 => rest,
            other => other,
        };
    }
    bytes
}

/// The owner's keys in `export` (binary or armoured transferable public keys, as `gpg --export`
/// writes them) that a session key may be encrypted to: RSA, ECDH and X25519 keys, primary keys
/// and subkeys alike. Nothing about a key's flags, signatures or expiry is looked at: a key
/// that has since expired or been revoked still opens mail sent to it. At most [`MAX_EXPORT`]
/// bytes are read, [`MAX_CERTIFICATES`] certificates looked at and [`MAX_DECRYPTION_KEYS`] given.
pub fn decryption_keys(export: &[u8]) -> Vec<DecryptionKey> {
    let mut out = Vec::new();
    if export.len() > MAX_EXPORT {
        return out;
    }
    let blocks = armour::blocks(export, Kind::PublicKey, MAX_CERTIFICATES, MAX_EXPORT);
    let blocks = if blocks.is_empty() { vec![export.to_vec()] } else { blocks };
    let mut certificates = MAX_CERTIFICATES;
    for block in &blocks {
        for certificate in split_certificates(block, certificates) {
            certificates = certificates.saturating_sub(1);
            let (tags, _) = packets(certificate, MAX_PACKETS_PER_CERTIFICATE.saturating_add(1));
            if tags.len() > MAX_PACKETS_PER_CERTIFICATE {
                continue;
            }
            let Some(Ok(key)) = crate::guard(|| pgp::composed::SignedPublicKey::from_bytes(certificate)) else { continue };
            for found in crate::guard(|| of(&key)).unwrap_or_default() {
                if out.len() >= MAX_DECRYPTION_KEYS {
                    return out;
                }
                out.push(found);
            }
        }
        if certificates == 0 {
            break;
        }
    }
    out
}

fn of(key: &pgp::composed::SignedPublicKey) -> Vec<DecryptionKey> {
    let primary = Fingerprint::of(&key.primary_key.fingerprint());
    let addresses: Vec<String> =
        key.details.users.iter().map(|u| mime::address(&String::from_utf8_lossy(u.id.id()))).filter(|a| !a.is_empty()).collect();
    let mut out = Vec::new();
    let mut add = |fingerprint: pgp::types::Fingerprint,
                   key_id: pgp::types::KeyId,
                   version: KeyVersion,
                   algorithm: PublicKeyAlgorithm,
                   fields: Option<Vec<u8>>| {
        if let Some(material) = fields.and_then(|f| material(algorithm, &f)) {
            out.push(DecryptionKey {
                fingerprint: Fingerprint::of(&fingerprint),
                key_id: KeyId::of(&key_id),
                primary: primary.clone(),
                addresses: addresses.clone(),
                version,
                material,
            });
        }
    };
    let k = &key.primary_key;
    add(k.fingerprint(), k.legacy_key_id(), k.version(), k.algorithm(), k.public_params().to_bytes().ok());
    for sub in &key.public_subkeys {
        let k = &sub.key;
        add(k.fingerprint(), k.legacy_key_id(), k.version(), k.algorithm(), k.public_params().to_bytes().ok());
    }
    out
}

fn material(algorithm: PublicKeyAlgorithm, fields: &[u8]) -> Option<Material> {
    let mut at = Reader::new(fields);
    let material = match algorithm {
        PublicKeyAlgorithm::RSA | PublicKeyAlgorithm::RSAEncrypt => Material::Rsa { n: at.mpi()?.to_vec(), e: at.mpi()?.to_vec() },
        PublicKeyAlgorithm::ECDH => {
            let oid = at.oid()?;
            let curve = Curve::from_oid(oid)?;
            let q = at.mpi()?;
            // KDF parameters (RFC 9580 §5.5.5.6): a length of 3, 0x01, the hash, the algorithm.
            let kdf = at.take(4)?;
            let [3, 1, hash, kek] = kdf else { return None };
            Material::Ecdh { curve, oid: oid.to_vec(), q: q.to_vec(), hash: *hash, kek: *kek }
        }
        PublicKeyAlgorithm::X25519 => Material::X25519 { q: at.take(32)?.try_into().ok()? },
        _ => return None,
    };
    at.is_empty().then_some(material)
}

// ---- the session key ----

/// The session key of what the agent decrypted for `recipient` with `key`: `value` is what
/// `PKDECRYPT` gave back and `padding` its `PADDING` status (see `mailcraft_assuan::SessionKey`).
/// For RSA, `value` is the decrypted message or the PKCS #1 v1.5 frame; for ECDH and X25519 it
/// is the shared secret, from which the key encryption key is derived and the session key
/// unwrapped. `None` for anything wrong, all alike.
pub fn session_key(recipient: &Recipient, key: &DecryptionKey, value: &[u8], padding: Option<u32>) -> Option<SessionKey> {
    match (&recipient.esk, &key.material) {
        (Esk::Rsa { .. }, Material::Rsa { .. }) => {
            let message = if padding.is_some() { Zeroizing::new(value.to_vec()) } else { Zeroizing::new(unframe(value)?.to_vec()) };
            from_checked_message(&message, recipient.v6)
        }
        (Esk::Ecdh { wrapped, .. }, Material::Ecdh { curve, oid, hash, kek, .. }) => {
            let z = shared_x(*curve, value)?;
            let kek_len = match SymmetricKeyAlgorithm::from(*kek) {
                SymmetricKeyAlgorithm::AES128 => 16,
                SymmetricKeyAlgorithm::AES192 => 24,
                SymmetricKeyAlgorithm::AES256 => 32,
                _ => return None,
            };
            // The KDF's parameters (RFC 9580 §11.5): the curve, the algorithm, the KDF's
            // fields, a fixed string and the recipient's fingerprint (20 octets: version 4).
            let fp = key.fingerprint.as_bytes();
            if fp.len() != 20 || key.version != KeyVersion::V4 {
                return None;
            }
            let mut param = vec![u8::try_from(oid.len()).ok()?];
            param.extend_from_slice(oid);
            param.extend_from_slice(&[18, 3, 1, *hash, *kek]);
            param.extend_from_slice(b"Anonymous Sender    ");
            param.extend_from_slice(fp);
            let derived = Zeroizing::new(kdf_hash(*hash, &[&[0, 0, 0, 1], &z, &param])?);
            let kek_bytes = derived.get(..kek_len)?;
            let unwrapped = Zeroizing::new(unwrap_key(kek_bytes, wrapped)?);
            // PKCS #5 padding to a multiple of 8 (RFC 9580 §11.5).
            let n = usize::from(*unwrapped.last()?);
            if n == 0 || n > 8 || n >= unwrapped.len() {
                return None;
            }
            let message = Zeroizing::new(unwrapped.get(..unwrapped.len().checked_sub(n)?)?.to_vec());
            from_checked_message(&message, recipient.v6)
        }
        (Esk::X25519 { ephemeral, wrapped, sym_alg }, Material::X25519 { q }) => {
            let shared = shared_x(Curve::Cv25519, value)?;
            // RFC 9580 §5.1.6: HKDF-SHA256 over the ephemeral key, the recipient's key and the
            // shared secret, with the info "OpenPGP X25519"; a 128-bit key encryption key.
            let mut ikm = Zeroizing::new(Vec::new());
            ikm.extend_from_slice(ephemeral);
            ikm.extend_from_slice(q);
            ikm.extend_from_slice(&shared);
            let hk = Hkdf::<sha2::Sha256>::new(None, &ikm);
            let mut kek = Zeroizing::new([0u8; 16]);
            hk.expand(b"OpenPGP X25519", kek.as_mut_slice()).ok()?;
            let unwrapped = Zeroizing::new(unwrap_key(kek.as_slice(), wrapped)?);
            // (No checksum: the key wrap is its own integrity check.)
            if let Some(alg) = sym_alg {
                Some(SessionKey { sym_alg: Some(*alg), key: unwrapped, v6: false })
            } else {
                Some(SessionKey { sym_alg: None, key: unwrapped, v6: true })
            }
        }
        _ => None,
    }
}

/// The message of a PKCS #1 v1.5 frame as gpg-agent leaves it (the leading 0x00 omitted, or
/// kept): `02`, at least eight non-zero bytes, `00`, the message.
fn unframe(value: &[u8]) -> Option<&[u8]> {
    let value = value.strip_prefix(&[0]).unwrap_or(value);
    let rest = value.strip_prefix(&[2])?;
    let zero = rest.iter().position(|b| *b == 0)?;
    if zero < 8 {
        return None;
    }
    rest.get(zero.checked_add(1)?..)
}

/// A decrypted message of RSA or ECDH: for a version 3 packet the symmetric algorithm, the key and
/// a two-octet checksum of the key; for version 6 the key and the checksum (RFC 9580 §5.1.1).
fn from_checked_message(message: &[u8], v6: bool) -> Option<SessionKey> {
    let (sym_alg, rest) = if v6 {
        (None, message)
    } else {
        let (alg, rest) = message.split_first()?;
        (Some(*alg), rest)
    };
    let split = rest.len().checked_sub(2)?;
    let (key, checksum) = (rest.get(..split)?, rest.get(split..)?);
    let sum = key.iter().fold(0u16, |acc, b| acc.wrapping_add(u16::from(*b)));
    if checksum != sum.to_be_bytes() || key.is_empty() {
        return None;
    }
    Some(SessionKey { sym_alg, key: Zeroizing::new(key.to_vec()), v6 })
}

/// The shared secret's x coordinate from what the agent gave back: `0x04 || x || y`, or `x`, or
/// (for Curve25519) `0x40 || u`.
fn shared_x(curve: Curve, value: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    let len = curve.field_len();
    let x = match value {
        [0x04, rest @ ..] if rest.len() == len.checked_mul(2)? => rest.get(..len)?,
        [0x40, rest @ ..] if curve == Curve::Cv25519 && rest.len() == len => rest,
        other if other.len() == len => other,
        // (A coordinate whose leading zeros were left off.)
        other if other.len() < len && !other.is_empty() && curve != Curve::Cv25519 => {
            let mut padded = vec![0u8; len.checked_sub(other.len())?];
            padded.extend_from_slice(other);
            return Some(Zeroizing::new(padded));
        }
        _ => return None,
    };
    Some(Zeroizing::new(x.to_vec()))
}

/// The hash of the KDF (RFC 9580 §9.5): SHA-256, SHA-384 or SHA-512.
fn kdf_hash(hash: u8, parts: &[&[u8]]) -> Option<Vec<u8>> {
    fn run<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
        let mut hasher = D::new();
        for p in parts {
            hasher.update(p);
        }
        hasher.finalize().to_vec()
    }
    Some(match hash {
        8 => run::<sha2::Sha256>(parts),
        9 => run::<sha2::Sha384>(parts),
        10 => run::<sha2::Sha512>(parts),
        _ => return None,
    })
}

/// RFC 3394 key unwrap with a 128-, 192- or 256-bit key encryption key.
fn unwrap_key(kek: &[u8], wrapped: &[u8]) -> Option<Vec<u8>> {
    if wrapped.len() > 1024 {
        return None;
    }
    match kek.len() {
        16 => KekAes128::try_from(kek).ok()?.unwrap_vec(wrapped).ok(),
        24 => KekAes192::try_from(kek).ok()?.unwrap_vec(wrapped).ok(),
        32 => KekAes256::try_from(kek).ok()?.unwrap_vec(wrapped).ok(),
        _ => None,
    }
}

// ---- decrypting ----

impl EncryptedMessage {
    /// Decrypts the data with `session` and reads what is inside: through compression (to at
    /// most [`MAX_PLAINTEXT`] bytes) and signature layers, to the literal data, and the
    /// signature packets that came with it.
    pub fn decrypt(&self, session: &SessionKey) -> Result<Plaintext, DecryptError> {
        let plain = match (session.sym_alg, session.v6) {
            (Some(alg), _) => PlainSessionKey::V3_4 { sym_alg: SymmetricKeyAlgorithm::from(alg), key: session.key.to_vec().into() },
            (None, true) => PlainSessionKey::V6 { key: session.key.to_vec().into() },
            (None, false) => return Err(DecryptError::Failed),
        };
        crate::guard(|| {
            let message = Message::from_bytes(self.packets.as_slice()).ok()?;
            let message = message.decrypt_with_session_key(plain).ok()?;
            read_plaintext(message)
        })
        .flatten()
        .ok_or(DecryptError::Failed)
    }
}

/// Reads the literal data and signatures out of a decrypted message.
fn read_plaintext(mut message: Message<'_>) -> Option<Plaintext> {
    for _ in 0..MAX_LAYERS {
        if message.is_encrypted() {
            // (A message encrypted twice isn't opened: nothing says the second key is the owner's.)
            return None;
        }
        message = message.decompress().ok()?;
    }
    if message.is_compressed() || message.is_encrypted() {
        return None;
    }
    let mut data = Zeroizing::new(Vec::new());
    let limit = u64::try_from(MAX_PLAINTEXT).ok()?.checked_add(1)?;
    (&mut message).take(limit).read_to_end(&mut data).ok()?;
    if data.len() > MAX_PLAINTEXT {
        return None;
    }
    let mut signatures = Vec::new();
    if let Message::Signed { reader, .. } = &message {
        for full in reader.signatures()?.iter().take(8) {
            let packet = pgp::packet::Packet::Signature(full.signature().clone());
            signatures.extend_from_slice(&packet.to_bytes().ok()?);
        }
        if signatures.len() > MAX_SIGNATURE_BYTES {
            signatures.clear();
        }
    }
    Some(Plaintext { data, signatures })
}

/// Opens the message with a secret key MailCraft holds itself: only an X25519-class key (X25519,
/// or ECDH on Curve25519: neither is RSA, nor a NIST curve, whose private-key operations never
/// run in this process), protected by `passphrase`. `None` for any fault, all alike, and for a
/// key that has an RSA part or isn't of that class for the recipient the message names.
pub fn decrypt_with_secret_key(message: &EncryptedMessage, secret_key: &[u8], passphrase: &str) -> Option<Plaintext> {
    let block = armour::blocks(secret_key, Kind::PrivateKey, 1, crate::keys::MAX_KEY_BYTES).into_iter().next();
    let binary = block.as_deref().unwrap_or(secret_key);
    crate::guard(|| {
        let key = SignedSecretKey::from_bytes(binary).ok()?;
        // Only keys of the class: the primary key and every subkey that holds secret material.
        let class = |a: PublicKeyAlgorithm| {
            matches!(a, PublicKeyAlgorithm::X25519 | PublicKeyAlgorithm::ECDH | PublicKeyAlgorithm::EdDSALegacy | PublicKeyAlgorithm::Ed25519)
        };
        let ecdh_ok = |params: &pgp::types::PublicParams| -> bool {
            match params {
                pgp::types::PublicParams::ECDH(p) => matches!(p, pgp::types::EcdhPublicParams::Curve25519Legacy { .. }),
                _ => true,
            }
        };
        if !class(key.primary_key.algorithm())
            || !ecdh_ok(key.primary_key.public_params())
            || key.secret_subkeys.iter().any(|s| !class(s.key.algorithm()) || !ecdh_ok(s.key.public_params()))
        {
            return None;
        }
        let parsed = Message::from_bytes(message.packets.as_slice()).ok()?;
        let decrypted = parsed.decrypt(&Password::from(passphrase), &key).ok()?;
        read_plaintext(decrypted)
    })
    .flatten()
}
