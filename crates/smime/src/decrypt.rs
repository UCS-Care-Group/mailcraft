// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening an S/MIME Encrypted message (#30 part 6): CMS EnvelopedData (RFC 5652 §6, RFC 8551
//! §2.3) and AuthEnvelopedData (RFC 5083, with AES-GCM, RFC 5084), written from those
//! specifications.
//!
//! What this does and doesn't do:
//!
//! - It reads the message's recipients and finds the ones that are the owner's certificates (by
//!   issuer and serial number, or by subject key identifier).
//! - **It never unwraps a content-encryption key itself.** The wrapped key goes to the key
//!   holder the caller hands in (the system's store, a PKCS#11 token: `apps/mailcraft`), which
//!   holds the RSA private key and gives the unwrapped key back. No RSA private-key operation
//!   runs in this crate or in this process (#30, decision 6). Key agreement (ECDH) isn't read
//!   at all yet: such a recipient is one this can't use.
//! - With the key, it decrypts the content here: AES-128, -192 or -256 in CBC mode (Outlook
//!   sends AES-256-CBC), triple DES in CBC mode (old senders), and AES-GCM in AuthEnvelopedData.
//!
//! The message is written by whoever sent it and is hostile. Every length is checked against
//! what is there, the recipients and the work are capped, nothing here panics on any bytes, and
//! the content, the key and everything between are overwritten when dropped.
//!
//! **One failure.** Whatever is wrong with the wrapped key, the padding, the algorithm
//! parameters or the content, the answer is [`DecryptError::Failed`] and nothing else (RFC 3218's
//! rule against a padding oracle): who sent the message can't tell from anything MailCraft shows
//! which of them it was. Only what is the same for every message and tells a sender nothing is
//! said apart: no recipient is one of the owner's certificates ([`DecryptError::NoKey`]), the
//! message uses key agreement ([`DecryptError::Unsupported`]), and what the key holder itself
//! says (a PIN is needed, the owner cancelled: [`DecryptError::Holder`]).

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeDecrypt, KeyIvInit};
use aes_gcm::aead::{AeadInPlace, KeyInit, consts::U12, consts::U16, generic_array::GenericArray};
use const_oid::ObjectIdentifier;
use der::Decode;
use mailcraft_model::ber::{self, Tlv};
use zeroize::Zeroizing;

use crate::cert::Cert;

/// The most bytes of CMS read: as large as the largest message any server is fetched for.
pub const MAX_CMS: usize = 150 << 20;
/// The most recipients looked at.
pub const MAX_RECIPIENTS: usize = 256;
/// The most certificates the owner can have among them.
pub const MAX_CERTIFICATES: usize = 256;
/// The most unwraps asked of the key holder for one message: every one may cost a PIN, a token
/// or a private-key operation.
pub const MAX_UNWRAPS: usize = 8;

const ENVELOPED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.3");
const AUTH_ENVELOPED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.1.23");
const RSA_ENCRYPTION: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const RSAES_OAEP: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.7");
const MGF1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.8");
const P_SPECIFIED: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.9");
const SHA1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.14.3.2.26");
const SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");
const AES128_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.2");
const AES192_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.22");
const AES256_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.42");
const AES128_GCM: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.6");
const AES192_GCM: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.26");
const AES256_GCM: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.46");
const DES_EDE3_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.3.7");

/// How a content-encryption key was wrapped with an RSA public key, as the recipient says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyTransport {
    /// RSAES-PKCS1-v1_5 (RFC 8017 §7.2): what Outlook and most programs send.
    Pkcs1v15,
    /// RSAES-OAEP with SHA-1 and MGF1-SHA-1, an empty label (RFC 3560's default).
    OaepSha1,
    /// RSAES-OAEP with SHA-256 and MGF1-SHA-256, an empty label.
    OaepSha256,
}

