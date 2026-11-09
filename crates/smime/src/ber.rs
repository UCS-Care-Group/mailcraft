// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading the TLVs of ASN.1 BER and DER (X.690), and turning BER into DER where they differ in
//! what CMS writers use: indefinite lengths (which streaming signers write) and OCTET STRINGs in
//! pieces (constructed). What is DER already is kept byte for byte, so that the bytes a signature
//! covers (signed attributes, a certificate's body) are those the signer wrote.
//!
//! The input is hostile: lengths are checked against what is there, nesting is bounded
//! ([`MAX_DEPTH`]), and the work is linear in the input.

/// How deep elements may nest.
pub const MAX_DEPTH: usize = 48;

/// The universal tag of an OCTET STRING, and of one in pieces.
const OCTET_STRING: u8 = 0x04;
const OCTET_STRING_PIECES: u8 = 0x24;

/// One element: its tag bytes, and its value (the content octets).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tlv<'a> {
    /// The tag's first byte (class, constructed bit, number below 31).
    pub tag: u8,
    /// The whole element, tag and length included.
    pub whole: &'a [u8],
    /// Its content octets.
    pub value: &'a [u8],
}

/// The tag bytes at the start of `bytes`: one, or a high tag number in up to four more.
fn tag_len(bytes: &[u8]) -> Option<usize> {
    let first = *bytes.first()?;
    if first & 0x1f != 0x1f {
        return Some(1);
    }
    // High tag number: base-128 bytes, the last without its top bit.
    for (i, b) in bytes.iter().enumerate().skip(1).take(4) {
        if b & 0x80 == 0 {
            return i.checked_add(1);
        }
    }
    None
}

/// The length at the start of `bytes`: `(length, bytes it took)`, `None` for an indefinite one
/// (`Some((None, 1))`) is told apart by the caller.
fn length(bytes: &[u8]) -> Option<(Option<usize>, usize)> {
    let first = *bytes.first()?;
    if first < 0x80 {
        return Some((Some(usize::from(first)), 1));
    }
    if first == 0x80 {
        return Some((None, 1));
    }
    let n = usize::from(first & 0x7f);
    if n > 4 {
        return None;
    }
    let digits = bytes.get(1..n.checked_add(1)?)?;
    let len = digits.iter().fold(0usize, |acc, d| (acc << 8) | usize::from(*d));
    Some((Some(len), n.checked_add(1)?))
}

/// The element at the start of `bytes` with a definite length, and what follows it. `None` for
/// an indefinite length, or one longer than what is there.
pub fn read(bytes: &[u8]) -> Option<(Tlv<'_>, &[u8])> {
    let t = tag_len(bytes)?;
    let tag = *bytes.first()?;
    let (len, l) = length(bytes.get(t..)?)?;
    let len = len?;
    let start = t.checked_add(l)?;
    let end = start.checked_add(len)?;
    let value = bytes.get(start..end)?;
    let whole = bytes.get(..end)?;
    Some((Tlv { tag, whole, value }, bytes.get(end..)?))
}

/// The elements of `value`, each with a definite length; `None` when one can't be read.
pub fn children(mut value: &[u8]) -> Option<Vec<Tlv<'_>>> {
    let mut out = Vec::new();
    while !value.is_empty() {
        let (tlv, rest) = read(value)?;
        out.push(tlv);
        value = rest;
    }
    Some(out)
}

/// `bytes` as DER as far as CMS needs it: indefinite lengths made definite and OCTET STRINGs in
/// pieces joined into one, everything else as it was. One element is read, and must be all of
/// `bytes` (bar trailing zero bytes, which some writers pad with). `None` when it isn't BER.
pub fn to_der(bytes: &[u8]) -> Option<Vec<u8>> {
    if let Some((_, rest)) = read(bytes)
        && rest.iter().all(|b| *b == 0)
        && der_shaped(bytes.get(..bytes.len().saturating_sub(rest.len()))?, 0)
    {
        return bytes.get(..bytes.len().saturating_sub(rest.len())).map(<[u8]>::to_vec);
    }
    let mut out = Vec::with_capacity(bytes.len());
    let used = element(bytes, 0, &mut out)?;
    let rest = bytes.get(used..)?;
    rest.iter().all(|b| *b == 0).then_some(out)
}

/// Whether the elements of `bytes` all have definite lengths and no OCTET STRING is in pieces,
/// as deep as they go: nothing to change.
fn der_shaped(mut bytes: &[u8], depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    while !bytes.is_empty() {
        let Some((tlv, rest)) = read(bytes) else { return false };
        if tlv.tag == OCTET_STRING_PIECES || (tlv.tag & 0x20 != 0 && !der_shaped(tlv.value, depth.saturating_add(1))) {
            return false;
        }
        bytes = rest;
    }
    true
}

/// Write the element at the start of `bytes` to `out` as DER; how many bytes of `bytes` it was.
fn element(bytes: &[u8], depth: usize, out: &mut Vec<u8>) -> Option<usize> {
    if depth > MAX_DEPTH {
        return None;
    }
    let t = tag_len(bytes)?;
    let tag_bytes = bytes.get(..t)?;
    let tag = *tag_bytes.first()?;
    let constructed = tag & 0x20 != 0;
    let (len, l) = length(bytes.get(t..)?)?;
    let start = t.checked_add(l)?;
    if !constructed {
        let len = len?;
        let end = start.checked_add(len)?;
        out.extend_from_slice(bytes.get(..end)?);
        return Some(end);
    }
    // The children, each made DER, into a buffer of their own.
    let mut inner = Vec::new();
    let used = match len {
        Some(len) => {
            let end = start.checked_add(len)?;
            let mut value = bytes.get(start..end)?;
            while !value.is_empty() {
                let n = element(value, depth.checked_add(1)?, &mut inner)?;
                value = value.get(n..)?;
            }
            // Already DER: kept as it was, length bytes and all.
            if tag != OCTET_STRING_PIECES && inner.as_slice() == bytes.get(start..end)? {
                out.extend_from_slice(bytes.get(..end)?);
                return Some(end);
            }
            end
        }
        None => {
            let mut at = start;
            loop {
                let rest = bytes.get(at..)?;
                if rest.starts_with(&[0, 0]) {
                    break at.checked_add(2)?;
                }
                let n = element(rest, depth.checked_add(1)?, &mut inner)?;
                if n == 0 {
                    return None;
                }
                at = at.checked_add(n)?;
            }
        }
    };
    if tag == OCTET_STRING_PIECES {
        // Its pieces are OCTET STRINGs (in DER now, so primitive): one OCTET STRING of them all.
        let mut joined = Vec::with_capacity(inner.len());
        let mut pieces = inner.as_slice();
        while !pieces.is_empty() {
            let (piece, rest) = read(pieces)?;
            if piece.tag != OCTET_STRING {
                return None;
            }
            joined.extend_from_slice(piece.value);
            pieces = rest;
        }
        out.push(OCTET_STRING);
        put_length(out, joined.len());
        out.extend_from_slice(&joined);
    } else {
        out.extend_from_slice(tag_bytes);
        put_length(out, inner.len());
        out.extend_from_slice(&inner);
    }
    Some(used)
}

/// A definite length in DER's shortest form.
fn put_length(out: &mut Vec<u8>, len: usize) {
    if len < 0x80 {
        out.push(len as u8);
        return;
    }
    let bytes = len.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count();
    let digits = bytes.get(skip..).unwrap_or_default();
    out.push(0x80 | digits.len() as u8);
    out.extend_from_slice(digits);
}
