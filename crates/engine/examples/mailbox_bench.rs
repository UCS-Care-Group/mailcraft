// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! How long keeping a real-sized mailbox on disk takes.
//!
//! `cargo run --release -p mailcraft-engine --example mailbox_bench [-- --messages 5000 --attachments 300 --keep DIR --split-only]`
//! `cargo run --release -p mailcraft-engine --example mailbox_bench -- --open DIR`
//! `cargo run --release -p mailcraft-engine --example mailbox_bench -- --open COPY --keep-offline 1m`
//! `cargo run --release -p mailcraft-engine --example mailbox_bench -- --messages 50000 --exchange-headers --split-only --keep DIR`
//!
//! `--exchange-headers`: every message carries about 6 KB of header lines of the kind Exchange
//! writes on all its mail (the trace, the spam filters' reports, signatures, its own stamps),
//! invented here (`example.org` hosts, random base64): what makes a real Microsoft account's
//! mailbox file several times the size of its messages' own fields (HS). With `--open`, the size
//! of the mailbox file is said with the memory opening it takes.
//!
//! `--keep-offline 1m` (with `--open`): the mailbox's account made a Microsoft account's, its
//! mail to keep offline set to a month (or another step) as of its newest message, and the disk
//! it takes before and after older mail goes back to its headers and preview (#143). It changes
//! the mailbox for good: give it a copy (`cp -r --reflink=auto DIR COPY`).
//!
//! Builds an invented mailbox the size of a few years of somebody's mail (5,000 messages with plain
//! and HTML bodies, 300 attachments from 20 KB to 5 MB, about 300 MB in all), then times saving and
//! opening it both ways: as one self-contained file (how every save used to go, and how an export
//! still does), and as the desktop app keeps it, with the content in a folder beside the mailbox
//! file (`mailcraft_engine::store`). Everything is written under the system's temporary folder and
//! removed afterwards; `--keep DIR` writes into `DIR` instead and leaves both mailboxes there, to try
//! the app on (`mailcraft --mailbox DIR/single.mailcraft`). `--split-only` leaves the single file
//! out (for a mailbox too large to serialise whole in memory).
//!
//! `--open DIR` only opens `DIR/mailbox.mailcraft` (one `--keep` left), in a process that has done
//! nothing else, and says how long that took and how much memory the process holds after it
//! (resident, and its peak; Linux): what a person opening that mailbox waits for and pays (R6.1).
//! Then, as the window goes on: the first message list, a read mark saved (the first save after
//! opening and three more), the word index read or built (R6.2: the first time, every message
//! is read into it, which takes a while), and a search everywhere for a word no message has,
//! answered from it, within the window's time and with all the time it wants, with the memory
//! held after all of it (R6.4). It saves into
//! `DIR`. To open with the page cache emptied first, without root: `sync`, then
//! `find DIR -type f -exec dd if={} iflag=nocache count=0 status=none \;` (GNU `dd`).
//!
//! The run holds the whole invented mailbox in memory, and the single file twice more while it is
//! written and read back: about three and a half times the content at its peak, two and a half
//! with `--split-only` (200,000 messages and 2,000 files are 5 GB of content and needed 10 GB).
//! Before building anything it works that out and refuses when the memory free for it can't hold
//! it with a quarter to spare (Linux: `/proc/meminfo` and the limit of its control group, so that
//! `systemd-run --user --scope -p MemoryMax=6G` is honoured too): the out-of-memory killer would
//! otherwise end it, and with it whatever shares its terminal. `--force` runs anyway.

use std::path::Path;
use std::time::{Duration, Instant};

use mailcraft_engine::Session;
use mailcraft_engine::io;
use mailcraft_model::{Address, Attachment, FolderKind, Mailbox, Message};

/// xorshift64*: the same mailbox every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// 0.0 ≤ x < 1.0.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn pick<'a>(&mut self, list: &[&'a str]) -> &'a str {
        list.get(self.below(list.len())).copied().unwrap_or("")
    }

    /// Bytes nothing compresses, like the pictures, PDFs and archives mail carries.
    fn bytes(&mut self, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len + 8);
        while out.len() < len {
            out.extend_from_slice(&self.next().to_le_bytes());
        }
        out.truncate(len);
        out
    }
}

const FIRST: &[&str] =
    &["Ada", "Bram", "Chidi", "Dana", "Elif", "Farid", "Greta", "Hugo", "Ines", "Jonas", "Keiko", "Liam", "Mira", "Nils", "Oona", "Pavel"];
