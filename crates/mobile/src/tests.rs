// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};

use mailcraft_engine::Session;
use mailcraft_model::{Attachment, FolderKind, Message};
use serde_json::{Value, json};

use crate::blocks::{BlockKindView, BlockView};
use crate::fake_tests::{FakePlatform, block_on, now, sample_core};
use crate::{Core, CoreError, Event, NetworkKind};

fn exec(core: &Core, id: &str, params: Value) -> Result<Value, CoreError> {
    block_on(core.execute(id.to_string(), params.to_string())).map(|s| serde_json::from_str(&s).unwrap())
}

fn inbox(core: &Core) -> u64 {
    let list = block_on(core.folders(0)).unwrap();
    list.folders.iter().find(|f| f.kind == "inbox").unwrap().id
}

#[test]
fn execute_runs_a_command_and_the_revision_event_follows() {
    let core = sample_core();
    let before = block_on(core.folders(0)).unwrap().rev;
    let window = block_on(core.messages(inbox(&core), "all".into(), 0, 5)).unwrap();
    let id = window.rows[0].id;
    let flagged = window.rows[0].flagged;
    exec(&core, "mail.flag", json!({"id": id})).unwrap();
    let after = block_on(core.messages(inbox(&core), "all".into(), 0, 5)).unwrap();
    assert_ne!(after.rows[0].flagged, flagged);
    assert!(after.rev > before);
    match block_on(core.next_event()) {
        Some(Event::RevChanged { rev }) => assert_eq!(rev, after.rev),
        other => panic!("{other:?}"),
    }
}

#[test]
fn execute_refuses_what_is_not_a_request() {
    let core = sample_core();
    assert!(matches!(block_on(core.execute("no.such.command".into(), "{}".into())), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.execute("mail.flag".into(), "{not json".into())), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.execute("x".repeat(10_000), "{}".into())), Err(CoreError::BadRequest { .. })));
    // A command's own refusal is a failure, and leaves the mailbox as it was.
    let rev = block_on(core.folders(0)).unwrap().rev;
    let refused = block_on(core.execute("mail.flag".into(), json!({"id": 18446744073709551615u64}).to_string()));
    assert!(refused.is_err());
    assert_eq!(block_on(core.folders(0)).unwrap().rev, rev);
    // Empty parameters mean none.
    assert!(block_on(core.execute("view.today".into(), String::new())).is_ok());
}

#[test]
fn a_platform_with_no_memory_is_an_error_not_a_panic() {
    assert!(matches!(Core::start(FakePlatform::with_budget(0), Session::empty), Err(CoreError::Init { .. })));
    let core = Core::start(FakePlatform::with_budget(1 << 20), Session::empty).unwrap();
    assert_eq!(core.memory_budget(), 1 << 20);
    assert_eq!(core.network(), NetworkKind::Online { metered: true, data_saving: false });
}

#[test]
fn a_panic_in_a_job_is_an_error_and_the_core_goes_on() {
    let core = sample_core();
    let r: Result<(), CoreError> = block_on(core.call(|_| -> Result<(), CoreError> { panic!("a job that goes wrong") }));
    assert_eq!(r, Err(CoreError::Internal));
    assert!(block_on(core.folders(0)).is_ok());
}

#[test]
fn a_call_dropped_before_it_starts_never_runs() {
    let core = sample_core();
    let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
    let (started_tx, started) = std::sync::mpsc::channel::<()>();
    // Hold the session thread.
    let hold = block_on_in_thread(core.clone(), move |c| {
        let _ = block_on(c.call(move |_| {
            let _ = started_tx.send(());
            let _ = gate_rx.recv();
            Ok(())
        }));
    });
    // The session thread is inside the job before the call to be dropped is made.
    started.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    let ran = Arc::new(AtomicBool::new(false));
    let flag = ran.clone();
    {
        let mut future = Box::pin(core.call(move |_| {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        }));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
        // (Dropped here, as the app's coroutine being cancelled drops it.)
    }
    gate_tx.send(()).unwrap();
    hold.join().unwrap();
    assert!(block_on(core.folders(0)).is_ok());
    assert!(!ran.load(Ordering::SeqCst));
}

fn block_on_in_thread(core: Arc<Core>, f: impl FnOnce(&Core) + Send + 'static) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || f(&core))
}

#[test]
fn shutdown_cancels_later_calls_and_ends_the_events() {
    let core = sample_core();
    core.shutdown();
    assert_eq!(block_on(core.folders(0)), Err(CoreError::Cancelled));
    assert_eq!(block_on(core.next_event()), None);
}

