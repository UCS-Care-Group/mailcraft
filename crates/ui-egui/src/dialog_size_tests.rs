// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#[test]
fn confirmation_payload_does_not_inflate_every_dialog() {
    // Calendar approval bindings and cut receipts belong to the confirmation, not to the
    // inline storage of every dialog. Existing owner-route tests exercise their contents.
    let bytes = std::mem::size_of::<crate::dialogs::Dialog>();
    assert!(bytes <= 256, "a dialog uses {bytes} inline bytes; keep the confirmation payload indirect");
}
