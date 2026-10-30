// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The server's side of an NTLMv2 sign-in, for the tests of the programs that sit on this crate
//! (#252): a CHALLENGE message, and a check of the AUTHENTICATE message against the password the
//! server holds (the NT proof, and the MIC when the client claims one). Written from [MS-NLMP]
//! 3.2.5 and the same sample messages as [`crate::ntlm`]; the password of a test is invented.

use crate::basic::base64;
use crate::ntlm::{AV_FLAGS, AV_TIMESTAMP, FLAG_MIC, MIC_AT, hmac_md5, ntowf_v2, parse_pairs, u16_at, u32_at, unbase64, utf16};

/// A CHALLENGE message (as the Base64 token of a `WWW-Authenticate: NTLM` header), with a
/// timestamp in its target information, as Exchange sends one.
pub fn challenge_token() -> (String, Vec<u8>) {
    let pairs: [(u16, Vec<u8>); 3] = [(2, utf16("EXAMPLE")), (1, utf16("MAIL")), (AV_TIMESTAMP, vec![1, 2, 3, 4, 5, 6, 7, 8])];
    let mut info = Vec::new();
    for (id, value) in &pairs {
        info.extend_from_slice(&id.to_le_bytes());
        info.extend_from_slice(&u16::try_from(value.len()).unwrap_or(0).to_le_bytes());
        info.extend_from_slice(value);
    }
    info.extend_from_slice(&[0, 0, 0, 0]);
    let len = u16::try_from(info.len()).unwrap_or(0).to_le_bytes();
    let mut out = Vec::new();
    out.extend_from_slice(b"NTLMSSP\0");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 56, 0, 0, 0]);
    out.extend_from_slice(&0xe28a_8233u32.to_le_bytes());
    out.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&len);
    out.extend_from_slice(&len);
    out.extend_from_slice(&56u32.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&info);
    (base64(&out), out)
}

/// Whether `authenticate` (the Base64 token of an AUTHENTICATE message) proves that its sender
/// knows `password` for `user` in `domain`, given the NEGOTIATE token the connection began with and
/// the CHALLENGE sent.
pub fn verifies(password: &str, user: &str, domain: &str, negotiate: &str, challenge: &[u8], authenticate: &str) -> bool {
    let (Some(negotiate), Some(message)) = (unbase64(negotiate), unbase64(authenticate)) else { return false };
    let Ok(key) = ntowf_v2(password, user, domain) else { return false };
    let field = |at: usize| -> Option<&[u8]> {
        let len = usize::from(u16_at(&message, at)?);
        let offset = usize::try_from(u32_at(&message, at.checked_add(4)?)?).ok()?;
        message.get(offset..offset.checked_add(len)?)
    };
    let Some(nt) = field(20) else { return false };
    let (Some(proof), Some(temp)) = (nt.get(..16), nt.get(16..)) else { return false };
    let Some(server_challenge) = challenge.get(24..32) else { return false };
    let Ok(expected) = hmac_md5(&key, &[server_challenge, temp]) else { return false };
    if expected.as_slice() != proof {
        return false;
    }
    let Ok(session_key) = hmac_md5(&key, &[proof]) else { return false };
    let info = temp.get(28..temp.len().saturating_sub(4)).unwrap_or(&[]);
    let claims_mic = parse_pairs(info)
        .map(|pairs| {
            pairs.iter().any(|(id, v)| *id == AV_FLAGS && v.get(..4).is_some_and(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) & FLAG_MIC != 0))
        })
        .unwrap_or(false);
    if claims_mic {
        let mut zeroed = message.clone();
        let Some(mic) = message.get(MIC_AT..MIC_AT + 16).map(<[u8]>::to_vec) else { return false };
        if let Some(slot) = zeroed.get_mut(MIC_AT..MIC_AT + 16) {
            slot.fill(0);
        }
        let Ok(sum) = hmac_md5(&session_key, &[&negotiate, challenge, &zeroed]) else { return false };
        return sum.as_slice() == mic.as_slice();
    }
    true
}
