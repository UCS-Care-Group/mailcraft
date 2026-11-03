// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! NTLMv2 sign-in for Exchange servers (#252, E4), written from Microsoft's public
//! specification [MS-NLMP] (sections 2.2, 3.1.5 and 3.3.2, and the sample messages of 4.2.4).
//!
//! NTLM over HTTP is three legs on **one connection** (the server ties the sign-in to the
//! connection, not to a request): the client sends NEGOTIATE, the server answers 401 with a
//! CHALLENGE, the client sends AUTHENTICATE, and the request that carries it is answered for
//! real. This crate does no I/O, so the legs are driven by the host: [`sign_in`] runs the first
//! two over an exchange the host gives it and hands back the `Authorization` value of the third.
//!
//! What is and is not done:
//!
//! - **Only NTLMv2.** NTLMv1 and the LM hash are not implemented, so they cannot be sent. The
//!   response key is `HMAC_MD5(MD4(UTF-16LE(password)), UTF-16LE(UPPER(user) + domain))`. The
//!   LMv2 response is sent when the server gave no timestamp; with a timestamp, 24 zero bytes
//!   (3.1.5.1.2).
//! - **No signing or sealing.** HTTP over TLS needs neither, so only Unicode, NTLM, extended
//!   session security, 128-bit and "request target" are negotiated, key exchange is not, and the
//!   session key is the session base key (3.1.5.1.2: no key exchange means no random key).
//! - **MIC.** When the server's target information has a timestamp the client proves the three
//!   messages were not tampered with: `MsvAvFlags` gets bit 0x2 and the MIC field is
//!   `HMAC_MD5(session key, NEGOTIATE ‖ CHALLENGE ‖ AUTHENTICATE with a zero MIC)`.
//! - **Channel binding and target name** (what Exchange calls Extended Protection): the
//!   AUTHENTICATE message carries `MsvAvChannelBindings`, the MD5 of the `gss_channel_bindings_struct`
//!   whose application data is `tls-server-end-point:` and the hash of the server's certificate
//!   ([`Context::channel_binding`]; sixteen zero bytes when the host has none), and
//!   `MsvAvTargetName`, the SPN `HTTP/host`. A server whose TLS ends in front of Exchange (a
//!   load balancer re-encrypting) sees a different certificate than Exchange does and fails the
//!   binding where Extended Protection is required; the host may then send none.
//! - **No machine name, no OS version.** The workstation name and `Version` fields are left empty
//!   (the flags that carry them are not set), so nothing of this computer goes to the server.
//!
//! The server's CHALLENGE is hostile: every offset and length is checked, the target
//! information is read within caps, and nothing indexes into it. A password never appears in an
//! error, a `Debug` output or a message of this crate; of it only the response key is kept.

use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use md4::{Digest, Md4};
use md5::Md5;

use crate::auth;
use crate::basic::base64;
use crate::http::Secret;

/// The signature that begins every NTLM message: `NTLMSSP\0`.
const SIGNATURE: &[u8; 8] = b"NTLMSSP\0";
const NEGOTIATE_MESSAGE: u32 = 1;
const CHALLENGE_MESSAGE: u32 = 2;
const AUTHENTICATE_MESSAGE: u32 = 3;

// NegotiateFlags (2.2.2.5).
const NEGOTIATE_UNICODE: u32 = 0x0000_0001;
const REQUEST_TARGET: u32 = 0x0000_0004;
const NEGOTIATE_NTLM: u32 = 0x0000_0200;
const NEGOTIATE_ALWAYS_SIGN: u32 = 0x0000_8000;
const NEGOTIATE_EXTENDED_SESSIONSECURITY: u32 = 0x0008_0000;
const NEGOTIATE_128: u32 = 0x2000_0000;
/// What the client asks for.
const OFFERED: u32 = NEGOTIATE_UNICODE | REQUEST_TARGET | NEGOTIATE_NTLM | NEGOTIATE_ALWAYS_SIGN | NEGOTIATE_EXTENDED_SESSIONSECURITY | NEGOTIATE_128;

