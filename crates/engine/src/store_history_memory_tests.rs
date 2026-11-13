// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the memory budget lets go of is let go of by the Undo and Redo steps too: they name the
//! content on disk as the mailbox does, so that a session that opened and changed many messages
//! holds about its budget, not its budget again for each step it can undo.

use std::collections::HashSet;

use mailcraft_model::FlagStatus;

use super::*;

/// The bytes of bodies, attachments and originals that the mailbox and every Undo and Redo step
/// hold in memory, each allocation counted once however many of them share it.
fn held_with_steps(s: &Session) -> u64 {
    let mut messages = HashSet::new();
    let mut data = HashSet::new();
    let mut held = 0u64;
    let maps = std::iter::once(&s.mailbox).chain(s.history.iter().map(|u| &u.mailbox)).chain(s.redo.iter().map(|u| &u.mailbox));
    for mb in maps {
        for m in mb.messages.values() {
            if !messages.insert(Arc::as_ptr(m) as usize) {
                continue;
            }
            held += (m.body.text.len() + m.body.html.as_ref().map_or(0, String::len) + m.hidden_text.len()) as u64;
            let files = m.attachments.iter().map(|a| &a.data).chain(m.original.iter().map(|o| &o.data));
            for bytes in files {
                if !bytes.is_empty() && data.insert(bytes.as_ptr() as usize) {
                    held += bytes.len() as u64;
                }
            }
        }
    }
    held
}

/// The bytes of message `m` brought whole into memory: its bodies and files.
fn whole_size(m: &Message) -> u64 {
    (m.body.text.len() + m.body.html.as_ref().map_or(0, String::len) + m.attachments.iter().map(|a| a.data.len()).sum::<usize>()) as u64
}

/// The text of message `id` as the session reads it, from memory or from disk.
fn text_of(s: &Session, id: Id) -> String {
    let m = s.mailbox.message(id).unwrap();
    if m.body_is_kept() { String::from_utf8(s.content(id, crate::Part::Text).unwrap().to_vec()).unwrap() } else { m.body.text.clone() }
}

#[test]
fn undo_steps_let_go_of_what_the_budget_lets_go_of() {
    let tmp = Temp::new("history-memory");
    let path = tmp.file();
    let mb = mailbox(90);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let kept: Vec<Id> = newest_first(&mb).into_iter().filter(|id| s.mailbox.message(*id).unwrap().body_is_kept()).collect();
    assert!(kept.len() > 40, "{}", kept.len());
    // Room for a few messages, their files and all.
    let largest = kept.iter().map(|id| whole_size(mb.message(*id).unwrap())).max().unwrap();
    s.loaded_budget = largest * 4;
    // Each is opened whole, then flagged (a copy of it, so the step before keeps the one it
    // was), and every few a move too: the history fills with steps that held whole messages.
    let inbox = s.mailbox.inbox().unwrap();
    let other = s.mailbox.folders.iter().find(|f| f.id != inbox && f.account == s.mailbox.accounts[0].id).unwrap().id;
    let mut moved = Vec::new();
    for (n, id) in kept.iter().enumerate() {
        s.sel.messages = vec![*id];
        s.load_message(*id).unwrap();
        s.run("mail.flag", &json!({"id": id})).unwrap();
        if n % 5 == 0 {
            s.run("mail.move", &json!({"ids": [id], "folder": other})).unwrap();
            moved.push(*id);
        }
    }
    s.sel.messages.clear();
    assert!(s.history.len() > 40);
    let held = held_with_steps(&s);
    assert!(held <= s.loaded_budget * 2, "{held} bytes held with the steps, for a budget of {}", s.loaded_budget);

    // Undone all the way, every message is as it was, its text read from disk; and redone, as
    // it became.
    while s.can_undo() {
        s.run("edit.undo", &json!({})).unwrap();
    }
    for id in &kept {
        let m = s.mailbox.message(*id).unwrap();
        assert_eq!((m.flag, m.folder), (FlagStatus::None, inbox));
        assert_eq!(text_of(&s, *id), mb.message(*id).unwrap().body.text);
    }
    while s.can_redo() {
        s.run("edit.redo", &json!({})).unwrap();
    }
    for id in &kept {
        let m = s.mailbox.message(*id).unwrap();
        assert_eq!(m.flag, FlagStatus::Flagged);
        assert_eq!(m.folder, if moved.contains(id) { other } else { inbox });
        assert_eq!(text_of(&s, *id), mb.message(*id).unwrap().body.text);
    }
    // Saved and opened again with everything read: nothing was lost on the way.
    save(&path, &s.mailbox).unwrap();
    clear_cache();
    let reopened = whole(&path);
    for id in &kept {
        let (was, now) = (mb.message(*id).unwrap(), reopened.message(*id).unwrap());
        assert_eq!((&now.body, &now.attachments), (&was.body, &was.attachments));
    }
}
