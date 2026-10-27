// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! How long a large mailbox takes to open (#519) and what a close waits for (#586): a made-up
//! mailbox of the shape of a large one (64,000 messages with about 6.8 KB of headers each, 374
//! folders, bodies and attachments in its content folder), opened a few times, saved, swept and
//! let go of. Ignored by `cargo test`; the Bench workflow runs it every week. Run it with
//!
//! ```text
//! cargo test --release -p mailcraft-engine open_bench -- --ignored --nocapture
//! ```
//!
//! The mailbox is made once (about 2 GB, under `MAILCRAFT_BENCH_DIR`, else the system's
//! temporary folder) and kept for the next run. `MAILCRAFT_BENCH_WARN_MS` names a time past which
//! the median open is said to be slow, without failing (as a warning a GitHub runner shows).

use std::time::{Duration, Instant};

use mailcraft_model::{Address, Attachment, Body, Mailbox, Message};

use super::*;

const MESSAGES: usize = 64_000;
const FOLDERS: usize = 374;
const RUNS: usize = 3;
/// Bumped when the mailbox made changes shape, so that an old one is made again.
const SHAPE: &str = "shape-2";

/// Words to make text of, none of anybody's mail.
const WORDS: [&str; 16] = [
    "harbour", "lantern", "meadow", "quarry", "saddle", "thimble", "orchard", "pebble", "copper", "falcon", "willow", "cobalt", "marble", "tundra",
    "velvet", "juniper",
];

/// `len` bytes or so of text that is different for each `seed`.
fn text(seed: usize, len: usize) -> String {
    let mut out = String::with_capacity(len + 16);
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.push_str(WORDS[x % WORDS.len()]);
        out.push(if x.is_multiple_of(11) { '\n' } else { ' ' });
    }
    out.push_str(&seed.to_string());
    out
}

/// About 6.8 KB of headers, as relays and filters write them.
fn headers(i: usize) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for n in 0..40 {
        out.push((
            "Received".to_string(),
            format!(
                "from relay{n}.mx.example.org (relay{n}.mx.example.org [192.0.2.{}]) by mx{}.example.org with ESMTPS id {i:08x}{n:04x}; Tue, 1 Sep 2026 10:{:02}:00 +0000",
                n % 250,
                n % 7,
                n % 60
            ),
        ));
    }
    for n in 0..8 {
        out.push((format!("X-Filter-Verdict-{n}"), text(i * 31 + n, 130)));
    }
    for (n, name) in ["DKIM-Signature", "ARC-Seal", "ARC-Message-Signature", "ARC-Authentication-Results"].iter().enumerate() {
        out.push(((*name).into(), text(i * 7 + n, 520)));
    }
    out
}

fn make(path: &Path) -> Mailbox {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let account = mb.accounts.first().map(|a| a.id).unwrap();
    let mut folders = vec![mb.inbox().unwrap()];
    let mut parent = None;
    for f in 0..FOLDERS - folders.len() {
        let id = mb.add_folder(account, parent, &format!("Project {f}")).unwrap();
        folders.push(id);
        // Some folders under others, as people keep them.
        parent = if f % 5 == 4 { None } else { Some(id) };
    }
    for i in 0..MESSAGES {
        let mut m = Message {
            folder: folders[i % folders.len()],
            subject: format!("Quarterly figures {i}"),
            from: Address::new("Blake Rivera", format!("blake{}@example.org", i % 300)),
            to: vec![Address::new("Avery Quinn", "avery@example.org")],
            date: 1_700_000_000 + i as i64 * 600,
            headers: headers(i),
            body: Body { text: text(i, 3_000), html: Some(format!("<html><body><p>{}</p></body></html>", text(i + MESSAGES, 20_000))) },
            ..Default::default()
        };
        if i % 3 == 0 {
            m.attachments.push(Attachment::new(format!("figures-{i}.csv"), "text/csv", text(i + 2 * MESSAGES, 3_000).into_bytes()));
        }
        mb.add_message(m).unwrap();
    }
    let saved = save(path, &mb).unwrap();
    eprintln!("made a mailbox of {} bytes", saved.bytes);
    mb
}

