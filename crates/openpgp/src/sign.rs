// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Making an OpenPGP Digital signature (RFC 9580 §5.2, RFC 3156 §5) with a private key this
//! program never holds: the owner's key holder (gpg-agent) signs a digest, and everything
//! around that digest is written here.
//!
//! - [`signing_keys`] reads the owner's Public keys (as `gpg --export` writes them) for the keys
//!   that may sign now, and [`SigningKey::matches`] tells which of them is a key the holder
//!   has, by its public material as the holder gives it (`READKEY`).
//! - [`sign_detached`] writes a version 4 detached signature over the bytes it is given: its
//!   fields are hashed with the data, the holder signs the digest, and the packet made of its
//!   answer is read back and checked against the key before it is armoured. A holder that
//!   answers with anything but a signature by that key over that digest gets
//!   [`SignError::BadSignature`], never a signature that doesn't check.
//!
//! No I/O, no clock and no secrets: the time, the data and the holder are the caller's.

use pgp::composed::{Deserializable, DetachedSignature, SignedPublicKey};
use pgp::crypto::public_key::PublicKeyAlgorithm;
use pgp::ser::Serialize;
use pgp::types::{KeyDetails, KeyVersion};
use sha2::Digest;

use crate::MAX_MESSAGE;
use crate::keys::{self, Budget, Fingerprint, KeyId, PublicKey};

/// The most bytes [`signing_keys`] reads: a large keyring's export.
const MAX_INPUT: usize = 32 << 20;
/// How many certificates (transferable public keys) one call looks at.
const MAX_CERTIFICATES: usize = 1_024;
/// How many signing keys one call gives back.
const MAX_SIGNING_KEYS: usize = 256;
/// How many packets one certificate may have to be read: third-party certifications included,
/// far more than any real key carries.
const MAX_PACKETS_PER_CERTIFICATE: usize = 4_096;
/// How many signature checks one call may cost in all (a certificate needs a handful).
const MAX_CHECKS: usize = 16_384;
/// The largest value of a signature a holder may give: an RSA key of 16,384 bits.
const MAX_VALUE: usize = 2_048;

/// The hashes a signature is made with (RFC 9580 §9.5). MD5 and SHA-1 are not among them, so
/// they can't be asked for; every one here is strong enough for every key this signs with
/// (RFC 9580 §5.2.3.1: an ECDSA key may take any hash of at least its curve's security).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashAlgorithm {
    Sha256,
    Sha384,
    Sha512,
}

impl HashAlgorithm {
    /// The `micalg` parameter of a `multipart/signed` message made with it (RFC 3156 §5).
    pub fn micalg(self) -> &'static str {
        match self {
            HashAlgorithm::Sha256 => "pgp-sha256",
            HashAlgorithm::Sha384 => "pgp-sha384",
            HashAlgorithm::Sha512 => "pgp-sha512",
        }
    }

    /// Its number in a signature packet (RFC 9580 §9.5).
    fn id(self) -> u8 {
        match self {
            HashAlgorithm::Sha256 => 8,
            HashAlgorithm::Sha384 => 9,
            HashAlgorithm::Sha512 => 10,
        }
    }

    fn digest(self, parts: &[&[u8]]) -> Vec<u8> {
        fn run<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
            let mut hasher = D::new();
            for part in parts {
                hasher.update(part);
            }
            hasher.finalize().to_vec()
        }
        match self {
            HashAlgorithm::Sha256 => run::<sha2::Sha256>(parts),
            HashAlgorithm::Sha384 => run::<sha2::Sha384>(parts),
            HashAlgorithm::Sha512 => run::<sha2::Sha512>(parts),
        }
    }
}

/// A signature value as a key holder (gpg-agent) gives it back: big-endian integers, with or
/// without leading zero bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureValue {
    /// RSA: `s`.
    Rsa { s: Vec<u8> },
    /// ECDSA and EdDSA: `r` and `s`.
    Ecc { r: Vec<u8>, s: Vec<u8> },
}

/// A key's public material as gpg-agent's `READKEY` gives it, to tell which agent key is which
/// OpenPGP key: `(rsa (n …)(e …))`, or `(ecc (curve …)(q …))`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentKey<'a> {
    Rsa { n: &'a [u8], e: &'a [u8] },
    Ecc { curve: &'a str, q: &'a [u8] },
}

/// The elliptic curves signed with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Curve {
    P256,
    P384,
    P521,
    Ed25519,
}

