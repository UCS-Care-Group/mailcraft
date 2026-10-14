//! PKCS#12 files (`.p12`, `.pfx`; RFC 7292): a certificate, its private key and the certificates
//! above it, under a passphrase.
//!
//! Read in password integrity and privacy mode, as Windows, macOS and OpenSSL write them: the MAC
//! (HMAC with SHA-1 or SHA-2) is checked first, so a wrong passphrase is said as such; bags are
//! encrypted with PBES2 (PBKDF2, AES or 3DES, OpenSSL 3's way) or the PKCS#12 schemes of older
//! programs (3DES, RC2). A file in public-key mode, or asking for more work than is sensible, is
//! refused.

use der::asn1::OctetString;
use der::{Decode, Encode};
use hmac::Mac as _;
use pkcs12::kdf::{Pkcs12KeyType, derive_key_utf8};
use x509_cert::Certificate;
use zeroize::Zeroizing;

use crate::keys::{certificate, smime_secret};
use crate::{CryptoError, Imported, Result};

/// Most iterations a key derivation in a file may ask for.
const MAX_ITERATIONS: u32 = 2_000_000;
/// Most bags read.
const MAX_BAGS: usize = 512;
/// scrypt: largest `N × r` (its memory is 128 × N × r bytes).
const MAX_SCRYPT_WORK: u64 = 1 << 20;

const ID_DATA: der::asn1::ObjectIdentifier = der::asn1::ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");
const ID_ENCRYPTED_DATA: der::asn1::ObjectIdentifier = der::asn1::ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.6");
const ID_PBES2: der::asn1::ObjectIdentifier = der::asn1::ObjectIdentifier::new_unwrap("1.2.840.113549.1.5.13");
const ID_SHA1: der::asn1::ObjectIdentifier = der::asn1::ObjectIdentifier::new_unwrap("1.3.14.3.2.26");
const ID_SHA256: der::asn1::ObjectIdentifier = der::asn1::ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");
const ID_SHA384: der::asn1::ObjectIdentifier = der::asn1::ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.2");
const ID_SHA512: der::asn1::ObjectIdentifier = der::asn1::ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.3");

/// Whether the bytes are a PKCS#12 file.
pub fn looks_like(bytes: &[u8]) -> bool {
    crate::ber::to_der(bytes).ok().is_some_and(|der| pkcs12::pfx::Pfx::from_der(&der).is_ok())
}

/// The certificates and private keys of a PKCS#12 file.
pub fn read(bytes: &[u8], passphrase: &str) -> Result<Imported> {
    let der = crate::ber::to_der(bytes)?;
    let pfx = pkcs12::pfx::Pfx::from_der(&der).map_err(|_| bad("this isn't a PKCS#12 file"))?;
    if pfx.auth_safe.content_type != ID_DATA {
        return Err(CryptoError::Unsupported("PKCS#12 files protected by a key rather than a passphrase".into()));
    }
    let data = pfx.auth_safe.content.decode_as::<OctetString>().map_err(|_| bad("the file's content can't be read"))?;
    let data = data.as_bytes();
    if let Some(mac) = &pfx.mac_data {
        check_mac(mac, data, passphrase)?;
    }
    let safes = Vec::<cms::content_info::ContentInfo>::from_der(data).map_err(|_| bad("the file's content can't be read"))?;
    let mut out = Imported::default();
    let mut bags = 0usize;
    for info in safes.iter().take(64) {
        let contents: Zeroizing<Vec<u8>> = if info.content_type == ID_DATA {
            Zeroizing::new(info.content.decode_as::<OctetString>().map_err(|_| bad("a part can't be read"))?.as_bytes().to_vec())
        } else if info.content_type == ID_ENCRYPTED_DATA {
            let enc: cms::encrypted_data::EncryptedData = info.content.decode_as().map_err(|_| bad("an encrypted part can't be read"))?;
            let Some(ciphertext) = &enc.enc_content_info.encrypted_content else { continue };
            decrypt(&enc.enc_content_info.content_enc_alg, ciphertext.as_bytes(), passphrase)?
        } else {
            continue;
        };
        let safe = pkcs12::safe_bag::SafeContents::from_der(&contents).map_err(|_| bad("a part can't be read"))?;
        for bag in safe {
            bags += 1;
            if bags > MAX_BAGS {
                break;
            }
            let Some(value) = explicit_inner(&bag.bag_value) else { continue };
            if bag.bag_id == pkcs12::PKCS_12_CERT_BAG_OID {
                let Ok(cert_bag) = pkcs12::cert_type::CertBag::from_der(&value) else { continue };
                if cert_bag.cert_id != pkcs12::PKCS_12_X509_CERT_OID {
                    continue;
                }
                if let Ok(cert) = Certificate::from_der(cert_bag.cert_value.as_bytes()) {
                    out.public.push(certificate(&cert)?);
                }
            } else if bag.bag_id == pkcs12::PKCS_12_KEY_BAG_OID {
                out.secret.push(smime_secret(&value)?);
            } else if bag.bag_id == pkcs12::PKCS_12_PKCS8_KEY_BAG_OID {
                let info = pkcs12::pbe_params::EncryptedPrivateKeyInfo::from_der(&value).map_err(|_| bad("the private key can't be read"))?;
                let key = decrypt(&info.encryption_algorithm, info.encrypted_data.as_bytes(), passphrase)?;
                out.secret.push(smime_secret(&key)?);
            }
        }
    }
    Ok(out)
}

/// The value inside an explicit `[0]` tag.
fn explicit_inner(tlv: &[u8]) -> Option<Vec<u8>> {
    der::Any::from_der(tlv).ok().map(|a| a.value().to_vec())
}