#[test]
#[ignore = "a benchmark: makes a mailbox of about 2 GB; see the module's documentation"]
fn open_bench() {
    let dir = std::env::var_os("MAILCRAFT_BENCH_DIR").map_or_else(|| std::env::temp_dir().join("mailcraft-open-bench"), PathBuf::from);
    let path = dir.join("mailbox.mailcraft");
    let marker = dir.join(SHAPE);
    if !marker.exists() {
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let started = Instant::now();
        drop(make(&path));
        fs::write(&marker, b"").unwrap();
        eprintln!("made it in {:.2} s", started.elapsed().as_secs_f64());
    }
    let mut took = Vec::new();
    for (how, launch) in [("as always", Launch::default()), ("for a window", Launch { checks_later: true, rows: Some(OPENING_ROWS) })] {
        let mut times = Vec::new();
        for run in 0..RUNS {
            // As at a launch: nothing known of the file yet (what was is let go of untimed).
            *lock(&shared(&path).state) = State::default();
            let started = Instant::now();
            let opened = open_as(&path, &OpenProgress::default(), launch).unwrap();
            let open_time = started.elapsed();
            let mut session = crate::Session::new(opened.mailbox);
            session.path = Some(path.clone());
            let total = started.elapsed();
            // What the window's host does once it shows: what is missing, then the bodies left for later.
            let checked = Instant::now();
            let (missing, bodies) = content_later(&path);
            session.mark_unavailable(&missing.refs);
            let missing = missing.count;
            let took = session.take_bodies(bodies);
            eprintln!(
                "{how}, run {run}: open {:.0} ms, with the session {:.0} ms ({} messages, {} loaded, {} deferred, {} missing; then {took} bodies taken in, {:.0} ms in all)",
                open_time.as_secs_f64() * 1e3,
                total.as_secs_f64() * 1e3,
                session.mailbox.messages.len(),
                opened.loaded,
                opened.deferred,
                opened.unavailable + missing,
                checked.elapsed().as_secs_f64() * 1e3
            );
            assert_eq!(session.mailbox.messages.len(), MESSAGES);
            assert_eq!(opened.unavailable + missing, 0);
            times.push(total);
            // Closing (#586): the last save the window's close waits for, after a change and
            // with none, then the mailbox let go of.
            let first = *session.mailbox.messages.keys().next().unwrap();
            for what in ["the first change since the opening", "another change", "no change"] {
                if what != "no change" {
                    let m = session.mailbox.message_mut(first).unwrap();
                    m.read = !m.read;
                }
                let saving = Instant::now();
                save(&path, &session.mailbox).unwrap();
                println!("CLOSE_BENCH {how}, run {run}: save after {what}: {:.0} ms", saving.elapsed().as_secs_f64() * 1e3);
            }
            // The look through the content folder for files nothing names, which a window starts
            // once it has opened its mailbox (and every six hours of saving).
            let sweeping = Instant::now();
            let swept = sweep(&path, GRACE).unwrap();
            println!("CLOSE_BENCH {how}, run {run}: sweep ({} removed): {:.0} ms", swept.removed, sweeping.elapsed().as_secs_f64() * 1e3);
            let letting_go = Instant::now();
            drop(session);
            println!("CLOSE_BENCH {how}, run {run}: the mailbox let go of: {:.0} ms", letting_go.elapsed().as_secs_f64() * 1e3);
        }
        times.sort();
        let median = times.get(RUNS / 2).copied().unwrap_or_default();
        eprintln!("OPEN_BENCH {how}: median {:.0} ms", median.as_secs_f64() * 1e3);
        took = times;
    }
    // Closing with the word index: dropped while it reads its file and the mailbox, and once done.
    *lock(&shared(&path).state) = State::default();
    let messages: Vec<Arc<Message>> = open(&path).unwrap().mailbox.messages.values().cloned().collect();
    let building = Instant::now();
    let mut words = crate::words::Words::open(&path, messages.clone());
    words.wait();
    println!("CLOSE_BENCH word index read or built: {:.0} ms", building.elapsed().as_secs_f64() * 1e3);
    let letting_go = Instant::now();
    drop(words);
    println!("CLOSE_BENCH word index let go of once done: {:.0} ms", letting_go.elapsed().as_secs_f64() * 1e3);
    let words = crate::words::Words::open(&path, messages);
    std::thread::sleep(Duration::from_millis(50));
    let letting_go = Instant::now();
    drop(words);
    println!("CLOSE_BENCH word index let go of while it reads: {:.0} ms", letting_go.elapsed().as_secs_f64() * 1e3);
    took.sort();
    let median = took.get(RUNS / 2).copied().unwrap_or_default();
    if let Some(warn) = std::env::var("MAILCRAFT_BENCH_WARN_MS").ok().and_then(|s| s.parse::<u64>().ok())
        && median > Duration::from_millis(warn)
    {
        // (On stdout, where a GitHub runner reads it as a warning on the run.)
        println!("::warning::opening a mailbox of {MESSAGES} messages took {:.0} ms, more than {warn} ms", median.as_secs_f64() * 1e3);
    }
}
