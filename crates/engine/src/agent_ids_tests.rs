// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An agent names the messages it destroys, moves, replies to or opens (#620): the selection is
//! the owner's.

use serde_json::json;

use crate::Session;
use crate::agent::*;

fn sample() -> Session {
    Session::new(crate::sample::mailbox(1_791_993_600, 0))
}

#[test]
fn every_command_that_needs_ids_is_refused_without_them() {
    let mut s = sample();
    let id = s.mailbox.messages.values().find(|m| !m.draft).map(|m| m.id).unwrap();
    s.sel.messages = vec![id];
    for command in NEEDS_IDS {
        for params in [json!({}), json!({"ids": []}), json!({"ids": null, "id": null}), json!({"folder": 1})] {
            match check(&s, command, &params, &Class::ALL) {
                Verdict::Refuse(why) => assert!(why.contains("`ids`"), "{command}: {why}"),
                _ => panic!("{command} {params} was not refused"),
            }
        }
        assert!(!matches!(check(&s, command, &json!({"ids": [id]}), &Class::ALL), Verdict::Refuse(w) if w.contains("needs `ids`")), "{command}");
        assert!(!matches!(check(&s, command, &json!({"id": id}), &Class::ALL), Verdict::Refuse(w) if w.contains("needs `ids`")), "{command}");
    }
}

#[test]
fn flagging_and_reading_still_take_the_selection() {
    let s = sample();
    for command in ["mail.flag", "mail.markRead", "mail.get"] {
        assert!(matches!(check(&s, command, &json!({}), &Class::ALL), Verdict::Run), "{command}");
    }
}

#[test]
fn a_delete_naming_no_messages_is_weighed_and_told_as_none_of_the_selection() {
    // The selection in Deleted Items, where a Delete removes it for good (#503).
    let mut s = sample();
    let id = s.mailbox.messages.values().find(|m| !m.draft).map(|m| m.id).unwrap();
    let deleted = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, mailcraft_model::FolderKind::Deleted).unwrap();
    s.run("mail.move", &json!({"ids": [id], "folder": deleted})).unwrap();
    s.sel.messages = vec![id];
    assert_eq!(classify(&s, "mail.delete", &json!({})), Access::Safe);
    assert_eq!(classify(&s, "mail.delete", &json!({"ids": [id]})), Access::Owner(Class::Delete));
    for command in ["mail.delete", "mail.deletePermanently"] {
        assert!(describe(&s, command, &json!({})).contains("0 messages"), "{command}");
    }
}
