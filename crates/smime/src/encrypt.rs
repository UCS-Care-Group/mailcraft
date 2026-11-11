// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encrypting to S/MIME recipients (#30 part 7): the CMS EnvelopedData of RFC 5652 §6, as
//! RFC 8551 §3.3 has it, for a MIME entity.
//!
//! Only Public keys are used here: a random content-encryption key encrypts the entity with
//! AES-256-CBC (RFC 3565; what mail programs read, Outlook among them), and that key is
//! encrypted to each recipient's certificate with RSA PKCS #1 v1.5 (`rsaEncryption`, RFC 3370
//! §4.2.1: the key transport every mail program reads; encrypting needs no private key, and the
//! attacks on the padding are the recipient's side of it). Each recipient is named by the issuer
//! and serial number of their certificate. A certificate with an elliptic-curve key isn't
//! encrypted to yet ([`EncryptError::Certificate`]).
//!
//! Nothing here asks the system for anything: the random numbers (the content key, the
//! initialisation vector and the padding) come from the `random` function the host hands in, which
//! fills a buffer from the operating system's source, so that this crate stays free of I/O and
//! builds for the browser. A source that fails ends the encryption; nothing is made with less.

use std::fmt;

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeEncrypt, KeyIvInit};
use cms::cert::IssuerAndSerialNumber;
use cms::content_info::{CmsVersion, ContentInfo};
use cms::enveloped_data::{EncryptedContentInfo, EnvelopedData, KeyTransRecipientInfo, RecipientIdentifier, RecipientInfo, RecipientInfos};
use const_oid::ObjectIdentifier;
use der::asn1::OctetString;
use der::{Any, Encode};
use spki::AlgorithmIdentifierOwned;
use zeroize::Zeroizing;

use crate::algorithms::{self, Fault};
use crate::cert::{Cert, MAX_CERT_BYTES};

/// The largest content encrypted: a message bigger than any server takes.
pub const MAX_ENCRYPTED_BYTES: usize = 150 * 1024 * 1024;
/// The most recipients one message is encrypted to.
pub const MAX_RECIPIENTS: usize = 200;

const ENVELOPED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.3");
const DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");
const AES256_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.42");

/// What is to be encrypted, and to whom.
#[derive(Clone, Copy, Debug)]
pub struct EncryptRequest<'a> {
    /// The MIME entity exactly as it is encrypted: its content fields, an empty line and its
    /// body (RFC 8551 §3.1).
    pub content: &'a [u8],
    /// The recipients' certificates, in DER (at most [`MAX_RECIPIENTS`]): the owner's own among
    /// them, so that what is sent can be read from Sent Items.
    pub recipients: &'a [Vec<u8>],
    /// When it is encrypted, seconds since 1970: a certificate not valid then isn't encrypted to.
    pub now: i64,
}

/// Why nothing was encrypted. Nothing in it is of the content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncryptError {
    /// There is nobody to encrypt to.
    NoRecipients,
    /// A certificate can't be encrypted to: what is wrong with it.
    Certificate(&'static str),
    /// The content, a certificate or the list of recipients is larger than is taken.
    TooLarge,
    /// The random numbers couldn't be had.
    Random,
    /// The message couldn't be written out.
    Encoding,
}

impl fmt::Display for EncryptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncryptError::NoRecipients => f.write_str("there is nobody to encrypt to"),
            EncryptError::Certificate(why) => write!(f, "a certificate can't be encrypted to: {why}"),
            EncryptError::TooLarge => f.write_str("the message or a certificate is too large to encrypt"),
            EncryptError::Random => f.write_str("the system's random numbers couldn't be had"),
            EncryptError::Encoding => f.write_str("the encrypted message couldn't be written"),
        }
    }
}

impl std::error::Error for EncryptError {}

/// Whether mail may be encrypted to the certificate `der` at `now`: it is for mail, valid, no
/// authority's, its key may encipher, and it is an RSA key of 2048 to 8192 bits. What is wrong
/// when it can't, in words.
pub fn can_encrypt_to(der: &[u8], now: i64) -> Result<(), &'static str> {
    certificate(der, now).map(|_| ())
}

/// The certificate `der`, when mail may be encrypted to it.
fn certificate(der: &[u8], now: i64) -> Result<Cert, &'static str> {
    if der.len() > MAX_CERT_BYTES {
        return Err("it is too large");
    }
    let cert = Cert::from_der(der).ok_or("it can't be read")?;
    if !cert.for_encryption() {
        return Err("it isn't for encrypting mail");
    }
    if !cert.valid_at(now) {
        return Err("it isn't valid now");
    }
    match algorithms::signing_kind(cert.public_key()) {
        Ok(algorithms::SigningKind::Rsa) => Ok(cert),
        Err(Fault::Weak) => Err("its RSA key is shorter than 2048 bits"),
        Err(Fault::Malformed) => Err("its key can't be read"),
        _ => Err("its key isn't an RSA key, the only kind encrypted to here"),
    }
}

/// The random numbers the host gave, as the RSA library asks for them. A host that fails once
/// fails every time after: nothing is ever made from less than it was asked for.
struct Random<'a> {
    fill: &'a mut dyn FnMut(&mut [u8]) -> bool,
    failed: bool,
}