impl Curve {
    /// The curve's OID as a key packet writes it (RFC 9580 §9.2).
    fn from_oid(oid: &[u8]) -> Option<Curve> {
        Some(match oid {
            [0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07] => Curve::P256,
            [0x2B, 0x81, 0x04, 0x00, 0x22] => Curve::P384,
            [0x2B, 0x81, 0x04, 0x00, 0x23] => Curve::P521,
            [0x2B, 0x06, 0x01, 0x04, 0x01, 0xDA, 0x47, 0x0F, 0x01] => Curve::Ed25519,
            _ => return None,
        })
    }

    /// A curve's name as gpg-agent (libgcrypt) writes it, or one of its other names or OIDs.
    fn from_name(name: &str) -> Option<Curve> {
        let name = name.trim().trim_matches('"').trim().to_ascii_lowercase();
        Some(match name.as_str() {
            "nist p-256" | "nistp256" | "p-256" | "secp256r1" | "prime256v1" | "1.2.840.10045.3.1.7" => Curve::P256,
            "nist p-384" | "nistp384" | "p-384" | "secp384r1" | "1.3.132.0.34" => Curve::P384,
            "nist p-521" | "nistp521" | "p-521" | "secp521r1" | "1.3.132.0.35" => Curve::P521,
            "ed25519" | "1.3.6.1.4.1.11591.15.1" | "1.3.101.112" => Curve::Ed25519,
            _ => return None,
        })
    }
}

/// What a key signs with, and its public material.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Material {
    /// RSA (1, or 3: sign only): `n` and `e`, without leading zeros.
    Rsa { id: u8, n: Vec<u8>, e: Vec<u8> },
    /// ECDSA (19): the curve and the point `q`.
    Ecdsa { curve: Curve, q: Vec<u8> },
    /// EdDSA in its legacy form (22) on Ed25519: the 32 bytes of the point.
    EddsaLegacy { q: Vec<u8> },
    /// Ed25519 (27): the 32 bytes of the point.
    Ed25519 { q: Vec<u8> },
}

impl Material {
    fn id(&self) -> u8 {
        match self {
            Material::Rsa { id, .. } => *id,
            Material::Ecdsa { .. } => 19,
            Material::EddsaLegacy { .. } => 22,
            Material::Ed25519 { .. } => 27,
        }
    }

    /// The material of a key with `algorithm`, from its public fields as a key packet writes
    /// them (RFC 9580 §5.5.5); `None` for one this doesn't sign with.
    fn read(algorithm: PublicKeyAlgorithm, fields: &[u8]) -> Option<Material> {
        let mut at = Reader(fields);
        let material = match algorithm {
            PublicKeyAlgorithm::RSA | PublicKeyAlgorithm::RSASign => {
                let id = if algorithm == PublicKeyAlgorithm::RSA { 1 } else { 3 };
                Material::Rsa { id, n: at.mpi()?.to_vec(), e: at.mpi()?.to_vec() }
            }
            PublicKeyAlgorithm::ECDSA => {
                let curve = Curve::from_oid(at.oid()?)?;
                if curve == Curve::Ed25519 {
                    return None;
                }
                Material::Ecdsa { curve, q: at.mpi()?.to_vec() }
            }
            PublicKeyAlgorithm::EdDSALegacy => {
                if Curve::from_oid(at.oid()?)? != Curve::Ed25519 {
                    return None;
                }
                Material::EddsaLegacy { q: ed25519_point(at.mpi()?)?.to_vec() }
            }
            PublicKeyAlgorithm::Ed25519 => Material::Ed25519 { q: at.take(32)?.to_vec() },
            _ => return None,
        };
        at.0.is_empty().then_some(material)
    }
}

/// The 32 bytes of an Ed25519 point, from its 0x40-prefixed form (RFC 9580 §11.2.1) or bare.
fn ed25519_point(q: &[u8]) -> Option<&[u8]> {
    let bare = match q {
        [0x40, rest @ ..] if rest.len() == 32 => rest,
        _ => q,
    };
    (bare.len() == 32).then_some(bare)
}

/// `bytes` without the zero bytes it starts with.
fn unpadded(bytes: &[u8]) -> &[u8] {
    let first = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
    bytes.get(first..).unwrap_or_default()
}

/// Reads the public fields of a key packet.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }

    /// An MPI (RFC 9580 §3.2): its bit count, then its bytes; given without leading zeros.
    fn mpi(&mut self) -> Option<&'a [u8]> {
        let bits = self.take(2)?;
        let bits = usize::from(u16::from_be_bytes([*bits.first()?, *bits.get(1)?]));
        Some(unpadded(self.take(bits.div_ceil(8))?))
    }

    /// A curve OID: its length, then its bytes.
    fn oid(&mut self) -> Option<&'a [u8]> {
        let len = *self.take(1)?.first()?;
        self.take(usize::from(len))
    }
}