/// What the key holder answers when asked to unwrap a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Unwrapped<E> {
    /// The unwrapped content-encryption key.
    Key(Zeroizing<Vec<u8>>),
    /// This recipient's key didn't unwrap it (a wrong key, bad padding: all alike): another
    /// recipient is tried.
    Refused,
    /// The holder needs something of the owner or can't go on (a PIN, a cancelled prompt, a
    /// token that went away): stop and say so. Never made of anything in the message.
    Stop(E),
}

/// Why a message wasn't opened.
#[derive(Debug, PartialEq, Eq)]
pub enum DecryptError<E> {
    /// Not CMS EnvelopedData or AuthEnvelopedData.
    NotEnveloped,
    /// None of its recipients is one of the owner's certificates.
    NoKey,
    /// The message is for the owner's key by key agreement (ECDH), which isn't read yet.
    Unsupported,
    /// The key holder stopped ([`Unwrapped::Stop`]).
    Holder(E),
    /// Anything else, all alike.
    Failed,
}

/// What a message held, and which certificate opened it.
pub struct Decrypted {
    /// The decrypted content: for mail, a MIME entity (RFC 8551 §3.1).
    pub content: Zeroizing<Vec<u8>>,
    /// The index among the certificates handed in of the one whose key opened it.
    pub certificate: usize,
}

impl std::fmt::Debug for Decrypted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Decrypted(<{} bytes>, certificate {})", self.content.len(), self.certificate)
    }
}

/// Whether `cms` is EnvelopedData or AuthEnvelopedData: an Encrypted message, not a Digital
/// signature. Never fails.
pub fn is_enveloped(cms: &[u8]) -> bool {
    if cms.len() > MAX_CMS {
        // (Told by its first bytes alone: too large to open, but still an Encrypted message.)
        return head_type(cms).is_some_and(|t| t == ENVELOPED_DATA || t == AUTH_ENVELOPED_DATA);
    }
    let Some(der) = ber::to_der(cms) else { return false };
    content_info(&der).is_some_and(|(t, _)| t == ENVELOPED_DATA || t == AUTH_ENVELOPED_DATA)
}

/// The content type of a ContentInfo from its first bytes, when its header is short enough to
/// read (BER lengths in the header may be indefinite: only the OID is read).
fn head_type(cms: &[u8]) -> Option<ObjectIdentifier> {
    let head = cms.get(..64.min(cms.len()))?;
    let (&tag, rest) = head.split_first()?;
    if tag != 0x30 {
        return None;
    }
    let (&first, rest) = rest.split_first()?;
    let rest = if first < 0x80 { rest } else { rest.get(usize::from(first & 0x7f)..)? };
    let (&oid_tag, rest) = rest.split_first()?;
    let (&len, rest) = rest.split_first()?;
    if oid_tag != 0x06 || len >= 0x80 {
        return None;
    }
    ObjectIdentifier::from_bytes(rest.get(..usize::from(len))?).ok()
}

/// A ContentInfo's type and its content (inside the `[0] EXPLICIT`).
fn content_info(der: &[u8]) -> Option<(ObjectIdentifier, Tlv<'_>)> {
    let (info, rest) = ber::read(der)?;
    if info.tag != 0x30 || !rest.is_empty() {
        return None;
    }
    let fields = ber::children(info.value)?;
    let [oid, explicit] = fields.as_slice() else { return None };
    if explicit.tag != 0xa0 || oid.tag != 0x06 {
        return None;
    }
    let (content, rest) = ber::read(explicit.value)?;
    rest.is_empty().then_some(())?;
    Some((ObjectIdentifier::from_der(oid.whole).ok()?, content))
}

/// How a recipient names the certificate it is for.
enum Rid<'a> {
    /// `IssuerAndSerialNumber`: the issuer's name in DER and the serial number's bytes.
    IssuerSerial { issuer: &'a [u8], serial: &'a [u8] },
    /// `[0] SubjectKeyIdentifier`.
    KeyId(&'a [u8]),
}

/// One recipient that unwraps with an RSA private key.
struct Recipient<'a> {
    rid: Rid<'a>,
    transport: KeyTransport,
    wrapped: &'a [u8],
}

