// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's header lines in two parts (HS): what is read by name stays on the message, the
//! rest is its header block, and the mailbox file and View Source have every line, in order.

use super::*;

/// An Exchange-like header, invented: trace and reports among what is read by name.
pub(super) fn exchange_headers(seed: usize) -> Vec<(String, String)> {
    let filler = "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo".repeat(40);
    vec![
        ("Received".into(), format!("from a{seed}.prod.example.org by b{seed}.prod.example.org with HTTPS; Mon, 1 Sep 2025 09:00:00 +0000")),
        ("Authentication-Results".into(), "mx.microsoft.com 1; spf=pass smtp.mailfrom=example.org; dmarc=pass header.from=example.org".into()),
        ("Received".into(), "from mail.example.org (192.0.2.1) by DB1PEPF00000001.mail.protection.outlook.com (10.167.242.1)".into()),
        ("X-MS-Exchange-Organization-AuthAs".into(), "Anonymous".into()),
        ("X-Microsoft-Antispam-Message-Info".into(), filler.clone()),
        ("DKIM-Signature".into(), format!("v=1; a=rsa-sha256; d=example.org; s=s1; b={filler}")),
        ("List-Id".into(), "<news.example.org>".into()),
        ("X-MS-Exchange-AntiSpam-MessageData-0".into(), filler),
        ("Return-Path".into(), "<bounce@example.org>".into()),
    ]
}

/// A mailbox whose messages carry Exchange-like headers, split as a message read is.
pub(super) fn with_exchange_headers(messages: usize) -> Mailbox {
    let mut mb = mailbox(messages);
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    for (n, id) in ids.into_iter().enumerate() {
        let mut m = Message::clone(mb.message(id).unwrap());
        m.headers = exchange_headers(n);
        m.split_headers();
        mb.messages.insert(id, Arc::new(m));
    }
    mb
}

#[test]
fn every_header_line_is_saved_and_comes_back_in_its_place() {
    let mb = with_exchange_headers(6);
    let id = *mb.messages.keys().next().unwrap();
    let m = mb.message(id).unwrap();
    assert!(m.headers.len() < exchange_headers(0).len() && !m.header_block.none_in_memory());
    let tmp = Temp::new("headers-round-trip");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    let back = whole(&path);
    for (id, m) in &mb.messages {
        let read = back.message(*id).unwrap();
        assert_eq!(read.all_headers(), m.all_headers(), "message {id}");
        assert_eq!(read.headers, m.headers);
    }
}

/// The content files beside the mailbox file at `path` that hold a message's header block.
fn block_files(path: &Path) -> Vec<PathBuf> {
    files(&blob_dir(path))
        .into_iter()
        .filter(|(file, _)| fs::read(file).is_ok_and(|bytes| mailcraft_model::HeaderBlock::read(&bytes).is_some_and(|b| !b.lines().is_empty())))
        .map(|(file, _)| file)
        .collect()
}

/// The packs of header blocks beside the mailbox file at `path` (#1376).
pub(super) fn packs(path: &Path) -> Vec<PathBuf> {
    pack::on_disk(&blob_dir(path)).into_iter().map(|(_, file)| file).collect()
}

/// How many bytes the packs beside the mailbox file at `path` hold.
pub(super) fn pack_bytes(path: &Path) -> u64 {
    packs(path).iter().map(|f| fs::metadata(f).unwrap().len()).sum()
}

/// The bytes of the header blocks of `mb` that go outside the record, each once, its messages'
/// header lines split as they are read.
pub(super) fn block_bytes(mb: &Mailbox) -> u64 {
    let mut seen = HashSet::new();
    let mut total = 0;
    for m in mb.messages.values() {
        let mut m = Message::clone(m);
        m.split_headers();
        let data = &m.header_block.data;
        if data.len() > INLINE_MAX && seen.insert(Digest::of(data)) {
            total += data.len() as u64;
        }
    }
    total
}

/// A mailbox file of format 3, as MailCraft 0.12.0 wrote it: every header line of a message in
/// its record. The mailbox is given back as it was, its header lines unsplit.
fn format_3(path: &Path, messages: usize) -> Mailbox {
    let mut mb = mailbox(messages);
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    for (n, id) in ids.into_iter().enumerate() {
        let mut m = Message::clone(mb.message(id).unwrap());
        m.headers = exchange_headers(n);
        mb.messages.insert(id, Arc::new(m));
    }
    save(path, &mb).unwrap();
    let mut bytes = fs::read(path).unwrap();
    bytes.splice(16..20, 3u32.to_le_bytes());
    fs::write(path, bytes).unwrap();
    assert_eq!(crate::store_before::format_on_disk(path), Some(3));
    mb
}