/// The key packet a signature is checked against.
#[derive(Clone, Debug)]
enum KeyPacket {
    Primary(pgp::packet::PublicKey),
    Sub(pgp::packet::PublicSubkey),
}

/// A version 4 primary key or subkey that may make Digital signatures now.
#[derive(Clone, Debug)]
pub struct SigningKey {
    fingerprint: Fingerprint,
    primary: Fingerprint,
    key_id: KeyId,
    addresses: Vec<String>,
    algorithm: &'static str,
    created: u64,
    expires: Option<u64>,
    material: Material,
    packet: KeyPacket,
}

impl SigningKey {
    /// This key's fingerprint (the subkey's, for a subkey).
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// The fingerprint of the certificate's primary key: what the owner knows the key by.
    pub fn primary(&self) -> &Fingerprint {
        &self.primary
    }

    /// This key's key id.
    pub fn key_id(&self) -> KeyId {
        self.key_id
    }

    /// The addresses of the User IDs the certificate certified itself (lower case).
    pub fn addresses(&self) -> &[String] {
        &self.addresses
    }

    /// The key's algorithm, by name ("RSA", "ECDSA", "EdDSA", "Ed25519").
    pub fn algorithm(&self) -> &'static str {
        self.algorithm
    }

    /// When this key was made, in seconds since 1970.
    pub fn created(&self) -> u64 {
        self.created
    }

    /// When it stops being valid (its own expiry or the primary key's, the earlier), in
    /// seconds since 1970; `None` when never.
    pub fn expires(&self) -> Option<u64> {
        self.expires
    }

    /// Whether `agent` (a key the holder has, as `READKEY` gives it) is this key: the same
    /// public material, leading zero bytes and the 0x40 in front of an Ed25519 point aside.
    pub fn matches(&self, agent: &AgentKey<'_>) -> bool {
        match (&self.material, agent) {
            (Material::Rsa { n, e, .. }, AgentKey::Rsa { n: an, e: ae }) => {
                !n.is_empty() && unpadded(an) == n.as_slice() && unpadded(ae) == e.as_slice()
            }
            (Material::Ecdsa { curve, q }, AgentKey::Ecc { curve: name, q: aq }) => {
                Curve::from_name(name) == Some(*curve) && !q.is_empty() && unpadded(aq) == q.as_slice()
            }
            (Material::EddsaLegacy { q } | Material::Ed25519 { q }, AgentKey::Ecc { curve: name, q: aq }) => {
                Curve::from_name(name) == Some(Curve::Ed25519) && ed25519_point(aq) == Some(q.as_slice())
            }
            _ => false,
        }
    }
}

/// The keys of `bytes` (binary or armoured transferable public keys, as many as `gpg --export`
/// writes) that can make a Digital signature at `now` (seconds since 1970): version 4, with a
/// self-signature that checks, not revoked, not expired, made no later than `now`, of an
/// algorithm this signs with and strong enough to be trusted, and allowed to sign (by the
/// signing key flag, or, with no key flags at all, a primary key whose algorithm signs). A
/// subkey also needs a binding signature that checks with a primary-key binding signature that
/// checks inside it (RFC 9580 §5.2.1, 0x19), and a primary key that is itself neither revoked
/// nor expired.
///
/// At most 32 MB is read, 1,024 certificates looked at and 256 keys given back; nothing here
/// panics on any bytes.
pub fn signing_keys(bytes: &[u8], now: u64) -> Vec<SigningKey> {
    let mut out = Vec::new();
    if bytes.len() > MAX_INPUT {
        return out;
    }
    let blocks = crate::armour::blocks(bytes, crate::armour::Kind::PublicKey, MAX_CERTIFICATES, MAX_INPUT);
    let blocks = if blocks.is_empty() { vec![bytes.to_vec()] } else { blocks };
    let mut certificates = MAX_CERTIFICATES;
    let mut budget = Budget::new(MAX_CHECKS);
    for block in &blocks {
        for certificate in split_certificates(block, certificates) {
            certificates = certificates.saturating_sub(1);
            let Some(Ok(key)) = crate::guard(|| SignedPublicKey::from_bytes(certificate)) else { continue };
            let Some(Some(key)) = crate::guard(|| PublicKey::checked(key, &mut budget)) else { continue };
            for signing in crate::guard(|| of(&key, now)).unwrap_or_default() {
                if out.len() >= MAX_SIGNING_KEYS {
                    return out;
                }
                out.push(signing);
            }
        }
        if certificates == 0 || budget.starved {
            break;
        }
    }
    out
}

