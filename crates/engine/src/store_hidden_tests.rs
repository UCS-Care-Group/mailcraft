// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a sender hid in a message (#599) goes outside the mailbox file with the body when it is
//! long, comes back with it, counts as content, is found by search, and goes when the message is
//! thinned.

use super::*;

const NEEDLE: &str = "zorblax";

fn long_hidden() -> String {
    format!("{NEEDLE} {}", "preheader words ".repeat(200))
}

/// A mailbox with `hidden` on one message with a long text; its id.
fn with_hidden(hidden: &str) -> (Mailbox, Id) {
    let mut mb = mailbox(24);
    let id = first_with(&mb, |m| m.body.text.len() > INLINE_MAX);
    let mut m = Message::clone(mb.message(id).unwrap());
    m.hidden_text = hidden.to_string();
    mb.messages.insert(id, Arc::new(m));
    (mb, id)
}

#[test]
fn a_long_hidden_text_leaves_the_record_comes_back_and_goes_with_a_thinned_message() {
    let (mb, id) = with_hidden(&long_hidden());
    assert!(long_hidden().len() > INLINE_MAX);
    let tmp = Temp::new("hidden-outside");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    // The record the mailbox file keeps has none of it, and names the content file.
    let record = prepare(id, mb.messages.get(&id).unwrap(), &mut HashMap::new()).unwrap();
    assert!(!String::from_utf8_lossy(&record.json).contains(NEEDLE));
    assert_eq!(record.parts.hidden.map(|p| p.r.len), Some(long_hidden().len() as u64));
    assert!(blob_file(&path, long_hidden().as_bytes()).is_file());

    // Opened with no room for bodies: not in memory, named, and the body is "kept".
    let mut s = session_without_bodies(&path);
    let m = s.mailbox.message(id).unwrap();
    assert!(m.hidden_text.is_empty() && m.kept.as_deref().is_some_and(|k| k.hidden.is_some()) && m.body_is_kept());
    assert!(m.size() >= long_hidden().len() as u64);
    // Read back with the body, counted in what is in memory and in the bytes of content.
    let before = crate::cmd::offline::content_bytes(m);
    assert_eq!(s.load_for_display(id), Ok(true));
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(m.hidden_text, long_hidden());
    assert!(!m.body_is_kept());
    assert!(crate::cmd::offline::content_bytes(m) >= before);
    assert_eq!(s.content(id, crate::Part::Hidden).unwrap().as_slice(), long_hidden().as_bytes());

    // Let go of again when it is safely on disk, and it is named once more.
    let lean = evictable(&path, id, m).expect("a body safely on disk can be let go of");
    assert!(lean.hidden_text.is_empty() && lean.kept.as_deref().is_some_and(|k| k.hidden.is_some()));

    // The save of a message that was never loaded writes what the file already named.
    let mut again = session_without_bodies(&path);
    let was_read = again.mailbox.message(id).unwrap().read;
    again.mailbox.message_mut(id).unwrap().read = !was_read;
    save(&path, &again.mailbox).unwrap();
    clear_cache();
    assert_eq!(whole(&path).message(id).unwrap().hidden_text, long_hidden());

    // Rules and search see it as they see the text kept on disk.
    let s = session_without_bodies(&path);
    assert!(s.mailbox.message(id).unwrap().hidden_text.is_empty());
    let loaded = s.loaded_message_where(id, |_| false).unwrap();
    assert!(mailcraft_model::search::Query::parse(NEEDLE).matches(&loaded, 0));
    assert!(mailcraft_model::Condition::BodyContains(NEEDLE.into()).matches(&loaded));

    // Thinned: it goes with the body, and the content file is no longer named by the message.
    let mut thin = Message::clone(whole(&path).message(id).unwrap());
    let names_before = content_names(id, &Arc::new(thin.clone()));
    crate::cmd::graph_down::thin_message(&mut thin, "preview".into());
    assert!(thin.hidden_text.is_empty() && thin.kept.is_none());
    let names_after = content_names(id, &Arc::new(thin));
    let hidden_ref = ContentRef { sha256: Digest::of(long_hidden().as_bytes()).0, len: long_hidden().len() as u64 };
    assert!(names_before.contains(&hidden_ref) && !names_after.contains(&hidden_ref));
}

