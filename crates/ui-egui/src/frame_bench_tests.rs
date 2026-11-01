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
//! `MAILCRAFT_FRAME_BENCH_SELECT` set, with another message selected before every frame).
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
    let mut app = MailApp::new(Session::at(mb, NOW, 0), Services::default());
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
    let _ = harness.state_mut().execute("ui.dismissReminder", serde_json::json!({"all": true}));
    harness.run_steps(2);
    report("steady", &mut harness, 20);

    // A change to the mailbox, as every Send/Receive makes: the list is built again.
    let ids: Vec<Id> = harness.state().session.mailbox.messages_in(inbox).map(|m| m.id).take(40).collect();
    for (i, id) in ids.iter().take(5).enumerate() {
        let _ = harness.state_mut().execute("mail.markUnread", serde_json::json!({"id": id}));
        let t = Instant::now();
        harness.step();
        eprintln!("FRAME_BENCH frame after a change {i}: {:.1} ms", ms(t.elapsed()));
    }
    report("steady after changes", &mut harness, 10);

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
        eprintln!("FRAME_BENCH frame after a scroll {i}: {:.1} ms", ms(t.elapsed()));
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
        while Instant::now() < until {
            if changes {
                let _ = harness.state_mut().execute("mail.toggleRead", serde_json::json!({"id": ids[n % 5]}));
            }
            if selects {
                let _ = harness.state_mut().execute("mail.select", serde_json::json!({"id": ids[10 + n % 20]}));
            }
            harness.step();
            n += 1;
        }
        eprintln!("FRAME_BENCH {n} frames in {seconds} s");
    }
}