/// The certificates in binary packets `bytes`, at most `max`: from each Public-Key packet (tag
/// 6) to the next (RFC 9580 §10.1), told by the packets' headers alone (§4.2). One with more
/// packets than any real key is left out; what follows bytes that aren't a packet is not read.
fn split_certificates(bytes: &[u8], max: usize) -> Vec<&[u8]> {
    fn close<'a>(bytes: &'a [u8], start: Option<usize>, end: usize, packets: usize, out: &mut Vec<&'a [u8]>) {
        if let Some(begin) = start
            && packets <= MAX_PACKETS_PER_CERTIFICATE
            && let Some(slice) = bytes.get(begin..end)
        {
            out.push(slice);
        }
    }
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut packets = 0usize;
    let mut at = 0usize;
    while out.len() < max {
        let Some((tag, len)) = packet_header(bytes.get(at..).unwrap_or_default()) else { break };
        if tag == 6 {
            close(bytes, start, at, packets, &mut out);
            start = Some(at);
            packets = 0;
        }
        packets = packets.saturating_add(1);
        at = at.saturating_add(len);
    }
    if out.len() < max {
        close(bytes, start, at, packets, &mut out);
    }
    out
}

/// The tag of the packet `bytes` starts with and its whole length (header and body); `None`
/// when it isn't a whole packet of a definite length.
fn packet_header(bytes: &[u8]) -> Option<(u8, usize)> {
    let first = *bytes.first()?;
    let byte = |i: usize| bytes.get(i).copied().map(usize::from);
    let (tag, header, body): (u8, usize, usize) = match first & 0xC0 {
        0xC0 => {
            let o1 = byte(1)?;
            let (header, body) = match o1 {
                0..=191 => (2, o1),
                192..=223 => (3, ((o1 - 192) << 8) + byte(2)? + 192),
                255 => (6, (byte(2)? << 24) | (byte(3)? << 16) | (byte(4)? << 8) | byte(5)?),
                // A partial length: never in a key.
                _ => return None,
            };
            (first & 0x3F, header, body)
        }
        0x80 => {
            let (header, body) = match first & 0x03 {
                0 => (2, byte(1)?),
                1 => (3, (byte(1)? << 8) | byte(2)?),
                2 => (5, (byte(1)? << 24) | (byte(2)? << 16) | (byte(3)? << 8) | byte(4)?),
                _ => return None,
            };
            ((first >> 2) & 0x0F, header, body)
        }
        _ => return None,
    };
    let len = header.checked_add(body)?;
    (len <= bytes.len()).then_some((tag, len))
}

/// The keys of a checked certificate that may sign at `now`.
fn of(key: &PublicKey, now: u64) -> Vec<SigningKey> {
    let mut out = Vec::new();
    let Some(primary) = key.primary() else { return out };
    let usable = |s: &keys::Signing| {
        s.version == KeyVersion::V4
            && s.revoked.is_none_or(|r| !r.covers(Some(now)))
            && !s.expired_at(now)
            && u64::from(s.created) <= now
            && s.weak.is_none()
    };
    if !usable(primary) {
        return out;
    }
    let inner = key.inner();
    for signing in &key.signing {
        if !signing.may_sign || !usable(signing) {
            continue;
        }
        let (packet, fields) = match signing.subkey {
            None => (KeyPacket::Primary(inner.primary_key.clone()), inner.primary_key.public_params().to_bytes()),
            Some(i) => {
                let Some(sub) = inner.public_subkeys.get(i) else { continue };
                (KeyPacket::Sub(sub.key.clone()), sub.key.public_params().to_bytes())
            }
        };
        let Some(material) = fields.ok().and_then(|f| Material::read(signing.algorithm, &f)) else { continue };
        let expires = match (signing.expires, primary.expires) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        out.push(SigningKey {
            fingerprint: signing.fingerprint.clone(),
            primary: key.fingerprint().clone(),
            key_id: signing.key_id,
            addresses: key.addresses().to_vec(),
            algorithm: keys::algorithm_name(signing.algorithm),
            created: u64::from(signing.created),
            expires,
            material,
            packet,
        });
    }
    out
}

