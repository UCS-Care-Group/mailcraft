//! PEM (RFC 7468): `-----BEGIN LABEL-----`, base64, `-----END LABEL-----`.

use base64::Engine as _;

/// Most blocks read from one file.
const MAX_BLOCKS: usize = 256;

/// The blocks of a PEM text: each label and its bytes. Text around the blocks is ignored (files
/// often carry a description), and a block whose base64 doesn't read is skipped.
pub fn blocks(text: &str) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(label) = line.trim().strip_prefix("-----BEGIN ").and_then(|l| l.strip_suffix("-----")) else { continue };
        let label = label.trim().to_string();
        let end = format!("-----END {label}-----");
        let mut body = String::new();
        let mut closed = false;
        for inner in lines.by_ref() {
            let inner = inner.trim();
            if inner == end {
                closed = true;
                break;
            }
            // Headers of the old encrypted form (`Proc-Type: 4,ENCRYPTED`) aren't base64.
            if inner.contains(':') {
                body.push('!');
                continue;
            }
            body.push_str(inner);
        }
        if !closed {
            break;
        }
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(body.as_bytes()) {
            out.push((label, bytes));
        }
        if out.len() >= MAX_BLOCKS {
            break;
        }
    }
    out
}

/// One block as PEM, in 64-column lines.
pub fn encode(label: &str, bytes: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_round_trip() {
        let text = format!("Bag attributes\n{}junk\n{}", encode("CERTIFICATE", b"abc"), encode("PRIVATE KEY", &[0u8; 100]));
        let b = blocks(&text);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0], ("CERTIFICATE".to_string(), b"abc".to_vec()));
        assert_eq!(b[1].1.len(), 100);
        assert!(blocks("-----BEGIN X-----\nYWJj\n").is_empty());
        assert!(blocks("-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nYWJj\n-----END RSA PRIVATE KEY-----").is_empty());
    }
}