/// The content as the message says it is encrypted.
struct Encrypted<'a> {
    cipher: Cipher,
    iv: &'a [u8],
    /// The authentication tag and the data it covers besides the content (AuthEnvelopedData).
    auth: Option<Auth<'a>>,
    ciphertext: Vec<u8>,
}

struct Auth<'a> {
    tag: &'a [u8],
    attributes: Option<Vec<u8>>,
    tag_len: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cipher {
    Aes128Cbc,
    Aes192Cbc,
    Aes256Cbc,
    TdesCbc,
    Aes128Gcm,
    Aes192Gcm,
    Aes256Gcm,
}

impl Cipher {
    fn of(oid: ObjectIdentifier) -> Option<Cipher> {
        Some(match oid {
            AES128_CBC => Cipher::Aes128Cbc,
            AES192_CBC => Cipher::Aes192Cbc,
            AES256_CBC => Cipher::Aes256Cbc,
            DES_EDE3_CBC => Cipher::TdesCbc,
            AES128_GCM => Cipher::Aes128Gcm,
            AES192_GCM => Cipher::Aes192Gcm,
            AES256_GCM => Cipher::Aes256Gcm,
            _ => return None,
        })
    }

    fn key_len(self) -> usize {
        match self {
            Cipher::Aes128Cbc | Cipher::Aes128Gcm => 16,
            Cipher::Aes192Cbc | Cipher::Aes192Gcm | Cipher::TdesCbc => 24,
            Cipher::Aes256Cbc | Cipher::Aes256Gcm => 32,
        }
    }

    fn authenticated(self) -> bool {
        matches!(self, Cipher::Aes128Gcm | Cipher::Aes192Gcm | Cipher::Aes256Gcm)
    }
}

/// An EnvelopedData read: its recipients that unwrap with RSA, whether some use key agreement,
/// and the content.
struct Envelope<'a> {
    recipients: Vec<Recipient<'a>>,
    other_recipients: bool,
    encrypted: Encrypted<'a>,
}

/// Opens the Encrypted message `cms` (the CMS of an `application/pkcs7-mime` part, decoded from
/// base64) for the owner's `certificates` (each in DER). `unwrap(i, transport, wrapped)` asks the
/// key holder to unwrap `wrapped` with the private key of `certificates[i]`.
///
/// At most [`MAX_UNWRAPS`] unwraps are asked. Never panics on any `cms`.
pub fn decrypt<E>(
    cms: &[u8],
    certificates: &[&[u8]],
    mut unwrap: impl FnMut(usize, KeyTransport, &[u8]) -> Unwrapped<E>,
) -> Result<Decrypted, DecryptError<E>> {
    if cms.len() > MAX_CMS {
        return Err(DecryptError::Failed);
    }
    let der = ber::to_der(cms).ok_or(DecryptError::NotEnveloped)?;
    let (kind, content) = content_info(&der).ok_or(DecryptError::NotEnveloped)?;
    if kind != ENVELOPED_DATA && kind != AUTH_ENVELOPED_DATA {
        return Err(DecryptError::NotEnveloped);
    }
    let envelope = read_envelope(content, kind == AUTH_ENVELOPED_DATA).ok_or(DecryptError::Failed)?;
    // (A certificate that can't be read is one nothing is for, and keeps its place.)
    let certs: Vec<Option<Cert>> = certificates.iter().take(MAX_CERTIFICATES).map(|der| Cert::from_der(der)).collect();
    let mut asked = 0usize;
    let mut matched = false;
    for recipient in &envelope.recipients {
        for (index, cert) in certs.iter().enumerate() {
            if !cert.as_ref().is_some_and(|c| names(&recipient.rid, c)) {
                continue;
            }
            matched = true;
            if asked >= MAX_UNWRAPS {
                return Err(DecryptError::Failed);
            }
            asked = asked.saturating_add(1);
            match unwrap(index, recipient.transport, recipient.wrapped) {
                Unwrapped::Stop(e) => return Err(DecryptError::Holder(e)),
                Unwrapped::Refused => {}
                Unwrapped::Key(key) => {
                    if let Some(content) = open(&envelope.encrypted, &key) {
                        return Ok(Decrypted { content, certificate: index });
                    }
                }
            }
        }
    }
    if matched {
        Err(DecryptError::Failed)
    } else if envelope.other_recipients {
        Err(DecryptError::Unsupported)
    } else {
        Err(DecryptError::NoKey)
    }
}

