//! Who vouches for a signer: a certificate's chain up to a root the owner trusts (RFC 5280,
//! simplified: names are matched exactly, each certificate's signature is checked with the next
//! one's key, every certificate on the way must be in force, every issuer must be a certificate
//! authority, and the signer's certificate, when it says what it is for, must be for e-mail).
//!
//! Revocation is not checked (no CRL or OCSP is fetched: that would tell a server who wrote to
//! the owner), and the UI says so.

use der::{Decode, Encode};
use spki::{AlgorithmIdentifierOwned, ObjectIdentifier, SubjectPublicKeyInfoOwned};
use x509_cert::Certificate;

use crate::keys::{ANY_EKU, EMAIL_PROTECTION, common_name};
use crate::{PublicKey, Scheme, Trust};

/// Longest chain followed.
const MAX_DEPTH: usize = 8;

const RSA: ObjectIdentifier = const_oid::db::rfc5912::RSA_ENCRYPTION;
const SHA256_RSA: ObjectIdentifier = const_oid::db::rfc5912::SHA_256_WITH_RSA_ENCRYPTION;
const SHA384_RSA: ObjectIdentifier = const_oid::db::rfc5912::SHA_384_WITH_RSA_ENCRYPTION;
const SHA512_RSA: ObjectIdentifier = const_oid::db::rfc5912::SHA_512_WITH_RSA_ENCRYPTION;
const SHA1_RSA: ObjectIdentifier = const_oid::db::rfc5912::SHA_1_WITH_RSA_ENCRYPTION;
const ECDSA_SHA256: ObjectIdentifier = const_oid::db::rfc5912::ECDSA_WITH_SHA_256;
const ECDSA_SHA384: ObjectIdentifier = const_oid::db::rfc5912::ECDSA_WITH_SHA_384;
const EC_PUBLIC_KEY: ObjectIdentifier = const_oid::db::rfc5912::ID_EC_PUBLIC_KEY;
pub(crate) const SHA256: ObjectIdentifier = const_oid::db::rfc5912::ID_SHA_256;
pub(crate) const SHA384: ObjectIdentifier = const_oid::db::rfc5912::ID_SHA_384;
pub(crate) const SHA512: ObjectIdentifier = const_oid::db::rfc5912::ID_SHA_512;
pub(crate) const SHA1: ObjectIdentifier = const_oid::db::rfc5912::ID_SHA_1;

/// Which digest goes with a signature algorithm, given the digest named beside it (CMS names
/// `rsaEncryption` with the digest apart).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hash {
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    pub(crate) fn of(oid: ObjectIdentifier) -> Result<Hash, String> {
        match oid {
            o if o == SHA256 => Ok(Hash::Sha256),
            o if o == SHA384 => Ok(Hash::Sha384),
            o if o == SHA512 => Ok(Hash::Sha512),
            o if o == SHA1 => Err("it was made with SHA-1, which can be forged".into()),
            _ => Err("it was made with a digest this program doesn't know".into()),
        }
    }

    pub(crate) fn digest(self, data: &[u8]) -> Vec<u8> {
        use sha2::Digest as _;
        match self {
            Hash::Sha256 => sha2::Sha256::digest(data).to_vec(),
            Hash::Sha384 => sha2::Sha384::digest(data).to_vec(),
            Hash::Sha512 => sha2::Sha512::digest(data).to_vec(),
        }
    }
}

/// Check `signature` over `message` with a public key, for a signature algorithm (and, for plain
/// `rsaEncryption`, the digest named beside it). `Err` says why it doesn't hold.
pub(crate) fn verify_signature(
    key: &SubjectPublicKeyInfoOwned,
    algorithm: &AlgorithmIdentifierOwned,
    digest: Option<ObjectIdentifier>,
    message: &[u8],
    signature: &[u8],
) -> Result<(), String> {
    let oid = algorithm.oid;
    let hash = match oid {
        o if o == SHA256_RSA || o == ECDSA_SHA256 => Hash::Sha256,
        o if o == SHA384_RSA || o == ECDSA_SHA384 => Hash::Sha384,
        o if o == SHA512_RSA => Hash::Sha512,
        o if o == SHA1_RSA => return Err("it was made with SHA-1, which can be forged".into()),
        o if o == RSA || o == EC_PUBLIC_KEY => Hash::of(digest.ok_or("no digest is named")?)?,
        _ => return Err("it was made with an algorithm this program doesn't check".into()),
    };
    let spki_der = key.to_der().map_err(|_| "the key can't be read".to_string())?;
    let hashed = hash.digest(message);
    if key.algorithm.oid == RSA {
        use pkcs8::DecodePublicKey as _;
        let public = rsa::RsaPublicKey::from_public_key_der(&spki_der).map_err(|_| "the key can't be read".to_string())?;
        let scheme = match hash {
            Hash::Sha256 => rsa::Pkcs1v15Sign::new::<sha2::Sha256>(),
            Hash::Sha384 => rsa::Pkcs1v15Sign::new::<sha2::Sha384>(),
            Hash::Sha512 => rsa::Pkcs1v15Sign::new::<sha2::Sha512>(),
        };
        return public.verify(scheme, &hashed, signature).map_err(|_| "the signature doesn't match".to_string());
    }
    if key.algorithm.oid == EC_PUBLIC_KEY {
        use p256::ecdsa::signature::hazmat::PrehashVerifier as _;
        use pkcs8::DecodePublicKey as _;
        let public =
            p256::ecdsa::VerifyingKey::from_public_key_der(&spki_der).map_err(|_| "only P-256 elliptic-curve keys are checked".to_string())?;
        let sig = p256::ecdsa::Signature::from_der(signature).map_err(|_| "the signature can't be read".to_string())?;
        return public.verify_prehash(&hashed, &sig).map_err(|_| "the signature doesn't match".to_string());
    }
    Err("the signer's key is of a kind this program doesn't check".into())
}