// AvIds (2.2.2.1).
const AV_EOL: u16 = 0;
pub(crate) const AV_FLAGS: u16 = 6;
pub(crate) const AV_TIMESTAMP: u16 = 7;
const AV_TARGET_NAME: u16 = 9;
const AV_CHANNEL_BINDINGS: u16 = 10;
/// `MsvAvFlags` bit 0x2: the AUTHENTICATE message has a MIC.
pub(crate) const FLAG_MIC: u32 = 0x2;

/// Most AV pairs, and most bytes of target information, taken from a server.
const MAX_PAIRS: usize = 64;
const MAX_TARGET_INFO: usize = 16 * 1024;
/// Most Base64 characters of a challenge taken, and the longest user name, domain and password.
const MAX_TOKEN: usize = 32 * 1024;
const MAX_PART: usize = 1_024;
/// Where the MIC sits in an AUTHENTICATE message: after the fixed fields and the version.
pub(crate) const MIC_AT: usize = 72;
/// Seconds between 1601-01-01 (the epoch of a FILETIME) and 1970-01-01.
const FILETIME_EPOCH: u64 = 11_644_473_600;

/// Why a sign-in could not be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NtlmError {
    /// No user name.
    NoUserName,
    /// A user name, a domain or a password with a control character, or too long to be one.
    Unusable,
    /// The server's answer is no NTLM CHALLENGE (or is cut off, or lies about its own sizes).
    BadChallenge,
    /// The server's target information is longer than any server sends.
    TooLarge,
    /// The server will not speak Unicode, and nothing else is spoken here.
    NotUnicode,
    /// A message could not be made (a field longer than the format allows).
    Internal,
}

impl fmt::Display for NtlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoUserName => "the user name is empty",
            Self::Unusable => "the user name, the domain or the password has a character that cannot be sent, or is too long",
            Self::BadChallenge => "the server's NTLM challenge is not one",
            Self::TooLarge => "the server's NTLM challenge is longer than any server sends",
            Self::NotUnicode => "the server will not sign in over NTLM with Unicode names",
            Self::Internal => "the NTLM message could not be made",
        })
    }
}

impl std::error::Error for NtlmError {}

/// Who signs in: the user name and domain as the server knows them, and the key that follows from
/// the password. The password itself is not kept, and nothing here shows the key.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    domain: String,
    user: String,
    /// `NTOWFv2(password, user, domain)`: all the password is ever used for.
    response_key: [u8; 16],
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials(<redacted>)")
    }
}

impl Credentials {
    /// `username` is `DOMAIN\user` or a user principal name (`user@example.org`, sent as the user
    /// name with no domain, as Windows does).
    pub fn new(username: &str, password: &str) -> Result<Credentials, NtlmError> {
        let (domain, user) = match username.split_once('\\') {
            Some((domain, user)) => (domain, user),
            None => ("", username),
        };
        if user.is_empty() {
            return Err(NtlmError::NoUserName);
        }
        let unusable = |text: &str| text.len() > MAX_PART || text.chars().any(char::is_control);
        if unusable(domain) || unusable(user) || unusable(password) || user.contains('\\') {
            return Err(NtlmError::Unusable);
        }
        let response_key = ntowf_v2(password, user, domain)?;
        Ok(Credentials { domain: domain.to_string(), user: user.to_string(), response_key })
    }
}

/// What the host knows at the time of the third leg.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    /// Eight random bytes from a CSPRNG: the client challenge. Never reused.
    pub client_challenge: [u8; 8],
    /// The time now, in seconds since 1970, for a server that sent no timestamp of its own.
    pub unix_seconds: u64,
    /// The host the connection is to, for the target name (`HTTP/host`).
    pub host: &'a str,
    /// The hash of the server's certificate, as `tls-server-end-point` (RFC 5929) defines it, when
    /// the host has it; none to send no binding.
    pub channel_binding: Option<&'a [u8]>,
}

impl fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context").field("bound", &self.channel_binding.is_some()).finish()
    }
}

/// The first leg sent, and what the third needs of it.
#[derive(Clone)]
pub struct Handshake {
    response_key: [u8; 16],
    domain: String,
    user: String,
    negotiate: Vec<u8>,
}

impl fmt::Debug for Handshake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Handshake(<redacted>)")
    }
}

