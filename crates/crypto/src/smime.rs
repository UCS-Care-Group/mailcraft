//! S/MIME (RFC 8551): CMS `SignedData` and `EnvelopedData` (RFC 5652).
//!
//! * [`verify_detached`]: the signature of a `multipart/signed` (`application/pkcs7-signature`).
//! * [`open`]: an `application/pkcs7-mime` part, enveloped (decrypted with the owner's key) or
//!   signed with the content inside (checked and unwrapped).
//! * [`sign_detached`]: a detached signature with the owner's certificate (RSA or P-256, SHA-256).
//! * [`encrypt`]: `EnvelopedData` for certificates (RSA key transport, AES-256-CBC).

use cms::cert::{CertificateChoices, IssuerAndSerialNumber};
use cms::content_info::ContentInfo;
use cms::enveloped_data::{EnvelopedData, RecipientIdentifier, RecipientInfo};
use cms::signed_data::{EncapsulatedContentInfo, SignedData, SignerIdentifier};
use der::asn1::{ObjectIdentifier, OctetString};
use der::{Decode, Encode};
use spki::AlgorithmIdentifierOwned;
use x509_cert::Certificate;
use zeroize::Zeroizing;

use crate::chain::{self, Hash};
use crate::keys::certificate;
use crate::{CryptoError, MAX_INPUT, PublicKey, Result, Scheme, SecretKey, SignatureCheck, Trust, Validity};

const ID_DATA: ObjectIdentifier = const_oid::db::rfc5911::ID_DATA;
const ID_SIGNED_DATA: ObjectIdentifier = const_oid::db::rfc5911::ID_SIGNED_DATA;
const ID_ENVELOPED_DATA: ObjectIdentifier = const_oid::db::rfc5911::ID_ENVELOPED_DATA;
const ID_MESSAGE_DIGEST: ObjectIdentifier = const_oid::db::rfc5911::ID_MESSAGE_DIGEST;
const ID_CONTENT_TYPE: ObjectIdentifier = const_oid::db::rfc5911::ID_CONTENT_TYPE;
const ID_SIGNING_TIME: ObjectIdentifier = const_oid::db::rfc5911::ID_SIGNING_TIME;
const ID_SKI: ObjectIdentifier = const_oid::db::rfc5280::ID_CE_SUBJECT_KEY_IDENTIFIER;
const RSAES_OAEP: ObjectIdentifier = const_oid::db::rfc5912::ID_RSAES_OAEP;
const AES128_CBC: ObjectIdentifier = const_oid::db::rfc5911::ID_AES_128_CBC;
const AES192_CBC: ObjectIdentifier = const_oid::db::rfc5911::ID_AES_192_CBC;
const AES256_CBC: ObjectIdentifier = const_oid::db::rfc5911::ID_AES_256_CBC;
const DES_EDE3_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.3.7");

/// Most signers looked at in one signature.
const MAX_SIGNERS: usize = 16;
/// Most certificates taken from one message.
const MAX_CARRIED: usize = 32;

/// What kind of CMS an `application/pkcs7-mime` part holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Signed,
    Enveloped,
}

/// What [`open`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    /// The MIME entity inside.
    pub content: Vec<u8>,
    /// It was encrypted (and is now decrypted).
    pub encrypted: bool,
    /// It was signed with the content inside: the check of that signature.
    pub signature: Option<SignatureCheck>,
}

/// The owner's certificate and the private key that goes with it.
#[derive(Clone, Debug)]
pub struct Identity<'a> {
    pub certificate: &'a PublicKey,
    pub secret: &'a SecretKey,
}

fn content_info(bytes: &[u8]) -> Result<ContentInfo> {
    if bytes.len() > MAX_INPUT {
        return Err(CryptoError::Malformed("it is too large".into()));
    }
    let der = crate::ber::to_der(bytes)?;
    ContentInfo::from_der(&der).map_err(|_| CryptoError::Malformed("the S/MIME data can't be read".into()))
}

/// Whether an `application/pkcs7-mime` part is signed or enveloped.
pub fn kind(bytes: &[u8]) -> Option<Kind> {
    let info = content_info(bytes).ok()?;
    match info.content_type {
        t if t == ID_SIGNED_DATA => Some(Kind::Signed),
        t if t == ID_ENVELOPED_DATA => Some(Kind::Enveloped),
        _ => None,
    }
}