const LAST: &[&str] = &["Aldrin", "Brooks", "Castell", "Dufour", "Eklund", "Fontaine", "Garza", "Holm", "Ibarra", "Jansen", "Kowal", "Lindqvist"];
const WORDS: &[&str] = &[
    "the",
    "report",
    "meeting",
    "budget",
    "please",
    "review",
    "attached",
    "schedule",
    "quarter",
    "figures",
    "draft",
    "thanks",
    "tomorrow",
    "update",
    "team",
    "project",
    "deadline",
    "invoice",
    "proposal",
    "feedback",
    "client",
    "delivery",
    "numbers",
    "plan",
    "agenda",
    "notes",
    "version",
    "changes",
    "approve",
    "follow",
    "regards",
    "morning",
    "afternoon",
    "question",
    "answer",
    "details",
    "summary",
    "forecast",
    "contract",
    "design",
];
const KINDS: &[(&str, &str)] =
    &[("pdf", "application/pdf"), ("png", "image/png"), ("jpg", "image/jpeg"), ("zip", "application/zip"), ("xlsx", "application/octet-stream")];

fn sentence(rng: &mut Rng, out: &mut String) {
    let n = 5 + rng.below(14);
    for i in 0..n {
        let w = rng.pick(WORDS);
        if i == 0 {
            let mut c = w.chars();
            out.extend(c.next().map(|f| f.to_ascii_uppercase()));
            out.push_str(c.as_str());
        } else {
            out.push(' ');
            out.push_str(w);
        }
    }
    out.push_str(". ");
}

/// Plain text of about `len` bytes, in paragraphs.
fn text(rng: &mut Rng, len: usize) -> String {
    let mut out = String::with_capacity(len + 200);
    while out.len() < len {
        for _ in 0..2 + rng.below(4) {
            sentence(rng, &mut out);
        }
        out.push_str("\n\n");
    }
    out
}

/// HTML of about `len` bytes, the way mail programs and newsletters write it: nested tables and
/// inline styles around not much text.
fn html(rng: &mut Rng, len: usize) -> String {
    let mut out = String::with_capacity(len + 600);
    out.push_str("<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"></head><body style=\"margin:0;padding:0\">");
    out.push_str("<table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\">");
    while out.len() < len {
        out.push_str("<tr><td style=\"font-family:Segoe UI,Arial,sans-serif;font-size:14px;line-height:21px;color:#1f1f1f;padding:8px 24px\"><p style=\"margin:0 0 12px 0\">");
        for _ in 0..1 + rng.below(4) {
            sentence(rng, &mut out);
        }
        out.push_str("</p></td></tr>");
    }
    out.push_str("</table></body></html>");
    out
}

fn person(rng: &mut Rng) -> Address {
    let (first, last) = (rng.pick(FIRST), rng.pick(LAST));
    Address::new(format!("{first} {last}"), format!("{}.{}@example.org", first.to_lowercase(), last.to_lowercase()))
}

/// The invented mailbox, and how many bytes of content it carries.
/// Random base64 of `len` characters.
fn base64(rng: &mut Rng, len: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    (0..len).map(|_| char::from(ALPHABET.get(rng.below(64)).copied().unwrap_or(b'A'))).collect()
}

/// An invented host name of a mail server, under `example.org`.
fn host(rng: &mut Rng) -> String {
    format!(
        "{}{:04}MB{:04}.eurprd{:02}.prod.example.org",
        rng.pick(&["AM0PR", "DB9PR", "PAXPR", "VI1PR"]),
        rng.below(10_000),
        rng.below(10_000),
        rng.below(100)
    )
}