#[test]
fn a_large_header_block_is_kept_beside_the_mailbox_file_and_read_when_wanted() {
    let mb = with_exchange_headers(4);
    let tmp = Temp::new("headers-outside");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    assert_eq!(crate::store_before::format_on_disk(&path), Some(INDEX_VERSION));
    let file = fs::read(&path).unwrap();
    assert!(!file.windows(17).any(|w| w == b"X-Microsoft-Antis"), "the block is not in the mailbox file");
    // In one pack, not a file each (#1376).
    assert!(block_files(&path).is_empty());
    assert_eq!(packs(&path).len(), 1);
    assert_eq!(pack_bytes(&path), block_bytes(&mb));
    clear_cache();
    let opened = open(&path).unwrap();
    for m in opened.mailbox.messages.values() {
        assert!(m.header_block.data.is_empty() && m.header_block.kept.is_some(), "not read at opening");
        assert_eq!(m.header("List-Id"), Some("<news.example.org>"), "what is read by name is there");
    }
    let back = whole(&path);
    for (id, m) in &mb.messages {
        assert_eq!(back.message(*id).unwrap().all_headers(), m.all_headers());
    }
}

#[test]
fn a_file_of_format_3_puts_its_header_blocks_beside_it_as_it_opens_and_is_written_anew() {
    let tmp = Temp::new("headers-format-3");
    let path = tmp.file();
    let mb = format_3(&path, 5);
    let had = fs::read(&path).unwrap();
    clear_cache();
    let opened = open_window(&path);
    // Each block went beside the file as it was read, and is named on its message.
    assert!(opened.mailbox.messages.values().all(|m| m.header_block.data.is_empty() && m.header_block.kept.is_some()));
    assert!(block_files(&path).is_empty());
    assert_eq!(pack_bytes(&path), block_bytes(&mb), "in a pack");
    // The file itself is as it was until a save: a stop now (a crash) loses nothing.
    assert_eq!(fs::read(&path).unwrap(), had);
    assert_eq!(sweep(&path, Duration::ZERO).unwrap().removed, 0, "a sweep leaves what the open put beside it");
    assert_eq!(pack_bytes(&path), block_bytes(&mb));
    clear_cache();
    let again = whole(&path);
    for (id, m) in &mb.messages {
        assert_eq!(again.message(*id).unwrap().all_headers(), m.headers, "opened again after a stop");
    }
    // The next save writes the file in this format, with the old one kept beside it.
    let opened = open(&path).unwrap();
    save(&path, &opened.mailbox).unwrap();
    assert_eq!(crate::store_before::format_on_disk(&path), Some(INDEX_VERSION));
    let before = fs::read_dir(&tmp.0).unwrap().flatten().find(|e| e.file_name().to_string_lossy().contains(".before-")).unwrap();
    assert_eq!(fs::read(before.path()).unwrap(), had);
    clear_cache();
    let after = whole(&path);
    for (id, m) in &mb.messages {
        assert_eq!(after.message(*id).unwrap().all_headers(), m.headers, "every line, in its place");
    }
    assert!(fs::metadata(&path).unwrap().len() < had.len() as u64);
}

#[test]
fn a_missing_or_damaged_header_block_leaves_the_message_without_it_and_nothing_else() {
    let mb = with_exchange_headers(3);
    let tmp = Temp::new("headers-missing");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    // Two blocks damaged where they are in the pack (#1376): one written over, one a byte changed.
    for (n, id) in ids[..2].iter().enumerate() {
        let data = &mb.message(*id).unwrap().header_block.data;
        let loc = pack::find(&blob_dir(&path), &Digest::of(data)).unwrap();
        let file = blob_dir(&path).join(pack::name(loc.pack).unwrap());
        let mut bytes = fs::read(&file).unwrap();
        let at = loc.at as usize;
        if n == 0 {
            bytes[at..at + 16].fill(0);
        } else {
            bytes[at + 1] ^= 0xff;
        }
        fs::write(&file, bytes).unwrap();
    }
    clear_cache();
    let opened = open(&path).unwrap();
    assert_eq!(opened.unavailable, 0, "a header block is not the message's content");
    let back = whole(&path);
    for id in &ids[..2] {
        let m = back.message(*id).unwrap();
        assert_eq!(m.all_headers(), mb.message(*id).unwrap().headers, "the lines read by name, and no more");
        assert_eq!(m.body.text, mb.message(*id).unwrap().body.text);
    }
    assert_eq!(back.message(ids[2]).unwrap().all_headers(), mb.message(ids[2]).unwrap().all_headers());
}

