//! Reading key files: certificates, PKCS#12, PKCS#8 and OpenPGP keys.

use der::{Decode, Encode};
use pgp::composed::{Deserializable, SignedPublicKey, SignedSecretKey};
use pgp::ser::Serialize as _;
use pgp::types::KeyDetails as _;
use sha2::Digest as _;
use x509_cert::Certificate;
use zeroize::Zeroizing;

use crate::{CryptoError, Imported, MAX_INPUT, PublicKey, Result, Scheme, SecretKey, hex, p12, pem};

/// Most keys taken from one file.
const MAX_KEYS: usize = 200;

/// The OID of `emailAddress` in a certificate's subject (PKCS#9).
const EMAIL_ADDRESS: der::asn1::ObjectIdentifier = der::asn1::ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.1");
/// `id-kp-emailProtection` (RFC 5280).
pub(crate) const EMAIL_PROTECTION: der::asn1::ObjectIdentifier = der::asn1::ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.4");
/// `anyExtendedKeyUsage`.
pub(crate) const ANY_EKU: der::asn1::ObjectIdentifier = der::asn1::ObjectIdentifier::new_unwrap("2.5.29.37.0");

/// Read a key file: a certificate (DER or PEM, `.cer`, `.crt`, `.pem`), a bundle of them
/// (`.p7b`), a PKCS#12 file with its passphrase (`.p12`, `.pfx`), a private key (PKCS#8, PKCS#1
/// or SEC 1, in PEM; encrypted PKCS#8 with its passphrase) or OpenPGP keys (`.asc` armored, or
/// binary). A private key found without its certificate keeps an empty fingerprint: the caller
/// matches it to a certificate it has ([`secret_belongs_to`]).
pub fn import(bytes: &[u8], passphrase: Option<&str>) -> Result<Imported> {
    if bytes.len() > MAX_INPUT {
        return Err(CryptoError::Malformed("the file is too large".into()));
    }
    let text = std::str::from_utf8(bytes).ok();
    let mut out = match text {
        Some(t) if t.contains("-----BEGIN PGP ") => openpgp_keys(bytes, passphrase)?,
        Some(t) if t.contains("-----BEGIN ") => pem_keys(t, passphrase)?,
        _ => binary_keys(bytes, passphrase)?,
    };
    out.public.truncate(MAX_KEYS);
    out.secret.truncate(MAX_KEYS);
    // A private key takes the fingerprint of the certificate that goes with it.
    for secret in out.secret.iter_mut().filter(|s| s.fingerprint.is_empty()) {
        if let Some(cert) = out.public.iter().find(|p| secret_belongs_to(secret, p)) {
            secret.fingerprint = cert.fingerprint.clone();
        }
    }
    if out.public.is_empty() && out.secret.is_empty() {
        return Err(CryptoError::Malformed("no certificate or key was found in it".into()));
    }
    Ok(out)
}

fn binary_keys(bytes: &[u8], passphrase: Option<&str>) -> Result<Imported> {
    if let Ok(cert) = Certificate::from_der(bytes) {
        return Ok(Imported { public: vec![certificate(&cert)?], secret: Vec::new() });
    }
    if let Some(certs) = pkcs7_certificates(bytes) {
        return Ok(Imported { public: certs, secret: Vec::new() });
    }
    if p12::looks_like(bytes) {
        return p12::read(bytes, passphrase.unwrap_or(""));
    }
    if bytes.first().is_some_and(|b| b & 0x80 != 0) {
        return openpgp_keys(bytes, passphrase);
    }
    Err(CryptoError::Malformed("this isn't a certificate or a key this program knows".into()))
}

