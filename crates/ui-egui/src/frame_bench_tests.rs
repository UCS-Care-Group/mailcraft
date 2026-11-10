// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! How long a frame of the main window takes over a mailbox the size of a large one: three
//! accounts, 240 folders and 326,000 messages (headers and a short body each), as an owner with
//! years of mail in a few Microsoft 365 mailboxes has. Ignored by `cargo test`; run it with
//!
//! ```text
//! cargo test --release -p mailcraft-ui-egui frame_bench -- --ignored --nocapture
//! ```
//!
//! It prints the time of a frame while nothing happens, after a change to the mailbox (as a
//! Send/Receive makes every minute), after a selection, a scroll, a folder change and a search:
//! what the window spends between two frames, before anything is drawn to the screen.
//! `MAILCRAFT_FRAME_BENCH_MESSAGES` makes it another size, and `MAILCRAFT_FRAME_BENCH_SECONDS`
//! keeps it going on steady frames for that long afterwards, for a profiler to look at (with
//! `MAILCRAFT_FRAME_BENCH_OPEN` set, with a message open in the reading pane; with
//! `MAILCRAFT_FRAME_BENCH_CHANGES` set, with a change to the mailbox before every frame; with
//! `MAILCRAFT_FRAME_BENCH_SELECT` set, with another message selected before every frame; with
//! `MAILCRAFT_FRAME_BENCH_RANGE=n` set, with the first row clicked and then Shift+clicked to row
//! `n` before every frame).
//!
//! Measured on 2026-11-01 (8 cores, release build), `main` before the counts (e26e4ad, with this
//! file added) against after; a frame's time in milliseconds, the median of a steady run or the
//! range of single frames:
//!
//! | Frame                                   | Before   | After  |
//! |-----------------------------------------|----------|--------|
//! | The first four frames, together         | 292      | 7      |
//! | Steady, nothing happening               | 61       | 0.7    |
//! | Steady with a message open              | 110      | 1.3    |
//! | After a scroll                          | 82–134   | 0.8–1.2 |
//! | After a folder change                   | 142      | 5      |
//! | Steady in another folder                | 61       | 0.6    |
//! | Back in the Inbox (98,000 messages)     | 132      | 39     |
//! | After a search                          | 75       | 5      |
//! | Steady while searching                  | 73       | 2.6    |
//! | After a change (a read mark)            | 51–135   | 50–65  |
//! | After a selection (a message opened)    | 93–180   | 72–102 |
//!
//! The frame after a change or a selection, which builds the Inbox's list again, is the hitch
//! that is left.
//!
//! A Shift+click, measured on 2026-11-02 before and after selecting became a reading command
//! (no copy of the mailbox taken and compared; the message opened is marked read after it): the
//! command's own time in milliseconds.
//!
//! | Shift+click over        | Before | After |
//! |-------------------------|--------|-------|
//! | 10 rows                 | 38–56  | 0.1–0.2 |
//! | 1,000 rows              | 34–47  | 0.7–1.3 |
//! | 10,000 rows             | 47–60  | 16    |
//! | 50,000 rows (33,807 messages) | 132–152 | 65 |
//!
//! A click or an arrow key that opens a message saves the same 40 ms of copying.
//!
//! The frame after a selection, measured on 2026-11-02 before and after the reading pane's
//! conversation came from an index of the conversations rather than a walk over the account's
//! 108,000 messages: 65–90 ms before, 1.5–3.6 ms after. (That index, keyed by subject, went
//! with #795's conversations by thread: see 2026-11-05 below.)
//!
//! A change to the mailbox, measured on 2026-11-02 before and after the messages were kept in
//! chunks shared between the mailbox and its copies (`mailcraft_model::Messages`), the folder
//! counts were brought in step with a change rather than counted again, and a change that
//! touches nothing the list is built from left the list standing:
//!
//! | What                                         | Before   | After    |
//! |----------------------------------------------|----------|----------|
//! | The mailbox's messages copied (each change)  | 19       | 0.1      |
//! | Compared with the copy                       | 14       | 0.1      |
//! | The copy let go of                           | 18       | 0.1      |
//! | Frame after a read mark                      | 40–65    | 0.7–3.7  |
//! | A flag: the command, then the frame          | ~40, 40–58 | 0.2, 0.8–1.5 |
//! | A move: the command, then the frame          | ~40, 40–58 | 0.7, 27–31 |
//! | The index of conversations brought in step   | 10       | 0.1      |
//!
//! A move changes what the Inbox lists, which is built again (98,000 rows, 27 ms).
//!
//! Then, the same day: the window holds the list's rows weakly (a read mark copied all of them
//! to put one row's unread count right: the command 10 ms, now 0.3–0.5); a Shift+click from a
//! listed anchor takes the span of rows without asking of each message whether it is listed
//! (10,000 rows 11–25 ms, now 7; 33,800 messages 50–70, now 21); Undo merges only the messages
//! its versions differ in, brings the counts in step itself and builds the list once, and the
//! known correspondents follow the map chunk by chunk:
//!
//! | Undo, the command and the frame after it    | Before      | After     |
//! |---------------------------------------------|-------------|-----------|
//! | Of a thousand deleted                        | 340, 107    | 58, 31    |
//! | Of every Inbox message marked read (98,000)  | 388, 127    | 115, 57   |
//!
//! Measured on 2026-11-05, with #795's conversations by thread (`mailcraft_model::ThreadIndex`,
//! built in one pass over every message and kept by `mailcraft_engine::conversations`, which
//! checks it chunk by chunk after a change: a chunk the mailbox and its copy share isn't looked
//! into, and in one they don't, a message that is the same `Arc` isn't hashed again). The
//! conversations are built in the window's first frame over the mailbox (205–270 ms, once), and
//! worked out again only when a message comes or goes or a header or a server's conversation id
//! changes:
//!
//! | The command, and the frame after it           | 2026-11-05 |
//! |-----------------------------------------------|------------|
//! | A click (the frame after a selection)         | 1.3–3.9    |
//! | A read mark                                   | 0.5, 0.7–1.0 |
//! | A flag                                        | 0.1–0.5, 0.7 |
//! | A move                                        | 0.4, 20–23 |
//! | Shift+click over 10 / 10,000 / 33,800 rows    | 0.0 / 4 / 14, then 1–3 |
//! | Mark All as Read (98,000), and its Undo       | 89, 44; 109, 40 |
//! | Delete a thousand, and its Undo               | 31, 14; 32, 13 |
//! | A message copied in (the conversations again) | 6–7, 280–310; its Undo 24–28, 270 |

