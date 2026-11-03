// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What this host's to-do job checks a no for its token against (#520): the account's own mail.
//! Nothing here leaves this machine.

use super::*;

#[test]
fn a_no_for_the_tasks_is_checked_against_how_the_account_has_its_mail() {
    use mailcraft_model::{Account, AccountKind};
    use mailcraft_net::oauth::Resource;
    // Kept in step through Graph: its sign-in never covered IMAP, so IMAP can't say.
    assert_eq!(tasks_mail(&Account { kind: AccountKind::Microsoft, ..Default::default() }), Resource::GraphMail);
    assert_eq!(tasks_mail(&Account { kind: AccountKind::Imap, ..Default::default() }), Resource::Mail);
}