/// Why a Digital signature couldn't be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignError<E> {
    /// The key is of a kind this doesn't sign with (its name).
    Unsupported(&'static str),
    /// What is to be signed is larger than any message sent.
    TooLarge,
    /// The key holder failed or refused: what it said.
    Holder(E),
    /// What the key holder gave back isn't a signature by this key over this data.
    BadSignature,
    /// The signature couldn't be written.
    Encoding,
}

impl<E: std::fmt::Display> std::fmt::Display for SignError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignError::Unsupported(what) => write!(f, "this key can't sign here: {what}"),
            SignError::TooLarge => f.write_str("the message is too large to sign"),
            SignError::Holder(e) => write!(f, "the key holder couldn't sign: {e}"),
            SignError::BadSignature => f.write_str("the key holder's signature doesn't check"),
            SignError::Encoding => f.write_str("the signature couldn't be written"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for SignError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SignError::Holder(e) => Some(e),
            _ => None,
        }
    }
}

/// A detached version 4 signature of type 0x00 (a binary document) over `data`, made at
/// `created` (seconds since 1970) by `key` with `hash`, armoured (`-----BEGIN PGP
/// SIGNATURE-----`, CR LF lines): the `application/pgp-signature` part of a PGP/MIME message
/// (RFC 3156 §5), whose `micalg` is [`HashAlgorithm::micalg`]. `data` is the signed part as it
/// is sent, in canonical form (CR LF line ends): the caller makes it so.
///
/// `sign_digest` is the key holder: it is handed the digest (of `hash`'s size) and gives back
/// the signature value: RSA PKCS #1 v1.5 over the digest with its DigestInfo, ECDSA over the
/// digest, or EdDSA with the digest as its message, as gpg-agent's `PKSIGN` makes them. What it
/// gives back is checked against the key over `data` before anything is written.
pub fn sign_detached<E>(
    data: &[u8],
    key: &SigningKey,
    created: u32,
    hash: HashAlgorithm,
    sign_digest: impl FnOnce(&[u8]) -> Result<SignatureValue, E>,
) -> Result<String, SignError<E>> {
    if data.len() > MAX_MESSAGE {
        return Err(SignError::TooLarge);
    }
    let version = match &key.packet {
        KeyPacket::Primary(k) => k.version(),
        KeyPacket::Sub(k) => k.version(),
    };
    if version != KeyVersion::V4 {
        return Err(SignError::Unsupported("a key of a version other than 4"));
    }
    let fingerprint = key.fingerprint.as_bytes();
    if fingerprint.len() != 20 {
        return Err(SignError::Unsupported("a key of a version other than 4"));
    }

    // RFC 9580 §5.2.3: version, type, algorithms, and the hashed subpackets: Signature Creation
    // Time (2, §5.2.3.11) and Issuer Fingerprint (33, §5.2.3.35).
    let mut hashed_subpackets = vec![5u8, 2];
    hashed_subpackets.extend_from_slice(&created.to_be_bytes());
    hashed_subpackets.extend_from_slice(&[22, 33, 4]);
    hashed_subpackets.extend_from_slice(fingerprint);
    let mut hashed = vec![4u8, 0x00, key.material.id(), hash.id()];
    hashed.extend_from_slice(&u16::try_from(hashed_subpackets.len()).map_err(|_| SignError::Encoding)?.to_be_bytes());
    hashed.extend_from_slice(&hashed_subpackets);
    // §5.2.4: the data, the hashed part, then 0x04 0xFF and the hashed part's length.
    let mut trailer = vec![0x04u8, 0xFF];
    trailer.extend_from_slice(&u32::try_from(hashed.len()).map_err(|_| SignError::Encoding)?.to_be_bytes());
    let digest = hash.digest(&[data, &hashed, &trailer]);

    let value = sign_digest(&digest).map_err(SignError::Holder)?;
    let value = encode_value(&key.material, value).ok_or(SignError::BadSignature)?;

    let mut body = hashed;
    // The unhashed area: Issuer Key ID (16, §5.2.3.12), for readers that look for it.
    body.extend_from_slice(&[0, 10, 9, 16]);
    body.extend_from_slice(&key.key_id.0);
    body.extend_from_slice(digest.get(..2).ok_or(SignError::Encoding)?);
    body.extend_from_slice(&value);
    let packet = packet(2, &body).ok_or(SignError::Encoding)?;

    // Read back as any reader would, and checked against the key over the data.
    let checks = crate::guard(|| {
        let Ok(signature) = DetachedSignature::from_bytes(packet.as_slice()) else { return false };
        match &key.packet {
            KeyPacket::Primary(k) => signature.verify(k, data).is_ok(),
            KeyPacket::Sub(k) => signature.verify(k, data).is_ok(),
        }
    });
    if checks != Some(true) {
        return Err(SignError::BadSignature);
    }
    Ok(armour(&packet))
}

