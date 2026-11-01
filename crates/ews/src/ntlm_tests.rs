// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! NTLMv2 (#252, E4): the sample values and messages of MS-NLMP 4.2.4, a full three-leg sign-in
//! against a verifier that holds the password, and a hostile CHALLENGE.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::arithmetic_side_effects)]

use proptest::prelude::*;

use super::*;

fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(|b| b.is_ascii_hexdigit()).collect();
    digits.chunks(2).map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect()
}

/// The CHALLENGE_MESSAGE of 4.2.4.3.
fn spec_challenge() -> Vec<u8> {
    hex("4e544c4d535350000200000 00c000c00 38000000 33828ae2 0123456789abcdef
         0000000000000000 24002400 44000000 060070170000000f
         53006500720076006500720002000c004400 6f006d00610069006e00 01000c0053006500720076006500720000000000")
}

/// The AUTHENTICATE_MESSAGE of 4.2.4.3.
fn spec_authenticate() -> Vec<u8> {
    hex("4e544c4d5353500003000000 18001800 6c000000 54005400 84000000 0c000c00 48000000 08000800 54000000
         10001000 5c000000 10001000 d8000000 358288e2 0501280a0000000f
         44006f006d00610069006e00 5500730065007200 43004f004d0050005500540045005200
         86c35097ac9cec102554764a57cccc19aaaaaaaaaaaaaaaa
         68cd0ab851e51c96aabc927bebef6a1c
         0101000000000000 0000000000000000 aaaaaaaaaaaaaaaa 00000000
         02000c0044006f006d00610069006e00 01000c00530065007200760065007200 00000000 00000000
         c5dad2544fc9799094ce1ce90bc9d03e")
}

fn spec_password_key() -> [u8; 16] {
    ntowf_v2("Password", "User", "Domain").unwrap()
}

#[test]
fn ntowf_v2_matches_the_sample() {
    assert_eq!(spec_password_key().to_vec(), hex("0c868a403bfd7a93a3001ef22ef02e3f"));
}

#[test]
fn the_challenge_of_the_sample_is_read() {
    let raw = spec_challenge();
    let challenge = Challenge::parse(&raw).unwrap();
    assert_eq!(challenge.flags, 0xe28a_8233);
    assert_eq!(challenge.server_challenge.to_vec(), hex("0123456789abcdef"));
    assert!(challenge.has_target_info);
    assert_eq!(challenge.pairs.len(), 2);
    assert_eq!(challenge.pairs.first().map(|p| p.0), Some(2));
    assert_eq!(challenge.timestamp(), None);
}

#[test]
fn nt_response_and_session_key_match_the_sample() {
    let raw = spec_challenge();
    // The target information as the server sent it (from byte 0x44, 0x24 long), end marker included.
    let target_info = &raw[0x44..0x44 + 0x24];
    let challenge = Challenge::parse(&raw).unwrap();
    let (response, session_key) = nt_response(&spec_password_key(), &challenge.server_challenge, &[0xaa; 8], &[0; 8], target_info).unwrap();
    assert_eq!(response[..16].to_vec(), hex("68cd0ab851e51c96aabc927bebef6a1c"));
    assert_eq!(session_key.to_vec(), hex("8de40ccadbc14a82f15cb0ad0de95ca3"));
    // And `temp` is the sample's.
    let temp = hex("0101000000000000 0000000000000000 aaaaaaaaaaaaaaaa 00000000
        02000c0044006f006d00610069006e00 01000c00530065007200760065007200 00000000 00000000");
    assert_eq!(response[16..].to_vec(), temp);
}

#[test]
fn lmv2_response_matches_the_sample() {
    let lm = hmac_md5(&spec_password_key(), &[&hex("0123456789abcdef"), &[0xaa; 8]]).unwrap();
    assert_eq!(lm.to_vec(), hex("86c35097ac9cec102554764a57cccc19"));
}

/// RC4 (for this test alone: the client does not negotiate key exchange, so the program has none).
fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: Vec<u8> = (0..=255).collect();
    let mut j = 0usize;
    for i in 0..256 {
        j = (j + usize::from(s[i]) + usize::from(key[i % key.len()])) % 256;
        s.swap(i, j);
    }
    let (mut i, mut j) = (0usize, 0usize);
    data.iter()
        .map(|byte| {
            i = (i + 1) % 256;
            j = (j + usize::from(s[i])) % 256;
            s.swap(i, j);
            byte ^ s[(usize::from(s[i]) + usize::from(s[j])) % 256]
        })
        .collect()
}

#[test]
fn the_authenticate_message_of_the_sample_is_made_byte_for_byte() {
    let raw = spec_challenge();
    let target_info = &raw[0x44..0x44 + 0x24];
    let challenge = Challenge::parse(&raw).unwrap();
    let (nt, session_base_key) = nt_response(&spec_password_key(), &challenge.server_challenge, &[0xaa; 8], &[0; 8], target_info).unwrap();
    let mut lm = hmac_md5(&spec_password_key(), &[&challenge.server_challenge, &[0xaa; 8]]).unwrap().to_vec();
    lm.extend_from_slice(&[0xaa; 8]);
    // 4.2.4.2.3: RC4 of the random session key (sixteen 0x55) under the key exchange key.
    let encrypted = rc4(&session_base_key, &[0x55; 16]);
    assert_eq!(encrypted, hex("c5dad2544fc9799094ce1ce90bc9d03e"));
    let parts = Parts {
        flags: 0xe288_8235,
        lm: &lm,
        nt: &nt,
        domain: &utf16("Domain"),
        user: &utf16("User"),
        workstation: &utf16("COMPUTER"),
        session_key: &encrypted,
    };
    // The sample has no version (0501280a0000000f is its version, which this program leaves zero),
    // so the sample is compared around it.
    let mine = authenticate_message(&parts, false).unwrap();
    let theirs = spec_authenticate();
    assert_eq!(mine.len(), theirs.len());
    assert_eq!(mine[..0x40], theirs[..0x40]);
    assert_eq!(mine[0x48..], theirs[0x48..]);
    assert_eq!(mine[0x40..0x48], [0u8; 8]);
}

#[test]
fn the_sample_challenge_is_answered_and_the_server_can_check_it() {
    let credentials = Credentials::new("Domain\\User", "Password").unwrap();
    let (handshake, first) = Handshake::negotiate(&credentials);
    assert!(first.expose().starts_with("NTLM "));
    let negotiate = unbase64(&first.expose()[5..]).unwrap();
    assert_eq!(negotiate.len(), 32);
    assert_eq!(&negotiate[..12], b"NTLMSSP\0\x01\0\0\0");
    let flags = u32_at(&negotiate, 12).unwrap();
    for wanted in [NEGOTIATE_UNICODE, NEGOTIATE_NTLM, NEGOTIATE_EXTENDED_SESSIONSECURITY, NEGOTIATE_128] {
        assert!(flags & wanted != 0);
    }
    // Nothing that would sign, seal or exchange a key, and no machine name or version.
    assert_eq!(flags & 0x0000_0030, 0, "sign or seal");
    assert_eq!(flags & 0x4000_0000, 0, "key exchange");
    assert_eq!(flags & 0x0200_0000 | flags & 0x0000_2000 | flags & 0x0000_1000, 0, "version, workstation, domain");
    let context = Context { client_challenge: [0xaa; 8], unix_seconds: 0, host: "mail.example.org", channel_binding: None };
    let token = base64(&spec_challenge());
    let answer = handshake.authenticate(&token, &context).unwrap();
    let message = unbase64(&answer.expose()[5..]).unwrap();
    // Without a server timestamp: no MIC (a 72-byte header), the LMv2 response, and our time.
    assert_eq!(u32_at(&message, 8), Some(3));
    let lm_len = usize::from(u16_at(&message, 12).unwrap());
    let lm_at = usize::try_from(u32_at(&message, 16).unwrap()).unwrap();
    assert_eq!(lm_len, 24);
    assert_eq!(message[lm_at..lm_at + 16], hex("86c35097ac9cec102554764a57cccc19")[..]);
}

/// A CHALLENGE with `pairs` as its target information (an end marker is added).
fn challenge_with(flags: u32, pairs: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let mut info = Vec::new();
    for (id, value) in pairs {
        info.extend_from_slice(&id.to_le_bytes());
        info.extend_from_slice(&u16::try_from(value.len()).unwrap().to_le_bytes());
        info.extend_from_slice(value);
    }
    info.extend_from_slice(&[0, 0, 0, 0]);
    let mut out = Vec::new();
    out.extend_from_slice(b"NTLMSSP\0");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 56, 0, 0, 0]);
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&u16::try_from(info.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&u16::try_from(info.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&56u32.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&info);
    out
}

/// What a server that holds the password checks of an AUTHENTICATE message: the NT proof, the MIC
/// when it is claimed, and the pairs of the client's target information.
struct Verified {
    pairs: Vec<(u16, Vec<u8>)>,
    mic_checked: bool,
    lm_is_zero: bool,
}

fn server_verifies(password: &str, user: &str, domain: &str, negotiate: &[u8], challenge: &[u8], message: &[u8]) -> Verified {
    let key = ntowf_v2(password, user, domain).unwrap();
    let field = |at: usize| {
        let len = usize::from(u16_at(message, at).unwrap());
        let offset = usize::try_from(u32_at(message, at + 4).unwrap()).unwrap();
        &message[offset..offset + len]
    };
    let nt = field(20);
    let (proof, temp) = nt.split_at(16);
    let server_challenge = &challenge[24..32];
    assert_eq!(hmac_md5(&key, &[server_challenge, temp]).unwrap().to_vec(), proof);
    let session_key = hmac_md5(&key, &[proof]).unwrap();
    // The client's target information is in `temp` after 28 bytes, up to the last four.
    let info = &temp[28..temp.len() - 4];
    let pairs = parse_pairs(info).unwrap();
    let claims_mic = pairs.iter().any(|(id, v)| *id == AV_FLAGS && u32::from_le_bytes(v[..4].try_into().unwrap()) & FLAG_MIC != 0);
    let mut mic_checked = false;
    if claims_mic {
        let mut zeroed = message.to_vec();
        let mic = zeroed[MIC_AT..MIC_AT + 16].to_vec();
        zeroed[MIC_AT..MIC_AT + 16].fill(0);
        assert_eq!(hmac_md5(&session_key, &[negotiate, challenge, &zeroed]).unwrap().to_vec(), mic);
        mic_checked = true;
    }
    Verified { pairs, mic_checked, lm_is_zero: field(12).iter().all(|b| *b == 0) }
}

fn run(password: &str, username: &str, challenge: &[u8], binding: Option<&[u8]>) -> (Vec<u8>, Vec<u8>) {
    let credentials = Credentials::new(username, password).unwrap();
    let (handshake, first) = Handshake::negotiate(&credentials);
    let negotiate = unbase64(&first.expose()[5..]).unwrap();
    let context = Context { client_challenge: [7; 8], unix_seconds: 1_800_000_000, host: "mail.example.org", channel_binding: binding };
    let answer = handshake.authenticate(&base64(challenge), &context).unwrap();
    (negotiate, unbase64(&answer.expose()[5..]).unwrap())
}

#[test]
fn a_server_timestamp_brings_the_mic_the_binding_the_target_name_and_a_zero_lm() {
    let timestamp = vec![1, 2, 3, 4, 5, 6, 7, 8];
    let challenge = challenge_with(
        0xe28a_8233,
        &[(2, utf16("EXAMPLE")), (1, utf16("MAIL")), (AV_FLAGS, 1u32.to_le_bytes().to_vec()), (AV_TIMESTAMP, timestamp.clone())],
    );
    let (negotiate, message) = run("S3cret!", "EXAMPLE\\ada", &challenge, Some(&[9; 32]));
    let verified = server_verifies("S3cret!", "ada", "EXAMPLE", &negotiate, &challenge, &message);
    assert!(verified.mic_checked);
    assert!(verified.lm_is_zero);
    // The server's own flags are kept (constrained: 1) with the MIC bit added.
    let flags = verified.pairs.iter().find(|p| p.0 == AV_FLAGS).unwrap();
    assert_eq!(flags.1, 3u32.to_le_bytes().to_vec());
    // The server's time is the time used (it is inside `temp`; the verifier read it back).
    let expected = channel_bindings(Some(&[9; 32]));
    assert_eq!(verified.pairs.iter().find(|p| p.0 == AV_CHANNEL_BINDINGS).unwrap().1, expected.to_vec());
    assert_eq!(verified.pairs.iter().find(|p| p.0 == AV_TARGET_NAME).unwrap().1, utf16("HTTP/mail.example.org"));
    assert_ne!(expected, [0; 16]);
}

#[test]
fn a_server_without_a_timestamp_gets_lmv2_no_mic_our_time_and_a_zero_binding() {
    let challenge = challenge_with(0xe28a_8233, &[(2, utf16("EXAMPLE")), (1, utf16("MAIL"))]);
    let (negotiate, message) = run("S3cret!", "ada@example.org", &challenge, None);
    // A user principal name is sent as the user name, with no domain.
    let verified = server_verifies("S3cret!", "ada@example.org", "", &negotiate, &challenge, &message);
    assert!(!verified.mic_checked);
    assert!(!verified.lm_is_zero);
    assert_eq!(verified.pairs.iter().find(|p| p.0 == AV_CHANNEL_BINDINGS).unwrap().1, vec![0u8; 16]);
    assert!(verified.pairs.iter().all(|p| p.0 != AV_FLAGS));
}

#[test]
fn the_time_of_the_clock_is_a_filetime() {
    // 2027-01-15 08:00:00 UTC = 1_800_000_000 seconds, 11_644_473_600 + that, in 100 ns ticks.
    assert_eq!(u64::from_le_bytes(filetime(1_800_000_000)), 13_444_473_600 * 10_000_000);
    assert_eq!(u64::from_le_bytes(filetime(0)), 116_444_736_000_000_000);
    assert_eq!(u64::from_le_bytes(filetime(u64::MAX)), u64::MAX);
}

#[test]
fn a_server_without_target_information_is_answered_with_none() {
    let mut challenge = challenge_with(0xe28a_8233, &[]);
    challenge.truncate(32);
    let (_, message) = run("pw", "u", &challenge, None);
    assert_eq!(u32_at(&message, 8), Some(3));
}

#[test]
fn a_server_that_does_not_speak_unicode_is_refused() {
    let challenge = challenge_with(0x0000_0002, &[(1, utf16("MAIL"))]);
    let credentials = Credentials::new("u", "p").unwrap();
    let (handshake, _) = Handshake::negotiate(&credentials);
    let context = Context { client_challenge: [0; 8], unix_seconds: 0, host: "h", channel_binding: None };
    assert_eq!(handshake.authenticate(&base64(&challenge), &context).unwrap_err(), NtlmError::NotUnicode);
}

#[test]
fn user_names_are_split_and_checked() {
    assert!(Credentials::new("DOMAIN\\ada", "pw").is_ok());
    assert!(Credentials::new("ada@example.org", "pw").is_ok());
    assert_eq!(Credentials::new("", "pw").unwrap_err(), NtlmError::NoUserName);
    assert_eq!(Credentials::new("DOMAIN\\", "pw").unwrap_err(), NtlmError::NoUserName);
    assert_eq!(Credentials::new("a\\b\\c", "pw").unwrap_err(), NtlmError::Unusable);
    assert_eq!(Credentials::new("ada", "p\r\nw").unwrap_err(), NtlmError::Unusable);
    assert_eq!(Credentials::new(&"a".repeat(2_000), "pw").unwrap_err(), NtlmError::Unusable);
    // An empty password is a password.
    assert!(Credentials::new("ada", "").is_ok());
}

#[test]
fn nothing_secret_is_shown() {
    let credentials = Credentials::new("EXAMPLE\\ada", "hunter2-hunter2").unwrap();
    let (handshake, header) = Handshake::negotiate(&credentials);
    let context = Context { client_challenge: [0x42; 8], unix_seconds: 0, host: "mail.example.org", channel_binding: Some(&[1; 32]) };
    let shown = format!("{credentials:?} {handshake:?} {header:?} {context:?}");
    for secret in ["hunter2", "ada", "EXAMPLE", "mail.example.org", "NTLM"] {
        assert!(!shown.contains(secret), "{shown}");
    }
    let challenge = challenge_with(0xe28a_8233, &[(1, utf16("MAIL"))]);
    let answer = handshake.authenticate(&base64(&challenge), &context).unwrap();
    assert!(!format!("{answer:?}").contains("NTLM"));
    for error in
        [NtlmError::NoUserName, NtlmError::Unusable, NtlmError::BadChallenge, NtlmError::TooLarge, NtlmError::NotUnicode, NtlmError::Internal]
    {
        let text = format!("{error} {error:?}");
        assert!(!text.contains("hunter2") && !text.contains("ada"));
    }
}

#[test]
fn sign_in_runs_the_first_two_legs_and_returns_the_third() {
    let credentials = Credentials::new("EXAMPLE\\ada", "pw").unwrap();
    let context = Context { client_challenge: [3; 8], unix_seconds: 1, host: "mail.example.org", channel_binding: None };
    let challenge = challenge_with(0xe28a_8233, &[(2, utf16("EXAMPLE")), (1, utf16("MAIL"))]);
    let mut sent = Vec::new();
    let third = sign_in(&credentials, &context, |value: &Secret| -> Result<Leg, ()> {
        sent.push(value.expose().to_string());
        Ok(Leg { status: 401, challenges: vec!["Negotiate".into(), format!("NTLM {}", base64(&challenge))] })
    })
    .unwrap();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].starts_with("NTLM "));
    assert!(third.expose().starts_with("NTLM "));
    assert_ne!(third.expose(), sent[0]);
}

