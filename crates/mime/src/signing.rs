// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Digitally signed message's body as it is sent (#30 R19): `multipart/signed` (RFC 1847
//! §2.1) holding the signed part ([`crate::signable`]) byte for byte and the signature beside
//! it, as S/MIME (RFC 8551 §3.5.3: `application/pkcs7-signature`, `smime.p7s`) or PGP/MIME
//! (RFC 3156 §5: `application/pgp-signature`) has it. The signature itself is made elsewhere
//! (`mailcraft-smime`, `mailcraft-openpgp`): this is only the MIME around it.
//!
//! What comes out is the content fields of the message (`MIME-Version`, `Content-Type`) and its
//! body, which a message keeps as the bytes it goes as ([`Message::original`]): every path that
//! sends it, and the copy kept in Sent Items, write those bytes.

use crate::Message;

use crate::transfer;

/// Why a message that was to go Digitally signed isn't, said by every path that sends it before
/// anything goes (#30 R19): it is never sent unsigned.
pub const NOT_SIGNED: &str = "it was to be Digitally signed and wasn't, so it wasn't sent";

/// Whether what `message` would be written as is a whole signed message: a signed body kept as
/// its original, `multipart/signed` throughout. A message that was to be Digitally signed at Send
/// and isn't this is refused by every path that sends mail (`mailcraft_mime_model::signature_missing`).
pub fn is_signed(message: &Message) -> bool {
    crate::writes_original(message) && message.original_bytes().and_then(crate::signed_parts).is_some_and(|p| p.whole)
}

/// Which kind of Digital signature the second part is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureKind {
    /// S/MIME: a CMS SignedData in DER, carried in base64.
    Smime,
    /// OpenPGP: an armoured signature (`-----BEGIN PGP SIGNATURE-----`), carried as it is.
    OpenPgp,
}

/// The `micalg` values written: SHA-2 only, as the signers make them (RFC 8551 §3.5.3.2,
/// RFC 3156 §5). Anything else is refused.
const MICALGS: [&str; 6] = ["sha-256", "sha-384", "sha-512", "pgp-sha256", "pgp-sha384", "pgp-sha512"];

/// Why a signed body can't be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    /// A `micalg` that isn't one of SHA-2's for the kind.
    Micalg,
    /// An OpenPGP signature that isn't armoured text.
    NotArmoured,
    /// Nothing to sign, or nothing signed.
    Empty,
    /// No boundary was found that occurs in neither part.
    Boundary,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Micalg => "the digest algorithm can't be named",
            Self::NotArmoured => "the OpenPGP signature isn't armoured text",
            Self::Empty => "there is nothing to sign",
            Self::Boundary => "the parts can't be told apart",
        })
    }
}

impl std::error::Error for LayoutError {}

/// The content fields and body of a `multipart/signed` message holding `signed` (the bytes the
/// signature was made over, as [`crate::signable`] wrote them) and `signature` (DER for S/MIME,
/// armoured text for OpenPGP), with `micalg` naming the digest. The boundary is made from the
/// content and occurs in neither part, so the same input always gives the same bytes. The
/// signature part ends with a line end of its own, as the signed one does: a server that writes
/// the message again (Microsoft 365) then has none to add.
pub fn multipart_signed(signed: &[u8], kind: SignatureKind, micalg: &str, signature: &[u8]) -> Result<Vec<u8>, LayoutError> {
    let pgp = kind == SignatureKind::OpenPgp;
    if !MICALGS.contains(&micalg) || micalg.starts_with("pgp-") != pgp {
        return Err(LayoutError::Micalg);
    }
    if signed.is_empty() || signature.is_empty() {
        return Err(LayoutError::Empty);
    }
    let (protocol, second) = match kind {
        SignatureKind::Smime => {
            let mut part = b"Content-Type: application/pkcs7-signature; name=\"smime.p7s\"\r\n\
Content-Transfer-Encoding: base64\r\n\
Content-Disposition: attachment; filename=\"smime.p7s\"\r\n\
Content-Description: S/MIME Cryptographic Signature\r\n\r\n"
                .to_vec();
            let lines = transfer::base64_lines(signature);
            part.extend_from_slice(&lines);
            if !part.ends_with(b"\r\n") {
                part.extend_from_slice(b"\r\n");
            }
            ("application/pkcs7-signature", part)
        }
        SignatureKind::OpenPgp => {
            let text = std::str::from_utf8(signature).map_err(|_| LayoutError::NotArmoured)?;
            let armoured = text.trim_start().starts_with("-----BEGIN PGP SIGNATURE-----")
                && text.trim_end().ends_with("-----END PGP SIGNATURE-----")
                && text.bytes().all(|b| matches!(b, b'\r' | b'\n' | 0x20..=0x7E));
            if !armoured {
                return Err(LayoutError::NotArmoured);
            }
            let mut part = b"Content-Type: application/pgp-signature; name=\"signature.asc\"\r\n\
Content-Description: OpenPGP digital signature\r\n\
Content-Disposition: attachment; filename=\"signature.asc\"\r\n\r\n"
                .to_vec();
            let lines: Vec<&str> = text.trim().lines().map(|l| l.trim_end_matches('\r')).collect();
            part.extend_from_slice(lines.join("\r\n").as_bytes());
            part.extend_from_slice(b"\r\n");
            ("application/pgp-signature", part)
        }
    };
    let boundary = boundary(&[signed, &second]).ok_or(LayoutError::Boundary)?;
    let mut out = Vec::with_capacity(signed.len() + second.len() + 512);
    out.extend_from_slice(b"MIME-Version: 1.0\r\n");
    out.extend_from_slice(
        format!("Content-Type: multipart/signed; protocol=\"{protocol}\"; micalg={micalg};\r\n boundary=\"{boundary}\"\r\n\r\n").as_bytes(),
    );
    out.extend_from_slice(b"This is a Digitally signed message in MIME format.\r\n\r\n");
    for part in [signed, &second] {
        out.extend_from_slice(b"--");
        out.extend_from_slice(boundary.as_bytes());
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(part);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"--");
    out.extend_from_slice(boundary.as_bytes());
    out.extend_from_slice(b"--\r\n");
    Ok(out)
}

/// The most boundaries tried: each try that fails needs a part to hold that very boundary.
const MAX_BOUNDARY_TRIES: u32 = 1024;

/// A boundary in neither part: an FNV-1a hash of them, with a counter that moves on while a
/// part holds it.
fn boundary(parts: &[&[u8]]) -> Option<String> {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.iter() {
            hash = (hash ^ u64::from(*b)).wrapping_mul(PRIME);
        }
        hash = (hash ^ 0x1ff).wrapping_mul(PRIME);
    }
    (0..MAX_BOUNDARY_TRIES)
        .map(|counter| format!("=_mc_signed_{hash:016x}_{counter}"))
        .find(|candidate| !parts.iter().any(|p| p.windows(candidate.len()).any(|w| w == candidate.as_bytes())))
}