/// The signature value's fields for `material`'s algorithm (RFC 9580 §5.2.3.1–§5.2.3.4): one
/// MPI for RSA, two for ECDSA and legacy EdDSA, 64 bytes for Ed25519. `None` when the value is
/// of the wrong kind or size for the key.
fn encode_value(material: &Material, value: SignatureValue) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    match (material, value) {
        (Material::Rsa { n, .. }, SignatureValue::Rsa { s }) => {
            let s = unpadded(&s);
            if s.is_empty() || s.len() > n.len() || s.len() > MAX_VALUE {
                return None;
            }
            mpi(&mut out, s)?;
        }
        (Material::Ecdsa { curve, .. }, SignatureValue::Ecc { r, s }) => {
            let size = match curve {
                Curve::P256 => 32,
                Curve::P384 => 48,
                Curve::P521 => 66,
                Curve::Ed25519 => return None,
            };
            for v in [unpadded(&r), unpadded(&s)] {
                if v.is_empty() || v.len() > size {
                    return None;
                }
                mpi(&mut out, v)?;
            }
        }
        (Material::EddsaLegacy { .. }, SignatureValue::Ecc { r, s }) => {
            for v in [unpadded(&r), unpadded(&s)] {
                if v.len() > 32 {
                    return None;
                }
                mpi(&mut out, v)?;
            }
        }
        (Material::Ed25519 { .. }, SignatureValue::Ecc { r, s }) => {
            for v in [unpadded(&r), unpadded(&s)] {
                let pad = 32usize.checked_sub(v.len())?;
                out.extend(std::iter::repeat_n(0u8, pad));
                out.extend_from_slice(v);
            }
        }
        _ => return None,
    }
    Some(out)
}

/// Writes `value` (no leading zeros) as an MPI: its length in bits, then its bytes (§3.2).
fn mpi(out: &mut Vec<u8>, value: &[u8]) -> Option<()> {
    let bits = match value.first() {
        None => 0,
        Some(first) => value.len().checked_sub(1)?.checked_mul(8)?.checked_add(8usize.checked_sub(first.leading_zeros() as usize)?)?,
    };
    out.extend_from_slice(&u16::try_from(bits).ok()?.to_be_bytes());
    out.extend_from_slice(value);
    Some(())
}

/// A packet of `tag` with `body`, in the new format (RFC 9580 §4.2.1).
fn packet(tag: u8, body: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![0xC0 | (tag & 0x3F)];
    let len = body.len();
    if len < 192 {
        out.push(u8::try_from(len).ok()?);
    } else if len < 8384 {
        let rest = len.checked_sub(192)?;
        out.push(u8::try_from((rest >> 8).checked_add(192)?).ok()?);
        out.push(u8::try_from(rest & 0xFF).ok()?);
    } else {
        out.push(0xFF);
        out.extend_from_slice(&u32::try_from(len).ok()?.to_be_bytes());
    }
    out.extend_from_slice(body);
    Some(out)
}

/// `packets` armoured as a signature (RFC 9580 §6.2), in CR LF lines of 64 characters, with no
/// checksum line (§6.1: a writer need not, and a reader must not need one).
fn armour(packets: &[u8]) -> String {
    let encoded = mime_base64(packets);
    let mut out = String::from("-----BEGIN PGP SIGNATURE-----\r\n\r\n");
    for line in encoded.as_bytes().chunks(64) {
        out.push_str(&String::from_utf8_lossy(line));
        out.push_str("\r\n");
    }
    out.push_str("-----END PGP SIGNATURE-----\r\n");
    out
}

/// `bytes` in base64 (RFC 4648 §4), one line.
fn mime_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let symbol = |v: u32| char::from(ALPHABET.get((v & 63) as usize).copied().unwrap_or(b'='));
    let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for chunk in bytes.chunks(3) {
        let b = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let v = (b(0) << 16) | (b(1) << 8) | b(2);
        out.push(symbol(v >> 18));
        out.push(symbol(v >> 12));
        out.push(if chunk.len() > 1 { symbol(v >> 6) } else { '=' });
        out.push(if chunk.len() > 2 { symbol(v) } else { '=' });
    }
    out
}
