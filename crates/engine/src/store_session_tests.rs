// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Content kept on disk as a session uses it: what Undo holds through a sweep, a search and a
//! rule that read bodies, and what bringing content in counts as (review of #87).

use std::sync::Mutex;

use mailcraft_model::{Action, Condition, ContentRef, Rule};

use super::*;
use crate::{CmdError, CommandSpec, Registry};

/// The message of `mb` with a long text and a file of its own, both kept outside the mailbox file.
fn with_text_and_file(mb: &Mailbox) -> Id {
    first_with(mb, |m| m.body.text.len() > INLINE_MAX && m.attachments.iter().any(|a| a.data.len() > INLINE_MAX && a.name.starts_with("file-")))
}

/// The index of `m`'s own file.
fn file_index(m: &Message) -> usize {
    m.attachments.iter().position(|a| a.name.starts_with("file-")).unwrap()
}

#[test]
fn undo_after_a_sweep_brings_back_the_files_of_a_message_read_before_it_was_deleted() {
    let tmp = Temp::new("undo-viewed");
    let path = tmp.file();
    let mb = mailbox(24);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let victim = with_text_and_file(&mb);
    let i = file_index(mb.message(victim).unwrap());
    // Read in the reading pane: its text comes in; its file stays on disk.
    s.sel.messages = vec![victim];
    assert_eq!(s.load_for_display(victim), Ok(true));
    assert!(!s.mailbox.message(victim).unwrap().attachments[i].is_loaded());
    // Shift+Delete, the autosave, the first sweep, then Ctrl+Z.
    s.run("mail.deletePermanently", &json!({"ids": [victim]})).unwrap();
    save(&path, &s.mailbox).unwrap();
    sweep(&path, Duration::ZERO).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    clear_cache();
    let was = mb.message(victim).unwrap();
    assert_eq!(s.content(victim, crate::Part::Attachment(i)).unwrap().as_slice(), was.attachments[i].data.as_slice());
    save(&path, &s.mailbox).unwrap();
    assert!(whole(&path) == mb);
}

#[test]
fn undo_after_a_sweep_brings_back_a_body_that_was_let_go_of_before_the_delete() {
    let tmp = Temp::new("undo-evicted");
    let path = tmp.file();
    let mb = mailbox(24);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let kept: Vec<Id> = newest_first(&mb).into_iter().filter(|id| s.mailbox.message(*id).unwrap().body_is_kept()).collect();
    let largest = kept.iter().map(|id| body_outside(mb.message(*id).unwrap())).max().unwrap();
    s.loaded_budget = largest;
    // The first is read, then the others, until it is let go of again.
    let victim = kept[0];
    assert_eq!(s.load_for_display(victim), Ok(true));
    for id in &kept[1..] {
        s.load_for_display(*id).unwrap();
    }
    assert!(s.mailbox.message(victim).unwrap().body_is_kept(), "its body went back to disk");
    s.run("mail.deletePermanently", &json!({"ids": [victim]})).unwrap();
    save(&path, &s.mailbox).unwrap();
    sweep(&path, Duration::ZERO).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    clear_cache();
    let was = mb.message(victim).unwrap();
    assert_eq!(s.content(victim, crate::Part::Text).unwrap().as_slice(), was.body.text.as_bytes());
    save(&path, &s.mailbox).unwrap();
    assert!(whole(&path) == mb);
}

#[test]
fn a_search_under_a_small_budget_finds_every_match_and_stays_within_it() {
    let tmp = Temp::new("search-budget");
    let path = tmp.file();
    let mut mb = mailbox(24);
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    let mut with_word = 0;
    for id in ids {
        let m = mb.message_mut(id).unwrap();
        if m.body.text.len() > INLINE_MAX {
            m.body.text.push_str("A word: zymurgy.\n");
            with_word += 1;
        }
    }
    assert_eq!(with_word, 16);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let largest = mb.messages.values().map(|m| body_outside(m)).max().unwrap();
    // Room for about one body: every one brought in pushes an earlier one out.
    s.loaded_budget = largest;
    s.view.search = "zymurgy".into();
    let rows = s.rows();
    assert_eq!((rows.len(), s.unsearched), (16, 0));
    assert!(s.resident_bytes() <= largest * 2, "{} bytes kept", s.resident_bytes());
    assert!(s.mailbox.messages.values().filter(|m| m.body_is_kept()).count() > 8, "most went back to disk");
    // Searched again with them on disk, it finds them again; a word nobody wrote, nothing.
    s.view.search = "zymurgy ".into();
    assert_eq!(s.rows().len(), 16);
    s.view.search = "xylophone".into();
    assert_eq!((s.rows().len(), s.unsearched), (0, 0));
    // However long it is given (past what the clock can say), it never panics.
    s.search_time = Duration::MAX;
    s.view.search = "zymurgy  ".into();
    assert_eq!(s.rows().len(), 16);
}