use std::time::Instant;

use mailcraft_engine::Session;
use mailcraft_model::{Account, AccountKind, Address, Body, FlagStatus, Id, Mailbox, Message};

use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;
const MESSAGES: usize = 326_000;
const ACCOUNTS: usize = 3;
const FOLDERS_PER_ACCOUNT: usize = 80;

/// Words to make text of, none of anybody's mail.
const WORDS: [&str; 16] = [
    "harbour", "lantern", "meadow", "quarry", "saddle", "thimble", "orchard", "pebble", "copper", "falcon", "willow", "cobalt", "marble", "tundra",
    "velvet", "juniper",
];

fn text(seed: usize, len: usize) -> String {
    let mut out = String::with_capacity(len + 16);
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.push_str(WORDS[x % WORDS.len()]);
        out.push(' ');
    }
    out
}

fn make(messages: usize) -> Mailbox {
    let mut mb = Mailbox::default();
    let mut inboxes = Vec::new();
    let mut folders: Vec<Vec<Id>> = Vec::new();
    for a in 0..ACCOUNTS {
        let account = mb.add_account(Account {
            name: format!("Mailbox {a}"),
            email: format!("avery{a}@example.org"),
            display_name: "Avery Quinn".into(),
            kind: AccountKind::Microsoft,
            ..Default::default()
        });
        let inbox = mb.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
        inboxes.push(inbox);
        let mut own = vec![inbox];
        let mut parent = None;
        for f in 0..FOLDERS_PER_ACCOUNT {
            let id = mb.add_folder(account, parent, &format!("Project {f}")).unwrap();
            own.push(id);
            parent = if f % 5 == 4 { None } else { Some(id) };
        }
        folders.push(own);
    }
    for i in 0..messages {
        let a = i % ACCOUNTS;
        // Three in ten in the Inbox, the rest spread over the folders.
        let folder = if i % 10 < 3 { inboxes[a] } else { folders[a][(i / 7) % folders[a].len()] };
        let m = Message {
            folder,
            subject: format!("{} {}", text(i, 24).trim(), i / 40),
            from: Address::new(format!("Sender {}", i % 500), format!("sender{}@example.org", i % 500)),
            to: vec![Address::new("Avery Quinn", format!("avery{a}@example.org"))],
            date: NOW - (messages - i) as i64 * 900,
            received: NOW - (messages - i) as i64 * 900,
            message_id: format!("<{i}@example.org>"),
            body: Body { text: text(i, 240), html: None },
            read: i % 5 != 0,
            flag: if i % 50 == 0 { FlagStatus::Flagged } else { FlagStatus::None },
            ..Default::default()
        };
        mb.add_message(m).unwrap();
    }
    mb
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// `n` frames, timed one by one: the median and the slowest.
fn frames(harness: &mut egui_kittest::Harness<'static, MailApp>, n: usize) -> (f64, f64) {
    let mut times = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        harness.step();
        times.push(ms(t.elapsed()));
    }
    times.sort_by(|a, b| a.total_cmp(b));
    (times[n / 2], times[n - 1])
}

