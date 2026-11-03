// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The buttons of an agent's question (#566).

use mailcraft_engine::agent::Class;

use crate::agent_gate::verbs;

#[test]
fn a_rule_made_or_switched_on_isnt_answered_with_delete() {
    for id in ["rules.add", "rules.keepOnServer", "rules.enable", "rules.move"] {
        assert_eq!(verbs(Class::Delete, id, false), (tl!("Allow"), tl!("Don't Allow")), "{id}");
    }
    for id in ["rules.remove", "mail.delete"] {
        assert_eq!(verbs(Class::Delete, id, false), (tl!("Delete"), tl!("Don't Delete")), "{id}");
    }
    // A rule made from a message runs at once on the mail from that sender: a yes deletes that
    // mail only when the rule does.
    assert_eq!(verbs(Class::Delete, "rules.fromMessage", true), (tl!("Delete"), tl!("Don't Delete")));
    assert_eq!(verbs(Class::Delete, "rules.fromMessage", false), (tl!("Allow"), tl!("Don't Allow")));
}