#[test]
fn the_sink_carries_the_other_events_in_order() {
    let core = sample_core();
    let sink = core.sink();
    sink.push(Event::SyncProgress { account: 1, phase: "Inbox".into(), done: 3, total: 9 });
    sink.push(Event::SignInNeeded { account: 1 });
    sink.push(Event::PermissionQuestion { account: 1, question: "tasks.sendLocal".into() });
    sink.push(Event::ReminderScheduleChanged);
    assert!(matches!(block_on(core.next_event()), Some(Event::SyncProgress { done: 3, .. })));
    assert!(matches!(block_on(core.next_event()), Some(Event::SignInNeeded { account: 1 })));
    assert!(matches!(block_on(core.next_event()), Some(Event::PermissionQuestion { .. })));
    assert!(matches!(block_on(core.next_event()), Some(Event::ReminderScheduleChanged)));
}

#[test]
fn the_event_queue_keeps_the_newest_when_nobody_reads() {
    let core = sample_core();
    let sink = core.sink();
    for n in 0..(crate::queue::MOST_QUEUED as u64 + 10) {
        sink.push(Event::SignInNeeded { account: n });
    }
    assert_eq!(sink.queue.len(), crate::queue::MOST_QUEUED);
    assert!(matches!(block_on(core.next_event()), Some(Event::SignInNeeded { account: 10 })));
}

#[test]
fn text_goes_to_the_catalogue() {
    let core = sample_core();
    assert_eq!(core.t("Inbox".into()), "Inbox");
    assert_eq!(core.t("text with no translation".into()), "text with no translation");
    assert_eq!(core.tn(1, "{n} message".into(), "{n} messages".into()), "1 message");
    assert_eq!(core.tn(3, "{n} message".into(), "{n} messages".into()), "3 messages");
    core.set_language("pseudo".into());
    assert_ne!(core.t("Inbox".into()), "Inbox");
    core.set_language("zz".into());
    assert_eq!(core.t("Inbox".into()), "Inbox");
}

#[test]
fn folders_come_with_their_counts() {
    let core = sample_core();
    let list = block_on(core.folders(0)).unwrap();
    assert_eq!(list.accounts.len(), 1);
    let inbox = list.folders.iter().find(|f| f.kind == "inbox").unwrap();
    assert!(inbox.total > 0 && inbox.unread > 0 && inbox.unread <= inbox.total);
    assert!(list.folders.iter().any(|f| f.name == "Atlas" && f.favorite));
    assert!(block_on(core.folders(999_999)).unwrap().folders.is_empty());
}

#[test]
fn a_message_window_is_a_slice_of_the_list_and_leaves_the_view_alone() {
    let core = sample_core();
    let folder = inbox(&core);
    let all = block_on(core.messages(folder, "all".into(), 0, 200)).unwrap();
    assert_eq!(all.total as usize, all.rows.len());
    let page = block_on(core.messages(folder, "all".into(), 1, 2)).unwrap();
    assert_eq!(page.rows.len(), 2.min(all.rows.len().saturating_sub(1)));
    assert_eq!(page.rows[0].id, all.rows[1].id);
    assert_eq!(page.total, all.total);
    let unread = block_on(core.messages(folder, "unread".into(), 0, 200)).unwrap();
    assert!(unread.rows.iter().all(|r| r.unread) && !unread.rows.is_empty());
    assert!(unread.total < all.total);
    assert!(block_on(core.messages(folder, "all".into(), 1_000_000, 5)).unwrap().rows.is_empty());
    // A count of 0 or of billions is held to the window.
    assert_eq!(block_on(core.messages(folder, "all".into(), 0, 0)).unwrap().rows.len(), 1);
    assert!(block_on(core.messages(folder, "all".into(), 0, u32::MAX)).unwrap().rows.len() <= 200);
    assert!(matches!(block_on(core.messages(folder, "nonsense".into(), 0, 5)), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.messages(0, "all".into(), 0, 5)), Err(CoreError::BadRequest { .. })));
    // The same window again is the same window: the view the list is built for is as it was.
    assert_eq!(block_on(core.messages(folder, "all".into(), 0, 200)).unwrap(), all);
}

#[test]
fn the_calendar_range_lists_what_happens_in_it() {
    let core = sample_core();
    let week = block_on(core.calendar("2026-10-12".into(), "2026-10-18".into())).unwrap();
    assert!(!week.events.is_empty());
    assert!(week.events.windows(2).all(|w| w[0].start <= w[1].start));
    assert!(matches!(block_on(core.calendar("2026-10-12".into(), "2027-12-31".into())), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.calendar("2026-10-18".into(), "2026-10-12".into())), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.calendar("soon".into(), "later".into())), Err(CoreError::BadRequest { .. })));
}