#[derive(Debug)]
struct NoRandom;

impl fmt::Display for NoRandom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no random numbers")
    }
}

impl std::error::Error for NoRandom {}

impl Random<'_> {
    fn bytes(&mut self, dst: &mut [u8]) -> Result<(), NoRandom> {
        if self.failed || !(self.fill)(dst) {
            self.failed = true;
            dst.fill(0);
            return Err(NoRandom);
        }
        Ok(())
    }
}

impl rsa::rand_core::TryRng for Random<'_> {
    type Error = NoRandom;
    fn try_next_u32(&mut self) -> Result<u32, NoRandom> {
        let mut b = [0u8; 4];
        self.bytes(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn try_next_u64(&mut self) -> Result<u64, NoRandom> {
        let mut b = [0u8; 8];
        self.bytes(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), NoRandom> {
        self.bytes(dst)
    }
}

impl rsa::rand_core::TryCryptoRng for Random<'_> {}

/// The DER of a ContentInfo holding an EnvelopedData of `request.content` for every recipient,
/// for the body of an `application/pkcs7-mime; smime-type=enveloped-data` part. `random` fills
/// its buffer with random bytes from the operating system and says whether it could.
pub fn encrypt(request: &EncryptRequest<'_>, random: &mut dyn FnMut(&mut [u8]) -> bool) -> Result<Vec<u8>, EncryptError> {
    if request.content.is_empty() {
        return Err(EncryptError::Encoding);
    }
    if request.content.len() > MAX_ENCRYPTED_BYTES || request.recipients.len() > MAX_RECIPIENTS {
        return Err(EncryptError::TooLarge);
    }
    if request.recipients.is_empty() {
        return Err(EncryptError::NoRecipients);
    }
    // Each recipient once, whatever way the list was made.
    let mut seen: Vec<&[u8]> = Vec::new();
    let mut certs = Vec::new();
    for der in request.recipients {
        if seen.contains(&der.as_slice()) {
            continue;
        }
        seen.push(der);
        certs.push(certificate(der, request.now).map_err(EncryptError::Certificate)?);
    }
    let mut random = Random { fill: random, failed: false };
    let mut key = Zeroizing::new([0u8; 32]);
    let mut iv = [0u8; 16];
    random.bytes(&mut *key).map_err(|_| EncryptError::Random)?;
    random.bytes(&mut iv).map_err(|_| EncryptError::Random)?;

    let encryptor = cbc::Encryptor::<aes::Aes256>::new_from_slices(&*key, &iv).map_err(|_| EncryptError::Encoding)?;
    // (Room for the padding: up to a whole block.)
    let mut ciphertext = vec![0u8; request.content.len().saturating_add(16)];
    ciphertext.get_mut(..request.content.len()).ok_or(EncryptError::Encoding)?.copy_from_slice(request.content);
    let len = encryptor.encrypt_padded::<Pkcs7>(&mut ciphertext, request.content.len()).map_err(|_| EncryptError::Encoding)?.len();
    ciphertext.truncate(len);

    let mut infos = Vec::new();
    for cert in &certs {
        let public = rsa_public(cert)?;
        let wrapped = public
            .encrypt(&mut random, rsa::Pkcs1v15Encrypt, &*key)
            .map_err(|_| if random.failed { EncryptError::Random } else { EncryptError::Certificate("its key can't encrypt") })?;
        let tbs = cert.certificate().tbs_certificate();
        infos.push(RecipientInfo::Ktri(KeyTransRecipientInfo {
            version: CmsVersion::V0,
            rid: RecipientIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
                issuer: tbs.issuer().clone(),
                serial_number: tbs.serial_number().clone(),
            }),
            key_enc_alg: AlgorithmIdentifierOwned { oid: algorithms::RSA, parameters: Some(Any::null()) },
            enc_key: OctetString::new(wrapped).map_err(|_| EncryptError::Encoding)?,
        }));
    }
    let iv_parameter = Any::encode_from(&OctetString::new(iv.to_vec()).map_err(|_| EncryptError::Encoding)?).map_err(|_| EncryptError::Encoding)?;
    let data = EnvelopedData {
        version: CmsVersion::V0,
        originator_info: None,
        recip_infos: RecipientInfos::try_from(infos).map_err(|_| EncryptError::Encoding)?,
        encrypted_content: EncryptedContentInfo {
            content_type: DATA,
            content_enc_alg: AlgorithmIdentifierOwned { oid: AES256_CBC, parameters: Some(iv_parameter) },
            encrypted_content: Some(OctetString::new(ciphertext).map_err(|_| EncryptError::Encoding)?),
        },
        unprotected_attrs: None,
    };
    let content = Any::encode_from(&data).map_err(|_| EncryptError::Encoding)?;
    ContentInfo { content_type: ENVELOPED_DATA, content }.to_der().map_err(|_| EncryptError::Encoding)
}

/// The RSA public key of a certificate [`certificate`] took.
fn rsa_public(cert: &Cert) -> Result<rsa::PublicKey, EncryptError> {
    algorithms::rsa_key(cert.public_key()).map_err(|_| EncryptError::Certificate("its key can't be read"))
}