/// About 6 KB of the kind of header lines Exchange writes on a message it delivers, invented: the
/// hops it went through, the verdicts and reports of the spam filters, the signatures, its stamps.
fn exchange_headers(rng: &mut Rng, i: usize) -> Vec<(String, String)> {
    let date = "Mon, 1 Sep 2025 09:00:00 +0000";
    let mut out: Vec<(String, String)> = Vec::new();
    let mut add = |name: &str, value: String| out.push((name.to_string(), value));
    for _ in 0..4 {
        let (a, b) = (host(rng), host(rng));
        let ids = (rng.below(65_536), rng.below(65_536), rng.below(9_999), rng.below(99));
        add(
            "Received",
            format!(
                "from {a} (2603:10a6:20b:{:x}::1) by {b} (2603:10a6:20b:{:x}::2) with Microsoft SMTP Server (version=TLS1_2, cipher=TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384) id 15.20.{}.{}; {date}",
                ids.0, ids.1, ids.2, ids.3
            ),
        );
    }
    add("ARC-Seal", format!("i=1; a=rsa-sha256; s=arcselector10001; d=example.org; cv=none; b={}", base64(rng, 340)));
    let (bh, b) = (base64(rng, 44), base64(rng, 340));
    add(
        "ARC-Message-Signature",
        format!(
            "i=1; a=rsa-sha256; c=relaxed/relaxed; d=example.org; s=arcselector10001; h=From:Date:Subject:Message-ID:Content-Type:MIME-Version:X-MS-Exchange-AntiSpam-MessageData-ChunkCount:X-MS-Exchange-AntiSpam-MessageData-0; bh={bh}; b={b}"
        ),
    );
    add("ARC-Authentication-Results", "i=1; mx.example.org 1; spf=pass smtp.mailfrom=example.org; dmarc=pass action=none header.from=example.org; dkim=pass header.d=example.org; arc=none".into());
    let (bh, b) = (base64(rng, 44), base64(rng, 340));
    add(
        "DKIM-Signature",
        format!(
            "v=1; a=rsa-sha256; c=relaxed/relaxed; d=example.org; s=selector2; h=From:Date:Subject:Message-ID:Content-Type:MIME-Version; bh={bh}; b={b}"
        ),
    );
    add("Authentication-Results", "dkim=none (message not signed) header.d=none;dmarc=none action=none header.from=example.org;".into());
    let edge = (rng.below(250), rng.below(10_000), rng.below(250));
    add(
        "Received",
        format!(
            "from mail.example.org (192.0.2.{}) by AM4PEPF0000{:04}.mail.protection.outlook.com (10.167.16.{}) with Microsoft SMTP Server; {date}",
            edge.0, edge.1, edge.2
        ),
    );
    add("Thread-Topic", "Synthetic thread".into());
    let index = base64(rng, 44);
    add("Thread-Index", index);
    add("X-MS-Has-Attach", String::new());
    add("X-MS-TNEF-Correlator", String::new());
    add("X-MS-PublicTrafficType", "Email".into());
    let (a, b) = (host(rng), host(rng));
    add("X-MS-TrafficTypeDiagnostic", format!("{a}:EE_|{b}:EE_"));
    let id = (rng.next() as u32, rng.below(65_536), rng.below(65_536), rng.next() >> 16);
    add("X-MS-Office365-Filtering-Correlation-Id", format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", id.0, i % 65_536, id.1, id.2, id.3));
    add("X-Microsoft-Antispam", "BCL:0;ARA:13230040|1800799024|366016|376014|38070700018;".into());
    let info = base64(rng, 1_800);
    add("X-Microsoft-Antispam-Message-Info", info);
    let h = host(rng);
    add(
        "X-Forefront-Antispam-Report",
        format!(
            "CIP:255.255.255.255;CTRY:;LANG:en;SCL:1;SRV:;IPV:NLI;SFV:NSPM;H:{h};PTR:;CAT:NONE;SFS:(13230040)(1800799024)(366016)(376014)(38070700018);DIR:OUT;SFP:1101;"
        ),
    );
    add("X-MS-Exchange-AntiSpam-MessageData-ChunkCount", "1".into());
    let data = base64(rng, 1_400);
    add("X-MS-Exchange-AntiSpam-MessageData-0", data);
    let (a, b) = (rng.next(), rng.next());
    add("X-MS-Exchange-CrossTenant-Network-Message-Id", format!("{a:016x}{b:016x}"));
    add("X-MS-Exchange-CrossTenant-AuthAs", "Internal".into());
    add("X-MS-Exchange-CrossTenant-OriginalArrivalTime", date.into());
    add("X-MS-Exchange-CrossTenant-FromEntityHeader", "Hosted".into());
    let (a, b) = (rng.next(), rng.next());
    add("X-MS-Exchange-CrossTenant-Id", format!("{a:016x}{b:016x}"));
    add("X-MS-Exchange-CrossTenant-MailboxType", "HOSTED".into());
    let h = host(rng);
    add("X-MS-Exchange-Transport-CrossTenantHeadersStamped", h);
    add("X-MS-Exchange-Organization-AuthAs", "Anonymous".into());
    add("X-MS-Exchange-Organization-MessageDirectionality", "Incoming".into());
    add("X-MS-Exchange-Organization-SCL", "1".into());
    let n = rng.below(1_000_000);
    add(
        "X-Microsoft-Antispam-Mailbox-Delivery",
        format!("ucf:0;jmr:0;auth:0;dest:I;ENG:(910001)(944506478)(944626604)(920097)(930097)({n});RF:JunkEmail;"),
    );
    add("Return-Path", "bounce@example.org".into());
    out
}

fn build(messages: usize, attachments: usize, exchange: bool) -> Result<(Mailbox, u64), String> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let folder = |mb: &Mailbox, kind: FolderKind| mb.folders.iter().find(|f| f.kind == kind).map(|f| f.id);
    let inbox = mb.inbox().ok_or("no inbox")?;
    let sent = folder(&mb, FolderKind::Sent).unwrap_or(inbox);
    let archive = folder(&mb, FolderKind::Archive).unwrap_or(inbox);
    let me = Address::new("Avery Quinn", "avery@example.org");
    // Which messages carry a file: spread evenly.
    let every = (messages / attachments.max(1)).max(1);
    let mut files: Vec<Attachment> = Vec::new();
    let mut size = 0u64;
    let start = 1_700_000_000i64;
    for i in 0..messages {
        let other = person(&mut rng);
        let place = match rng.below(20) {
            0..=13 => inbox,
            14..=16 => sent,
            _ => archive,
        };
        let (from, to) = if place == sent { (me.clone(), other) } else { (other, me.clone()) };
        let mut subject = String::new();
        for k in 0..3 + rng.below(5) {
            if k > 0 {
                subject.push(' ');
            }
            subject.push_str(rng.pick(WORDS));
        }
        // Most mail is short; some is long.
        let body_len = 300 + (rng.unit().powi(2) * 8000.0) as usize;
        let mut m = Message {
            folder: place,
            message_id: format!("{i:06}.{:016x}@mail.example.org", rng.next()),
            from,
            to: vec![to],
            subject,
            date: start + (i as i64) * 12_345,
            received: start + (i as i64) * 12_345 + 40,
            read: rng.below(4) != 0,
            headers: vec![("X-Mailer".into(), "Example Mailer 4.2".into()), ("Thread-Index".into(), format!("{:032x}", rng.next()))],
            ..Default::default()
        };
        if exchange {
            m.headers = exchange_headers(&mut rng, i);
            // (As a message read from its server has them: what nothing reads by name apart.)
            m.split_headers();
        }
        m.body.text = text(&mut rng, body_len);
        if rng.below(10) < 6 {
            // 4 KB to 80 KB, mostly towards the small end.
            let len = 4_000 + (rng.unit().powi(3) * 76_000.0) as usize;
            m.body.html = Some(html(&mut rng, len));
        }
        if i % every == 0 && files.len() < attachments {
            // One in twelve is a file that went round before (a forward, a reply to all).
            let again = if files.len() > 8 && rng.below(12) == 0 { files.get(rng.below(files.len())).cloned() } else { None };
            let file = match again {
                Some(file) => file,
                None => {
                    // 20 KB to 5 MB, each size as likely as the one twice as big.
                    let len = (20_000.0 * (250.0f64).powf(rng.unit())) as usize;
                    let (ext, mime) = KINDS.get(rng.below(KINDS.len())).copied().unwrap_or(("bin", "application/octet-stream"));
                    Attachment::new(format!("{}-{}.{ext}", rng.pick(WORDS), files.len()), mime, rng.bytes(len))
                }
            };
            files.push(file.clone());
            m.attachments.push(file);
        }
        size += m.size();
        mb.add_message(m).map_err(|e| e.to_string())?;
    }
    Ok((mb, size))
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

fn ms(d: Duration) -> String {
    format!("{:>8.1} ms", d.as_secs_f64() * 1000.0)
}

fn timed<T>(work: impl FnOnce() -> T) -> (T, Duration) {
    let t = Instant::now();
    let out = work();
    (out, t.elapsed())
}

fn dir_size(dir: &Path) -> (u64, u64) {
    let (mut bytes, mut files) = (0, 0);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => stack.push(e.path()),
                Ok(m) => {
                    bytes += m.len();
                    files += 1;
                }
                Err(_) => {}
            }
        }
    }
    (bytes, files)
}