#[test]
fn a_hostile_name_for_a_header_block_names_none() {
    let max = mailcraft_model::headers::MAX_HEADER_BLOCK as u64;
    let name = Digest::of(b"x").hex();
    let at = Some(pack::Loc { pack: 1, at: 0 });
    for (hash, len) in [(name.clone(), max + 1), (name.clone(), u64::MAX), ("not a name".into(), 10), (String::new(), 0)] {
        for at in [None, at] {
            let wire = WireParts { headers: Some(HeaderWire { hash: hash.clone(), len, at }), ..Default::default() };
            assert!(wire.parts().headers.is_none() && wire.parts().headers_at.is_none() && wire.pack().is_none());
        }
    }
    let wire = WireParts { headers: Some(HeaderWire { hash: name, len: max, at }), ..Default::default() };
    assert!(wire.parts().headers.is_some() && wire.parts().headers_at == at);
}

proptest! {
    #[test]
    fn any_content_names_never_name_more_than_a_block_can_be(
        name in ".{0,80}",
        len in any::<u64>(),
        pack in any::<u64>(),
        at in any::<u64>(),
        junk in proptest::collection::vec(any::<u8>(), 0..256),
    ) {
        let max = mailcraft_model::headers::MAX_HEADER_BLOCK as u64;
        let json = serde_json::to_vec(&json!({"headers": [name, len]})).unwrap();
        let packed = serde_json::to_vec(&json!({"headers": [name, len, pack, at]})).unwrap();
        for bytes in [json, packed, junk] {
            if let Ok(wire) = load_limit::read_json::<WireParts>(&bytes, &open_budget()) {
                prop_assert!(wire.parts().headers.is_none_or(|p| p.r.len <= max));
            }
        }
    }
}

/// Opened as the window opens a file, without the save `open_for_window` makes after it.
fn open_window(path: &Path) -> Opened {
    open_as(path, &OpenProgress::default(), Launch { checks_later: false, rows: Some(OPENING_ROWS) }).unwrap()
}

/// Every content file beside the mailbox at `path`, made a day old: past any sweep's grace.
fn a_day_old(path: &Path) {
    for (file, _) in files(&blob_dir(path)) {
        fs::OpenOptions::new().write(true).open(&file).unwrap().set_modified(SystemTime::now() - Duration::from_secs(86_400)).unwrap();
    }
}

#[test]
fn an_opening_that_only_reads_a_file_of_format_3_puts_nothing_beside_it() {
    let tmp = Temp::new("headers-format-3-read");
    let path = tmp.file();
    let mb = format_3(&path, 3);
    let had = fs::read(&path).unwrap();
    clear_cache();
    let opened = open(&path).unwrap();
    assert!(block_files(&path).is_empty() && packs(&path).is_empty());
    assert_eq!(fs::read(&path).unwrap(), had);
    for (id, m) in &mb.messages {
        assert_eq!(opened.mailbox.message(*id).unwrap().all_headers(), m.headers, "held in memory as before");
    }
}

#[test]
fn the_window_writes_a_file_of_format_3_anew_as_it_opens_it() {
    let tmp = Temp::new("headers-format-3-window");
    let path = tmp.file();
    let mb = format_3(&path, 3);
    clear_cache();
    open_for_window(&path, &OpenProgress::default()).unwrap();
    assert_eq!(crate::store_before::format_on_disk(&path), Some(INDEX_VERSION));
    clear_cache();
    let back = whole(&path);
    for (id, m) in &mb.messages {
        assert_eq!(back.message(*id).unwrap().all_headers(), m.headers);
    }
}

#[test]
fn what_an_opening_put_beside_a_file_outlives_a_sweep_that_reads_the_file() {
    let tmp = Temp::new("headers-format-3-sweep");
    let path = tmp.file();
    let mb = format_3(&path, 4);
    clear_cache();
    let opened = open_window(&path);
    // The file changed under the open (another program, a clock): the sweep reads it, and a file of
    // format 3 names none of the blocks.
    fs::OpenOptions::new().write(true).open(&path).unwrap().set_modified(SystemTime::now() - Duration::from_secs(3_600)).unwrap();
    a_day_old(&path);
    assert_eq!(sweep(&path, Duration::ZERO).unwrap().removed, 0);
    assert_eq!(pack_bytes(&path), block_bytes(&mb));
    save(&path, &opened.mailbox).unwrap();
    clear_cache();
    let back = whole(&path);
    for (id, m) in &mb.messages {
        assert_eq!(back.message(*id).unwrap().all_headers(), m.headers);
    }
}