#[test]
fn a_short_hidden_text_stays_in_the_record_and_old_files_load() {
    let (mb, id) = with_hidden("a short preheader");
    let record = prepare(id, mb.messages.get(&id).unwrap(), &mut HashMap::new()).unwrap();
    assert!(record.parts.hidden.is_none() && String::from_utf8_lossy(&record.json).contains("a short preheader"));
    let tmp = Temp::new("hidden-inline");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    assert_eq!(whole(&path).message(id).unwrap().hidden_text, "a short preheader");
    // A file with no `hidden` among its content names (every file before this) opens.
    let (plain, id) = (mailbox(24), 0);
    let _ = id;
    save(&path, &plain).unwrap();
    clear_cache();
    assert!(whole(&path) == plain);
}

/// The message as it reads with no hidden text: its text and HTML are all there, by every road.
fn assert_body_intact(s: &mut Session, mb: &Mailbox, id: Id) {
    let was = mb.message(id).unwrap();
    assert!(!s.mailbox.message(id).unwrap().kept.as_deref().is_some_and(|k| k.failed), "the body is not unavailable");
    assert_eq!(s.load_for_display(id), Ok(true));
    let m = s.mailbox.message(id).unwrap();
    assert_eq!((m.body.text.as_str(), m.body.html.as_deref()), (was.body.text.as_str(), was.body.html.as_deref()));
    assert!(m.hidden_text.is_empty() && !m.body_is_kept());
    assert_eq!(s.content(id, crate::Part::Text).unwrap().as_slice(), was.body.text.as_bytes());
    let whole = s.loaded_message(id).unwrap();
    assert_eq!(whole.body.text, was.body.text);
}

/// A hidden-text file that is missing or damaged is no hidden text, and nothing more.
#[test]
fn a_missing_hidden_file_leaves_the_body_readable() {
    let (mb, id) = with_hidden(&long_hidden());
    let tmp = Temp::new("hidden-missing");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    fs::remove_file(blob_file(&path, long_hidden().as_bytes())).unwrap();
    let mut s = session_without_bodies(&path);
    assert!(s.readable_whole(s.mailbox.message(id).unwrap()));
    assert_body_intact(&mut s, &mb, id);
    // Damaged (its length is not what the file names) is the same.
    save(&path, &mb).unwrap();
    clear_cache();
    fs::write(blob_file(&path, long_hidden().as_bytes()), b"not it").unwrap();
    let mut s = session_without_bodies(&path);
    assert_body_intact(&mut s, &mb, id);
}

/// What looks for missing files once the window shows ends the same way.
#[test]
fn a_hidden_file_found_missing_later_marks_nothing_unavailable() {
    let (mb, id) = with_hidden(&long_hidden());
    let tmp = Temp::new("hidden-later");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let hidden = ContentRef { sha256: Digest::of(long_hidden().as_bytes()).0, len: long_hidden().len() as u64 };
    s.mark_unavailable(&[hidden]);
    let m = s.mailbox.message(id).unwrap();
    assert!(!m.kept.as_deref().is_some_and(|k| k.failed || k.hidden.is_some()), "{:?}", m.kept);
    assert!(!m.body.text.contains("unavailable"));
    clear_cache();
    assert_body_intact(&mut s, &mb, id);
}

/// An older build knows nothing of the file, so its sweep takes it once it is a few minutes old;
/// the mailbox file still names it.
#[test]
fn the_hidden_file_an_older_builds_sweep_took_is_no_hidden_text() {
    let (mb, id) = with_hidden(&long_hidden());
    let tmp = Temp::new("hidden-old-sweep");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    let file = blob_file(&path, long_hidden().as_bytes());
    age(&file, GRACE + Duration::from_secs(60));
    fs::remove_file(&file).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    assert_body_intact(&mut s, &mb, id);
    // Saved again, the record stops naming it, and the message opens as it reads.
    s.mailbox.message_mut(id).unwrap().read = !mb.message(id).unwrap().read;
    save(&path, &s.mailbox).unwrap();
    clear_cache();
    let again = whole(&path);
    assert!(again.message(id).unwrap().hidden_text.is_empty());
    assert_eq!(again.message(id).unwrap().body.text, mb.message(id).unwrap().body.text);
}