/// What a leg of an exchange brought back: the status and every `WWW-Authenticate`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leg {
    /// The status code.
    pub status: u16,
    /// Every `WWW-Authenticate` header, as the server wrote them.
    pub challenges: Vec<String>,
}

/// Why [`sign_in`] stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum SignInError<E> {
    /// The exchange failed (the connection, most likely).
    Exchange(E),
    /// The server answered the NEGOTIATE leg with this status instead of the 401 and a challenge
    /// that was asked for.
    Unexpected(u16),
    /// The server's answer cannot be used.
    Ntlm(NtlmError),
}

impl<E> fmt::Display for SignInError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exchange(_) => f.write_str("the connection failed during the NTLM sign-in"),
            Self::Unexpected(status) => write!(f, "the server answered {status} to the NTLM sign-in instead of a challenge"),
            Self::Ntlm(error) => error.fmt(f),
        }
    }
}

/// The first two legs of an NTLM sign-in, and the `Authorization` value of the third.
///
/// `exchange` sends one request **on the held connection** with the `Authorization` value it is
/// given (a request with no body: `Content-Length: 0`) and returns the status and the
/// `WWW-Authenticate` headers of the answer. It is called once, with the NEGOTIATE message. The
/// host then sends its real request on the same connection with the value returned here; an
/// answer of 401 to that is a refused sign-in (a wrong user name or password, most likely). No
/// further request on the connection needs an `Authorization`.
pub fn sign_in<E>(
    credentials: &Credentials,
    context: &Context<'_>,
    exchange: impl FnOnce(&Secret) -> Result<Leg, E>,
) -> Result<Secret, SignInError<E>> {
    let (handshake, negotiate) = Handshake::negotiate(credentials);
    let leg = exchange(&negotiate).map_err(SignInError::Exchange)?;
    if leg.status != 401 {
        return Err(SignInError::Unexpected(leg.status));
    }
    let token = auth::ntlm_token(&leg.challenges).ok_or(SignInError::Ntlm(NtlmError::BadChallenge))?;
    handshake.authenticate(token, context).map_err(SignInError::Ntlm)
}

impl Handshake {
    /// The first leg: the `Authorization` value `NTLM <NEGOTIATE message>`, and the state the
    /// third needs.
    pub fn negotiate(credentials: &Credentials) -> (Handshake, Secret) {
        let negotiate = negotiate_message();
        let header = Secret::new(&format!("NTLM {}", base64(&negotiate)));
        let handshake =
            Handshake { response_key: credentials.response_key, domain: credentials.domain.clone(), user: credentials.user.clone(), negotiate };
        (handshake, header)
    }

    /// The third leg: the `Authorization` value `NTLM <AUTHENTICATE message>`, from the Base64
    /// token of the server's challenge (`NTLM <token>` of its `WWW-Authenticate`).
    pub fn authenticate(self, token: &str, context: &Context<'_>) -> Result<Secret, NtlmError> {
        let raw = unbase64(token).ok_or(NtlmError::BadChallenge)?;
        let challenge = Challenge::parse(&raw)?;
        if challenge.flags & NEGOTIATE_UNICODE == 0 {
            return Err(NtlmError::NotUnicode);
        }
        let timestamp = challenge.timestamp();
        let time = timestamp.unwrap_or_else(|| filetime(context.unix_seconds));
        let target_info = challenge.reply_target_info(timestamp.is_some(), context);
        let (nt, session_key) = nt_response(&self.response_key, &challenge.server_challenge, &context.client_challenge, &time, &target_info)?;
        let lm = if timestamp.is_some() {
            vec![0u8; 24]
        } else {
            let mut lm = hmac_md5(&self.response_key, &[&challenge.server_challenge, &context.client_challenge])?.to_vec();
            lm.extend_from_slice(&context.client_challenge);
            lm
        };
        let domain = utf16(&self.domain);
        let user = utf16(&self.user);
        let parts = Parts { flags: OFFERED & challenge.flags, lm: &lm, nt: &nt, domain: &domain, user: &user, workstation: &[], session_key: &[] };
        let mic = timestamp.is_some();
        let mut message = authenticate_message(&parts, mic)?;
        if mic {
            // The MIC is made over the three messages with its own field zero, then put in.
            let sum = hmac_md5(&session_key, &[&self.negotiate, &raw, &message])?;
            let field = message.get_mut(MIC_AT..MIC_AT.saturating_add(16)).ok_or(NtlmError::Internal)?;
            field.copy_from_slice(&sum);
        }
        Ok(Secret::new(&format!("NTLM {}", base64(&message))))
    }
}

