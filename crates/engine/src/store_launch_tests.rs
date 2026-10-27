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
    assert_eq!(check_parts(&path).count, 0);
    // Looked for later: none yet, and the same three once they are.
    let later = open_launched(&path, Launch { checks_later: true, rows });
    assert_eq!((later.unavailable, later.unchecked), (0, true));
    assert_eq!(later.loaded + later.deferred, parts_outside(&mb));
    assert!(later.notice().is_none());
    assert_eq!(check_parts(&path).count, 3);
    // Once only.
    assert_eq!(check_parts(&path).count, 0);
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
    assert_eq!(check_parts(&path).count, 0);
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

#[test]
fn a_body_found_missing_later_reads_as_unavailable_and_the_mailbox_can_still_be_written_whole() {
    let tmp = Temp::new("missing-marked");
    let path = tmp.file();
    let mb = mailbox(24);
    save(&path, &mb).unwrap();
    clear_cache();
    let newest = newest_first(&mb);
    let old_text = *newest.iter().rev().find(|id| mb.message(**id).unwrap().body.text.len() > INLINE_MAX).unwrap();
    fs::remove_file(blob_file(&path, mb.message(old_text).unwrap().body.text.as_bytes())).unwrap();
    let old_html = *newest.iter().rev().find(|id| **id != old_text && mb.message(**id).unwrap().body.html.is_some()).unwrap();
    fs::remove_file(blob_file(&path, mb.message(old_html).unwrap().body.html.as_ref().unwrap().as_bytes())).unwrap();
    // As an opening that looks at once has it.
    let now = open_launched(&path, Launch::default());
    let opened = open_launched(&path, Launch { checks_later: true, rows: Some(3) });
    let mut s = Session::new(opened.mailbox);
    s.path = Some(path.clone());
    assert!(s.loaded_mailbox().is_err(), "not yet known to be missing");
    let missing = check_parts(&path);
    assert_eq!(missing.count, 2);
    assert_eq!(s.mark_unavailable(&missing.refs), 2);
    for id in [old_text, old_html] {
        let (m, was) = (s.mailbox.message(id).unwrap(), now.mailbox.message(id).unwrap());
        assert_eq!((&m.body, m.kept.as_deref().map(|k| k.failed)), (&was.body, was.kept.as_deref().map(|k| k.failed)), "message {id}");
        assert!(!s.needs_load_for_display(id));
    }
    // File › Save As and the like write it with the notice in its place, as they always have.
    let whole = s.loaded_mailbox().unwrap();
    assert_eq!(whole.message(old_text).unwrap().body.text, UNAVAILABLE);
    // Said once: nothing more is marked.
    assert_eq!(s.mark_unavailable(&missing.refs), 0);
}

#[test]
fn the_bodies_the_budget_holds_come_after_the_window_shows_as_an_opening_would_have_read_them() {
    let tmp = Temp::new("bodies-later");
    let path = tmp.file();
    let mb = mailbox(260);
    save(&path, &mb).unwrap();
    clear_cache();
    let now = open_launched(&path, Launch::default());
    let opened = open_launched(&path, Launch { checks_later: true, rows: Some(OPENING_ROWS) });
    let mut s = Session::new(opened.mailbox);
    s.path = Some(path.clone());
    // The oldest rows are past the first 200: no preview yet.
    let oldest = *newest_first(&mb).iter().rev().find(|id| mb.message(**id).unwrap().body.text.len() > INLINE_MAX).unwrap();
    let preview_now = crate::list::preview(now.mailbox.messages.get(&oldest).unwrap(), 80);
    assert!(!preview_now.is_empty());
    assert!(s.mailbox.message(oldest).unwrap().body_is_kept());
    let later = bodies_later(&path);
    assert!(!later.is_empty());
    assert!(bodies_later(&path).is_empty(), "once only");
    let before = s.loads();
    // In batches, as the host hands them over.
    for batch in later.chunks(64) {
        let (read, failed) = read_bodies(&path, batch);
        assert!(failed.is_empty());
        s.take_bodies(read);
    }
    assert_ne!(s.loads(), before, "what reads bodies sees they came");
    for (id, m) in &s.mailbox.messages {
        let was = now.mailbox.message(*id).unwrap();
        assert_eq!((&m.body, m.body_is_kept()), (&was.body, was.body_is_kept()), "message {id}");
    }
    assert_eq!(crate::list::preview(s.mailbox.messages.get(&oldest).unwrap(), 80), preview_now);
    // Brought in this way, a body isn't counted as one the owner brought in (nothing is let go of).
    assert_eq!(s.resident_bytes(), 0);
}

#[test]
fn a_body_whose_file_is_damaged_reads_and_is_counted_as_an_opening_would_have_it() {
    let tmp = Temp::new("damaged-later");
    let path = tmp.file();
    let mb = mailbox(24);
    save(&path, &mb).unwrap();
    clear_cache();
    // The file of an old message's text is there, but one of its bytes is not what it was.
    let old_text = *newest_first(&mb).iter().rev().find(|id| mb.message(**id).unwrap().body.text.len() > INLINE_MAX).unwrap();
    let file = blob_file(&path, mb.message(old_text).unwrap().body.text.as_bytes());
    let mut bytes = fs::read(&file).unwrap();
    bytes[0] ^= 1;
    fs::write(&file, bytes).unwrap();
    let now = open_launched(&path, Launch::default());
    assert_eq!(now.unavailable, 1);
    let opened = open_launched(&path, Launch { checks_later: true, rows: Some(1) });
    let mut s = Session::new(opened.mailbox);
    s.path = Some(path.clone());
    let (missing, bodies) = content_later(&path);
    assert_eq!(missing.count, now.unavailable);
    s.mark_unavailable(&missing.refs);
    s.take_bodies(bodies);
    let whole = s.loaded_mailbox().unwrap();
    for (id, m) in &s.mailbox.messages {
        let was = now.mailbox.message(*id).unwrap();
        assert_eq!((&m.body, m.kept.as_deref().map(|k| k.failed)), (&was.body, was.kept.as_deref().map(|k| k.failed)), "message {id}");
    }
    assert_eq!(whole.message(old_text).unwrap().body.text, UNAVAILABLE);
    // Once only.
    assert_eq!(content_later(&path).0.count, 0);
}

#[test]
fn a_body_missing_outright_is_counted_once_though_its_read_fails_too() {
    let tmp = Temp::new("missing-once");
    let path = tmp.file();
    let mb = mailbox(24);
    save(&path, &mb).unwrap();
    clear_cache();
    let old_text = *newest_first(&mb).iter().rev().find(|id| mb.message(**id).unwrap().body.text.len() > INLINE_MAX).unwrap();
    fs::remove_file(blob_file(&path, mb.message(old_text).unwrap().body.text.as_bytes())).unwrap();
    let now = open_launched(&path, Launch::default());
    open_launched(&path, Launch { checks_later: true, rows: Some(1) });
    assert_eq!(content_later(&path).0.count, now.unavailable);
}
