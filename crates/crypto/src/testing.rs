//! Keys and certificates made on the spot, for tests (here and in the crates above, with the
//! `testing` feature). Every identity is synthetic (`example.org`); nothing here is a real key.

use std::str::FromStr as _;
use std::time::Duration;

use der::Encode as _;
use pgp::composed::{EncryptionCaps, KeyType, SecretKeyParamsBuilder, SignedSecretKey, SubkeyParamsBuilder};
use pgp::ser::Serialize as _;
use pkcs8::EncodePrivateKey as _;
use x509_cert::builder::{Builder as _, CertificateBuilder, Profile};
use zeroize::Zeroizing;

use crate::keys::{certificate, openpgp_public};
use crate::{CryptoError, PublicKey, Result, Scheme, SecretKey};

/// A certificate and its private key.
#[derive(Clone, Debug)]
pub struct Identity {
    pub certificate: PublicKey,
    pub secret: SecretKey,
}

impl Identity {
    pub fn as_smime(&self) -> crate::smime::Identity<'_> {
        crate::smime::Identity { certificate: &self.certificate, secret: &self.secret }
    }
}

fn err(e: impl std::fmt::Display) -> CryptoError {
    CryptoError::Failed(e.to_string())
}

fn rsa_key() -> Result<rsa::RsaPrivateKey> {
    rsa::RsaPrivateKey::new(&mut crate::rng(), 2048).map_err(err)
}

fn make(
    profile: Profile,
    name: &str,
    email: Option<&str>,
    signer_key: Option<&rsa::RsaPrivateKey>,
    serial: u32,
    validity: x509_cert::time::Validity,
) -> Result<Identity> {
    use x509_cert::ext::pkix::name::GeneralName;
    let key = rsa_key()?;
    let spki = spki::SubjectPublicKeyInfoOwned::from_key(key.to_public_key()).map_err(err)?;
    let subject = x509_cert::name::Name::from_str(&format!("CN={name},O=Example")).map_err(err)?;
    let signing = rsa::pkcs1v15::SigningKey::<sha2::Sha256>::new(signer_key.cloned().unwrap_or_else(|| key.clone()));
    let mut builder =
        CertificateBuilder::new(profile, x509_cert::serial_number::SerialNumber::from(serial), validity, subject, spki, &signing).map_err(err)?;
    if let Some(email) = email {
        let san = x509_cert::ext::pkix::SubjectAltName(vec![GeneralName::Rfc822Name(der::asn1::Ia5String::new(email).map_err(err)?)]);
        builder.add_extension(&san).map_err(err)?;
        let eku = x509_cert::ext::pkix::ExtendedKeyUsage(vec![crate::keys::EMAIL_PROTECTION]);
        builder.add_extension(&eku).map_err(err)?;
    }
    let cert = builder.build::<rsa::pkcs1v15::Signature>().map_err(err)?;
    let public = certificate(&cert)?;
    let der = key.to_pkcs8_der().map_err(err)?;
    let secret = SecretKey { scheme: Scheme::Smime, fingerprint: public.fingerprint.clone(), data: Zeroizing::new(der.as_bytes().to_vec()) };
    Ok(Identity { certificate: public, secret })
}

/// From a month ago to a year from now (so that a test clock pinned to today is inside it).
fn year() -> Result<x509_cert::time::Validity> {
    use std::time::SystemTime;
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_err(err)?;
    let time = |d: Duration| -> Result<x509_cert::time::Time> {
        Ok(x509_cert::time::Time::UtcTime(der::asn1::UtcTime::from_date_time(der::DateTime::from_unix_duration(d).map_err(err)?).map_err(err)?))
    };
    Ok(x509_cert::time::Validity {
        not_before: time(now.saturating_sub(Duration::from_secs(30 * 86_400)))?,
        not_after: time(now + Duration::from_secs(365 * 86_400))?,
    })
}

/// A root certificate authority.
pub fn smime_root(name: &str) -> Result<Identity> {
    make(Profile::Root, name, None, None, 1, year()?)
}

/// A person's certificate for `email`, issued by `issuer`.
pub fn smime_leaf(issuer: &Identity, name: &str, email: &str) -> Result<Identity> {
    use der::Decode as _;
    use pkcs8::DecodePrivateKey as _;
    let issuer_cert = x509_cert::Certificate::from_der(&issuer.certificate.data).map_err(err)?;
    let issuer_key = rsa::RsaPrivateKey::from_pkcs8_der(&issuer.secret.data).map_err(err)?;
    let profile = Profile::Leaf { issuer: issuer_cert.tbs_certificate.subject.clone(), enable_key_agreement: false, enable_key_encipherment: true };
    make(profile, name, Some(email), Some(&issuer_key), 2 + email.len() as u32, year()?)
}

/// A person's certificate issued by its own key.
fn own_leaf(name: &str) -> Result<Profile> {
    let subject = x509_cert::name::Name::from_str(&format!("CN={name},O=Example")).map_err(err)?;
    Ok(Profile::Leaf { issuer: subject, enable_key_agreement: false, enable_key_encipherment: true })
}

/// A self-signed certificate for `email` (what many people use for S/MIME among themselves).
pub fn smime_self_signed(name: &str, email: &str) -> Result<Identity> {
    make(own_leaf(name)?, name, Some(email), None, 7, year()?)
}

/// A certificate for `email` that ran out a day ago.
pub fn smime_expired(name: &str, email: &str) -> Result<Identity> {
    use std::time::SystemTime;
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_err(err)?;
    let from = der::DateTime::from_unix_duration(now.saturating_sub(Duration::from_secs(30 * 86_400))).map_err(err)?;
    let to = der::DateTime::from_unix_duration(now.saturating_sub(Duration::from_secs(86_400))).map_err(err)?;
    let time = |t| -> Result<x509_cert::time::Time> { Ok(x509_cert::time::Time::UtcTime(der::asn1::UtcTime::from_date_time(t).map_err(err)?)) };
    let validity = x509_cert::time::Validity { not_before: time(from)?, not_after: time(to)? };
    make(own_leaf(name)?, name, Some(email), None, 9, validity)
}