/// The mailbox as one self-contained file: what every save wrote before the content moved out.
fn single_file(mailbox: &Mailbox, dir: &Path, keep: bool) -> Result<(), String> {
    println!("\nOne self-contained file (attachments as base64 inside it)");
    let path = dir.join("single.mailcraft");
    let (bytes, serialise) = timed(|| io::write_mailbox(mailbox));
    let bytes = bytes?;
    let (written, write) = timed(|| std::fs::write(&path, &bytes));
    written.map_err(|e| e.to_string())?;
    println!("  save: serialise            {}", ms(serialise));
    println!("  save: write {:<14} {}", mb(bytes.len() as u64), ms(write));
    println!("  save: in all               {}   <- the window waits for this", ms(serialise + write));
    drop(bytes);
    // Again after one message is marked read: the same work, whatever changed.
    let mut s = Session::new(mailbox.clone());
    let first = s.mailbox.messages.keys().next().copied().unwrap_or(0);
    s.mailbox.message_mut(first).map_err(|e| e.to_string())?.read ^= true;
    let (again, resave) = timed(|| io::write_mailbox(&s.mailbox).and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string())));
    again?;
    println!("  save after one read mark   {}", ms(resave));
    let (opened, open) = timed(|| io::open_path(&path));
    let opened = opened?;
    println!("  open                       {}", ms(open));
    if opened.messages.len() != mailbox.messages.len() {
        return Err("the single file didn't come back whole".into());
    }
    println!("  on disk: {} in one file", mb(std::fs::metadata(&path).map_or(0, |m| m.len())));
    if !keep {
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

/// The mailbox as the desktop app keeps it: the mailbox file, and the content in a folder beside it.
fn split(mailbox: &Mailbox, dir: &Path) -> Result<(), String> {
    use mailcraft_engine::io::BackgroundSave as _;
    use mailcraft_engine::store;
    println!("\nThe mailbox file with its content folder (mailbox.mailcraft + mailbox.mailcraft.blobs/)");
    let path = dir.join("mailbox.mailcraft");
    // Before R6.3 the window copied the mailbox each time it handed it to the saver (every few
    // seconds until it heard back), and, closed or told to Save or Open while the saver was at
    // the first save, saved in place: that took its turn after the saver's, so the window waited
    // for all of a first save. Timed here as a first save in place, into a folder of its own.
    let (copy, copied) = timed(|| mailbox.clone());
    drop(copy);
    let aside = dir.join("in-place");
    std::fs::create_dir_all(&aside).map_err(|e| e.to_string())?;
    let (saved, in_place) = timed(|| store::save(&aside.join("mailbox.mailcraft"), mailbox));
    saved?;
    let _ = std::fs::remove_dir_all(&aside);
    println!("  before: a first save in place {}   <- the window waited for this (closing, Save, Open)", ms(in_place));
    println!("          and copied the mailbox each time it handed it over: {}", ms(copied).trim());
    // The first save, in the background as the desktop app does it (R6.3): what the window
    // waits for is handing the mailbox over; how far the saver has got is looked at meanwhile.
    let saver = store::Saver::spawn(|| {}).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let (_, handed) = timed(|| saver.submit(&path, mailbox, 1));
    let (mut looks, mut slowest, mut said) = (0u32, Duration::ZERO, Instant::now());
    loop {
        let (progress, look) = timed(|| saver.progress());
        looks += 1;
        slowest = slowest.max(look);
        let Some(p) = progress else { break };
        if said.elapsed() >= Duration::from_secs(30) {
            println!("      after {:>6.1} s: {:?} {} of {}", started.elapsed().as_secs_f64(), p.stage, p.done, p.total);
            said = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let took = started.elapsed();
    for report in saver.reports() {
        report.result?;
    }
    drop(saver);
    println!("  first save (an old single file becoming this form), in the background: {}", ms(took));
    println!(
        "      the window waits {} to hand it over, at most {} for each of {looks} looks at the progress",
        ms(handed).trim(),
        ms(slowest).trim()
    );
    let (bytes, files) = dir_size(&store::blob_dir(&path));
    println!("      {files} content files, {}; mailbox file {}", mb(bytes), mb(std::fs::metadata(&path).map_or(0, |m| m.len())));

    let mut s = Session::new(mailbox.clone());
    let ids: Vec<u64> = s.mailbox.messages.keys().copied().collect();
    let flip = |s: &mut Session, n: usize| -> Result<(), String> {
        let id = ids.get(n).copied().unwrap_or(0);
        s.mailbox.message_mut(id).map_err(|e| e.to_string())?.read ^= true;
        Ok(())
    };
    flip(&mut s, 0)?;
    let (saved, took) = timed(|| store::save(&path, &s.mailbox));
    let saved = saved?;
    println!("  save after one read mark   {}   wrote {} ({} content files)", ms(took), mb(saved.bytes), saved.blobs);
    let mut times = Vec::new();
    for n in 1..=20 {
        flip(&mut s, n)?;
        let (saved, took) = timed(|| store::save(&path, &s.mailbox));
        saved?;
        times.push(took);
    }
    times.sort();
    println!(
        "      20 more: median {}, slowest {}",
        ms(times.get(times.len() / 2).copied().unwrap_or_default()).trim(),
        ms(times.last().copied().unwrap_or_default()).trim()
    );

    // A Send/Receive round: fifty new messages, five of them with a file.
    let mut rng = Rng(42);
    let inbox = s.mailbox.inbox().ok_or("no inbox")?;
    for i in 0..50 {
        let mut m = Message { folder: inbox, subject: format!("new {i}"), from: person(&mut rng), ..Default::default() };
        m.body.text = text(&mut rng, 2_000);
        m.body.html = Some(html(&mut rng, 20_000));
        if i % 10 == 0 {
            m.attachments.push(Attachment::new(format!("new-{i}.pdf"), "application/pdf", rng.bytes(400_000)));
        }
        s.mailbox.add_message(m).map_err(|e| e.to_string())?;
    }
    let (saved, took) = timed(|| store::save(&path, &s.mailbox));
    let saved = saved?;
    println!("  save after 50 new messages {}   wrote {} + {} in {} content files", ms(took), mb(saved.bytes), mb(saved.blob_bytes), saved.blobs);

    // What the window waits for when a background thread does the writing.
    let saver = store::Saver::spawn(|| {}).map_err(|e| e.to_string())?;
    flip(&mut s, 30)?;
    let (_, handed) = timed(|| saver.submit(&path, &s.mailbox, 1));
    let (_, flushed) = timed(|| saver.flush());
    println!("  in the background: the window waits {}; written {} later", ms(handed).trim(), ms(flushed).trim());
    for report in saver.reports() {
        report.result?;
    }

    let (opened, took) = timed(|| store::open(&path));
    let opened = opened?;
    println!(
        "  open                       {}   ({} parts in memory, {} left on disk, {} unavailable)",
        ms(took),
        opened.loaded,
        opened.deferred,
        opened.unavailable
    );
    // What was left on disk comes back as it was saved.
    let mut whole = opened.mailbox.clone();
    store::load_all(&path, &mut whole)?;
    if whole != s.mailbox {
        return Err("the mailbox didn't come back as it was saved".into());
    }
    drop(whole);
    // The first save after opening works every message out once more (their text is hashed again).
    let mut s = Session::new(opened.mailbox);
    flip(&mut s, 40)?;
    let (saved, took) = timed(|| store::save(&path, &s.mailbox));
    println!("  first save after opening   {}   wrote {}", ms(took), mb(saved?.bytes));
    let (swept, took) = timed(|| store::sweep(&path, store::GRACE));
    let swept = swept?;
    println!("  sweep                      {}   ({} kept, {} removed)", ms(took), swept.kept, swept.removed);
    let (bytes, files) = dir_size(&store::blob_dir(&path));
    println!("  on disk: mailbox file {}, content {} in {files} files", mb(std::fs::metadata(&path).map_or(0, |m| m.len())), mb(bytes));
    Ok(())
}

/// What the process holds in memory now and at most so far, in bytes (Linux: `/proc/self/status`).
fn memory() -> Option<(u64, u64)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = |name: &str| -> Option<u64> {
        let line = status.lines().find(|l| l.starts_with(name))?;
        line.split_whitespace().nth(1)?.parse::<u64>().ok()?.checked_mul(1024)
    };
    Some((field("VmRSS:")?, field("VmHWM:")?))
}

/// What the process holds at most so far, for the progress lines.
fn peak() -> String {
    memory().map_or_else(|| "not readable".into(), |(_, peak)| mb(peak))
}

/// About how many bytes of content `build` makes: the means of the sizes it draws (a body of
/// 300 + 8,000·u² bytes of text, HTML of 4,000 + 76,000·u³ for six in ten, a file of 20 KB·250^u
/// for eleven in twelve of the attachments).
fn expected_content(messages: usize, attachments: usize) -> u64 {
    let body = 300.0 + 8_000.0 / 3.0 + 0.6 * (4_000.0 + 76_000.0 / 4.0);
    let file = 20_000.0 * 249.0 / 250f64.ln() * 11.0 / 12.0;
    let attachments = attachments.min(messages);
    (messages as f64 * body + attachments as f64 * file) as u64
}

/// How many bytes this process may still take before it is killed for them: what the machine has
/// available, or less when its control group (a `systemd-run --scope`, a container) is held to a
/// limit. `None` where that can't be read.
fn memory_free() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = meminfo.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let mut free = line.split_whitespace().nth(1)?.parse::<u64>().ok()?.checked_mul(1024)?;
    // cgroup v2: "0::/user.slice/…"; every group up to the root may hold a limit.
    let own = std::fs::read_to_string("/proc/self/cgroup").ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("0::").map(str::to_owned)));
    let mut group = own.map(|g| Path::new("/sys/fs/cgroup").join(g.trim_start_matches('/')));
    while let Some(dir) = group {
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok().and_then(|v| v.trim().parse::<u64>().ok());
        if let (Some(max), Some(used)) = (read("memory.max"), read("memory.current")) {
            free = free.min(max.saturating_sub(used));
        }
        group = dir.parent().filter(|p| p.starts_with("/sys/fs/cgroup")).map(Path::to_path_buf);
    }
    Some(free)
}