#[test]
fn sign_in_stops_on_what_is_not_a_challenge() {
    let credentials = Credentials::new("ada", "pw").unwrap();
    let context = Context { client_challenge: [3; 8], unix_seconds: 1, host: "h", channel_binding: None };
    let answer =
        |status: u16, challenges: Vec<String>| sign_in(&credentials, &context, |_: &Secret| -> Result<Leg, ()> { Ok(Leg { status, challenges }) });
    assert_eq!(answer(200, vec![]).unwrap_err(), SignInError::Unexpected(200));
    assert_eq!(answer(403, vec![]).unwrap_err(), SignInError::Unexpected(403));
    assert_eq!(answer(401, vec!["NTLM".into()]).unwrap_err(), SignInError::Ntlm(NtlmError::BadChallenge));
    assert_eq!(answer(401, vec!["NTLM !!!!".into()]).unwrap_err(), SignInError::Ntlm(NtlmError::BadChallenge));
    let failed = sign_in(&credentials, &context, |_: &Secret| -> Result<Leg, &'static str> { Err("closed") });
    assert_eq!(failed.unwrap_err(), SignInError::Exchange("closed"));
}

#[test]
fn base64_round_trips() {
    for len in 0..40usize {
        let bytes: Vec<u8> = (0..len).map(|n| u8::try_from(n * 7 % 256).unwrap()).collect();
        if bytes.is_empty() {
            assert_eq!(unbase64(&base64(&bytes)), None);
        } else {
            assert_eq!(unbase64(&base64(&bytes)), Some(bytes.clone()));
            assert_eq!(unbase64(base64(&bytes).trim_end_matches('=')), Some(bytes));
        }
    }
    assert_eq!(unbase64("A"), None);
    assert_eq!(unbase64("ab!d"), None);
    assert_eq!(unbase64(&"A".repeat(MAX_TOKEN + 4)), None);
}

#[test]
fn hostile_target_information_is_refused() {
    // Too many pairs.
    let many: Vec<(u16, Vec<u8>)> = (0..=MAX_PAIRS).map(|_| (1u16, vec![0u8; 2])).collect();
    assert_eq!(Challenge::parse(&challenge_with(0xe28a_8233, &many)).err(), Some(NtlmError::TooLarge));
    // Too much information.
    let big = vec![(1u16, vec![0u8; 60_000])];
    assert_eq!(Challenge::parse(&challenge_with(0xe28a_8233, &big)).err(), Some(NtlmError::TooLarge));
    // A pair that runs past the end.
    let mut cut = challenge_with(0xe28a_8233, &[(1, vec![0u8; 8])]);
    cut.truncate(cut.len() - 8);
    assert!(Challenge::parse(&cut).is_err());
    // An offset past the end, and one that wraps.
    let mut far = challenge_with(0xe28a_8233, &[(1, vec![0u8; 8])]);
    far[44..48].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Challenge::parse(&far).is_err());
    // Not a challenge.
    assert!(Challenge::parse(&[]).is_err());
    assert!(Challenge::parse(b"NTLMSSP\0\x01\0\0\0").is_err());
    let mut wrong = challenge_with(0xe28a_8233, &[]);
    wrong[8] = 3;
    assert!(Challenge::parse(&wrong).is_err());
}