/// Whether `rid` names `cert`.
fn names(rid: &Rid<'_>, cert: &Cert) -> bool {
    match rid {
        Rid::IssuerSerial { issuer, serial } => cert.issuer() == *issuer && trimmed(cert.serial()) == trimmed(serial),
        Rid::KeyId(id) => cert.key_id().is_some_and(|k| k.as_slice() == *id),
    }
}

/// An integer's bytes without the zero bytes it starts with.
fn trimmed(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
    bytes.get(start..).unwrap_or_default()
}

fn read_envelope(content: Tlv<'_>, auth: bool) -> Option<Envelope<'_>> {
    if content.tag != 0x30 {
        return None;
    }
    let fields = ber::children(content.value)?;
    let mut fields = fields.into_iter();
    let version = fields.next()?;
    (version.tag == 0x02).then_some(())?;
    let mut next = fields.next()?;
    // originatorInfo [0] IMPLICIT: certificates and CRLs the sender brought; nothing here uses it.
    if next.tag == 0xa0 {
        next = fields.next()?;
    }
    (next.tag == 0x31).then_some(())?;
    let infos = ber::children(next.value)?;
    if infos.len() > MAX_RECIPIENTS {
        return None;
    }
    let mut recipients = Vec::new();
    let mut other_recipients = false;
    for info in &infos {
        match info.tag {
            0x30 => recipients.extend(key_transport(info)),
            // Key agreement (ECDH, RFC 5753), key encryption keys, passwords and others.
            _ => other_recipients = true,
        }
    }
    let encrypted_info = fields.next()?;
    let encrypted = read_encrypted(&encrypted_info, auth, &mut fields)?;
    Some(Envelope { recipients, other_recipients, encrypted })
}

/// A `KeyTransRecipientInfo` that unwraps with RSA; `None` for one that doesn't read or isn't.
fn key_transport<'a>(info: &Tlv<'a>) -> Option<Recipient<'a>> {
    let fields = ber::children(info.value)?;
    let [version, rid, algorithm, wrapped] = fields.as_slice() else { return None };
    (version.tag == 0x02 && wrapped.tag == 0x04).then_some(())?;
    let rid = match rid.tag {
        0x30 => {
            let parts = ber::children(rid.value)?;
            let [issuer, serial] = parts.as_slice() else { return None };
            (issuer.tag == 0x30 && serial.tag == 0x02).then_some(())?;
            Rid::IssuerSerial { issuer: issuer.whole, serial: serial.value }
        }
        0x80 => Rid::KeyId(rid.value),
        _ => return None,
    };
    let (oid, parameters) = algorithm_identifier(algorithm)?;
    let transport = match oid {
        RSA_ENCRYPTION => KeyTransport::Pkcs1v15,
        RSAES_OAEP => oaep(parameters)?,
        _ => return None,
    };
    Some(Recipient { rid, transport, wrapped: wrapped.value })
}

/// An AlgorithmIdentifier's OID and its parameters, if it has any.
fn algorithm_identifier<'a>(tlv: &Tlv<'a>) -> Option<(ObjectIdentifier, Option<Tlv<'a>>)> {
    if tlv.tag != 0x30 {
        return None;
    }
    let fields = ber::children(tlv.value)?;
    let mut fields = fields.into_iter();
    let oid = fields.next()?;
    (oid.tag == 0x06).then_some(())?;
    let parameters = fields.next();
    fields.next().is_none().then_some(())?;
    Some((ObjectIdentifier::from_der(oid.whole).ok()?, parameters))
}

