//! BER to DER, as far as CMS needs it.
//!
//! Mail programs that write S/MIME as they go (OpenSSL's `smime`, NSS) use BER's indefinite
//! lengths and cut long OCTET STRINGs into pieces; the DER reader under `cms` takes neither. This
//! rewrites such input with definite lengths and with every constructed OCTET STRING joined into
//! one, which is what a DER reader expects. It sorts nothing: a SET is left in the order it came
//! in (which is what a signature over signed attributes was made on).
//!
//! The input is hostile: lengths are checked against what is there, nesting is capped, and the
//! output is never larger than the input plus a few bytes per element.

use crate::{CryptoError, Result};

/// Deepest nesting followed.
const MAX_DEPTH: usize = 48;

/// Most elements read from one input.
const MAX_ELEMENTS: usize = 1_000_000;

/// The universal tag of OCTET STRING, primitive and constructed.
const OCTET_STRING: u8 = 0x04;
const CONSTRUCTED: u8 = 0x20;

/// The same structure with definite lengths and whole OCTET STRINGs. DER stays as it is.
pub fn to_der(input: &[u8]) -> Result<Vec<u8>> {
    let mut reader = Reader { input, pos: 0, elements: 0 };
    let element = reader.element(0)?;
    let mut out = Vec::with_capacity(input.len());
    element.write(&mut out);
    Ok(out)
}

/// One element, read.
enum Element {
    Primitive { tag: Vec<u8>, value: Vec<u8> },
    Constructed { tag: Vec<u8>, children: Vec<Element> },
}

impl Element {
    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Element::Primitive { tag, value } => {
                out.extend_from_slice(tag);
                write_length(out, value.len());
                out.extend_from_slice(value);
            }
            Element::Constructed { tag, children } => {
                let mut inner = Vec::new();
                for child in children {
                    child.write(&mut inner);
                }
                out.extend_from_slice(tag);
                write_length(out, inner.len());
                out.extend_from_slice(&inner);
            }
        }
    }

    /// The bytes of a (possibly cut-up) OCTET STRING.
    fn octets(&self, out: &mut Vec<u8>) {
        match self {
            Element::Primitive { value, .. } => out.extend_from_slice(value),
            Element::Constructed { children, .. } => {
                for child in children {
                    child.octets(out);
                }
            }
        }
    }
}

fn write_length(out: &mut Vec<u8>, len: usize) {
    if len < 0x80 {
        out.push(len as u8);
        return;
    }
    let bytes = len.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count();
    let significant = bytes.get(skip..).unwrap_or_default();
    out.push(0x80 | significant.len() as u8);
    out.extend_from_slice(significant);
}

struct Reader<'a> {
    input: &'a [u8],
    pos: usize,
    elements: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8> {
        let b = *self.input.get(self.pos).ok_or_else(|| bad("the data ends early"))?;
        self.pos += 1;
        Ok(b)
    }

    fn tag(&mut self) -> Result<Vec<u8>> {
        let first = self.byte()?;
        let mut tag = vec![first];
        if first & 0x1f == 0x1f {
            // High tag number: base-128 digits, the last without its top bit.
            loop {
                let b = self.byte()?;
                tag.push(b);
                if tag.len() > 6 {
                    return Err(bad("a tag is too long"));
                }
                if b & 0x80 == 0 {
                    break;
                }
            }
        }
        Ok(tag)
    }

    /// `None` for an indefinite length.
    fn length(&mut self) -> Result<Option<usize>> {
        let first = self.byte()?;
        if first < 0x80 {
            return Ok(Some(usize::from(first)));
        }
        let count = usize::from(first & 0x7f);
        if count == 0 {
            return Ok(None);
        }
        if count > 4 {
            return Err(bad("a length is too long"));
        }
        let mut len = 0usize;
        for _ in 0..count {
            let b = self.byte()?;
            len = len.checked_mul(256).and_then(|l| l.checked_add(usize::from(b))).ok_or_else(|| bad("a length is too long"))?;
        }
        if len > self.input.len().saturating_sub(self.pos) {
            return Err(bad("a length runs past the end"));
        }
        Ok(Some(len))
    }

    fn at_end_of_contents(&self) -> bool {
        self.input.get(self.pos..self.pos.saturating_add(2)) == Some(&[0, 0][..])
    }

    fn element(&mut self, depth: usize) -> Result<Element> {
        if depth > MAX_DEPTH {
            return Err(bad("the data is nested too deeply"));
        }
        self.elements += 1;
        if self.elements > MAX_ELEMENTS {
            return Err(bad("the data has too many parts"));
        }
        let tag = self.tag()?;
        let constructed = tag.first().is_some_and(|t| t & CONSTRUCTED != 0);
        let length = self.length()?;
        if !constructed {
            let len = length.ok_or_else(|| bad("a primitive value has no length"))?;
            let end = self.pos.checked_add(len).ok_or_else(|| bad("a length is too long"))?;
            let value = self.input.get(self.pos..end).ok_or_else(|| bad("the data ends early"))?.to_vec();
            self.pos = end;
            return Ok(Element::Primitive { tag, value });
        }
        let mut children = Vec::new();
        match length {
            Some(len) => {
                let end = self.pos.checked_add(len).ok_or_else(|| bad("a length is too long"))?;
                while self.pos < end {
                    children.push(self.element(depth + 1)?);
                }
                if self.pos != end {
                    return Err(bad("a value runs past its length"));
                }
            }
            None => {
                while !self.at_end_of_contents() {
                    if self.pos >= self.input.len() {
                        return Err(bad("the data ends early"));
                    }
                    children.push(self.element(depth + 1)?);
                }
                self.pos += 2;
            }
        }
        if tag.as_slice() == [OCTET_STRING | CONSTRUCTED] {
            // Join the pieces into one primitive OCTET STRING.
            let mut value = Vec::new();
            for child in &children {
                child.octets(&mut value);
            }
            return Ok(Element::Primitive { tag: vec![OCTET_STRING], value });
        }
        Ok(Element::Constructed { tag, children })
    }
}

fn bad(why: &str) -> CryptoError {
    CryptoError::Malformed(why.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indefinite_lengths_and_cut_octet_strings_become_der() {
        // SEQUENCE (indefinite) { OCTET STRING (constructed, indefinite) { "ab", "c" }, INTEGER 5 }
        let ber = [0x30, 0x80, 0x24, 0x80, 0x04, 0x02, b'a', b'b', 0x04, 0x01, b'c', 0x00, 0x00, 0x02, 0x01, 0x05, 0x00, 0x00];
        assert_eq!(to_der(&ber).unwrap(), [0x30, 0x08, 0x04, 0x03, b'a', b'b', b'c', 0x02, 0x01, 0x05]);
        // DER is left as it is.
        let der = [0x30, 0x03, 0x02, 0x01, 0x07];
        assert_eq!(to_der(&der).unwrap(), der);
    }

    #[test]
    fn hostile_input_is_refused() {
        for bad in [
            &[0x30, 0x84, 0xff, 0xff, 0xff, 0xff][..],
            &[0x30, 0x80, 0x02, 0x01],
            &[0x04, 0x05, 1],
            &[],
            &[0x1f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f],
        ] {
            assert!(to_der(bad).is_err(), "{bad:?}");
        }
        let deep: Vec<u8> = std::iter::repeat_n([0x30u8, 0x80], 100).flatten().collect();
        assert!(to_der(&deep).is_err());
        let long = vec![0x00u8; 300];
        let mut big = vec![0x30, 0x82, 0x01, 0x2c];
        big.extend_from_slice(&long);
        assert!(to_der(&big).is_ok());
    }
}