/// The NEGOTIATE message: the flags, and no domain, no workstation and no version.
fn negotiate_message() -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(SIGNATURE);
    out.extend_from_slice(&NEGOTIATE_MESSAGE.to_le_bytes());
    out.extend_from_slice(&OFFERED.to_le_bytes());
    // DomainNameFields and WorkstationFields: empty, at the end of the message.
    for _ in 0..2 {
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&32u32.to_le_bytes());
    }
    out
}

/// The pieces of an AUTHENTICATE message.
struct Parts<'a> {
    flags: u32,
    lm: &'a [u8],
    nt: &'a [u8],
    domain: &'a [u8],
    user: &'a [u8],
    workstation: &'a [u8],
    session_key: &'a [u8],
}

/// An AUTHENTICATE message (2.2.1.3): the fixed fields, a zero version, a zero MIC when `mic`,
/// then the payload in the order Domain, User, Workstation, LM, NT, session key.
fn authenticate_message(parts: &Parts<'_>, mic: bool) -> Result<Vec<u8>, NtlmError> {
    let header = if mic { MIC_AT.saturating_add(16) } else { MIC_AT };
    let payload = [parts.domain, parts.user, parts.workstation, parts.lm, parts.nt, parts.session_key];
    let mut at = header;
    let mut places = Vec::with_capacity(payload.len());
    for field in payload {
        places.push((field.len(), at));
        at = at.saturating_add(field.len());
    }
    let mut out = Vec::with_capacity(at);
    out.extend_from_slice(SIGNATURE);
    out.extend_from_slice(&AUTHENTICATE_MESSAGE.to_le_bytes());
    // The fixed fields come in the order LM, NT, Domain, User, Workstation, session key.
    for which in [3usize, 4, 0, 1, 2, 5] {
        let (len, offset) = places.get(which).copied().ok_or(NtlmError::Internal)?;
        let len = u16::try_from(len).map_err(|_| NtlmError::Internal)?;
        let offset = u32::try_from(offset).map_err(|_| NtlmError::Internal)?;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
    }
    out.extend_from_slice(&parts.flags.to_le_bytes());
    out.extend_from_slice(&[0u8; 8]);
    if mic {
        out.extend_from_slice(&[0u8; 16]);
    }
    for field in payload {
        out.extend_from_slice(field);
    }
    Ok(out)
}

/// A CHALLENGE message (2.2.1.2), as far as a client needs it.
struct Challenge {
    flags: u32,
    server_challenge: [u8; 8],
    /// The AV pairs of the target information, without the end marker.
    pairs: Vec<(u16, Vec<u8>)>,
    /// Whether the server sent target information at all.
    has_target_info: bool,
}

impl Challenge {
    fn parse(raw: &[u8]) -> Result<Challenge, NtlmError> {
        let bad = NtlmError::BadChallenge;
        if raw.get(..8) != Some(&SIGNATURE[..]) || u32_at(raw, 8) != Some(CHALLENGE_MESSAGE) {
            return Err(bad);
        }
        let flags = u32_at(raw, 20).ok_or(bad)?;
        let server_challenge = <[u8; 8]>::try_from(raw.get(24..32).ok_or(bad)?).map_err(|_| bad)?;
        // A message that ends after the reserved field has no target information.
        let info = match (u16_at(raw, 40), u32_at(raw, 44)) {
            (Some(len), Some(offset)) => {
                let len = usize::from(len);
                let start = usize::try_from(offset).map_err(|_| bad)?;
                let end = start.checked_add(len).ok_or(bad)?;
                Some(raw.get(start..end).ok_or(bad)?)
            }
            _ => None,
        };
        let (pairs, has_target_info) = match info {
            Some(info) if !info.is_empty() => (parse_pairs(info)?, true),
            _ => (Vec::new(), false),
        };
        Ok(Challenge { flags, server_challenge, pairs, has_target_info })
    }