proptest! {
    #[test]
    fn arbitrary_bytes_are_never_a_panic(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        let _ = Challenge::parse(&bytes);
        let _ = unbase64(&String::from_utf8_lossy(&bytes));
    }

    #[test]
    fn a_challenge_shaped_noise_is_never_a_panic(
        flags in any::<u32>(),
        tail in proptest::collection::vec(any::<u8>(), 0..300),
        len in any::<u16>(),
        offset in any::<u32>(),
        timestamp in any::<bool>(),
    ) {
        let mut raw = challenge_with(flags, &if timestamp { vec![(AV_TIMESTAMP, vec![1; 8])] } else { vec![] });
        raw.truncate(40);
        raw.extend_from_slice(&len.to_le_bytes());
        raw.extend_from_slice(&len.to_le_bytes());
        raw.extend_from_slice(&offset.to_le_bytes());
        raw.extend_from_slice(&[0; 8]);
        raw.extend_from_slice(&tail);
        let credentials = Credentials::new("EXAMPLE\\ada", "pw").unwrap();
        let (handshake, _) = Handshake::negotiate(&credentials);
        let context = Context { client_challenge: [1; 8], unix_seconds: u64::MAX, host: "h", channel_binding: Some(&[0; 64]) };
        let _ = handshake.authenticate(&base64(&raw), &context);
    }

    #[test]
    fn arbitrary_text_is_never_a_panic_as_a_token_or_a_user(token in ".{0,200}", user in ".{0,40}", password in ".{0,40}") {
        if let Ok(credentials) = Credentials::new(&user, &password) {
            let (handshake, _) = Handshake::negotiate(&credentials);
            let context = Context { client_challenge: [1; 8], unix_seconds: 1, host: "", channel_binding: None };
            let _ = handshake.authenticate(&token, &context);
        }
    }
}