fn pem_keys(text: &str, passphrase: Option<&str>) -> Result<Imported> {
    let mut out = Imported::default();
    let mut needs_passphrase = false;
    for (label, der) in pem::blocks(text) {
        match label.as_str() {
            "CERTIFICATE" | "X509 CERTIFICATE" | "TRUSTED CERTIFICATE" => {
                if let Ok(cert) = Certificate::from_der(&der) {
                    out.public.push(certificate(&cert)?);
                }
            }
            "PKCS7" | "CMS" => out.public.extend(pkcs7_certificates(&der).unwrap_or_default()),
            "PRIVATE KEY" => out.secret.push(smime_secret(&der)?),
            "ENCRYPTED PRIVATE KEY" => match passphrase {
                Some(p) => out.secret.push(smime_secret(&decrypt_pkcs8(&der, p)?)?),
                None => needs_passphrase = true,
            },
            "RSA PRIVATE KEY" => {
                use pkcs8::EncodePrivateKey as _;
                let key = <rsa::RsaPrivateKey as pkcs1::DecodeRsaPrivateKey>::from_pkcs1_der(&der).map_err(|_| bad("the RSA key can't be read"))?;
                let doc = key.to_pkcs8_der().map_err(|_| bad("the RSA key can't be read"))?;
                out.secret.push(smime_secret(doc.as_bytes())?);
            }
            "EC PRIVATE KEY" => {
                use pkcs8::EncodePrivateKey as _;
                let key =
                    p256::SecretKey::from_sec1_der(&der).map_err(|_| CryptoError::Unsupported("only P-256 elliptic-curve keys are read".into()))?;
                let doc = key.to_pkcs8_der().map_err(|_| bad("the key can't be read"))?;
                out.secret.push(smime_secret(doc.as_bytes())?);
            }
            _ => {}
        }
        if out.public.len() + out.secret.len() >= MAX_KEYS {
            break;
        }
    }
    if needs_passphrase && out.secret.is_empty() {
        return Err(CryptoError::Passphrase);
    }
    Ok(out)
}

/// An encrypted PKCS#8 key (PBES2, as OpenSSL writes it) with its passphrase.
pub(crate) fn decrypt_pkcs8(der: &[u8], passphrase: &str) -> Result<Zeroizing<Vec<u8>>> {
    let info = pkcs8::EncryptedPrivateKeyInfo::try_from(der).map_err(|_| bad("the encrypted key can't be read"))?;
    p12::check_pbes2_work(&info.encryption_algorithm)?;
    let doc = info.decrypt(passphrase.as_bytes()).map_err(|_| CryptoError::Passphrase)?;
    Ok(Zeroizing::new(doc.as_bytes().to_vec()))
}

/// A PKCS#8 private key (RSA or P-256) as kept in memory.
pub(crate) fn smime_secret(der: &[u8]) -> Result<SecretKey> {
    let info = pkcs8::PrivateKeyInfo::try_from(der).map_err(|_| bad("the private key can't be read"))?;
    let oid = info.algorithm.oid;
    if oid != const_oid::db::rfc5912::RSA_ENCRYPTION && oid != const_oid::db::rfc5912::ID_EC_PUBLIC_KEY {
        return Err(CryptoError::Unsupported("only RSA and P-256 keys are used for S/MIME".into()));
    }
    Ok(SecretKey { scheme: Scheme::Smime, fingerprint: String::new(), data: Zeroizing::new(der.to_vec()) })
}

/// The public key (SubjectPublicKeyInfo, DER) of an S/MIME private key.
pub(crate) fn secret_spki(secret: &SecretKey) -> Option<Vec<u8>> {
    use pkcs8::{DecodePrivateKey as _, EncodePublicKey as _};
    if let Ok(key) = rsa::RsaPrivateKey::from_pkcs8_der(&secret.data) {
        return key.to_public_key().to_public_key_der().ok().map(|d| d.as_bytes().to_vec());
    }
    let key = p256::SecretKey::from_pkcs8_der(&secret.data).ok()?;
    key.public_key().to_public_key_der().ok().map(|d| d.as_bytes().to_vec())
}