#[test]
fn a_block_gone_before_the_save_never_fails_it() {
    let tmp = Temp::new("headers-format-3-gone");
    let path = tmp.file();
    let mb = format_3(&path, 3);
    clear_cache();
    let mut opened = open_window(&path);
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    // One message is gone from the mailbox, and every header block from the disk.
    opened.mailbox.messages.remove(&ids[0]);
    packs(&path).into_iter().for_each(|f| fs::remove_file(f).unwrap());
    save(&path, &opened.mailbox).unwrap();
    save(&path, &opened.mailbox).unwrap();
    clear_cache();
    let back = whole(&path);
    for id in &ids[1..] {
        let m = back.message(*id).unwrap();
        assert_eq!(m.header("List-Id"), Some("<news.example.org>"), "the lines read by name stay");
        assert_eq!(m.body.text, mb.message(*id).unwrap().body.text);
    }
}

#[test]
fn a_file_of_format_3_that_names_a_block_that_isnt_there_still_saves() {
    let tmp = Temp::new("headers-format-3-hostile");
    let path = tmp.file();
    let mb = with_exchange_headers(3);
    save(&path, &mb).unwrap();
    let mut bytes = fs::read(&path).unwrap();
    bytes.splice(16..20, 3u32.to_le_bytes());
    fs::write(&path, bytes).unwrap();
    packs(&path).into_iter().for_each(|f| fs::remove_file(f).unwrap());
    clear_cache();
    let opened = open_window(&path);
    save(&path, &opened.mailbox).unwrap();
    clear_cache();
    let back = whole(&path);
    for id in mb.messages.keys() {
        assert_eq!(back.message(*id).unwrap().header("List-Id"), Some("<news.example.org>"));
    }
}

#[test]
fn what_writes_a_message_whole_writes_every_header_line_kept_beside_the_mailbox_file() {
    let mb = with_exchange_headers(3);
    let tmp = Temp::new("headers-writers");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    clear_cache();
    let (mut s, _) = session_on(&path, &mb, 3);
    let id = *mb.messages.keys().next().unwrap();
    let was = mb.message(id).unwrap();
    assert!(!s.mailbox.message(id).unwrap().header_block.is_loaded(), "kept beside the file");
    let opts = mailcraft_mime_model::WriteOptions::default();
    let expected = mailcraft_mime_model::write(was, &opts);
    // Byte for byte what the message wrote before its block went outside: what a copy put on
    // another server, the Outbox and a forward as attachment are made of.
    assert_eq!(mailcraft_mime_model::write(&s.loaded_message(id).unwrap(), &opts), expected);
    let outbox = s.loaded_outbox(vec![Message::clone(s.mailbox.message(id).unwrap())]);
    assert_eq!(mailcraft_mime_model::write(&outbox[0], &opts), expected);
    // Save Message As (.eml) and Export Folder (.mbox): every line, in its place.
    let eml = s.run("file.exportMessage", &json!({"id": id})).unwrap();
    let eml = crate::cmd::base64_decode(eml["data"].as_str().unwrap()).unwrap();
    let lines = |m: &Message| m.all_headers().iter().map(|(n, v)| format!("{n}: {v}")).collect::<Vec<_>>();
    assert_eq!(lines(&mailcraft_mime_model::parse(&eml)), lines(&mailcraft_mime_model::parse(&expected)));
    let mbox = s.run("file.exportFolder", &json!({"id": was.folder})).unwrap();
    let all = mailcraft_mime_model::parse_mbox(&crate::cmd::base64_decode(mbox["data"].as_str().unwrap()).unwrap());
    assert!(all.iter().any(|m| m.message_id == was.message_id && lines(m) == lines(&mailcraft_mime_model::parse(&expected))));
    // Save As (one file): every line, in the shape every MailCraft reads.
    let whole = s.loaded_mailbox().unwrap();
    assert_eq!(whole.message(id).unwrap().all_headers(), was.all_headers());
    let single = crate::io::write_mailbox(&whole).unwrap();
    assert!(String::from_utf8_lossy(&single).contains("X-MS-Exchange-AntiSpam-MessageData-0"));
    // View Source: every line, in its place.
    let source = s.run("mail.viewSource", &json!({"id": id})).unwrap();
    let source = source["source"].as_str().unwrap();
    let mut at = 0;
    for (name, value) in was.all_headers() {
        let line = format!("{name}: {value}\n");
        let found = source.get(at..).and_then(|rest| rest.find(&line)).unwrap_or_else(|| panic!("{line} in {source}"));
        at += found + line.len();
    }
    // Its block gone from the disk: what reads it says so in the log, and writes the rest.
    packs(&path).into_iter().for_each(|f| fs::remove_file(f).unwrap());
    clear_cache();
    let without = s.loaded_message(id).unwrap();
    assert_eq!(without.all_headers(), was.headers);
    assert!(s.run("mail.viewSource", &json!({"id": id})).unwrap()["source"].as_str().unwrap().contains("List-Id: <news.example.org>"));
}