    /// The server's own time (`MsvAvTimestamp`), when it sent it: the first such pair that is
    /// well formed (eight bytes). A malformed one is passed over, as if it were not there.
    fn timestamp(&self) -> Option<[u8; 8]> {
        self.pairs.iter().filter(|(id, _)| *id == AV_TIMESTAMP).find_map(|(_, value)| <[u8; 8]>::try_from(value.as_slice()).ok())
    }

    /// The target information that goes back, end marker included (empty when the server sent
    /// none): the server's pairs, `MsvAvFlags` with the MIC bit when there is a MIC, and the
    /// channel binding and the target name (3.1.5.1.2).
    fn reply_target_info(&self, mic: bool, context: &Context<'_>) -> Vec<u8> {
        if !self.has_target_info {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut flagged = false;
        for (id, value) in &self.pairs {
            match *id {
                // Ours replace anything the server put there.
                AV_CHANNEL_BINDINGS | AV_TARGET_NAME => {}
                AV_FLAGS if value.len() == 4 => {
                    let old = <[u8; 4]>::try_from(value.as_slice()).map(u32::from_le_bytes).unwrap_or(0);
                    flagged = true;
                    push_pair(&mut out, AV_FLAGS, &(if mic { old | FLAG_MIC } else { old }).to_le_bytes());
                }
                _ => push_pair(&mut out, *id, value),
            }
        }
        if mic && !flagged {
            push_pair(&mut out, AV_FLAGS, &FLAG_MIC.to_le_bytes());
        }
        push_pair(&mut out, AV_CHANNEL_BINDINGS, &channel_bindings(context.channel_binding));
        let spn = if context.host.is_empty() { Vec::new() } else { utf16(&format!("HTTP/{}", context.host)) };
        push_pair(&mut out, AV_TARGET_NAME, &spn);
        push_pair(&mut out, AV_EOL, &[]);
        out
    }
}

/// The AV pairs of `info`, up to the end marker. A pair that runs past the end, too many pairs
/// or too much information is refused.
pub(crate) fn parse_pairs(info: &[u8]) -> Result<Vec<(u16, Vec<u8>)>, NtlmError> {
    if info.len() > MAX_TARGET_INFO {
        return Err(NtlmError::TooLarge);
    }
    let mut pairs = Vec::new();
    let mut at = 0usize;
    // A list that ends without its end marker is read as far as it goes.
    while let (Some(id), Some(len)) = (u16_at(info, at), u16_at(info, at.saturating_add(2))) {
        if id == AV_EOL {
            break;
        }
        let start = at.saturating_add(4);
        let end = start.checked_add(usize::from(len)).ok_or(NtlmError::BadChallenge)?;
        let value = info.get(start..end).ok_or(NtlmError::BadChallenge)?;
        if pairs.len() >= MAX_PAIRS {
            return Err(NtlmError::TooLarge);
        }
        pairs.push((id, value.to_vec()));
        at = end;
    }
    Ok(pairs)
}

fn push_pair(out: &mut Vec<u8>, id: u16, value: &[u8]) {
    // (No value is longer than the 16 KiB taken from the server, and ours are short.)
    let len = u16::try_from(value.len()).unwrap_or(u16::MAX);
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(value.get(..usize::from(len)).unwrap_or(&[]));
}

/// `MsvAvChannelBindings`: the MD5 of the `gss_channel_bindings_struct` (RFC 4121 4.1.1.2) whose
/// application data is `tls-server-end-point:` and the certificate hash; sixteen zero bytes when
/// there is no binding.
fn channel_bindings(end_point_hash: Option<&[u8]>) -> [u8; 16] {
    let Some(hash) = end_point_hash else { return [0; 16] };
    let data = [b"tls-server-end-point:".as_slice(), hash].concat();
    let mut structure = vec![0u8; 16];
    structure.extend_from_slice(&u32::try_from(data.len()).unwrap_or(u32::MAX).to_le_bytes());
    structure.extend_from_slice(&data);
    Md5::digest(&structure).into()
}

/// `NTOWFv2(password, user, domain)` (3.3.2): `HMAC_MD5(MD4(UNICODE(password)), UNICODE(UPPER(user) + domain))`.
pub(crate) fn ntowf_v2(password: &str, user: &str, domain: &str) -> Result<[u8; 16], NtlmError> {
    let hash: [u8; 16] = Md4::digest(utf16(password)).into();
    // Windows upper-cases one character to one character: a character whose full Unicode
    // upper case is several (the German sharp s becomes "SS" in Rust) is kept as it is.
    let upper: String = user
        .chars()
        .map(|c| {
            let mut mapped = c.to_uppercase();
            match (mapped.next(), mapped.next()) {
                (Some(one), None) => one,
                _ => c,
            }
        })
        .collect();
    let name = utf16(&format!("{upper}{domain}"));
    hmac_md5(&hash, &[&name])
}

/// The NT challenge response and the session base key (`ComputeResponse`, 3.3.2):
/// `NTProofStr ‖ temp`, with `temp = 01 01 ‖ Z(6) ‖ time ‖ client challenge ‖ Z(4) ‖ target info ‖ Z(4)`.
fn nt_response(
    response_key: &[u8; 16],
    server_challenge: &[u8; 8],
    client_challenge: &[u8; 8],
    time: &[u8; 8],
    target_info: &[u8],
) -> Result<(Vec<u8>, [u8; 16]), NtlmError> {
    let mut temp = vec![1u8, 1, 0, 0, 0, 0, 0, 0];
    temp.extend_from_slice(time);
    temp.extend_from_slice(client_challenge);
    temp.extend_from_slice(&[0; 4]);
    temp.extend_from_slice(target_info);
    temp.extend_from_slice(&[0; 4]);
    let proof = hmac_md5(response_key, &[server_challenge, &temp])?;
    let session_base_key = hmac_md5(response_key, &[&proof])?;
    let mut response = proof.to_vec();
    response.extend_from_slice(&temp);
    Ok((response, session_base_key))
}

pub(crate) fn hmac_md5(key: &[u8], parts: &[&[u8]]) -> Result<[u8; 16], NtlmError> {
    let mut mac = <Hmac<Md5> as KeyInit>::new_from_slice(key).map_err(|_| NtlmError::Internal)?;
    for part in parts {
        mac.update(part);
    }
    Ok(mac.finalize().into_bytes().into())
}

/// A FILETIME (100-nanosecond ticks since 1601) from seconds since 1970, little-endian.
fn filetime(unix_seconds: u64) -> [u8; 8] {
    unix_seconds.saturating_add(FILETIME_EPOCH).saturating_mul(10_000_000).to_le_bytes()
}

pub(crate) fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

pub(crate) fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    let end = at.checked_add(2)?;
    Some(u16::from_le_bytes(<[u8; 2]>::try_from(bytes.get(at..end)?).ok()?))
}

pub(crate) fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    Some(u32::from_le_bytes(<[u8; 4]>::try_from(bytes.get(at..end)?).ok()?))
}

/// Standard Base64 (RFC 4648), padding optional; anything else, or a token over [`MAX_TOKEN`], is none.
pub(crate) fn unbase64(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let text = text.trim().trim_end_matches('=');
    if text.is_empty() || text.len() > MAX_TOKEN {
        return None;
    }
    let mut out = Vec::with_capacity(text.len().saturating_mul(3) / 4);
    for chunk in text.as_bytes().chunks(4) {
        if chunk.len() == 1 {
            return None;
        }
        let mut group = 0u32;
        for slot in 0..4 {
            let value = match chunk.get(slot) {
                Some(&b) => u32::try_from(ALPHABET.iter().position(|a| *a == b)?).ok()?,
                None => 0,
            };
            group = (group << 6) | value;
        }
        // Three bytes make four characters; fewer characters make fewer bytes.
        let bytes = group.to_be_bytes();
        out.extend_from_slice(bytes.get(1..chunk.len())?);
    }
    Some(out)
}

#[cfg(test)]
#[path = "ntlm_tests.rs"]
mod ntlm_tests;