/// RSAES-OAEP's parameters (RFC 8017 A.2.1, RFC 3560): SHA-1 or SHA-256 with MGF1 of the same
/// hash and an empty label. Any other combination isn't one the key holders do.
fn oaep(parameters: Option<Tlv<'_>>) -> Option<KeyTransport> {
    let Some(parameters) = parameters else { return Some(KeyTransport::OaepSha1) };
    if parameters.tag == 0x05 {
        return Some(KeyTransport::OaepSha1);
    }
    (parameters.tag == 0x30).then_some(())?;
    let mut hash = SHA1;
    let mut mgf_hash = SHA1;
    for field in ber::children(parameters.value)? {
        let inner = ber::read(field.value).filter(|(_, rest)| rest.is_empty())?.0;
        match field.tag {
            0xa0 => hash = algorithm_identifier(&inner)?.0,
            0xa1 => {
                let (oid, parameters) = algorithm_identifier(&inner)?;
                (oid == MGF1).then_some(())?;
                mgf_hash = algorithm_identifier(&parameters?)?.0;
            }
            0xa2 => {
                let (oid, parameters) = algorithm_identifier(&inner)?;
                // pSpecifiedEmpty: an empty label.
                (oid == P_SPECIFIED && parameters.is_some_and(|p| p.tag == 0x04 && p.value.is_empty())).then_some(())?;
            }
            _ => return None,
        }
    }
    match (hash, mgf_hash) {
        (SHA1, SHA1) => Some(KeyTransport::OaepSha1),
        (SHA256, SHA256) => Some(KeyTransport::OaepSha256),
        _ => None,
    }
}

/// The EncryptedContentInfo (`info`), and for AuthEnvelopedData the fields after it.
fn read_encrypted<'a>(info: &Tlv<'a>, auth: bool, rest: &mut impl Iterator<Item = Tlv<'a>>) -> Option<Encrypted<'a>> {
    (info.tag == 0x30).then_some(())?;
    let fields = ber::children(info.value)?;
    let mut fields = fields.into_iter();
    let content_type = fields.next()?;
    (content_type.tag == 0x06).then_some(())?;
    let algorithm = fields.next()?;
    let (oid, parameters) = algorithm_identifier(&algorithm)?;
    let cipher = Cipher::of(oid)?;
    // AuthEnvelopedData is for the AEAD and EnvelopedData for the others (RFC 5083, RFC 8551).
    (cipher.authenticated() == auth).then_some(())?;
    let data = fields.next()?;
    fields.next().is_none().then_some(())?;
    let ciphertext = octets(&data)?;
    let parameters = parameters?;
    let (iv, tag_len) = if auth {
        // GCMParameters ::= SEQUENCE { aes-nonce OCTET STRING, aes-ICVlen INTEGER DEFAULT 12 }
        (parameters.tag == 0x30).then_some(())?;
        let parts = ber::children(parameters.value)?;
        let mut parts = parts.into_iter();
        let nonce = parts.next()?;
        (nonce.tag == 0x04).then_some(())?;
        let length = match parts.next() {
            None => 12,
            Some(n) if n.tag == 0x02 && n.value.len() == 1 => usize::from(*n.value.first()?),
            Some(_) => return None,
        };
        parts.next().is_none().then_some(())?;
        (nonce.value, length)
    } else {
        (parameters.tag == 0x04).then_some(())?;
        (parameters.value, 0)
    };
    let auth = if auth {
        // authAttrs [1] IMPLICIT, mac OCTET STRING, unauthAttrs [2] IMPLICIT.
        let mut next = rest.next()?;
        let attributes = if next.tag == 0xa1 {
            // The attributes are authenticated as their DER with the SET OF tag (RFC 5083 §2.2).
            let mut set = next.whole.to_vec();
            *set.first_mut()? = 0x31;
            next = rest.next()?;
            Some(set)
        } else {
            None
        };
        (next.tag == 0x04).then_some(())?;
        Some(Auth { tag: next.value, attributes, tag_len })
    } else {
        None
    };
    Some(Encrypted { cipher, iv, auth, ciphertext })
}