#[test]
fn a_rule_that_reads_the_body_reads_one_kept_on_disk() {
    let tmp = Temp::new("rule-body");
    let path = tmp.file();
    let mut mb = mailbox(24);
    let old = *newest_first(&mb).iter().rev().find(|id| mb.message(**id).unwrap().body.text.len() > INLINE_MAX).unwrap();
    mb.message_mut(old).unwrap().body.text.push_str("Only here: quixotic.\n");
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    assert!(s.mailbox.message(old).unwrap().body_is_kept());
    let inbox = s.mailbox.inbox().unwrap();
    let target = s.mailbox.folders.iter().find(|f| f.id != inbox).unwrap().id;
    let id = s.mailbox.alloc_id();
    s.mailbox.rules.push(Rule {
        id,
        name: "Quixotic".into(),
        enabled: true,
        conditions: vec![Condition::BodyContains("quixotic".into())],
        actions: vec![Action::MoveTo(target)],
        stop: false,
    });
    let said = s.run("rules.run", &json!({"folder": inbox})).unwrap();
    assert_eq!(said["matched"].as_u64(), Some(1));
    assert_eq!(s.mailbox.message(old).unwrap().folder, target);
}

/// A command that brings a whole message in, changes it, and fails.
fn load_then_fail(s: &mut Session, v: &Value) -> crate::CmdResult {
    let id = v["id"].as_u64().unwrap_or(0);
    s.load_message(id)?;
    if let Ok(m) = s.mailbox.message_mut(id) {
        m.read = true;
    }
    Err(CmdError::Failed("it fails".into()))
}

/// The bytes of `m`'s body and files in memory.
fn held(m: &Message) -> u64 {
    (m.body.text.len() + m.body.html.as_ref().map_or(0, String::len) + m.attachments.iter().map(|a| a.data.len()).sum::<usize>()) as u64
}

#[test]
fn bringing_content_in_is_no_change_and_a_failed_command_keeps_what_it_brought_counted() {
    let tmp = Temp::new("load-no-change");
    let path = tmp.file();
    let mb = mailbox(24);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let kept: Vec<Id> = newest_first(&mb).into_iter().filter(|id| s.mailbox.message(*id).unwrap().body_is_kept()).collect();
    // A command that reads a message brings its text in: nothing to undo, save or redraw.
    let (rev, first) = (s.rev(), kept[0]);
    let before = held(s.mailbox.message(first).unwrap());
    s.run("mail.get", &json!({"id": first})).unwrap();
    assert!(!s.mailbox.message(first).unwrap().body_is_kept());
    assert!(s.rev() == rev && !s.dirty && !s.can_undo());
    assert_eq!(s.resident_bytes(), held(s.mailbox.message(first).unwrap()) - before);

    // A command that fails after bringing content in leaves the mailbox as it was, but for that
    // content, which stays, counted.
    let mut specs = crate::cmd::registry().all().to_vec();
    specs.push(CommandSpec::new("test.loadThenFail", "Load Then Fail", "Test", load_then_fail));
    s.registry = Arc::new(Registry::new(specs));
    let other = with_text_and_file(&mb);
    let (counted, before) = (s.resident_bytes(), held(s.mailbox.message(other).unwrap()));
    assert!(s.run("test.loadThenFail", &json!({"id": other})).is_err());
    let m = s.mailbox.message(other).unwrap();
    assert!(!m.read && m.is_loaded());
    assert_eq!(s.resident_bytes(), counted + held(m) - before);
    assert!(!s.dirty && !s.can_undo());
}

#[test]
fn a_body_brought_in_before_a_file_that_fails_is_counted() {
    let tmp = Temp::new("load-partial");
    let path = tmp.file();
    let mb = mailbox(24);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let id = with_text_and_file(&mb);
    let was = mb.message(id).unwrap();
    fs::remove_file(blob_file(&path, &was.attachments[file_index(was)].data)).unwrap();
    let before = held(s.mailbox.message(id).unwrap());
    assert!(s.load_message(id).is_err());
    let m = s.mailbox.message(id).unwrap();
    assert!(!m.body_is_kept() && m.body.text == was.body.text);
    assert_eq!(s.resident_bytes(), held(m) - before);
    assert!(s.resident_bytes() > 0);
}