/// Whether a private key is the other half of a certificate or an OpenPGP key.
pub fn secret_belongs_to(secret: &SecretKey, public: &PublicKey) -> bool {
    if secret.scheme != public.scheme {
        return false;
    }
    match secret.scheme {
        Scheme::OpenPgp => !secret.fingerprint.is_empty() && secret.fingerprint == public.fingerprint,
        Scheme::Smime => {
            let Ok(cert) = Certificate::from_der(&public.data) else { return false };
            let Ok(spki) = cert.tbs_certificate.subject_public_key_info.to_der() else { return false };
            secret_spki(secret).is_some_and(|s| s == spki)
        }
    }
}

/// The certificates of a PKCS#7 / CMS `SignedData` that carries certificates only (`.p7b`).
fn pkcs7_certificates(bytes: &[u8]) -> Option<Vec<PublicKey>> {
    let der = crate::ber::to_der(bytes).ok()?;
    let info = cms::content_info::ContentInfo::from_der(&der).ok()?;
    if info.content_type != const_oid::db::rfc5911::ID_SIGNED_DATA {
        return None;
    }
    let signed: cms::signed_data::SignedData = info.content.decode_as().ok()?;
    let certs = signed.certificates?;
    Some(
        certs
            .0
            .iter()
            .filter_map(|c| match c {
                cms::cert::CertificateChoices::Certificate(cert) => certificate(cert).ok(),
                _ => None,
            })
            .take(MAX_KEYS)
            .collect(),
    )
}

/// What a certificate says about itself.
pub(crate) fn certificate(cert: &Certificate) -> Result<PublicKey> {
    let data = cert.to_der().map_err(|_| bad("the certificate can't be read"))?;
    let tbs = &cert.tbs_certificate;
    let mut emails: Vec<String> = Vec::new();
    let mut push = |e: String| {
        let e = e.trim().to_ascii_lowercase();
        if e.contains('@') && e.len() <= 320 && !emails.contains(&e) {
            emails.push(e);
        }
    };
    for rdn in &tbs.subject.0 {
        for atv in rdn.0.iter() {
            if atv.oid == EMAIL_ADDRESS {
                push(any_text(&atv.value));
            }
        }
    }
    let mut is_ca = false;
    let mut usage: Option<x509_cert::ext::pkix::KeyUsage> = None;
    for ext in tbs.extensions.iter().flatten() {
        let value = ext.extn_value.as_bytes();
        if ext.extn_id == const_oid::db::rfc5280::ID_CE_SUBJECT_ALT_NAME {
            if let Ok(san) = x509_cert::ext::pkix::SubjectAltName::from_der(value) {
                for name in san.0 {
                    if let x509_cert::ext::pkix::name::GeneralName::Rfc822Name(e) = name {
                        push(e.to_string());
                    }
                }
            }
        } else if ext.extn_id == const_oid::db::rfc5280::ID_CE_BASIC_CONSTRAINTS {
            is_ca = x509_cert::ext::pkix::BasicConstraints::from_der(value).is_ok_and(|b| b.ca);
        } else if ext.extn_id == const_oid::db::rfc5280::ID_CE_KEY_USAGE {
            usage = x509_cert::ext::pkix::KeyUsage::from_der(value).ok();
        }
    }
    use x509_cert::ext::pkix::KeyUsages;
    let can_sign = usage.is_none_or(|u| u.0.contains(KeyUsages::DigitalSignature) || u.0.contains(KeyUsages::NonRepudiation));
    let rsa = tbs.subject_public_key_info.algorithm.oid == const_oid::db::rfc5912::RSA_ENCRYPTION;
    let can_encrypt = rsa && usage.is_none_or(|u| u.0.contains(KeyUsages::KeyEncipherment));
    let self_signed = tbs.subject.to_der().ok() == tbs.issuer.to_der().ok();
    Ok(PublicKey {
        scheme: Scheme::Smime,
        fingerprint: hex(&sha2::Sha256::digest(&data)),
        name: common_name(&tbs.subject).unwrap_or_else(|| emails.first().cloned().unwrap_or_default()),
        issuer: common_name(&tbs.issuer).unwrap_or_default(),
        emails,
        not_before: Some(tbs.validity.not_before.to_unix_duration().as_secs() as i64),
        not_after: Some(tbs.validity.not_after.to_unix_duration().as_secs() as i64),
        is_ca,
        self_signed,
        can_sign,
        can_encrypt,
        data,
    })
}