/// An OCTET STRING's octets, in one piece or in pieces (as `encryptedContent [0] IMPLICIT` is,
/// primitive or constructed).
fn octets(tlv: &Tlv<'_>) -> Option<Vec<u8>> {
    if tlv.tag != 0x80 && tlv.tag != 0xa0 {
        return None;
    }
    if tlv.tag == 0x80 {
        return Some(tlv.value.to_vec());
    }
    let mut out = Vec::new();
    for piece in ber::children(tlv.value)? {
        (piece.tag == 0x04).then_some(())?;
        out.extend_from_slice(piece.value);
    }
    Some(out)
}

/// The content decrypted with `key`; `None` for any fault at all.
fn open(encrypted: &Encrypted<'_>, key: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if key.len() != encrypted.cipher.key_len() {
        return None;
    }
    match (&encrypted.auth, encrypted.cipher) {
        (Some(auth), Cipher::Aes128Gcm) => gcm::<aes_gcm::aes::Aes128>(key, encrypted, auth),
        (Some(auth), Cipher::Aes192Gcm) => gcm::<aes_gcm::aes::Aes192>(key, encrypted, auth),
        (Some(auth), Cipher::Aes256Gcm) => gcm::<aes_gcm::aes::Aes256>(key, encrypted, auth),
        (None, Cipher::Aes128Cbc) => cbc::<aes::Aes128>(key, encrypted),
        (None, Cipher::Aes192Cbc) => cbc::<aes::Aes192>(key, encrypted),
        (None, Cipher::Aes256Cbc) => cbc::<aes::Aes256>(key, encrypted),
        (None, Cipher::TdesCbc) => cbc::<des::TdesEde3>(key, encrypted),
        _ => None,
    }
}

fn cbc<C>(key: &[u8], encrypted: &Encrypted<'_>) -> Option<Zeroizing<Vec<u8>>>
where
    C: aes::cipher::BlockCipherDecrypt + aes::cipher::KeyInit,
    cbc::Decryptor<C>: KeyIvInit + BlockModeDecrypt,
{
    let decryptor = cbc::Decryptor::<C>::new_from_slices(key, encrypted.iv).ok()?;
    let mut buffer = Zeroizing::new(encrypted.ciphertext.clone());
    // (A padding fault is an ordinary failure like every other: nothing says it was the padding.)
    let len = decryptor.decrypt_padded::<Pkcs7>(&mut buffer).ok()?.len();
    buffer.truncate(len);
    Some(buffer)
}

fn gcm<C>(key: &[u8], encrypted: &Encrypted<'_>, auth: &Auth<'_>) -> Option<Zeroizing<Vec<u8>>>
where
    C: aes_gcm::aes::cipher::BlockSizeUser<BlockSize = aes_gcm::aes::cipher::consts::U16>
        + aes_gcm::aes::cipher::BlockEncrypt
        + aes_gcm::aes::cipher::KeyInit
        + aes_gcm::aes::cipher::BlockCipher,
{
    if encrypted.iv.len() != 12 || auth.tag.len() != auth.tag_len {
        return None;
    }
    let nonce = GenericArray::from_slice(encrypted.iv);
    let aad = auth.attributes.as_deref().unwrap_or_default();
    let mut buffer = Zeroizing::new(encrypted.ciphertext.clone());
    let ok = match auth.tag_len {
        12 => {
            let cipher = aes_gcm::AesGcm::<C, U12, U12>::new_from_slice(key).ok()?;
            cipher.decrypt_in_place_detached(nonce, aad, &mut buffer, GenericArray::from_slice(auth.tag)).is_ok()
        }
        16 => {
            let cipher = aes_gcm::AesGcm::<C, U12, U16>::new_from_slice(key).ok()?;
            cipher.decrypt_in_place_detached(nonce, aad, &mut buffer, GenericArray::from_slice(auth.tag)).is_ok()
        }
        _ => false,
    };
    ok.then_some(buffer)
}