/// Open the mailbox kept in `dir` and nothing else: how long it takes, and what it costs.
fn open_only(dir: &Path) -> Result<(), String> {
    use mailcraft_engine::store;
    let path = dir.join("mailbox.mailcraft");
    let before = memory();
    let (opened, took) = timed(|| store::open(&path));
    let opened = opened?;
    let after = memory();
    let bodies: u64 = opened.mailbox.messages.values().map(|m| (m.body.text.len() + m.body.html.as_ref().map_or(0, String::len)) as u64).sum();
    let size = std::fs::metadata(&path).map_or(0, |m| m.len());
    println!("{}: {} messages, the mailbox file {}", path.display(), opened.mailbox.messages.len(), mb(size));
    println!("  open                       {}", ms(took));
    println!(
        "  {} parts in memory ({} of bodies), {} left on disk, {} unavailable (body budget {})",
        opened.loaded,
        mb(bodies),
        opened.deferred,
        opened.unavailable,
        mb(store::body_budget())
    );
    match (before, after) {
        (Some((rss0, _)), Some((rss, peak))) => {
            println!("  memory: {} resident after opening (from {} before), peak {}", mb(rss), mb(rss0), mb(peak));
        }
        _ => println!("  memory: not readable on this system"),
    }
    // What the window does next: a session on it, and the message list it shows first.
    let (mut s, took) = timed(|| {
        let mut s = Session::new(opened.mailbox);
        s.path = Some(path.clone());
        s
    });
    let (rows, listed) = timed(|| s.rows().len());
    println!("  session {}, first message list {} ({rows} rows)", ms(took).trim(), ms(listed).trim());
    // A read mark saved: the first save after opening, then the next ones (what the saver does
    // in the background after each change).
    let ids: Vec<u64> = s.mailbox.messages.keys().copied().collect();
    let mut saves = Vec::new();
    for n in 0..4 {
        let id = ids.get(n * 997 % ids.len().max(1)).copied().unwrap_or(0);
        s.mailbox.message_mut(id).map_err(|e| e.to_string())?.read ^= true;
        let (saved, took) = timed(|| store::save(&path, &s.mailbox));
        let saved = saved?;
        saves.push(format!("{} ({})", ms(took).trim(), mb(saved.bytes)));
    }
    let mut saves = saves.into_iter();
    println!("  save after a read mark: first after opening {}", saves.next().unwrap_or_default());
    println!("      then {}", saves.collect::<Vec<_>>().join(", "));
    // The word index (R6.2): read from beside the mailbox, or built in the background where it
    // lacks messages (the first time, every one), with the memory held after it.
    s.index_words = true;
    let words_file = store::words_file(&path);
    let had = std::fs::metadata(&words_file).map_or(0, |m| m.len());
    let (_, took) = timed(|| s.wait_for_word_index());
    println!(
        "  word index: {} {}   ({:?}; its file {}, {} before)",
        if had == 0 { "built in" } else { "read and brought in step in" },
        ms(took).trim(),
        s.word_index(),
        mb(std::fs::metadata(&words_file).map_or(0, |m| m.len())),
        mb(had)
    );
    println!("      the process {} resident, peak {}", memory().map_or_else(|| "not readable".into(), |(rss, _)| mb(rss)), peak());
    // Full text across everything, for words no message has: answered from the word index now
    // that it holds every message, as the window searches (within its time), and with all the
    // time it wants (another word, so nothing of the first is reused).
    for (label, limit, word) in [("within the window's time", s.search_time, "zyxwvutsrq"), ("to the end", Duration::MAX, "qrstuvwxyz")] {
        s.search_time = limit;
        let (found, took) = timed(|| s.run("view.search", &serde_json::json!({"query": word, "scope": "everywhere"})));
        let found = found.map_err(|e| e.to_string())?;
        println!(
            "  search everywhere, {label}: {}   ({} found, {} not searched)",
            ms(took),
            found.get("found").and_then(serde_json::Value::as_u64).unwrap_or(0),
            s.unsearched
        );
        println!(
            "      the session holds {} of what it brought in; the process {} resident",
            mb(s.resident_bytes()),
            memory().map_or_else(|| "not readable".into(), |(rss, _)| mb(rss))
        );
        let _ = s.run("view.clearSearch", &serde_json::json!({}));
    }
    match memory() {
        Some((rss, peak)) => println!("  memory after the saves and searches: {} resident, peak {}", mb(rss), mb(peak)),
        None => println!("  memory: not readable on this system"),
    }
    Ok(())
}

