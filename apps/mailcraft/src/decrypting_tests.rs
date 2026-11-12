// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What is asked of gpg-agent to open an Encrypted message (#30 part 6). The keys and the
//! messages are the tests of `mailcraft-openpgp` and `mailcraft-smime`; these are the host's own
//! parts. Every value is invented.

use mailcraft_engine::decrypt::{DecryptJob, Failure};
use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_openpgp::AgentRequest;
use mailcraft_ui_egui::decrypting::DecryptHost;

use crate::decrypting::{enc_val, padding, signed};
use crate::key_holders::RsaPadding;

#[test]
fn an_rsa_value_with_its_top_bit_set_is_never_read_as_negative() {
    assert_eq!(signed(vec![0x80, 1]), vec![0, 0x80, 1]);
    assert_eq!(signed(vec![0x7F, 1]), vec![0x7F, 1]);
    assert!(signed(Vec::new()).is_empty());
}

#[test]
fn the_agent_is_asked_in_its_own_words() {
    let rsa = enc_val(AgentRequest::Rsa { a: vec![0x90, 2] }).to_canonical();
    assert_eq!(rsa, b"(7:enc-val(3:rsa(1:a3:\x00\x90\x02)))");
    let ecdh = enc_val(AgentRequest::Ecdh { s: vec![1, 2], e: vec![0x40, 3] }).to_canonical();
    assert_eq!(ecdh, b"(7:enc-val(4:ecdh(1:s2:\x01\x02)(1:e2:\x40\x03)))");
}

#[test]
fn a_keys_wrapping_names_the_stores_padding() {
    use mailcraft_smime::KeyTransport as T;
    assert_eq!(padding(T::Pkcs1v15), RsaPadding::Pkcs1v15);
    assert_eq!(padding(T::OaepSha1), RsaPadding::OaepSha1);
    assert_eq!(padding(T::OaepSha256), RsaPadding::OaepSha256);
}

#[test]
fn a_message_that_isnt_what_its_format_says_is_not_opened_and_nothing_panics() {
    let host = crate::signing::host();
    for format in [SignatureFormat::Smime, SignatureFormat::OpenPgp] {
        let job = DecryptJob { id: 1, format, original: std::sync::Arc::new(b"From: a@example.org\r\n\r\nHello.\r\n".to_vec()) };
        let answer = host.open(job).recv().unwrap();
        assert_eq!(answer.err(), Some(Failure::Failed));
    }
}

#[test]
fn the_agents_refusals_are_told_apart_by_who_can_fix_them() {
    use crate::decrypting::{Step, refusal};
    use mailcraft_assuan::ErrorKind;
    assert!(matches!(refusal(Some(ErrorKind::Canceled)), Step::Stop(Failure::Cancelled)));
    // A wrong passphrase or PIN is the owner's, not the message's.
    assert!(matches!(refusal(Some(ErrorKind::BadPassphrase)), Step::Stop(Failure::WrongPassphrase)));
    assert!(matches!(refusal(Some(ErrorKind::BadPin)), Step::Stop(Failure::WrongPassphrase)));
    // A key the agent has no secret half of: the next is tried, and "no key" is what is said.
    assert!(matches!(refusal(Some(ErrorKind::NoSecretKey)), Step::Next(Some(Failure::NoKey))));
    assert!(matches!(refusal(Some(ErrorKind::NoPinentry)), Step::Stop(Failure::Unavailable)));
    assert!(matches!(refusal(Some(ErrorKind::Timeout)), Step::Stop(Failure::Unavailable)));
    // Not the agent's own words (the connection went): unreachable.
    assert!(matches!(refusal(None), Step::Stop(Failure::Unavailable)));
    // Anything else is one more key tried, and nothing said of it.
    assert!(matches!(refusal(Some(ErrorKind::Other)), Step::Next(None)));
}