/// Check a detached signature (`application/pkcs7-signature`) over `content` (the signed MIME
/// entity, canonical CRLF). The signer's certificate is looked for in the signature and among
/// `known`; it is checked up to `anchors` at `now`.
pub fn verify_detached(content: &[u8], signature: &[u8], known: &[PublicKey], anchors: &[PublicKey], now: i64) -> SignatureCheck {
    let signed = match content_info(signature).and_then(|i| signed_data(&i)) {
        Ok(s) => s,
        Err(e) => return SignatureCheck::failed(Scheme::Smime, Validity::Invalid(e.to_string())),
    };
    verify_signed(&signed, content, known, anchors, now)
}

fn signed_data(info: &ContentInfo) -> Result<SignedData> {
    if info.content_type != ID_SIGNED_DATA {
        return Err(CryptoError::Malformed("it isn't a signature".into()));
    }
    info.content.decode_as().map_err(|_| CryptoError::Malformed("the signature can't be read".into()))
}

fn carried(signed: &SignedData) -> Vec<Certificate> {
    signed
        .certificates
        .iter()
        .flat_map(|set| set.0.iter())
        .filter_map(|c| match c {
            CertificateChoices::Certificate(cert) => Some(cert.clone()),
            _ => None,
        })
        .take(MAX_CARRIED)
        .collect()
}

fn subject_key_id(cert: &Certificate) -> Option<Vec<u8>> {
    let ext = cert.tbs_certificate.extensions.iter().flatten().find(|e| e.extn_id == ID_SKI)?;
    OctetString::from_der(ext.extn_value.as_bytes()).ok().map(|o| o.as_bytes().to_vec())
}

fn matches_issuer_serial(cert: &Certificate, id: &IssuerAndSerialNumber) -> bool {
    cert.tbs_certificate.serial_number == id.serial_number && cert.tbs_certificate.issuer.to_der().ok() == id.issuer.to_der().ok()
}

fn matches_signer(cert: &Certificate, sid: &SignerIdentifier) -> bool {
    match sid {
        SignerIdentifier::IssuerAndSerialNumber(id) => matches_issuer_serial(cert, id),
        SignerIdentifier::SubjectKeyIdentifier(ski) => subject_key_id(cert).is_some_and(|k| k == ski.0.as_bytes()),
    }
}

fn verify_signed(signed: &SignedData, content: &[u8], known: &[PublicKey], anchors: &[PublicKey], now: i64) -> SignatureCheck {
    let carried_certs = carried(signed);
    let carried_keys: Vec<PublicKey> = carried_certs.iter().filter_map(|c| certificate(c).ok()).collect();
    let known_certs: Vec<Certificate> =
        known.iter().filter(|k| k.scheme == Scheme::Smime).filter_map(|k| Certificate::from_der(&k.data).ok()).collect();
    let mut check = SignatureCheck::failed(Scheme::Smime, Validity::Invalid("the signature names no signer".into()));
    check.carried = carried_keys.clone();
    for info in signed.signer_infos.0.iter().take(MAX_SIGNERS) {
        let Some(cert) = carried_certs.iter().chain(known_certs.iter()).find(|c| matches_signer(c, &info.sid)) else {
            check.validity = Validity::UnknownSigner;
            continue;
        };
        let signer = certificate(cert).ok();
        let (validity, signed_at) = check_signer_info(info, cert, content, signed.encap_content_info.econtent_type);
        check.signer = signer;
        check.signed_at = signed_at;
        check.validity = validity;
        if check.validity == Validity::Valid {
            let pool: Vec<Certificate> = carried_certs.iter().chain(known_certs.iter()).cloned().collect();
            let anchor_certs: Vec<Certificate> =
                anchors.iter().filter(|k| k.scheme == Scheme::Smime).filter_map(|k| Certificate::from_der(&k.data).ok()).collect();
            check.trust = chain::verify_cert(cert, &pool, &anchor_certs, now);
            break;
        }
    }
    check
}

