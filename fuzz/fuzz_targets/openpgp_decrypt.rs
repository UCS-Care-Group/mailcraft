// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Encrypted OpenPGP message (#30 part 6): the bytes read as a message (PGP/MIME and inline),
//! its recipients turned into what gpg-agent is asked, the same bytes offered as the owner's
//! keys (as `gpg --export` writes them) and as what the agent gave back for each recipient,
//! and as a secret key an in-process decryption is tried with.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let keys = mailcraft_openpgp::decryption_keys(data);
    let Some(message) = mailcraft_openpgp::encrypted_message(data) else { return };
    for recipient in message.recipients() {
        let _ = mailcraft_openpgp::agent_request(recipient);
        for key in &keys {
            let _ = key.is_recipient(recipient);
            let _ = mailcraft_openpgp::session_key(recipient, key, data, Some(0));
            let _ = mailcraft_openpgp::session_key(recipient, key, data, None);
        }
    }
    let _ = mailcraft_openpgp::decrypt_with_secret_key(&message, data, "passphrase");
});