#[test]
fn people_and_tasks_and_today_answer() {
    let core = sample_core();
    let people = block_on(core.people(String::new(), 0, 200)).unwrap();
    assert!(people.total > 0 && people.people.len() == people.total as usize);
    assert!(people.people.windows(2).all(|w| w[0].name.to_lowercase() <= w[1].name.to_lowercase()));
    let name = people.people[0].name.clone();
    let found = block_on(core.people(name.to_uppercase(), 0, 10)).unwrap();
    assert!(found.people.iter().any(|p| p.name == name));
    assert_eq!(block_on(core.people("zzzzzz-nobody".into(), 0, 10)).unwrap().total, 0);

    let lists = block_on(core.task_lists()).unwrap();
    assert!(!lists.lists.is_empty());
    let list = &lists.lists[0];
    let tasks = block_on(core.tasks(list.account, list.list.clone(), false, 0, 50)).unwrap();
    assert_eq!(tasks.total, list.open);
    assert!(tasks.tasks.iter().all(|t| !t.completed));
    assert!(tasks.tasks.windows(2).all(|w| w[0].due.is_some() || w[1].due.is_none()));

    let today = block_on(core.today()).unwrap();
    assert_eq!(today.date, "2026-10-14");
    assert!(today.unread > 0);
    assert!(today.tasks.iter().all(|t| t.due.as_deref().is_some_and(|d| d <= "2026-10-14")));
    assert!(!today.tasks.is_empty());
}

fn first_with_html(core: &Core) -> u64 {
    let rows = block_on(core.messages(inbox(core), "all".into(), 0, 200)).unwrap().rows;
    rows.iter().find(|r| r.subject.contains("invoice")).unwrap().id
}