fn check_signer_info(
    info: &cms::signed_data::SignerInfo,
    cert: &Certificate,
    content: &[u8],
    econtent_type: ObjectIdentifier,
) -> (Validity, Option<i64>) {
    let hash = match Hash::of(info.digest_alg.oid) {
        Ok(h) => h,
        Err(e) => return (Validity::Unsupported(e), None),
    };
    let digest = hash.digest(content);
    let mut signed_at = None;
    let message: Vec<u8> = match &info.signed_attrs {
        Some(attrs) => {
            let mut digest_ok = false;
            for attr in attrs.iter() {
                let Some(value) = attr.values.get(0) else { continue };
                if attr.oid == ID_MESSAGE_DIGEST {
                    digest_ok = value.decode_as::<OctetString>().is_ok_and(|d| d.as_bytes() == digest.as_slice());
                } else if attr.oid == ID_CONTENT_TYPE {
                    if value.decode_as::<ObjectIdentifier>().ok() != Some(econtent_type) {
                        return (Validity::Invalid("the signature is for another kind of content".into()), None);
                    }
                } else if attr.oid == ID_SIGNING_TIME {
                    signed_at =
                        value.to_der().ok().and_then(|d| x509_cert::time::Time::from_der(&d).ok()).map(|t| t.to_unix_duration().as_secs() as i64);
                }
            }
            if !digest_ok {
                return (Validity::Invalid("the message was changed after it was signed".into()), signed_at);
            }
            match attrs.to_der() {
                Ok(d) => d,
                Err(_) => return (Validity::Invalid("the signature can't be read".into()), None),
            }
        }
        None => content.to_vec(),
    };
    match chain::verify_signature(
        &cert.tbs_certificate.subject_public_key_info,
        &info.signature_algorithm,
        Some(info.digest_alg.oid),
        &message,
        info.signature.as_bytes(),
    ) {
        Ok(()) => (Validity::Valid, signed_at),
        Err(e) if e.contains("SHA-1") || e.contains("doesn't check") || e.contains("doesn't know") => (Validity::Unsupported(e), signed_at),
        Err(e) => (Validity::Invalid(e), signed_at),
    }
}