/// A source of content that notes the length of everything asked of it.
struct Noting {
    asked: Mutex<Vec<u64>>,
}

impl crate::io::ContentSource for Noting {
    fn fetch(&self, mailbox: &Path, r: &ContentRef) -> Result<Arc<Vec<u8>>, String> {
        self.asked.lock().unwrap().push(r.len);
        Content.fetch(mailbox, r)
    }
}

#[test]
fn a_reply_reads_the_text_and_its_pictures_and_a_calendar_part_too_large_is_not_read() {
    let tmp = Temp::new("reply-parts");
    let path = tmp.file();
    let mut mb = mailbox(4);
    let inbox = mb.inbox().unwrap();
    let mut m = Message { folder: inbox, subject: "With a picture".into(), from: Address::new("Ada", "ada@example.org"), ..Default::default() };
    m.body.text = long_text(3);
    m.body.html = Some(format!("<html><body><p>{}</p><img src=\"cid:logo@pic.example\"></body></html>", long_text(4)));
    let logo = Attachment { content_id: "logo@pic.example".into(), ..Attachment::new("logo.png", "image/png", noise(31, 3_001)) };
    m.attachments.push(logo);
    m.attachments.push(Attachment::new("report.bin", "application/octet-stream", noise(32, 9_003)));
    // An invitation too large to be read, and one that is not.
    m.attachments.push(Attachment::new("huge.ics", "text/calendar", noise(33, crate::session::MAX_CALENDAR_PART + 1)));
    m.attachments.push(Attachment::new("invite.ics", "text/calendar", b"BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n".repeat(40)));
    let id = mb.add_message(m).unwrap();
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let noting = Arc::new(Noting { asked: Mutex::new(Vec::new()) });
    s.content_source = Some(noting.clone());

    s.run("mail.reply", &json!({"id": id})).unwrap();
    let asked = std::mem::take(&mut *noting.asked.lock().unwrap());
    assert!(asked.contains(&3_001), "the picture is read: {asked:?}");
    assert!(!asked.contains(&9_003), "the file isn't: {asked:?}");
    let quote = s.compose.as_ref().unwrap().quote.as_ref().unwrap();
    assert_eq!(quote.pictures.len(), 1);
    assert_eq!(quote.pictures[0].data.len(), 3_001);
    s.compose = None;

    // Shown, the small invitation comes in; the large one stays where it is, and is not wanted.
    s.sel.messages = vec![id];
    s.load_for_display(id).unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.attachments[3].is_loaded() && !m.attachments[2].is_loaded() && !m.attachments[1].is_loaded());
    assert!(!s.needs_load_for_display(id));
    noting.asked.lock().unwrap().clear();
    let _ = s.run("calendar.invitation", &json!({"message": id}));
    let large = (crate::session::MAX_CALENDAR_PART + 1) as u64;
    assert!(!noting.asked.lock().unwrap().contains(&large));
}

#[test]
fn a_result_opened_during_a_search_that_reads_bodies_does_not_search_again() {
    let tmp = Temp::new("search-open");
    let path = tmp.file();
    let mut mb = mailbox(24);
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    for id in ids {
        let m = mb.message_mut(id).unwrap();
        if m.body.text.len() > INLINE_MAX {
            m.body.text.push_str("A word: zymurgy.\n");
        }
    }
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let noting = Arc::new(Noting { asked: Mutex::new(Vec::new()) });
    s.content_source = Some(noting.clone());
    s.loaded_budget = mb.messages.values().map(|m| body_outside(m)).max().unwrap();
    s.view.search = "zymurgy".into();
    let rows = s.rows();
    assert_eq!((rows.len(), s.unsearched), (16, 0));
    // A result whose body went back to disk is opened: its body comes in, and only it.
    let opened = rows.iter().map(|r| r.id).find(|id| s.mailbox.message(*id).unwrap().body_is_kept()).unwrap();
    s.sel.messages = vec![opened];
    noting.asked.lock().unwrap().clear();
    assert_eq!(s.load_for_display(opened), Ok(true));
    assert!(!noting.asked.lock().unwrap().is_empty());
    noting.asked.lock().unwrap().clear();
    // The list is built again (the load is seen) without bringing anything in, and still finds all.
    let again = s.rows();
    assert!(noting.asked.lock().unwrap().is_empty(), "searched again: {:?}", noting.asked.lock().unwrap());
    assert_eq!((again.len(), s.unsearched), (16, 0));
    // A different search does read again.
    s.view.search = "zymurgy ".into();
    assert_eq!(s.rows().len(), 16);
    assert!(!noting.asked.lock().unwrap().is_empty());
}