/// Mail to keep offline (#143) on the mailbox kept in `dir`: its account is made a Microsoft
/// account's (every folder and message given a server id, as if Graph had brought them down),
/// its window set to `keep`, the clock put at its newest message, and what is outside goes back
/// to its headers and preview: how long that takes, and the disk the mailbox takes before and
/// after. It changes the mailbox in `dir` for good: run it on a copy.
fn keep_offline_only(dir: &Path, keep: &str) -> Result<(), String> {
    use mailcraft_engine::store;
    use mailcraft_model::sync::ServerMessage;
    use mailcraft_model::{AccountKind, ServerFolder};
    let path = dir.join("mailbox.mailcraft");
    let disk = |what: &str| {
        let (content, files) = dir_size(&store::blob_dir(&path));
        let file = std::fs::metadata(&path).map_or(0, |m| m.len());
        println!("  on disk {what}: mailbox file {}, content {} in {files} files, {} in all", mb(file), mb(content), mb(file + content));
    };
    let (opened, took) = timed(|| store::open(&path));
    let opened = opened?;
    println!("{}: {} messages, opened in {}", path.display(), opened.mailbox.messages.len(), ms(took).trim());
    disk("as opened");
    let mut mb_ = opened.mailbox;
    let newest = mb_.messages.values().map(|m| m.received).max().unwrap_or(0);
    let ids: Vec<u64> = mb_.accounts.iter().map(|a| a.id).collect();
    for a in &mut mb_.accounts {
        a.kind = AccountKind::Microsoft;
    }
    for f in &mut mb_.folders {
        f.server = Some(ServerFolder { id: format!("F{}", f.id), ..Default::default() });
    }
    let messages: Vec<u64> = mb_.messages.keys().copied().collect();
    for id in messages {
        let m = mb_.message_mut(id).map_err(|e| e.to_string())?;
        m.server = Some(ServerMessage { id: format!("M{id}"), folder: format!("F{}", m.folder), ..Default::default() });
    }
    let mut s = Session::at(mb_, newest, 0);
    s.path = Some(path.clone());
    for id in ids {
        s.run("account.edit", &serde_json::json!({"id": id, "keepOffline": keep})).map_err(|e| e.to_string())?;
    }
    let (saved, took) = timed(|| s.run("file.save", &serde_json::json!({})));
    saved.map_err(|e| e.to_string())?;
    println!("  as a Microsoft account's, window {keep}, the clock at its newest message: saved in {}", ms(took).trim());
    let (size, took) =
        timed(|| s.run("account.offlineSize", &serde_json::json!({"account": s.mailbox.accounts.first().map_or(0, |a| a.id), "keepOffline": keep})));
    let size = size.map_err(|e| e.to_string())?;
    println!("  account.offlineSize {}: {} messages, {}", ms(took).trim(), size["messages"], size["size"]);
    let started = Instant::now();
    let (mut thinned, mut rounds) = (0u64, 0u32);
    loop {
        let said = s.run("account.thinOffline", &serde_json::json!({"most": 100_000})).map_err(|e| e.to_string())?;
        thinned += said["thinned"].as_u64().unwrap_or(0);
        rounds += 1;
        if said["left"].as_u64().unwrap_or(0) == 0 || rounds > 100 {
            break;
        }
    }
    let thin_took = started.elapsed();
    println!("  {thinned} messages back to headers and preview in {} ({rounds} calls)", ms(thin_took).trim());
    let (saved, took) = timed(|| s.run("file.save", &serde_json::json!({})));
    saved.map_err(|e| e.to_string())?;
    println!("  saved, and their content files removed, in {}", ms(took).trim());
    disk("after");
    match memory() {
        Some((rss, peak)) => println!("  memory: {} resident, peak {}", mb(rss), mb(peak)),
        None => println!("  memory: not readable on this system"),
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(dir) = args.iter().position(|a| a == "--open").and_then(|i| args.get(i + 1)) {
        if let Some(keep) = args.iter().position(|a| a == "--keep-offline").and_then(|i| args.get(i + 1)) {
            return keep_offline_only(Path::new(dir), keep);
        }
        return open_only(Path::new(dir));
    }
    let arg = |name: &str, default: usize| {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(default)
    };
    let (messages, attachments) = (arg("--messages", 5000), arg("--attachments", 300));
    let split_only = args.iter().any(|a| a == "--split-only");
    let content = expected_content(messages, attachments);
    let needed = content.saturating_mul(if split_only { 5 } else { 7 }) / 2;
    match memory_free() {
        // A quarter is left for everything else, which goes on growing while a long run runs.
        Some(free) if needed > free / 4 * 3 && !args.iter().any(|a| a == "--force") => {
            return Err(format!(
                "{messages} messages and {attachments} attachments are about {} of content, and this run would need about {} of memory \
                 at its peak; {} is free, and a quarter of that is left for everything else. Ask for fewer messages{}, or pass --force to run anyway.",
                mb(content),
                mb(needed),
                mb(free),
                if split_only { "" } else { ", add --split-only" }
            ));
        }
        Some(free) => println!("about {} of content; the peak should be near {} of the {} free", mb(content), mb(needed), mb(free)),
        None => {}
    }
    let exchange = args.iter().any(|a| a == "--exchange-headers");
    let (made, built) = timed(|| build(messages, attachments, exchange));
    let (mailbox, size) = made?;
    println!("{messages} messages, {attachments} attachments, {} of content (built in {}; peak memory {})", mb(size), ms(built).trim(), peak());
    let (copy, clone) = timed(|| mailbox.clone());
    println!("a snapshot (clone of the mailbox) {}", ms(clone));
    drop(copy);
    let keep = args.iter().position(|a| a == "--keep").and_then(|i| args.get(i + 1)).map(std::path::PathBuf::from);
    let dir = keep.clone().unwrap_or_else(|| std::env::temp_dir().join(format!("mailcraft-bench-{}", std::process::id())));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let single =
        if split_only { Ok(()) } else { single_file(&mailbox, &dir, keep.is_some()).map(|()| println!("  peak memory so far          {}", peak())) };
    let result = single.and_then(|()| split(&mailbox, &dir)).map(|()| println!("  peak memory in all          {}", peak()));
    match keep {
        Some(dir) => println!("\nkept in {}", dir.display()),
        None => {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    result
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
