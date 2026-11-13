// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the memory budget lets go of is let go of by the Undo and Redo steps too: they name the
//! content on disk as the mailbox does, so that a session that opened and changed many messages
//! holds about its budget, not its budget again for each step it can undo.

use std::collections::HashSet;

use mailcraft_model::sync::ServerMessage;
use mailcraft_model::{AccountKind, FlagStatus, ServerFolder};

use super::*;

/// The bytes of bodies, attachments and originals that the mailbox and every Undo and Redo step
/// hold in memory (what a step noted of a Microsoft account's mail too), each allocation counted
/// once however many of them share it.
fn held_with_steps(s: &Session) -> u64 {
    let mut messages = HashSet::new();
    let mut data = HashSet::new();
    let mut held = 0u64;
    let steps = || s.history.iter().chain(s.redo.iter());
    let all =
        s.mailbox.messages.values().chain(steps().flat_map(|u| u.mailbox.messages.values())).chain(steps().flat_map(|u| u.touched.noted_messages()));
    for m in all {
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

/// `mb` with its account a Microsoft account, its folders and messages on the server.
fn microsoft(mut mb: Mailbox) -> Mailbox {
    let account = mb.accounts[0].id;
    mb.accounts[0].kind = AccountKind::Microsoft;
    for (n, f) in mb.folders.iter_mut().filter(|f| f.account == account).enumerate() {
        f.server = Some(ServerFolder { id: format!("F-{n}"), name: f.name.clone(), ..Default::default() });
    }
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    for (n, id) in ids.iter().enumerate() {
        let m = mb.message_mut(*id).unwrap();
        m.server = Some(ServerMessage { id: format!("M-{n}"), folder: "F-0".into(), ..Default::default() });
    }
    mb
}

/// Message `id`'s text rewritten to `text`, as a change of its own.
fn rewrite(s: &mut Session, v: &Value) -> crate::CmdResult {
    let id = v["id"].as_u64().unwrap_or(0);
    s.load_message(id)?;
    if let Ok(m) = s.mailbox.message_mut(id) {
        m.body.text = v["text"].as_str().unwrap_or_default().into();
        m.body.html = None;
    }
    Ok(Value::Null)
}

/// `mb` saved and opened anew with its long bodies left on disk, its budget room for a few
/// messages; then each of those opened whole and flagged, and every fifth moved: the history
/// fills with steps that held whole messages. Answers the session, the messages kept on disk,
/// those moved and where to.
fn opened_and_changed(path: &Path, mb: &Mailbox) -> (Session, Vec<Id>, Vec<Id>, Id) {
    save(path, mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(path);
    let mut specs = crate::cmd::registry().all().to_vec();
    specs.push(crate::CommandSpec::new("test.rewrite", "Rewrite", "Test", rewrite));
    s.registry = Arc::new(crate::Registry::new(specs));
    let kept: Vec<Id> = newest_first(mb).into_iter().filter(|id| s.mailbox.message(*id).unwrap().body_is_kept()).collect();
    assert!(kept.len() > 40, "{}", kept.len());
    let largest = kept.iter().map(|id| whole_size(mb.message(*id).unwrap())).max().unwrap();
    s.loaded_budget = largest * 4;
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
    (s, kept, moved, other)
}

/// Saved, and what nothing names any more on disk swept at once: the steps keep what they name.
fn saved_and_swept(s: &mut Session, path: &Path) {
    s.run("file.save", &json!({})).unwrap();
    sweep(path, Duration::ZERO).unwrap();
}

/// Undone all the way, every message is as it was, its text read from disk; and redone, as it
/// became; and saved and opened again, nothing was lost on the way.
fn undone_and_redone(s: &mut Session, path: &Path, mb: &Mailbox, kept: &[Id], moved: &[Id], other: Id) {
    let inbox = s.mailbox.inbox().unwrap();
    while s.can_undo() {
        s.run("edit.undo", &json!({})).unwrap();
    }
    for id in kept {
        let m = s.mailbox.message(*id).unwrap();
        assert_eq!((m.flag, m.folder), (FlagStatus::None, inbox));
        assert_eq!(text_of(s, *id), mb.message(*id).unwrap().body.text);
    }
    while s.can_redo() {
        s.run("edit.redo", &json!({})).unwrap();
    }
    for id in kept {
        let m = s.mailbox.message(*id).unwrap();
        assert_eq!(m.flag, FlagStatus::Flagged);
        assert_eq!(m.folder, if moved.contains(id) { other } else { inbox });
        assert_eq!(text_of(s, *id), mb.message(*id).unwrap().body.text);
    }
    save(path, &s.mailbox).unwrap();
    clear_cache();
    let reopened = whole(path);
    for id in kept {
        let (was, now) = (mb.message(*id).unwrap(), reopened.message(*id).unwrap());
        assert_eq!((&now.body, &now.attachments), (&was.body, &was.attachments));
    }
}

#[test]
fn undo_steps_let_go_of_what_the_budget_lets_go_of() {
    let tmp = Temp::new("history-memory");
    let path = tmp.file();
    let mb = mailbox(90);
    let (mut s, kept, moved, other) = opened_and_changed(&path, &mb);
    let held = held_with_steps(&s);
    assert!(held <= s.loaded_budget * 2, "{held} bytes held with the steps, for a budget of {}", s.loaded_budget);
    saved_and_swept(&mut s, &path);
    undone_and_redone(&mut s, &path, &mb, &kept, &moved, other);
}

/// The same for a Microsoft account, whose steps also note each message as the change left it.
#[test]
fn undo_steps_of_a_microsoft_account_let_go_of_what_the_budget_lets_go_of() {
    let tmp = Temp::new("history-memory-ms");
    let path = tmp.file();
    let mb = microsoft(mailbox(90));
    let (mut s, kept, moved, other) = opened_and_changed(&path, &mb);
    let held = held_with_steps(&s);
    assert!(held <= s.loaded_budget * 2, "{held} bytes held with the steps, for a budget of {}", s.loaded_budget);
    saved_and_swept(&mut s, &path);
    undone_and_redone(&mut s, &path, &mb, &kept, &moved, other);
}

/// A message whose text changed after an older step kept it: let go of in the mailbox, the older
/// step keeps the text it had (it isn't the one on disk), and an undo brings it back after a save
/// and a sweep took the old text off disk.
#[test]
fn a_step_older_than_a_change_of_the_text_keeps_the_text_it_had() {
    for mb in [mailbox(90), microsoft(mailbox(90))] {
        let tmp = Temp::new("history-memory-rewrite");
        let path = tmp.file();
        let (mut s, kept, _, _) = opened_and_changed(&path, &mb);
        let id = kept[0];
        s.load_message(id).unwrap();
        // Long enough to be kept on disk beside the mailbox, not in it.
        let rewritten = "Rewritten. ".repeat(INLINE_MAX);
        s.run("test.rewrite", &json!({"id": id, "text": rewritten})).unwrap();
        saved_and_swept(&mut s, &path);
        // The others opened again: the budget lets go of the rewritten one in the mailbox.
        for other in &kept[1..] {
            s.load_message(*other).unwrap();
        }
        assert!(s.mailbox.message(id).unwrap().body_is_kept());
        assert_eq!(text_of(&s, id), rewritten);
        saved_and_swept(&mut s, &path);
        while s.can_undo() {
            s.run("edit.undo", &json!({})).unwrap();
        }
        assert_eq!(text_of(&s, id), mb.message(id).unwrap().body.text);
        while s.can_redo() {
            s.run("edit.redo", &json!({})).unwrap();
        }
        assert_eq!(text_of(&s, id), rewritten);
    }
}

#[test]
fn a_window_out_of_sight_lets_go_of_every_body_but_those_kept() {
    let tmp = Temp::new("let-go-all");
    let path = tmp.file();
    let mb = mailbox(40);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let kept: Vec<Id> = newest_first(&mb).into_iter().filter(|id| s.mailbox.message(*id).unwrap().body_is_kept()).collect();
    // Everything brought in, under a budget that holds it all, and a step to undo for each flag.
    s.loaded_budget = u64::MAX;
    for id in &kept {
        s.load_message(*id).unwrap();
        s.run("mail.flag", &json!({"id": id})).unwrap();
    }
    let open = kept[0];
    s.sel.messages = vec![kept[1]];
    let before = held_with_steps(&s);
    assert!(s.resident_bytes() > 0);
    // A few at a time, as the window does it: each call goes on from where the last stopped.
    let keep: HashSet<Id> = [open].into_iter().collect();
    let mut from = None;
    let mut calls = 0;
    loop {
        from = s.let_go_of_content(from, &keep, 3);
        calls += 1;
        if from.is_none() {
            break;
        }
        assert!(calls < 100);
    }
    assert!(calls > 1);
    // What the kept message holds is left (as it is and as its step before the flag has it),
    // and short bodies kept in the mailbox file; the rest went, from the steps too.
    let m = s.mailbox.message(open).unwrap();
    assert!(!m.body_is_kept(), "the message open in a window keeps its body");
    let selected = s.mailbox.message(kept[1]).unwrap();
    assert!(!selected.body_is_kept(), "the one selected keeps its body, as the budget leaves it");
    let left = whole_size(m) + whole_size(selected);
    let after = held_with_steps(&s);
    let slack = INLINE_MAX as u64 * 2 * mb.messages.len() as u64;
    assert!(after <= 2 * left + slack, "{after} of {before} bytes still held ({left} kept)");
    assert!(before > 4 * after, "{after} of {before} bytes still held");
    assert!(s.resident_bytes() <= left, "{} counted", s.resident_bytes());
    // Read again when wanted, and Undo all the way finds every body.
    assert_eq!(text_of(&s, kept[2]), mb.message(kept[2]).unwrap().body.text);
    while s.can_undo() {
        s.run("edit.undo", &json!({})).unwrap();
    }
    for id in &kept {
        assert_eq!(text_of(&s, *id), mb.message(*id).unwrap().body.text);
    }
}

/// Messages whose content can't go (changed since the save, so not on disk as they are) count
/// against a slice all the same: one call never walks the whole mailbox.
#[test]
fn a_slice_counts_what_it_could_not_let_go_of() {
    let tmp = Temp::new("let-go-slice");
    let path = tmp.file();
    let mb = mailbox(40);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    s.loaded_budget = u64::MAX;
    let kept: Vec<Id> = newest_first(&mb).into_iter().filter(|id| s.mailbox.message(*id).unwrap().body_is_kept()).collect();
    assert!(kept.len() > 6);
    for id in &kept {
        s.load_message(*id).unwrap();
        let m = s.mailbox.message_mut(*id).unwrap();
        m.body.text.push_str("Changed here, not saved.\n");
        (m.body.html, m.attachments, m.original) = (None, Vec::new(), None);
    }
    let first = s.let_go_of_content(None, &HashSet::new(), 3);
    assert!(first.is_some(), "a slice of 3 stops after 3 looked at");
    assert!(kept.iter().all(|id| s.mailbox.message(*id).unwrap().body.text.ends_with("not saved.\n")), "a changed text stays");
}