/// Whether `issuer`'s key made `cert`'s signature.
fn signed_by(cert: &Certificate, issuer: &Certificate) -> bool {
    let Ok(tbs) = cert.tbs_certificate.to_der() else { return false };
    let Some(sig) = cert.signature.as_bytes() else { return false };
    verify_signature(&issuer.tbs_certificate.subject_public_key_info, &cert.signature_algorithm, None, &tbs, sig).is_ok()
}

fn in_force(cert: &Certificate, now: i64) -> bool {
    let v = &cert.tbs_certificate.validity;
    let from = v.not_before.to_unix_duration().as_secs() as i64;
    let to = v.not_after.to_unix_duration().as_secs() as i64;
    from <= now && now <= to
}

fn is_ca(cert: &Certificate) -> bool {
    cert.tbs_certificate.extensions.iter().flatten().any(|e| {
        e.extn_id == const_oid::db::rfc5280::ID_CE_BASIC_CONSTRAINTS
            && x509_cert::ext::pkix::BasicConstraints::from_der(e.extn_value.as_bytes()).is_ok_and(|b| b.ca)
    })
}

/// Whether a certificate that says what it is for says e-mail.
fn for_email(cert: &Certificate) -> bool {
    for e in cert.tbs_certificate.extensions.iter().flatten() {
        if e.extn_id == const_oid::db::rfc5280::ID_CE_EXT_KEY_USAGE {
            return x509_cert::ext::pkix::ExtendedKeyUsage::from_der(e.extn_value.as_bytes())
                .is_ok_and(|u| u.0.iter().any(|o| *o == EMAIL_PROTECTION || *o == ANY_EKU));
        }
    }
    true
}

fn same_name(a: &x509_cert::name::Name, b: &x509_cert::name::Name) -> bool {
    a.to_der().ok().is_some_and(|x| b.to_der().ok().is_some_and(|y| x == y))
}

fn parse_all(keys: &[PublicKey]) -> Vec<Certificate> {
    keys.iter().filter(|k| k.scheme == Scheme::Smime).filter_map(|k| Certificate::from_der(&k.data).ok()).collect()
}

/// Who vouches for the signer's certificate `leaf`: a root among `anchors` (the certificates
/// the owner trusts), through certificates the message carried or that are kept here (`pool`),
/// at `now` (seconds since 1970).
pub fn verify(leaf: &PublicKey, pool: &[PublicKey], anchors: &[PublicKey], now: i64) -> Trust {
    let Ok(leaf) = Certificate::from_der(&leaf.data) else { return Trust::NotVerified("the certificate can't be read".into()) };
    verify_cert(&leaf, &parse_all(pool), &parse_all(anchors), now)
}

pub(crate) fn verify_cert(leaf: &Certificate, pool: &[Certificate], anchors: &[Certificate], now: i64) -> Trust {
    if !for_email(leaf) {
        return Trust::NotVerified("the certificate isn't issued for e-mail".into());
    }
    let mut current = leaf.clone();
    for _ in 0..MAX_DEPTH {
        if !in_force(&current, now) {
            let who = common_name(&current.tbs_certificate.subject).unwrap_or_default();
            return Trust::NotVerified(format!("the certificate of {who} has expired or isn't in force yet"));
        }
        let current_der = current.to_der().ok();
        if let Some(anchor) = anchors.iter().find(|a| a.to_der().ok() == current_der) {
            return Trust::Verified(common_name(&anchor.tbs_certificate.subject).unwrap_or_default());
        }
        let issuer_name = &current.tbs_certificate.issuer;
        if let Some(anchor) = anchors.iter().find(|a| same_name(&a.tbs_certificate.subject, issuer_name) && signed_by(&current, a)) {
            if !in_force(anchor, now) {
                return Trust::NotVerified("the root certificate has expired".into());
            }
            return Trust::Verified(common_name(&anchor.tbs_certificate.subject).unwrap_or_default());
        }
        let Some(next) = pool
            .iter()
            .find(|c| same_name(&c.tbs_certificate.subject, issuer_name) && c.to_der().ok() != current_der && is_ca(c) && signed_by(&current, c))
        else {
            let who = common_name(issuer_name).unwrap_or_else(|| "its issuer".into());
            return Trust::NotVerified(format!("nobody you trust vouches for it (issued by {who})"));
        };
        current = next.clone();
    }
    Trust::NotVerified("the chain of certificates is too long".into())
}
