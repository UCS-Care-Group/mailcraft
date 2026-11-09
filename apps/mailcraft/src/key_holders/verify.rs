// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whether a signature a token made is the certificate's: checked when the token's private key
//! was paired with the certificate only through a public-key object with the same `CKA_ID`
//! (`pkcs11.rs`), which may be a stale one of another key.
//!
//! Written from the public specifications: RSAVP1, EMSA-PKCS1-v1_5 and EMSA-PSS-VERIFY with MGF1
//! (RFC 8017 §5.2.2, §9.2, §9.1.2, B.2.1); ECDSA verification (SEC 1 v2 §4.1.4) on the curves of
//! FIPS 186-4 D.1.2.3 (P-256) and D.1.2.4 (P-384). Every input is public (a certificate's key, a
//! digest, a signature), so the arithmetic need not run in constant time; sizes are capped so
//! that a hostile certificate can't make it run long.

use num_bigint::BigUint;
use x509_cert::Certificate;
use x509_cert::der::asn1::UintRef;
use x509_cert::der::{Decode, Reader, SliceReader};

use super::certificate::{HeldCertificate, without_leading_zeros};
use super::encoding::digest_info;
use super::{DigestAlgorithm, KeyKind, SignatureScheme};

/// The largest RSA modulus checked, in bytes (16384 bits).
const MAX_MODULUS_LEN: usize = 2048;
/// The largest RSA public exponent checked, in bytes.
const MAX_EXPONENT_LEN: usize = 8;

/// Whether `signature` (raw RSA, or ECDSA's `r || s`) is the certificate's key's over `digest`.
pub fn signed_by(certificate: &HeldCertificate, scheme: SignatureScheme, algorithm: DigestAlgorithm, digest: &[u8], signature: &[u8]) -> bool {
    match (certificate.key, scheme) {
        (KeyKind::Rsa { bits }, SignatureScheme::RsaPkcs1v15 | SignatureScheme::RsaPss) => {
            rsa(&certificate.der, bits, scheme, algorithm, digest, signature).unwrap_or(false)
        }
        (KeyKind::EcP256, SignatureScheme::Ecdsa) => Curve::p256().is_some_and(|c| c.verify(&certificate.public_key, digest, signature)),
        (KeyKind::EcP384, SignatureScheme::Ecdsa) => Curve::p384().is_some_and(|c| c.verify(&certificate.public_key, digest, signature)),
        _ => false,
    }
}

/// The RSA public key of the certificate `der`: modulus and exponent, big-endian.
fn rsa_public_key(der: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let cert = Certificate::from_der(der).ok()?;
    let key = cert.tbs_certificate().subject_public_key_info().subject_public_key.raw_bytes();
    let mut reader = SliceReader::new(key).ok()?;
    reader
        .sequence(|seq| {
            let n = UintRef::decode(seq)?;
            let e = UintRef::decode(seq)?;
            Ok::<_, x509_cert::der::Error>((without_leading_zeros(n.as_bytes()).to_vec(), without_leading_zeros(e.as_bytes()).to_vec()))
        })
        .ok()
}

fn rsa(der: &[u8], bits: u32, scheme: SignatureScheme, algorithm: DigestAlgorithm, digest: &[u8], signature: &[u8]) -> Option<bool> {
    let (n, e) = rsa_public_key(der)?;
    if n.len() > MAX_MODULUS_LEN || e.len() > MAX_EXPONENT_LEN || e.is_empty() || signature.len() != n.len() {
        return Some(false);
    }
    let modulus = BigUint::from_bytes_be(&n);
    let s = BigUint::from_bytes_be(signature);
    if s >= modulus {
        return Some(false);
    }
    // RSAVP1, then I2OSP to the modulus's length.
    let m = s.modpow(&BigUint::from_bytes_be(&e), &modulus).to_bytes_be();
    let mut em = vec![0u8; n.len().checked_sub(m.len())?];
    em.extend(m);
    Some(match scheme {
        SignatureScheme::RsaPkcs1v15 => {
            let t = digest_info(algorithm, digest).ok()?;
            let padding = n.len().checked_sub(t.len())?.checked_sub(3)?;
            if padding < 8 {
                return Some(false);
            }
            let mut expected = vec![0x00, 0x01];
            expected.resize(2 + padding, 0xff);
            expected.push(0x00);
            expected.extend(t);
            em == expected
        }
        SignatureScheme::RsaPss => pss(&em, bits.checked_sub(1)?, algorithm, digest)?,
        SignatureScheme::Ecdsa => false,
    })
}