/// The common name of a distinguished name, else its organisation.
pub(crate) fn common_name(name: &x509_cert::name::Name) -> Option<String> {
    for oid in [const_oid::db::rfc4519::CN, const_oid::db::rfc4519::O] {
        for rdn in &name.0 {
            for atv in rdn.0.iter() {
                if atv.oid == oid {
                    let text = any_text(&atv.value);
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
        }
    }
    None
}

/// The text of a directory string, whatever string type it is in.
fn any_text(value: &der::Any) -> String {
    use der::{Tag, Tagged as _};
    let bytes = value.value();
    let text = match value.tag() {
        Tag::BmpString => {
            let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
            String::from_utf16_lossy(&units)
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    };
    text.chars().filter(|c| !c.is_control()).take(256).collect()
}

/// OpenPGP keys, armored or binary. A secret key protected by a passphrase is unlocked with it,
/// and kept unlocked in memory (it goes to the system's credential store, if anywhere).
fn openpgp_keys(bytes: &[u8], passphrase: Option<&str>) -> Result<Imported> {
    let mut out = Imported::default();
    let armored = bytes.windows(32).any(|w| w.starts_with(b"-----BEGIN PGP PRIVATE KEY BLOCK"));
    let secret_like = armored || bytes.first().is_some_and(|b| *b == 0x95 || *b == 0xc5 || *b == 0x94);
    if secret_like && let Ok((key, _)) = SignedSecretKey::from_reader_single(bytes) {
        let key = unlock_openpgp(key, passphrase)?;
        let public = key.to_public_key();
        let info = openpgp_public(&public)?;
        let mut data = Vec::new();
        key.to_writer(&mut data).map_err(|_| bad("the key can't be written"))?;
        out.secret.push(SecretKey { scheme: Scheme::OpenPgp, fingerprint: info.fingerprint.clone(), data: Zeroizing::new(data) });
        out.public.push(info);
        return Ok(out);
    }
    let (keys, _) = SignedPublicKey::from_reader_many(bytes).map_err(|_| bad("the OpenPGP key can't be read"))?;
    for key in keys.take(MAX_KEYS) {
        let Ok(key) = key else { continue };
        if key.verify_bindings().is_err() {
            continue;
        }
        out.public.push(openpgp_public(&key)?);
    }
    Ok(out)
}

fn unlock_openpgp(mut key: SignedSecretKey, passphrase: Option<&str>) -> Result<SignedSecretKey> {
    key.verify_bindings().map_err(|_| bad("the key's own signatures don't match it"))?;
    let password: pgp::types::Password = passphrase.unwrap_or("").into();
    if key.primary_key.secret_params().is_encrypted() {
        key.primary_key.remove_password(&password).map_err(|_| CryptoError::Passphrase)?;
    }
    for sub in &mut key.secret_subkeys {
        if sub.key.secret_params().is_encrypted() {
            sub.key.remove_password(&password).map_err(|_| CryptoError::Passphrase)?;
        }
    }
    Ok(key)
}

/// What an OpenPGP public key says about itself.
pub(crate) fn openpgp_public(key: &SignedPublicKey) -> Result<PublicKey> {
    let mut data = Vec::new();
    key.to_writer(&mut data).map_err(|_| bad("the key can't be written"))?;
    let mut name = String::new();
    let mut emails = Vec::new();
    for user in &key.details.users {
        let id = String::from_utf8_lossy(user.id.id()).into_owned();
        let (n, e) = split_user_id(&id);
        if name.is_empty() {
            name = n;
        }
        if let Some(e) = e
            && !emails.contains(&e)
        {
            emails.push(e);
        }
    }
    let algorithm = key.primary_key.algorithm();
    let can_encrypt = algorithm.can_encrypt() || key.public_subkeys.iter().any(|s| s.key.algorithm().can_encrypt());
    let can_sign = algorithm.can_sign() || key.public_subkeys.iter().any(|s| s.key.algorithm().can_sign());
    let created = key.primary_key.created_at();
    Ok(PublicKey {
        scheme: Scheme::OpenPgp,
        fingerprint: hex(key.fingerprint().as_bytes()),
        name: if name.is_empty() { emails.first().cloned().unwrap_or_default() } else { name },
        emails,
        issuer: String::new(),
        not_before: Some(i64::from(created.as_secs())),
        not_after: None,
        is_ca: false,
        self_signed: true,
        can_sign,
        can_encrypt,
        data,
    })
}

/// `Name <address>` → the name and the address (lower-case); a bare address is both.
fn split_user_id(id: &str) -> (String, Option<String>) {
    let id: String = id.chars().filter(|c| !c.is_control()).take(512).collect();
    if let (Some(open), Some(close)) = (id.rfind('<'), id.rfind('>'))
        && open < close
    {
        let email = id.get(open + 1..close).unwrap_or("").trim().to_ascii_lowercase();
        let name = id.get(..open).unwrap_or("").trim().trim_matches('"').to_string();
        return (name, Some(email).filter(|e| e.contains('@')));
    }
    let t = id.trim();
    if t.contains('@') && !t.contains(' ') { (t.to_string(), Some(t.to_ascii_lowercase())) } else { (t.to_string(), None) }
}

/// Read a key kept earlier ([`PublicKey::data`]) again.
pub fn read_public(scheme: Scheme, data: &[u8]) -> Result<PublicKey> {
    match scheme {
        Scheme::Smime => certificate(&Certificate::from_der(data).map_err(|_| bad("the certificate can't be read"))?),
        Scheme::OpenPgp => {
            let (key, _) = SignedPublicKey::from_reader_single(data).map_err(|_| bad("the key can't be read"))?;
            openpgp_public(&key)
        }
    }
}

/// A public key as a file: a certificate as PEM (`pem`) or DER (`.cer`), an OpenPGP key armored.
pub fn export_public(key: &PublicKey, pem_form: bool) -> Result<Vec<u8>> {
    match key.scheme {
        Scheme::Smime if pem_form => Ok(pem::encode("CERTIFICATE", &key.data).into_bytes()),
        Scheme::Smime => Ok(key.data.clone()),
        Scheme::OpenPgp => {
            let (k, _) = SignedPublicKey::from_reader_single(key.data.as_slice()).map_err(|_| bad("the key can't be read"))?;
            k.to_armored_bytes(Default::default()).map_err(|_| bad("the key can't be written"))
        }
    }
}

/// An OpenPGP secret key kept in memory, read back.
pub(crate) fn openpgp_secret(secret: &SecretKey) -> Result<SignedSecretKey> {
    let (key, _) = SignedSecretKey::from_reader_single(secret.data.as_slice()).map_err(|_| bad("the secret key can't be read"))?;
    Ok(key)
}

fn bad(why: &str) -> CryptoError {
    CryptoError::Malformed(why.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_ids_split() {
        assert_eq!(split_user_id("Ada Lovelace <Ada@Example.org>"), ("Ada Lovelace".into(), Some("ada@example.org".into())));
        assert_eq!(split_user_id("ada@example.org"), ("ada@example.org".into(), Some("ada@example.org".into())));
        assert_eq!(split_user_id("Just a name"), ("Just a name".into(), None));
        assert_eq!(split_user_id("> <"), ("> <".into(), None));
    }

    #[test]
    fn junk_is_no_key() {
        for junk in [
            &b""[..],
            b"hello",
            b"-----BEGIN PGP PUBLIC KEY BLOCK-----\nxx\n-----END PGP PUBLIC KEY BLOCK-----",
            &[0x30, 0x03, 0x02, 0x01, 0x01],
            &[0x99, 0x00],
        ] {
            assert!(import(junk, None).is_err(), "{junk:?}");
        }
    }
}
