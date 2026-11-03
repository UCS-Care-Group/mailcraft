// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The buttons of an agent's question (#566).

use mailcraft_engine::agent::Class;

use crate::agent_gate::verbs;

#[test]
fn a_rule_made_or_switched_on_isnt_answered_with_delete() {
    for id in ["rules.add", "rules.keepOnServer", "rules.enable", "rules.move"] {
        assert_eq!(verbs(Class::Delete, id), (tl!("Allow"), tl!("Don't Allow")), "{id}");
    }
    for id in ["rules.fromMessage", "rules.remove", "mail.delete"] {
        assert_eq!(verbs(Class::Delete, id), (tl!("Delete"), tl!("Don't Delete")), "{id}");
    }
}