fn ring_algorithm(algorithm: DigestAlgorithm) -> &'static ring::digest::Algorithm {
    match algorithm {
        DigestAlgorithm::Sha256 => &ring::digest::SHA256,
        DigestAlgorithm::Sha384 => &ring::digest::SHA384,
        DigestAlgorithm::Sha512 => &ring::digest::SHA512,
    }
}

/// EMSA-PSS-VERIFY (RFC 8017 §9.1.2), the salt as long as the hash (how tokens are asked to sign
/// here). `em` is RSAVP1's output at the modulus's length; `em_bits` is the modulus's bits less one.
fn pss(em: &[u8], em_bits: u32, algorithm: DigestAlgorithm, digest: &[u8]) -> Option<bool> {
    let h_len = algorithm.digest_len();
    let s_len = h_len;
    let em_len = usize::try_from(em_bits.div_ceil(8)).ok()?;
    // With a modulus of 8k+1 bits, EM is one byte shorter than the modulus, whose first is zero.
    let em = match em.len().checked_sub(em_len)? {
        0 => em,
        1 if em.first() == Some(&0) => em.get(1..)?,
        _ => return Some(false),
    };
    if em_len < h_len.checked_add(s_len)?.checked_add(2)? || em.last() != Some(&0xbc) {
        return Some(false);
    }
    let db_len = em_len.checked_sub(h_len)?.checked_sub(1)?;
    let masked_db = em.get(..db_len)?;
    let h = em.get(db_len..db_len.checked_add(h_len)?)?;
    let unused_bits = u32::try_from(em_len.checked_mul(8)?).ok()?.checked_sub(em_bits)?;
    let top_mask = 0xffu8.checked_shr(unused_bits).unwrap_or(0);
    if masked_db.first().is_some_and(|b| b & !top_mask != 0) {
        return Some(false);
    }
    let mut db: Vec<u8> = masked_db.iter().zip(mgf1(algorithm, h, db_len)).map(|(a, b)| a ^ b).collect();
    if let Some(first) = db.first_mut() {
        *first &= top_mask;
    }
    let zeros = em_len.checked_sub(h_len)?.checked_sub(s_len)?.checked_sub(2)?;
    if db.get(..zeros)?.iter().any(|b| *b != 0) || db.get(zeros) != Some(&0x01) {
        return Some(false);
    }
    let salt = db.get(db_len.checked_sub(s_len)?..)?;
    let mut m_prime = vec![0u8; 8];
    m_prime.extend_from_slice(digest);
    m_prime.extend_from_slice(salt);
    Some(ring::digest::digest(ring_algorithm(algorithm), &m_prime).as_ref() == h)
}

/// MGF1 (RFC 8017 B.2.1): `len` bytes from `seed`.
fn mgf1(algorithm: DigestAlgorithm, seed: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut counter: u32 = 0;
    while out.len() < len {
        let mut block = seed.to_vec();
        block.extend(counter.to_be_bytes());
        out.extend_from_slice(ring::digest::digest(ring_algorithm(algorithm), &block).as_ref());
        counter = counter.saturating_add(1);
    }
    out.truncate(len);
    out
}

/// A point in affine coordinates; `None` is the point at infinity.
type Point = Option<(BigUint, BigUint)>;

/// A NIST prime curve y² = x³ − 3x + b over GF(p), with base point `g` of order `n`.
struct Curve {
    p: BigUint,
    b: BigUint,
    n: BigUint,
    g: (BigUint, BigUint),
    /// The length of a coordinate, in bytes.
    len: usize,
}

fn number(hex: &str) -> Option<BigUint> {
    BigUint::parse_bytes(hex.as_bytes(), 16)
}

impl Curve {
    /// P-256, FIPS 186-4 D.1.2.3.
    fn p256() -> Option<Self> {
        Some(Self {
            p: number("ffffffff00000001000000000000000000000000ffffffffffffffffffffffff")?,
            b: number("5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b")?,
            n: number("ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551")?,
            g: (
                number("6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296")?,
                number("4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5")?,
            ),
            len: 32,
        })
    }