/// Open an `application/pkcs7-mime` part: decrypt enveloped data with one of the owner's
/// `identities`, or check and unwrap signed data with the content inside.
pub fn open(bytes: &[u8], identities: &[Identity<'_>], known: &[PublicKey], anchors: &[PublicKey], now: i64) -> Result<Opened> {
    let info = content_info(bytes)?;
    if info.content_type == ID_SIGNED_DATA {
        let signed = signed_data(&info)?;
        let content = signed
            .encap_content_info
            .econtent
            .as_ref()
            .ok_or_else(|| CryptoError::Malformed("the signed message has no content".into()))?
            .decode_as::<OctetString>()
            .map_err(|_| CryptoError::Malformed("the signed content can't be read".into()))?
            .as_bytes()
            .to_vec();
        let check = verify_signed(&signed, &content, known, anchors, now);
        return Ok(Opened { content, encrypted: false, signature: Some(check) });
    }
    if info.content_type != ID_ENVELOPED_DATA {
        return Err(CryptoError::Unsupported("this kind of S/MIME message".into()));
    }
    let env: EnvelopedData = info.content.decode_as().map_err(|_| CryptoError::Malformed("the encrypted message can't be read".into()))?;
    let cek = content_key(&env, identities)?;
    let content = decrypt_content(&env, &cek)?;
    Ok(Opened { content, encrypted: true, signature: None })
}

fn content_key(env: &EnvelopedData, identities: &[Identity<'_>]) -> Result<Zeroizing<Vec<u8>>> {
    use pkcs8::DecodePrivateKey as _;
    let mut found_recipient = false;
    for ri in env.recip_infos.0.iter().take(256) {
        let RecipientInfo::Ktri(ktri) = ri else { continue };
        for id in identities {
            let Ok(cert) = Certificate::from_der(&id.certificate.data) else { continue };
            let ours = match &ktri.rid {
                RecipientIdentifier::IssuerAndSerialNumber(ias) => matches_issuer_serial(&cert, ias),
                RecipientIdentifier::SubjectKeyIdentifier(ski) => subject_key_id(&cert).is_some_and(|k| k == ski.0.as_bytes()),
            };
            if !ours {
                continue;
            }
            found_recipient = true;
            let key = rsa::RsaPrivateKey::from_pkcs8_der(&id.secret.data)
                .map_err(|_| CryptoError::Unsupported("only RSA keys decrypt S/MIME here".into()))?;
            let mut rng = crate::rng();
            let enc = ktri.enc_key.as_bytes();
            let plain = if ktri.key_enc_alg.oid == RSAES_OAEP {
                key.decrypt_blinded(&mut rng, rsa::Oaep::new::<sha2::Sha256>(), enc)
                    .or_else(|_| key.decrypt_blinded(&mut rng, rsa::Oaep::new::<sha1::Sha1>(), enc))
            } else {
                key.decrypt_blinded(&mut rng, rsa::Pkcs1v15Encrypt, enc)
            };
            if let Ok(plain) = plain {
                return Ok(Zeroizing::new(plain));
            }
        }
    }
    if found_recipient {
        Err(CryptoError::Failed("the message couldn't be decrypted with your key".into()))
    } else {
        Err(CryptoError::NoKey("the message was encrypted for keys that aren't here".into()))
    }
}

fn decrypt_content(env: &EnvelopedData, cek: &[u8]) -> Result<Vec<u8>> {
    use cipher::{BlockDecryptMut as _, KeyIvInit as _, block_padding::Pkcs7};
    let eci = &env.encrypted_content;
    let data = eci.encrypted_content.as_ref().ok_or_else(|| CryptoError::Malformed("the encrypted message has no content".into()))?;
    let iv = eci
        .content_enc_alg
        .parameters
        .as_ref()
        .and_then(|p| p.decode_as::<OctetString>().ok())
        .ok_or_else(|| CryptoError::Malformed("the encryption has no starting value".into()))?;
    let (data, iv) = (data.as_bytes(), iv.as_bytes());
    let failed = || CryptoError::Failed("the message couldn't be decrypted".into());
    let oid = eci.content_enc_alg.oid;
    let plain = if oid == AES256_CBC {
        cbc::Decryptor::<aes::Aes256>::new_from_slices(cek, iv).map_err(|_| failed())?.decrypt_padded_vec_mut::<Pkcs7>(data).map_err(|_| failed())?
    } else if oid == AES192_CBC {
        cbc::Decryptor::<aes::Aes192>::new_from_slices(cek, iv).map_err(|_| failed())?.decrypt_padded_vec_mut::<Pkcs7>(data).map_err(|_| failed())?
    } else if oid == AES128_CBC {
        cbc::Decryptor::<aes::Aes128>::new_from_slices(cek, iv).map_err(|_| failed())?.decrypt_padded_vec_mut::<Pkcs7>(data).map_err(|_| failed())?
    } else if oid == DES_EDE3_CBC {
        cbc::Decryptor::<des::TdesEde3>::new_from_slices(cek, iv)
            .map_err(|_| failed())?
            .decrypt_padded_vec_mut::<Pkcs7>(data)
            .map_err(|_| failed())?
    } else {
        return Err(CryptoError::Unsupported("the message is encrypted with a cipher this program doesn't read".into()));
    };
    Ok(plain)
}

fn issuer_and_serial(cert: &Certificate) -> IssuerAndSerialNumber {
    IssuerAndSerialNumber { issuer: cert.tbs_certificate.issuer.clone(), serial_number: cert.tbs_certificate.serial_number.clone() }
}

/// A detached signature over `content` (the MIME entity, canonical CRLF) with the owner's
/// certificate and key, at `now` (seconds since 1970), SHA-256. `chain` adds the certificates
/// above the owner's, so that a reader can follow it.
pub fn sign_detached(content: &[u8], identity: &Identity<'_>, chain: &[PublicKey], now: i64) -> Result<Vec<u8>> {
    use pkcs8::DecodePrivateKey as _;
    let cert = Certificate::from_der(&identity.certificate.data).map_err(|_| CryptoError::Malformed("your certificate can't be read".into()))?;
    let digest = Hash::Sha256.digest(content);
    let eci = EncapsulatedContentInfo { econtent_type: ID_DATA, econtent: None };
    let digest_alg = AlgorithmIdentifierOwned { oid: chain::SHA256, parameters: None };
    let time = der::DateTime::from_unix_duration(std::time::Duration::from_secs(u64::try_from(now).unwrap_or(0)))
        .map_err(|_| CryptoError::Failed("the clock is wrong".into()))?;
    let time =
        x509_cert::time::Time::UtcTime(der::asn1::UtcTime::from_date_time(time).map_err(|_| CryptoError::Failed("the clock is wrong".into()))?);
    let signing_time = x509_cert::attr::Attribute {
        oid: ID_SIGNING_TIME,
        values: der::asn1::SetOfVec::try_from(vec![der::Any::encode_from(&time).map_err(failed)?]).map_err(failed)?,
    };
    let mut builder = cms::builder::SignedDataBuilder::new(&eci);
    builder.add_digest_algorithm(digest_alg.clone()).map_err(failed)?;
    builder.add_certificate(CertificateChoices::Certificate(cert.clone())).map_err(failed)?;
    for extra in chain.iter().filter(|c| c.scheme == Scheme::Smime && c.fingerprint != identity.certificate.fingerprint).take(8) {
        if let Ok(c) = Certificate::from_der(&extra.data) {
            builder.add_certificate(CertificateChoices::Certificate(c)).map_err(failed)?;
        }
    }
    let sid = SignerIdentifier::IssuerAndSerialNumber(issuer_and_serial(&cert));
    let built = if let Ok(key) = rsa::RsaPrivateKey::from_pkcs8_der(&identity.secret.data) {
        let signer = rsa::pkcs1v15::SigningKey::<sha2::Sha256>::new(key);
        let mut info = cms::builder::SignerInfoBuilder::new(&signer, sid, digest_alg, &eci, Some(&digest)).map_err(failed)?;
        info.add_signed_attribute(signing_time).map_err(failed)?;
        builder.add_signer_info::<_, rsa::pkcs1v15::Signature>(info).map_err(failed)?.build().map_err(failed)?
    } else if let Ok(key) = p256::ecdsa::SigningKey::from_pkcs8_der(&identity.secret.data) {
        let mut info = cms::builder::SignerInfoBuilder::new(&key, sid, digest_alg, &eci, Some(&digest)).map_err(failed)?;
        info.add_signed_attribute(signing_time).map_err(failed)?;
        builder.add_signer_info::<_, p256::ecdsa::DerSignature>(info).map_err(failed)?.build().map_err(failed)?
    } else {
        return Err(CryptoError::Unsupported("your key is of a kind this program doesn't sign with".into()));
    };
    built.to_der().map_err(failed)
}

/// `EnvelopedData` for `recipients` (each an RSA certificate), as DER: the content encrypted
/// with AES-256-CBC under a fresh key, that key encrypted to each of them.
pub fn encrypt(content: &[u8], recipients: &[PublicKey]) -> Result<Vec<u8>> {
    use pkcs8::DecodePublicKey as _;
    if recipients.is_empty() {
        return Err(CryptoError::NoKey("there is nobody to encrypt to".into()));
    }
    let mut rngs: Vec<rand::rngs::OsRng> = vec![crate::rng(); recipients.len()];
    let mut builder =
        cms::builder::EnvelopedDataBuilder::new(None, content, cms::builder::ContentEncryptionAlgorithm::Aes256Cbc, None).map_err(failed)?;
    for (recipient, rng) in recipients.iter().zip(rngs.iter_mut()) {
        let cert = Certificate::from_der(&recipient.data)
            .map_err(|_| CryptoError::Malformed(format!("the certificate of {} can't be read", recipient.name)))?;
        let spki = cert.tbs_certificate.subject_public_key_info.to_der().map_err(failed)?;
        let key = rsa::RsaPublicKey::from_public_key_der(&spki)
            .map_err(|_| CryptoError::Unsupported(format!("the certificate of {} isn't an RSA certificate", recipient.name)))?;
        let rid = RecipientIdentifier::IssuerAndSerialNumber(issuer_and_serial(&cert));
        let ri = cms::builder::KeyTransRecipientInfoBuilder::new(rid, cms::builder::KeyEncryptionInfo::Rsa(key), rng).map_err(failed)?;
        builder.add_recipient_info(ri).map_err(failed)?;
    }
    let env = builder.build_with_rng(&mut crate::rng()).map_err(failed)?;
    let info = ContentInfo { content_type: ID_ENVELOPED_DATA, content: der::Any::encode_from(&env).map_err(failed)? };
    info.to_der().map_err(failed)
}

fn failed(e: impl std::fmt::Display) -> CryptoError {
    CryptoError::Failed(format!("S/MIME: {e}"))
}

/// Whether a signature check says the signer is vouched for.
pub fn is_verified(check: &SignatureCheck) -> bool {
    matches!(check.trust, Trust::Verified(_))
}