/// A hostile file that names a hidden text larger than the cap gets none of it read.
#[test]
fn a_hidden_file_named_past_the_cap_is_not_read() {
    let big = "w ".repeat(mailcraft_model::MAX_HIDDEN_TEXT);
    let (mb, id) = with_hidden(&big);
    let tmp = Temp::new("hidden-big");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    let s = session_without_bodies(&path);
    let m = s.mailbox.message(id).unwrap();
    assert!(m.hidden_text.is_empty() && m.kept.as_deref().is_none_or(|k| k.hidden.is_none()), "{:?}", m.kept);
    let opened = with_budget(1 << 30, || open(&path).unwrap());
    assert!(opened.mailbox.message(id).unwrap().hidden_text.is_empty());
}

/// Nothing visible is lost with a hidden-text file an older build's sweep took, so the owner is
/// told of nothing, on any open and after a save, whichever way the opening looks for files.
#[test]
fn a_missing_hidden_file_is_no_notice_on_any_open() {
    let (mb, id) = with_hidden(&long_hidden());
    let tmp = Temp::new("hidden-no-notice");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    fs::remove_file(blob_file(&path, long_hidden().as_bytes())).unwrap();
    clear_cache();
    let opens = |budget: u64, launch: Launch| {
        *lock(&shared(&path).state) = State::default();
        with_budget(budget, || open_as(&path, &OpenProgress::default(), launch).unwrap())
    };
    for (budget, launch) in [
        (1 << 30, Launch::default()),
        (1, Launch::default()),
        (1 << 30, Launch { checks_later: true, rows: Some(3) }),
        (1 << 30, Launch { checks_later: true, rows: None }),
    ] {
        let opened = opens(budget, launch);
        assert_eq!(opened.unavailable, 0, "{budget} {launch:?}");
        assert!(opened.notice().is_none(), "{:?}", opened.notice());
        assert_eq!(check_parts(&path).count, 0);
        // Saved, and opened again: still nothing to tell.
        let mut mailbox = opened.mailbox.clone();
        let pinned = mailbox.message(id).unwrap().pinned;
        mailbox.message_mut(id).unwrap().pinned = !pinned;
        save(&path, &mailbox).unwrap();
        let again = opens(budget, launch);
        assert_eq!(again.unavailable, 0);
        assert!(again.notice().is_none());
        assert_eq!(check_parts(&path).count, 0);
    }
}

/// The bodies read after the window shows (rows: 0 leaves them all for then) tell of nothing
/// either, nor does the look for files after it.
#[test]
fn a_missing_hidden_file_is_no_failure_when_the_bodies_are_read_later() {
    let (mb, id) = with_hidden(&long_hidden());
    let tmp = Temp::new("hidden-later-read");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    fs::remove_file(blob_file(&path, long_hidden().as_bytes())).unwrap();
    clear_cache();
    *lock(&shared(&path).state) = State::default();
    let opened = with_budget(1 << 30, || open_as(&path, &OpenProgress::default(), Launch { checks_later: true, rows: Some(0) }).unwrap());
    assert!(opened.notice().is_none());
    let (missing, read) = content_later(&path);
    assert_eq!(missing.count, 0, "{:?}", missing.refs);
    assert!(missing.refs.is_empty());
    // The bodies come, with no hidden text, and are taken in as the window takes them.
    let mut s = Session::new(opened.mailbox);
    s.path = Some(path.clone());
    s.take_bodies(read);
    assert_eq!(s.mark_unavailable(&missing.refs), 0);
    assert_body_intact(&mut s, &mb, id);
}

/// A hidden file that goes after the mailbox is open leaves the body, by either way of bringing
/// it in.
#[test]
fn a_hidden_file_that_goes_after_the_open_leaves_the_body_readable() {
    let (mb, id) = with_hidden(&long_hidden());
    let tmp = Temp::new("hidden-goes-after");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    assert!(s.mailbox.message(id).unwrap().kept.as_deref().is_some_and(|k| k.hidden.is_some()));
    fs::remove_file(blob_file(&path, long_hidden().as_bytes())).unwrap();
    clear_cache();
    assert_body_intact(&mut s, &mb, id);
    // The whole mailbox in memory, as an export needs it.
    let opened = with_budget(1, || open(&path).unwrap());
    let mut all = opened.mailbox;
    assert!(load_all(&path, &mut all).is_ok());
    assert_eq!(all.message(id).unwrap().body.text, mb.message(id).unwrap().body.text);
    assert!(all.message(id).unwrap().hidden_text.is_empty());
}