fn iterations(n: i32) -> Result<i32> {
    if n < 1 || n as u32 > MAX_ITERATIONS {
        return Err(CryptoError::Unsupported("the file asks for an unreasonable amount of work".into()));
    }
    Ok(n)
}

fn check_mac(mac: &pkcs12::mac_data::MacData, data: &[u8], passphrase: &str) -> Result<()> {
    let rounds = iterations(mac.iterations)?;
    let salt = mac.mac_salt.as_bytes();
    let expected = mac.mac.digest.as_bytes();
    let oid = mac.mac.algorithm.oid;
    macro_rules! check {
        ($d:ty) => {{
            let len = <$d as sha2::Digest>::output_size();
            let key = derive_key_utf8::<$d>(passphrase, salt, Pkcs12KeyType::Mac, rounds, len).map_err(|_| CryptoError::Passphrase)?;
            let mut h = <hmac::Hmac<$d> as hmac::Mac>::new_from_slice(&key).map_err(|_| CryptoError::Passphrase)?;
            h.update(data);
            h.verify_slice(expected).is_ok()
        }};
    }
    let ok = if oid == ID_SHA1 {
        check!(sha1::Sha1)
    } else if oid == ID_SHA256 {
        check!(sha2::Sha256)
    } else if oid == ID_SHA384 {
        check!(sha2::Sha384)
    } else if oid == ID_SHA512 {
        check!(sha2::Sha512)
    } else {
        return Err(CryptoError::Unsupported("the file's integrity check uses an unknown digest".into()));
    };
    if ok { Ok(()) } else { Err(CryptoError::Passphrase) }
}

/// Undo a password-based encryption of a PKCS#12 bag or part.
fn decrypt(alg: &spki::AlgorithmIdentifierOwned, data: &[u8], passphrase: &str) -> Result<Zeroizing<Vec<u8>>> {
    use cipher::{BlockDecryptMut as _, InnerIvInit as _, KeyIvInit as _, block_padding::Pkcs7};
    if alg.oid == ID_PBES2 {
        let der = alg.to_der().map_err(|_| bad("the encryption can't be read"))?;
        let scheme = pkcs5::EncryptionScheme::from_der(&der).map_err(|_| bad("the encryption can't be read"))?;
        check_pbes2_work(&scheme)?;
        let plain = scheme.decrypt(passphrase.as_bytes(), data).map_err(|_| CryptoError::Passphrase)?;
        return Ok(Zeroizing::new(plain));
    }
    let params = alg.parameters.as_ref().ok_or_else(|| bad("the encryption has no parameters"))?;
    let params: pkcs12::pbe_params::Pkcs12PbeParams = params.decode_as().map_err(|_| bad("the encryption can't be read"))?;
    let rounds = iterations(params.iterations)?;
    let salt = params.salt.as_bytes();
    let derive = |kind: Pkcs12KeyType, len: usize| -> Result<Zeroizing<Vec<u8>>> {
        derive_key_utf8::<sha1::Sha1>(passphrase, salt, kind, rounds, len).map(Zeroizing::new).map_err(|_| CryptoError::Passphrase)
    };
    let iv = derive(Pkcs12KeyType::Iv, 8)?;
    let plain = if alg.oid == pkcs12::PKCS_12_PBE_WITH_SHAAND3_KEY_TRIPLE_DES_CBC {
        let key = derive(Pkcs12KeyType::EncryptionKey, 24)?;
        cbc::Decryptor::<des::TdesEde3>::new_from_slices(&key, &iv)
            .map_err(|_| CryptoError::Passphrase)?
            .decrypt_padded_vec_mut::<Pkcs7>(data)
            .map_err(|_| CryptoError::Passphrase)?
    } else if alg.oid == pkcs12::PKCS_12_PBEWITH_SHAAND40_BIT_RC2_CBC || alg.oid == pkcs12::PKCS_12_PBE_WITH_SHAAND128_BIT_RC2_CBC {
        let bits = if alg.oid == pkcs12::PKCS_12_PBEWITH_SHAAND40_BIT_RC2_CBC { 40 } else { 128 };
        let key = derive(Pkcs12KeyType::EncryptionKey, bits / 8)?;
        let rc2 = rc2::Rc2::new_with_eff_key_len(&key, bits);
        cbc::Decryptor::<rc2::Rc2>::inner_iv_slice_init(rc2, &iv)
            .map_err(|_| CryptoError::Passphrase)?
            .decrypt_padded_vec_mut::<Pkcs7>(data)
            .map_err(|_| CryptoError::Passphrase)?
    } else {
        return Err(CryptoError::Unsupported("the file is encrypted in a way this program doesn't read".into()));
    };
    Ok(Zeroizing::new(plain))
}

/// Refuse a PBES2 scheme that asks for more work than a key file should.
pub(crate) fn check_pbes2_work(scheme: &pkcs5::EncryptionScheme<'_>) -> Result<()> {
    let too_much = || CryptoError::Unsupported("the file asks for an unreasonable amount of work".into());
    match scheme {
        pkcs5::EncryptionScheme::Pbes2(params) => match &params.kdf {
            pkcs5::pbes2::Kdf::Pbkdf2(p) if p.iteration_count == 0 || p.iteration_count > MAX_ITERATIONS => Err(too_much()),
            pkcs5::pbes2::Kdf::Scrypt(p) => {
                if p.cost_parameter.saturating_mul(u64::from(p.block_size)) > MAX_SCRYPT_WORK || p.parallelization > 4 {
                    Err(too_much())
                } else {
                    Ok(())
                }
            }
            _ => Ok(()),
        },
        _ => Ok(()),
    }
}

fn bad(why: &str) -> CryptoError {
    CryptoError::Malformed(why.to_string())
}