#[test]
fn the_channel_binding_hash_is_pinned_by_an_independent_vector() {
    // MD5(Z(16) ‖ 35 00 00 00 ‖ "tls-server-end-point:" ‖ 09 × 32), computed with Python's hashlib.
    assert_eq!(channel_bindings(Some(&[9; 32])).to_vec(), hex("b477d84bb5e1a11ab950338f6386aea0"));
    assert_eq!(channel_bindings(None), [0; 16]);
}

#[test]
fn a_pair_that_runs_past_the_target_information_is_refused() {
    // The target information's own length is consistent (it ends where the message ends), but the
    // pair inside says it is longer than what is left.
    let mut raw = challenge_with(0xe28a_8233, &[(1, utf16("MAIL"))]);
    raw[58..60].copy_from_slice(&200u16.to_le_bytes());
    assert_eq!(Challenge::parse(&raw).err(), Some(NtlmError::BadChallenge));
}

#[test]
fn the_user_name_is_upper_cased_one_character_to_one() {
    // "\u{df}" has the full upper case "SS", which Windows does not use: it stays.
    let kept = ntowf_v2("pw", "ma\u{df}", "D").unwrap();
    let mut same = [0u8; 16];
    same.copy_from_slice(&Md4::digest(utf16("pw")));
    assert_eq!(kept, hmac_md5(&same, &[&utf16("MA\u{df}D")]).unwrap());
    assert_ne!(kept, hmac_md5(&same, &[&utf16("MASSD")]).unwrap());
    assert_eq!(ntowf_v2("pw", "ada", "D").unwrap(), hmac_md5(&same, &[&utf16("ADAD")]).unwrap());
}