#[test]
fn a_pending_draft_and_an_outbox_message_reopened_from_disk_go_up_with_their_real_content() {
    use mailcraft_model::FolderKind;
    use mailcraft_model::sync::{DraftState, ServerDraft};
    let tmp = Temp::new("graph-up-whole");
    let path = tmp.file();
    let mut s = Session::at(Mailbox::with_local_account("", ""), 1_800_000_000, 0);
    let account = s.run("account.add", &json!({"email": "avery@mail.example", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let drafts = s.mailbox.folder_of_kind(account, FolderKind::Drafts).unwrap();
    let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    let file = noise(41, 9_001);
    let text = long_text(5);
    let message = |folder: Id, subject: &str| {
        let mut m =
            Message { account, folder, draft: true, subject: subject.into(), to: vec![Address::new("Kit", "kit@example.org")], ..Default::default() };
        m.body.text = text.clone();
        m.attachments.push(Attachment::new("plan.bin", "application/octet-stream", file.clone()));
        m
    };
    // Never up yet: everything goes. Up before, then retitled: only the subject goes.
    let mut fresh = message(drafts, "Fresh");
    fresh.server_draft = Some(ServerDraft { marker: "mailcraft-draft-a".into(), pending: true, saves: 1, ..Default::default() });
    let mut retitled = message(drafts, "Before");
    retitled.server = Some(mailcraft_model::sync::ServerMessage { id: "AAkA-retitled".into(), ..Default::default() });
    retitled.server_draft = Some(ServerDraft {
        marker: "mailcraft-draft-b".into(),
        synced: Some(DraftState::of(&retitled)),
        pending: true,
        saves: 2,
        ..Default::default()
    });
    retitled.subject = "After".into();
    let mut sending = message(outbox, "Going");
    sending.draft = false;
    let fresh = s.mailbox.add_message(fresh).unwrap();
    let retitled = s.mailbox.add_message(retitled).unwrap();
    let sending = s.mailbox.add_message(sending).unwrap();
    save(&path, &s.mailbox).unwrap();
    clear_cache();
    // After a restart, their bodies and files are on disk.
    let s = session_without_bodies(&path);
    for id in [fresh, retitled, sending] {
        let m = s.mailbox.message(id).unwrap();
        assert!(m.attachments.iter().all(|a| !a.is_loaded()), "kept on disk");
    }
    let req = crate::cmd::graph_mail::graph_mail_request(&s, account).unwrap();
    let up = |id: Id| req.drafts.iter().find(|d| d.message == id).unwrap();
    let whole = up(fresh);
    assert!(whole.body.as_ref().unwrap().content.contains(text.lines().next().unwrap()));
    let here = &whole.attachments.as_ref().unwrap().here;
    assert_eq!((here.len(), here[0].data.as_slice(), here[0].size), (1, file.as_slice(), file.len() as u64));
    let changed = up(retitled);
    assert_eq!(changed.subject.as_deref(), Some("After"));
    assert!(changed.body.is_none() && changed.attachments.is_none(), "nothing else looks changed");
    let send = req.sends.iter().find(|u| u.message == sending).unwrap();
    let mime = send.mime.as_ref().unwrap();
    assert_eq!((mime.body.text.as_str(), mime.attachments[0].data.as_slice()), (text.as_str(), file.as_slice()));
    // Reading them changed nothing in the mailbox.
    assert!(!s.dirty);
    // A file gone from disk: that one isn't sent or sent up empty, it waits.
    fs::remove_file(blob_file(&path, &file)).unwrap();
    clear_cache();
    let req = crate::cmd::graph_mail::graph_mail_request(&s, account).unwrap();
    assert!(req.sends.is_empty() && req.drafts.is_empty(), "{:?} {:?}", req.sends.len(), req.drafts.len());
}

/// A session whose "Brewing notes" conversation has two messages in `kind`: the newer with
/// "zymurgy" in a short body kept in the file, the older with it in a long body back on disk
/// after a search; conversation view, that search, and the conversation's row clicked (#503).
fn found_on_disk(tmp: &Temp, kind: mailcraft_model::FolderKind) -> (Session, Id, Id) {
    let path = tmp.file();
    let mut mb = mailbox(24);
    let folder = mb.folder_of_kind(mb.accounts[0].id, kind).unwrap();
    let by_subject = |mb: &Mailbox, subject: &str| first_with(mb, |m| m.subject == subject);
    let (newer, older) = (by_subject(&mb, "Message 21"), by_subject(&mb, "Message 7"));
    // Every message in that folder: the search reads the bodies on disk there, newest first.
    let all: Vec<Id> = mb.messages.keys().copied().collect();
    for id in all {
        mb.message_mut(id).unwrap().folder = folder;
    }
    for (id, word) in [(newer, " zymurgy"), (older, "A word: zymurgy.\n")] {
        let m = mb.message_mut(id).unwrap();
        m.subject = "Brewing notes".into();
        m.body.text.push_str(word);
    }
    assert!(mb.message(newer).unwrap().body.text.len() < INLINE_MAX && mb.message(older).unwrap().body.text.len() > INLINE_MAX);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    s.loaded_budget = mb.messages.values().map(|m| body_outside(m)).max().unwrap();
    s.run("folder.select", &json!({"id": folder})).unwrap();
    s.run("view.conversations", &json!({"value": true})).unwrap();
    s.run("view.search", &json!({"query": "zymurgy"})).unwrap();
    let rows = s.rows();
    assert_eq!(rows.iter().find(|r| r.id == newer).map(|r| r.members.clone()), Some(vec![newer, older]), "the window's row has both");
    assert!(s.mailbox.message(older).unwrap().body_is_kept(), "the older one's body went back to disk");
    s.run("mail.select", &json!({"id": newer})).unwrap();
    (s, newer, older)
}

/// What the owner is asked about an agent's call, as the window asks it (`agent_gate::gate`);
/// then, on the owner's yes, the call run for the agent as the window runs it. The agent names the
/// messages of the row it found, as `mail.list` lists them (#620).
fn asked_then_run(s: &mut Session, id: &str, row: Id, mut params: Value) -> String {
    let list = s.run("mail.list", &json!({})).unwrap();
    let item = list["messages"].as_array().and_then(|items| items.iter().find(|m| m["id"] == row).cloned()).unwrap();
    params["ids"] = item["conversation"].clone();
    let (class, what) = match crate::agent::check(s, id, &params, &[]) {
        crate::agent::Verdict::Ask(class, what) => (class, what),
        other => panic!("{other:?}"),
    };
    assert_eq!(crate::agent::check(s, id, &params, &[class]), crate::agent::Verdict::Run);
    s.agent = Some(vec![class]);
    s.run(id, &params).unwrap();
    s.agent = None;
    what
}

#[test]
fn an_agents_delete_for_good_in_a_search_is_asked_about_what_it_deletes() {
    let tmp = Temp::new("agent-for-good");
    let (mut s, newer, older) = found_on_disk(&tmp, mailcraft_model::FolderKind::Deleted);
    let asked = asked_then_run(&mut s, "mail.delete", newer, json!({}));
    let gone = [newer, older].into_iter().filter(|id| s.mailbox.message(*id).is_none()).count();
    assert_eq!((asked.as_str(), gone), ("delete 2 messages (\u{201c}Brewing notes\u{201d}, \u{201c}Brewing notes\u{201d}) for good", 2));
}

#[test]
fn an_agents_move_to_the_outbox_in_a_search_is_asked_about_what_it_sends() {
    let tmp = Temp::new("agent-outbox");
    let (mut s, newer, older) = found_on_disk(&tmp, mailcraft_model::FolderKind::Inbox);
    let outbox = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, mailcraft_model::FolderKind::Outbox).unwrap();
    let asked = asked_then_run(&mut s, "mail.move", newer, json!({"folder": outbox}));
    let sent = [newer, older].into_iter().filter(|id| s.mailbox.message(*id).is_some_and(|m| m.folder == outbox)).count();
    assert_eq!(
        (asked.as_str(), sent),
        ("put 2 messages (\u{201c}Brewing notes\u{201d}, \u{201c}Brewing notes\u{201d}) in the Outbox, which sends them", 2)
    );
}