fn openpgp_key(name: &str, email: &str) -> Result<SignedSecretKey> {
    let mut sign = SubkeyParamsBuilder::default();
    sign.key_type(KeyType::Ed25519Legacy).can_sign(true).can_encrypt(EncryptionCaps::None).can_authenticate(false);
    let mut enc = SubkeyParamsBuilder::default();
    enc.key_type(KeyType::ECDH(pgp::crypto::ecc_curve::ECCCurve::Curve25519Legacy))
        .can_sign(false)
        .can_encrypt(EncryptionCaps::All)
        .can_authenticate(false);
    let mut params = SecretKeyParamsBuilder::default();
    params
        .key_type(KeyType::Ed25519Legacy)
        .can_certify(true)
        .can_sign(false)
        .can_encrypt(EncryptionCaps::None)
        .primary_user_id(format!("{name} <{email}>"))
        .subkeys(vec![sign.build().map_err(err)?, enc.build().map_err(err)?]);
    params.build().map_err(err)?.generate(crate::rng()).map_err(err)
}

/// An OpenPGP key for `email` (Ed25519 signing, Curve25519 encryption).
pub fn openpgp(name: &str, email: &str) -> Result<Identity> {
    let key = openpgp_key(name, email)?;
    let public = openpgp_public(&key.to_public_key())?;
    let mut data = Vec::new();
    key.to_writer(&mut data).map_err(err)?;
    Ok(Identity {
        secret: SecretKey { scheme: Scheme::OpenPgp, fingerprint: public.fingerprint.clone(), data: Zeroizing::new(data) },
        certificate: public,
    })
}

/// An armored OpenPGP secret key for `email` under `passphrase`, as a key file.
pub fn openpgp_protected_file(name: &str, email: &str, passphrase: &str) -> Result<Vec<u8>> {
    let mut key = openpgp_key(name, email)?;
    let pw: pgp::types::Password = passphrase.into();
    key.primary_key.set_password(crate::rng(), &pw).map_err(err)?;
    for sub in &mut key.secret_subkeys {
        sub.key.set_password(crate::rng(), &pw).map_err(err)?;
    }
    key.to_armored_bytes(Default::default()).map_err(err)
}

// A small DER writer for the PKCS#12 file below.
fn tlv(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = value.len();
    if len < 0x80 {
        out.push(len as u8);
    } else if len < 0x100 {
        out.extend([0x81, len as u8]);
    } else if len < 0x10000 {
        out.extend([0x82, (len >> 8) as u8, len as u8]);
    } else {
        out.extend([0x83, (len >> 16) as u8, (len >> 8) as u8, len as u8]);
    }
    out.extend_from_slice(value);
    out
}

fn seq(parts: &[Vec<u8>]) -> Vec<u8> {
    tlv(0x30, &parts.concat())
}

fn oid(o: &str) -> Result<Vec<u8>> {
    der::asn1::ObjectIdentifier::new(o).map_err(err)?.to_der().map_err(err)
}

/// A PKCS#12 file holding an identity, as OpenSSL 3 writes one: the key in a shrouded bag
/// (PBES2: PBKDF2-SHA256, AES-256-CBC), the certificate in the clear, an HMAC-SHA256 MAC.
pub fn p12_file(identity: &Identity, passphrase: &str) -> Result<Vec<u8>> {
    use hmac::Mac as _;
    let info = pkcs8::PrivateKeyInfo::try_from(identity.secret.data.as_slice()).map_err(err)?;
    let params = pkcs5::pbes2::Parameters::pbkdf2_sha256_aes256cbc(2048, &[7u8; 16], &[9u8; 16]).map_err(err)?;
    let shrouded = info.encrypt_with_params(params, passphrase.as_bytes()).map_err(err)?;
    let id_data = oid("1.2.840.113549.1.7.1")?;
    let key_bag = seq(&[oid("1.2.840.113549.1.12.10.1.2")?, tlv(0xa0, shrouded.as_bytes())]);
    let cert_bag = seq(&[
        oid("1.2.840.113549.1.12.10.1.3")?,
        tlv(0xa0, &seq(&[oid("1.2.840.113549.1.9.22.1")?, tlv(0xa0, &tlv(0x04, &identity.certificate.data))])),
    ]);
    let safe = |bag: Vec<u8>| seq(&[id_data.clone(), tlv(0xa0, &tlv(0x04, &seq(&[bag])))]);
    let auth_safe = seq(&[safe(cert_bag), safe(key_bag)]);
    let salt = [3u8; 8];
    let iterations = 2048;
    let mac_key = pkcs12::kdf::derive_key_utf8::<sha2::Sha256>(passphrase, &salt, pkcs12::kdf::Pkcs12KeyType::Mac, iterations, 32).map_err(err)?;
    let mut mac = <hmac::Hmac<sha2::Sha256> as hmac::Mac>::new_from_slice(&mac_key).map_err(err)?;
    mac.update(&auth_safe);
    let digest = mac.finalize().into_bytes();
    let mac_data =
        seq(&[seq(&[seq(&[oid("2.16.840.1.101.3.4.2.1")?, vec![0x05, 0x00]]), tlv(0x04, &digest)]), tlv(0x04, &salt), tlv(0x02, &[0x08, 0x00])]);
    Ok(seq(&[tlv(0x02, &[3]), seq(&[id_data, tlv(0xa0, &tlv(0x04, &auth_safe))]), mac_data]))
}