/// How long turning the last frame's shapes into triangles takes, as the window does before it
/// draws (the harness itself doesn't).
fn tessellate(harness: &egui_kittest::Harness<'static, MailApp>) -> (f64, usize) {
    let out = harness.output().clone();
    let t = Instant::now();
    let meshes = harness.ctx.tessellate(out.shapes, out.pixels_per_point);
    (ms(t.elapsed()), meshes.len())
}

fn report(what: &str, harness: &mut egui_kittest::Harness<'static, MailApp>, n: usize) {
    let (median, worst) = frames(harness, n);
    eprintln!("FRAME_BENCH {what}: median {median:.1} ms, worst {worst:.1} ms over {n} frames");
}

#[test]
#[ignore = "a benchmark: builds a mailbox of 326,000 messages in memory; see the module's documentation"]
fn frame_bench() {
    let messages = std::env::var("MAILCRAFT_FRAME_BENCH_MESSAGES").ok().and_then(|s| s.parse().ok()).unwrap_or(MESSAGES);
    let making = Instant::now();
    let mb = make(messages);
    eprintln!("FRAME_BENCH made {} messages in {:.0} ms", mb.messages.len(), ms(making.elapsed()));
    let inbox = mb.inbox().unwrap();
    {
        let t = Instant::now();
        let threads = mailcraft_model::ThreadIndex::build(mb.messages.values().map(|m| m.as_ref()));
        eprintln!("FRAME_BENCH the conversations (ThreadIndex), built: {:.1} ms, {} steps", ms(t.elapsed()), threads.work());
    }
    let t = Instant::now();
    let mut app = MailApp::new(Session::at(mb, NOW, 0), Services::default());
    eprintln!("FRAME_BENCH the window made over it: {:.0} ms", ms(t.elapsed()));
    // What the window's first frame does before anything else (`Session::prepare_conversations`;
    // the harness runs that frame as it is built, so it is done here, timed).
    let t = Instant::now();
    let built = app.session.prepare_conversations();
    eprintln!("FRAME_BENCH the conversations, worked out for the window: {} in {:.1} ms", if built { "built" } else { "in step" }, ms(t.elapsed()));
    app.autosave = false;
    app.auto_sync = false;
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    let first = Instant::now();
    harness.run_steps(4);
    eprintln!("FRAME_BENCH first four frames: {:.0} ms", ms(first.elapsed()));
    // (Asked again here, they are in step and nothing is built.)
    let t = Instant::now();
    let built = harness.state_mut().session.prepare_conversations();
    eprintln!("FRAME_BENCH the conversations after the first frames: {} in {:.1} ms", if built { "built now" } else { "in step" }, ms(t.elapsed()));
    // (The conversations were worked out in those frames: `Session::prepare_conversations`.)
    let _ = harness.state_mut().execute("ui.dismissReminder", serde_json::json!({"all": true}));
    harness.run_steps(2);
    report("steady", &mut harness, 20);

    // A change to the mailbox, as every Send/Receive makes: the list is built again.
    let ids: Vec<Id> = harness.state().session.mailbox.messages_in(inbox).map(|m| m.id).take(40).collect();
    for (i, id) in ids.iter().take(5).enumerate() {
        let t = Instant::now();
        let _ = harness.state_mut().execute("mail.markUnread", serde_json::json!({"id": id}));
        let took = ms(t.elapsed());
        let t = Instant::now();
        harness.step();
        eprintln!("FRAME_BENCH frame after a change {i}: command {took:.1} ms, next frame {:.1} ms", ms(t.elapsed()));
    }
    report("steady after changes", &mut harness, 10);

    // A flag and a move, which change what the list shows (not read marks alone): the command,
    // and the frame after it.
    let archive = harness.state().session.mailbox.folders.iter().find(|f| f.kind == mailcraft_model::FolderKind::Archive).map(|f| f.id).unwrap();
    for (i, (command, params)) in [
        ("mail.flag", serde_json::json!({"id": ids[30]})),
        ("mail.flag", serde_json::json!({"id": ids[31]})),
        ("mail.move", serde_json::json!({"id": ids[32], "folder": archive})),
        ("mail.move", serde_json::json!({"id": ids[33], "folder": archive})),
    ]
    .into_iter()
    .enumerate()
    {
        let t = Instant::now();
        let _ = harness.state_mut().run(command, params);
        let took = ms(t.elapsed());
        let t = Instant::now();
        harness.step();
        eprintln!("FRAME_BENCH {command} {i}: command {took:.1} ms, next frame {:.1} ms", ms(t.elapsed()));
    }

    for (i, id) in ids.iter().skip(5).take(5).enumerate() {
        let _ = harness.state_mut().execute("mail.select", serde_json::json!({"id": id}));
        let t = Instant::now();
        harness.step();
        eprintln!("FRAME_BENCH frame after a selection {i}: {:.1} ms", ms(t.elapsed()));
    }
    report("steady with a message open", &mut harness, 10);

    for i in 0..5 {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -400.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        });
        let t = Instant::now();
        harness.step();
        let frame = ms(t.elapsed());
        let (tess, meshes) = tessellate(&harness);
        eprintln!("FRAME_BENCH frame after a scroll {i}: {frame:.1} ms, tessellated in {tess:.1} ms ({meshes} meshes)");
    }

    let other = harness.state().session.mailbox.folders.iter().find(|f| f.name == "Project 3").map(|f| f.id).unwrap();
    let _ = harness.state_mut().execute("folder.select", serde_json::json!({"id": other}));
    let t = Instant::now();
    harness.step();
    eprintln!("FRAME_BENCH frame after a folder change: {:.1} ms", ms(t.elapsed()));
    report("steady in another folder", &mut harness, 10);
    let _ = harness.state_mut().execute("folder.select", serde_json::json!({"id": inbox}));
    let t = Instant::now();
    harness.step();
    eprintln!("FRAME_BENCH frame back in the Inbox: {:.1} ms", ms(t.elapsed()));

    let _ = harness.state_mut().execute("view.search", serde_json::json!({"query": "harbour lantern"}));
    let t = Instant::now();
    harness.step();
    eprintln!("FRAME_BENCH frame after a search: {:.1} ms", ms(t.elapsed()));
    report("steady while searching", &mut harness, 10);
    let _ = harness.state_mut().execute("view.clearSearch", serde_json::json!({}));
    harness.step();

    // A Shift+click from the first row to rows further down, as the list's own click runs it.
    let rows: Vec<(Id, u16)> = harness.state_mut().session.rows().iter().map(|r| (r.id, r.copy)).collect();
    for reach in [10usize, 100, 1_000, 10_000, 50_000] {
        let Some(&(first, _)) = rows.first() else { break };
        let Some(&(to, copy)) = rows.get(reach.min(rows.len().saturating_sub(1))) else { break };
        let _ = harness.state_mut().run("mail.select", serde_json::json!({"id": first}));
        harness.step();
        let t = Instant::now();
        let _ = harness.state_mut().run("mail.select", serde_json::json!({"id": to, "copy": copy, "range": true}));
        let command = ms(t.elapsed());
        let t = Instant::now();
        harness.step();
        let frame = ms(t.elapsed());
        let selected = harness.state().session.sel.messages.len();
        eprintln!("FRAME_BENCH shift+click over {reach} rows ({selected} selected): command {command:.1} ms, next frame {frame:.1} ms");
        report(&format!("steady with {selected} selected"), &mut harness, 5);
    }
    let _ = harness.state_mut().run("mail.select", serde_json::json!({"ids": []}));
    harness.step();

    // Whole-folder work: Select All, every one of them marked read and unread again, a thousand
    // deleted, and each undone.
    let inbox_ids: Vec<Id> = harness.state_mut().session.rows().iter().map(|r| r.id).collect();
    let thousand: Vec<Id> = inbox_ids.iter().copied().take(1000).collect();
    for (what, command, params) in [
        ("select all", "mail.selectAll", serde_json::json!({})),
        ("mark all read", "mail.markRead", serde_json::json!({"ids": inbox_ids})),
        ("undo it", "edit.undo", serde_json::json!({})),
        ("delete a thousand", "mail.delete", serde_json::json!({"ids": thousand})),
        ("undo the delete", "edit.undo", serde_json::json!({})),
        // A message more (as mail that comes is): the conversations are worked out again.
        ("copy one in (a message comes)", "mail.copy", serde_json::json!({"ids": [ids[0]], "folder": inbox})),
        ("undo the copy", "edit.undo", serde_json::json!({})),
    ] {
        let t = Instant::now();
        let r = harness.state_mut().run(command, params);
        let took = ms(t.elapsed());
        let t = Instant::now();
        harness.step();
        let frame = ms(t.elapsed());
        // (Worked out again in that frame when the command changed what groups a message or how
        // many there are; checked and kept otherwise.)
        let t = Instant::now();
        let built = harness.state_mut().session.prepare_conversations();
        eprintln!(
            "FRAME_BENCH {what}: command {took:.1} ms{}, next frame {frame:.1} ms; the conversations then {} in {:.1} ms",
            if r.is_err() { " (failed)" } else { "" },
            if built { "built" } else { "in step" },
            ms(t.elapsed())
        );
    }
    let _ = harness.state_mut().run("mail.select", serde_json::json!({"ids": []}));
    harness.step();

    if std::env::var_os("MAILCRAFT_FRAME_BENCH_OPEN").is_some() {
        let _ = harness.state_mut().execute("mail.select", serde_json::json!({"id": ids[6]}));
        harness.run_steps(2);
        report("steady with a message open, before the loop", &mut harness, 10);
    }
    if let Some(seconds) = std::env::var("MAILCRAFT_FRAME_BENCH_SECONDS").ok().and_then(|s| s.parse::<u64>().ok()) {
        eprintln!("FRAME_BENCH looping steady frames for {seconds} s (pid {})", std::process::id());
        let until = Instant::now() + std::time::Duration::from_secs(seconds);
        let mut n = 0usize;
        let changes = std::env::var_os("MAILCRAFT_FRAME_BENCH_CHANGES").is_some();
        let selects = std::env::var_os("MAILCRAFT_FRAME_BENCH_SELECT").is_some();
        let ranges = std::env::var_os("MAILCRAFT_FRAME_BENCH_RANGE").is_some();
        while Instant::now() < until {
            if changes {
                match std::env::var("MAILCRAFT_FRAME_BENCH_CHANGES").as_deref() {
                    Ok("flag") => drop(harness.state_mut().execute("mail.toggleFlag", serde_json::json!({"id": ids[n % 5]}))),
                    // Into the Archive and back: the Inbox's list is built again each time.
                    Ok("move") => {
                        let to = if n.is_multiple_of(2) { archive } else { inbox };
                        drop(harness.state_mut().execute("mail.move", serde_json::json!({"id": ids[34], "folder": to})));
                    }
                    // A hundred deleted, then undone.
                    Ok("undo") => {
                        if n.is_multiple_of(2) {
                            let some: Vec<Id> = harness.state_mut().session.rows().iter().take(100).map(|r| r.id).collect();
                            drop(harness.state_mut().execute("mail.delete", serde_json::json!({"ids": some})));
                        } else {
                            drop(harness.state_mut().execute("edit.undo", serde_json::json!({})));
                        }
                    }
                    _ => drop(harness.state_mut().execute("mail.toggleRead", serde_json::json!({"id": ids[n % 5]}))),
                }
            }
            if selects {
                let _ = harness.state_mut().execute("mail.select", serde_json::json!({"id": ids[10 + n % 20]}));
            }
            if ranges {
                let reach: usize = std::env::var("MAILCRAFT_FRAME_BENCH_RANGE").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
                let from = rows.first().copied().unwrap_or_default();
                let to = rows.get(reach).or(rows.last()).copied().unwrap_or_default();
                let _ = harness.state_mut().run("mail.select", serde_json::json!({"id": from.0, "copy": from.1}));
                let _ = harness.state_mut().run("mail.select", serde_json::json!({"id": to.0, "copy": to.1, "range": true}));
            }
            harness.step();
            n += 1;
        }
        eprintln!("FRAME_BENCH {n} frames in {seconds} s");
    }
}
