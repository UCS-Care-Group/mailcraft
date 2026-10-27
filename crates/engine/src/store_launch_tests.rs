// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening a mailbox for a window that shows it at once (#519): content files looked for
//! afterwards, and bodies brought in only for the rows the window opens on.

use super::*;

/// The mailbox at `path` opened as `launch` says, with a budget for every body.
fn open_launched(path: &Path, launch: Launch) -> Opened {
    *lock(&shared(path).state) = State::default();
    with_budget(1 << 30, || open_as(path, &OpenProgress::default(), launch).unwrap())
}

#[test]
fn content_files_looked_for_later_are_counted_as_the_opening_would_have() {
    let tmp = Temp::new("checks-later");
    let path = tmp.file();
    let mb = mailbox(24);
    save(&path, &mb).unwrap();
    clear_cache();
    // Two attachments and the text of an old message go missing.
    let gone_files: Vec<Id> =
        mb.messages.values().filter(|m| m.attachments.iter().any(|a| a.name.starts_with("file-"))).map(|m| m.id).take(2).collect();
    for id in &gone_files {
        fs::remove_file(blob_file(&path, &mb.message(*id).unwrap().attachments[0].data)).unwrap();
    }
    let old_text = *newest_first(&mb).iter().rev().find(|id| mb.message(**id).unwrap().body.text.len() > INLINE_MAX).unwrap();
    fs::remove_file(blob_file(&path, mb.message(old_text).unwrap().body.text.as_bytes())).unwrap();
    let rows = Some(3);
    // Looked for while opening, as always: three are missing.
    let now = open_launched(&path, Launch { checks_later: false, rows });
    assert_eq!((now.unavailable, now.unchecked), (3, false));
    assert_eq!(check_parts(&path), 0);
    // Looked for later: none yet, and the same three once they are.
    let later = open_launched(&path, Launch { checks_later: true, rows });
    assert_eq!((later.unavailable, later.unchecked), (0, true));
    assert_eq!(later.loaded + later.deferred, parts_outside(&mb));
    assert!(later.notice().is_none());
    assert_eq!(check_parts(&path), 3);
    // Once only.
    assert_eq!(check_parts(&path), 0);
    // Meanwhile the missing text is a body on disk like any other, and says so once it is wanted.
    let mut s = Session::new(later.mailbox);
    s.path = Some(path.clone());
    assert!(s.needs_load_for_display(old_text));
    let err = s.load_for_display(old_text).unwrap_err().to_string();
    assert!(err.contains("missing"), "{err}");
    let m = s.mailbox.message(old_text).unwrap();
    assert!(m.body.text == UNAVAILABLE && m.kept.as_ref().is_some_and(|k| k.failed) && !s.needs_load_for_display(old_text));
    // Saved, the file names what it named, missing or not.
    save(&path, &s.mailbox).unwrap();
    *lock(&shared(&path).state) = State::default();
    assert_eq!(with_budget(1 << 30, || open(&path).unwrap()).unavailable, 3);
}

#[test]
fn a_save_while_content_is_still_to_be_looked_for_is_not_held_up_and_loses_nothing() {
    let tmp = Temp::new("checks-later-save");
    let path = tmp.file();
    let mut mb = mailbox(12);
    save(&path, &mb).unwrap();
    let opened = open_launched(&path, Launch { checks_later: true, rows: Some(2) });
    assert!(opened.unchecked);
    let first = *mb.messages.keys().next().unwrap();
    mb.message_mut(first).unwrap().read = true;
    let mut theirs = opened.mailbox;
    Arc::make_mut(theirs.messages.get_mut(&first).unwrap()).read = true;
    assert!(!save(&path, &theirs).unwrap().compacted);
    assert_eq!(check_parts(&path), 0);
    assert!(whole(&path) == mb);
}

#[test]
fn only_the_rows_the_window_opens_on_have_their_bodies_brought_in_and_the_rest_come_when_shown() {
    let tmp = Temp::new("rows");
    let path = tmp.file();
    let mut mb = mailbox(24);
    // Some of the newest mail is in another folder: the window opens on the Inbox.
    let account = mb.accounts[0].id;
    let other = mb.add_folder(account, None, "Projects").unwrap();
    let newest = newest_first(&mb);
    for id in newest.iter().step_by(3) {
        mb.message_mut(*id).unwrap().folder = other;
    }
    save(&path, &mb).unwrap();
    clear_cache();
    let inbox = mb.inbox().unwrap();
    let rows: Vec<Id> = newest.iter().copied().filter(|id| mb.message(*id).unwrap().folder == inbox).take(4).collect();
    let opened = open_launched(&path, Launch { checks_later: false, rows: Some(4) });
    assert_eq!(opened.unavailable, 0);
    for (id, m) in &opened.mailbox.messages {
        assert_eq!(!m.body_is_kept(), rows.contains(id) || body_outside(mb.message(*id).unwrap()) == 0, "message {id}");
    }
    // Opened with the budget alone, every body is there.
    let all = open_launched(&path, Launch::default());
    assert!(all.mailbox.messages.values().all(|m| !m.body_is_kept()));
    // A message whose body wasn't brought in shows it when it is shown.
    let opened = open_launched(&path, Launch { checks_later: false, rows: Some(4) });
    let mut s = Session::new(opened.mailbox);
    s.path = Some(path.clone());
    let not_in = *newest.iter().find(|id| !rows.contains(id) && body_outside(mb.message(**id).unwrap()) > 0).unwrap();
    assert!(s.mailbox.message(not_in).unwrap().body_is_kept() && s.needs_load_for_display(not_in));
    assert!(s.load_for_display(not_in).unwrap());
    let shown = s.mailbox.message(not_in).unwrap();
    let was = mb.message(not_in).unwrap();
    assert_eq!((&shown.body.text, &shown.body.html), (&was.body.text, &was.body.html));
    assert!(!s.needs_load_for_display(not_in));
}