    /// P-384, FIPS 186-4 D.1.2.4.
    fn p384() -> Option<Self> {
        Some(Self {
            p: number("fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffeffffffff0000000000000000ffffffff")?,
            b: number("b3312fa7e23ee7e4988e056be3f82d19181d9c6efe8141120314088f5013875ac656398d8a2ed19d2a85c8edd3ec2aef")?,
            n: number("ffffffffffffffffffffffffffffffffffffffffffffffffc7634d81f4372ddf581a0db248b0a77aecec196accc52973")?,
            g: (
                number("aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760ab7")?,
                number("3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea0e5f")?,
            ),
            len: 48,
        })
    }

    fn sub(&self, a: &BigUint, b: &BigUint) -> BigUint {
        (a + &self.p - (b % &self.p)) % &self.p
    }

    /// The inverse of `a` modulo the prime `m` (Fermat).
    fn inverse(a: &BigUint, m: &BigUint) -> BigUint {
        a.modpow(&(m - 2u32), m)
    }

    fn on_curve(&self, (x, y): &(BigUint, BigUint)) -> bool {
        let left = (y * y) % &self.p;
        let right = self.sub(&((x * x * x + &self.b) % &self.p), &((x * 3u32) % &self.p));
        x < &self.p && y < &self.p && left == right
    }

    fn add(&self, a: &Point, b: &Point) -> Point {
        let ((x1, y1), (x2, y2)) = match (a, b) {
            (None, q) | (q, None) => return q.clone(),
            (Some(a), Some(b)) => (a, b),
        };
        let p = &self.p;
        let lambda = if x1 == x2 {
            if (y1 + y2) % p == BigUint::ZERO {
                return None;
            }
            // Doubling, a = −3: (3x² − 3) / 2y.
            let top = self.sub(&((x1 * x1 * 3u32) % p), &BigUint::from(3u32));
            (top * Self::inverse(&((y1 * 2u32) % p), p)) % p
        } else {
            (self.sub(y2, y1) * Self::inverse(&self.sub(x2, x1), p)) % p
        };
        let x3 = self.sub(&self.sub(&((&lambda * &lambda) % p), x1), x2);
        let y3 = self.sub(&((&lambda * self.sub(x1, &x3)) % p), y1);
        Some((x3, y3))
    }

    fn times(&self, k: &BigUint, point: &Point) -> Point {
        let mut result: Point = None;
        for i in (0..k.bits()).rev() {
            result = self.add(&result, &result);
            if k.bit(i) {
                result = self.add(&result, point);
            }
        }
        result
    }

    /// SEC 1 v2 §4.1.4 against the uncompressed point `public` (`04 || x || y`); `signature` is
    /// `r || s`.
    fn verify(&self, public: &[u8], digest: &[u8], signature: &[u8]) -> bool {
        let len = self.len;
        if public.len() != 1 + 2 * len || public.first() != Some(&0x04) || signature.len() != 2 * len {
            return false;
        }
        let (Some(x), Some(y), Some(r), Some(s)) = (public.get(1..=len), public.get(1 + len..), signature.get(..len), signature.get(len..)) else {
            return false;
        };
        let q = (BigUint::from_bytes_be(x), BigUint::from_bytes_be(y));
        let (r, s) = (BigUint::from_bytes_be(r), BigUint::from_bytes_be(s));
        let n = &self.n;
        if !self.on_curve(&q) || r == BigUint::ZERO || s == BigUint::ZERO || &r >= n || &s >= n {
            return false;
        }
        // The digest's leftmost bits, as many as the order has.
        let mut e = BigUint::from_bytes_be(digest);
        let digest_bits = u64::try_from(digest.len()).unwrap_or(u64::MAX).saturating_mul(8);
        if digest_bits > n.bits() {
            e >>= digest_bits - n.bits();
        }
        let w = Self::inverse(&s, n);
        let u1 = (e * &w) % n;
        let u2 = (&r * &w) % n;
        match self.add(&self.times(&u1, &Some(self.g.clone())), &self.times(&u2, &Some(q))) {
            Some((x, _)) => x % n == r,
            None => false,
        }
    }
}