#[test]
fn only_a_well_formed_timestamp_is_taken() {
    let raw = challenge_with(0xe28a_8233, &[(AV_TIMESTAMP, vec![1, 2, 3]), (AV_TIMESTAMP, vec![9; 8])]);
    assert_eq!(Challenge::parse(&raw).unwrap().timestamp(), Some([9; 8]));
    let bad = challenge_with(0xe28a_8233, &[(AV_TIMESTAMP, vec![1, 2, 3])]);
    assert_eq!(Challenge::parse(&bad).unwrap().timestamp(), None);
}

#[test]
fn a_realistic_challenge_of_hundreds_of_characters_signs_in() {
    let challenge = challenge_with(
        0xe28a_8233,
        &[
            (2, utf16("EXAMPLE")),
            (1, utf16("MAIL01")),
            (4, utf16("example.org")),
            (3, utf16("mail01.corp.example.org")),
            (6, 0u32.to_le_bytes().to_vec()),
            (5, utf16("corp.example.org")),
            (AV_TIMESTAMP, vec![1; 8]),
        ],
    );
    let header = format!("NTLM {}", base64(&challenge));
    assert!(header.len() > 300, "{}", header.len());
    let credentials = Credentials::new("EXAMPLE\\ada", "pw").unwrap();
    let context = Context { client_challenge: [3; 8], unix_seconds: 1, host: "mail.example.org", channel_binding: Some(&[1; 32]) };
    let third = sign_in(&credentials, &context, |_: &Secret| -> Result<Leg, ()> { Ok(Leg { status: 401, challenges: vec![header.clone()] }) });
    assert!(third.is_ok());
}