#[test]
fn a_message_is_read_as_display_blocks() {
    let core = sample_core();
    let id = first_with_html(&core);
    let view = block_on(core.message(id, 390.0, false)).unwrap();
    assert_eq!(view.subject, "Your October invoice is ready");
    assert!(view.unread, "reading it doesn't mark it read");
    assert!(view.blocks.iter().any(|b| matches!(b.kind, BlockKindView::Heading { .. })));
    let linked = view.blocks.iter().flat_map(|b| &b.spans).any(|s| s.link.as_deref() == Some("https://cloudline.example/billing"));
    assert!(linked);
    // A plain-text message reads from its text.
    let rows = block_on(core.messages(inbox(&core), "all".into(), 0, 200)).unwrap().rows;
    let plain = rows.iter().find(|r| r.subject.starts_with("Q4 roadmap")).unwrap().id;
    let view = block_on(core.message(plain, f32::NAN, true)).unwrap();
    assert!(!view.blocks.is_empty());
    assert!(matches!(block_on(core.message(u64::MAX, 390.0, false)), Err(CoreError::Failed { .. })));
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89";

/// A core whose mailbox has a message with a picture of each kind: a part, one written into the
/// message, one on the web, and a part that isn't a picture.
fn pictures_core() -> (Arc<Core>, u64) {
    use base64_free::encode;
    let html = format!(
        "<p>Hello</p><img src=\"cid:logo\" width=\"20\"><img src=\"data:image/png;base64,{}\"><img src=\"https://tracker.example/x.png\" width=\"40\"><img src=\"cid:junk\">",
        encode(PNG)
    );
    let core = Core::start(FakePlatform::new(), move || {
        let mut mb = mailcraft_model::Mailbox::with_local_account("Avery", "avery@example.org");
        let account = mb.accounts[0].id;
        let folder = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
        let mut m = Message { account, folder, subject: "Pictures".into(), ..Message::default() };
        m.body.text = "Hello".into();
        m.body.html = Some(html);
        m.attachments = vec![
            Attachment { content_id: "logo".into(), inline: true, ..Attachment::new("logo.png", "image/png", PNG.to_vec()) },
            Attachment { content_id: "junk".into(), inline: true, ..Attachment::new("junk.png", "image/png", b"not a picture".to_vec()) },
        ];
        mb.add_message(m).unwrap();
        Session::at(mb, now(), 0)
    })
    .unwrap();
    let id = block_on(core.messages(inbox(&core), "all".into(), 0, 5)).unwrap().rows[0].id;
    (core, id)
}

/// Base 64 for the test's own picture, without a dependency for it.
mod base64_free {
    pub fn encode(bytes: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(char::from(A[((n >> (18 - 6 * i)) & 63) as usize]));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}

fn images(blocks: &[BlockView]) -> Vec<(String, Option<String>)> {
    blocks
        .iter()
        .filter_map(|b| match &b.kind {
            BlockKindView::Image { src, picture, .. } => Some((src.clone(), picture.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn pictures_come_by_reference_and_only_as_pictures() {
    let (core, id) = pictures_core();
    let view = block_on(core.message(id, 400.0, false)).unwrap();
    let all = images(&view.blocks);
    assert_eq!(all.len(), 4, "{all:?}");
    assert_eq!(all[0].1.as_deref(), Some("cid:logo"));
    let data_ref = all[1].1.clone().unwrap();
    assert!(data_ref.starts_with("data:0:"));
    assert_eq!(all[2].1, None, "a picture on the web has no bytes here");
    assert_eq!(view.remote_pictures, 1);

    for reference in ["cid:logo".to_string(), data_ref] {
        let picture = block_on(core.picture(id, reference)).unwrap();
        assert_eq!(picture.mime, "image/png");
        assert_eq!(picture.bytes, PNG);
    }
    // A part that is not a picture, no such part, and nonsense are errors.
    assert!(matches!(block_on(core.picture(id, "cid:junk".into())), Err(CoreError::Failed { .. })));
    for reference in ["cid:nothing", "data:9:400:0", "data:x", "https://tracker.example/x.png", "", "data:"] {
        assert!(block_on(core.picture(id, reference.into())).is_err(), "{reference}");
    }
    assert!(block_on(core.picture(u64::MAX, "cid:logo".into())).is_err());
}

#[test]
fn records_stay_bounded_for_a_deep_and_wide_message() {
    let html = format!("{}x{}", "<table><tr><td>".repeat(40), "</td></tr></table>".repeat(40));
    let wide = format!("<table><tr>{}</tr></table>", "<td>c</td>".repeat(500));
    let core = Core::start(FakePlatform::new(), move || {
        let mut mb = mailcraft_model::Mailbox::with_local_account("Avery", "avery@example.org");
        let account = mb.accounts[0].id;
        let folder = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
        for (subject, html) in [("deep", html.clone()), ("wide", wide.clone())] {
            let mut m = Message { account, folder, subject: subject.into(), ..Message::default() };
            m.body.html = Some(html);
            mb.add_message(m).unwrap();
        }
        Session::at(mb, now(), 0)
    })
    .unwrap();
    for row in block_on(core.messages(inbox(&core), "all".into(), 0, 10)).unwrap().rows {
        let view = block_on(core.message(row.id, 300.0, false)).unwrap();
        assert!(!view.blocks.is_empty());
    }
}

#[test]
fn a_message_window_leaves_the_selection_and_the_list_cache_of_the_view_alone() {
    let core = sample_core();
    let folder = inbox(&core);
    let other = block_on(core.folders(0)).unwrap().folders.iter().find(|f| f.id != folder && f.total > 0).map(|f| f.id).unwrap_or(folder);
    exec(&core, "folder.select", json!({"id": folder})).unwrap();
    let id = block_on(core.messages(folder, "unread".into(), 0, 1)).unwrap().rows[0].id;
    exec(&core, "mail.select", json!({"id": id})).unwrap();
    let state = |core: &Core| block_on(core.call(|s| Ok((format!("{:?}", s.sel), format!("{:?}", s.view), s.rows().len())))).unwrap();
    let before = state(&core);
    for filter in ["all", "unread", "flagged"] {
        assert!(block_on(core.messages(other, filter.into(), 0, 5)).is_ok());
    }
    assert_eq!(state(&core), before);
}

#[test]
fn a_message_with_a_big_attachment_reads_without_the_attachment() {
    let core = Core::start(FakePlatform::new(), || {
        let mut mb = mailcraft_model::Mailbox::with_local_account("Avery", "avery@example.org");
        let account = mb.accounts[0].id;
        let folder = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
        let mut m = Message { account, folder, subject: "Big".into(), ..Message::default() };
        m.body.text = "See the file".into();
        m.attachments = vec![Attachment::new("big.bin", "application/octet-stream", vec![7u8; 8 << 20])];
        mb.add_message(m).unwrap();
        Session::at(mb, now(), 0)
    })
    .unwrap();
    let id = block_on(core.messages(inbox(&core), "all".into(), 0, 5)).unwrap().rows[0].id;
    let view = block_on(core.message(id, 390.0, false)).unwrap();
    assert!(!view.blocks.is_empty());
    assert_eq!(view.attachments.len(), 1);
    assert_eq!(view.attachments[0].size, 8 << 20);
}
