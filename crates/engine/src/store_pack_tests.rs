// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Header blocks packed (#1376): written into a few pack files rather than a file each, read back
//! checked, and the space of what nothing names given back by the whole rewrite.

use super::headers::{block_bytes, exchange_headers, pack_bytes, packs, with_exchange_headers};
use super::*;

/// The mailbox at `path` read whole, each message's every header line as it is kept.
fn lines_of(path: &Path) -> BTreeMap<Id, Vec<(String, String)>> {
    whole(path).messages.iter().map(|(id, m)| (*id, m.all_headers())).collect()
}

fn lines(mb: &Mailbox) -> BTreeMap<Id, Vec<(String, String)>> {
    mb.messages.iter().map(|(id, m)| (*id, m.all_headers())).collect()
}

/// The pack in use beside `path`, and its bytes.
fn the_pack(path: &Path) -> (PathBuf, Vec<u8>) {
    let all = packs(path);
    assert_eq!(all.len(), 1, "{all:?}");
    let bytes = fs::read(&all[0]).unwrap();
    (all[0].clone(), bytes)
}

/// Write the whole mailbox file at `path` now, as a save does when it rewrites it.
fn rewrite(path: &Path, mb: &Mailbox) -> Saved {
    let shared = shared(path);
    let mut st = lock(&shared.state);
    let live = pack_live(&blob_dir(path), st.disk.as_ref(), mb, &[]);
    compact(&mut st, &resolve(path), mb, None, live, &[], &Progress::default()).unwrap()
}

#[test]
fn header_blocks_go_into_one_pack_and_come_back_whole() {
    let mb = with_exchange_headers(40);
    let tmp = Temp::new("pack-round-trip");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    let (file, bytes) = the_pack(&path);
    assert_eq!(file.file_name().unwrap(), "headers-1.pack");
    assert_eq!(bytes.len() as u64, block_bytes(&mb));
    // No content file of its own for any block.
    assert!(files(&blob_dir(&path)).iter().all(|(f, _)| f == &file || mailcraft_model::HeaderBlock::read(&fs::read(f).unwrap()).is_none()));
    // The record names where each is.
    let record = String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned();
    assert!(record.contains(",1,0]"), "a block at the start of pack 1");
    clear_cache();
    assert_eq!(lines_of(&path), lines(&mb));
    // A message read and saved again writes nothing more into the pack.
    let opened = open(&path).unwrap();
    let mut changed = opened.mailbox.clone();
    let id = *changed.messages.keys().next().unwrap();
    let mut m = Message::clone(changed.message(id).unwrap());
    m.read = !m.read;
    changed.messages.insert(id, Arc::new(m));
    save(&path, &changed).unwrap();
    assert_eq!(the_pack(&path).1, bytes);
    clear_cache();
    assert_eq!(lines_of(&path), lines(&mb));
}

#[test]
fn a_missing_pack_leaves_every_block_unavailable_and_the_mailbox_opens() {
    let mb = with_exchange_headers(5);
    let tmp = Temp::new("pack-missing");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    fs::remove_file(the_pack(&path).0).unwrap();
    clear_cache();
    let opened = open(&path).unwrap();
    assert_eq!(opened.unavailable, 0, "a header block is not the message's content");
    for (id, m) in &whole(&path).messages {
        let was = mb.message(*id).unwrap();
        assert_eq!(m.all_headers(), was.headers, "the lines read by name, and no more");
        assert_eq!(m.body.text, was.body.text);
    }
    // And it saves, naming what it named.
    save(&path, &opened.mailbox).unwrap();
    assert!(open(&path).is_ok());
}

#[test]
fn a_truncated_pack_loses_only_the_blocks_past_its_end() {
    let mb = with_exchange_headers(6);
    let tmp = Temp::new("pack-truncated");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    let (file, bytes) = the_pack(&path);
    let dir = blob_dir(&path);
    // Cut short in the middle of the last block.
    let last = mb.messages.values().map(|m| pack::find(&dir, &Digest::of(&m.header_block.data)).unwrap()).max_by_key(|l| l.at).unwrap();
    fs::OpenOptions::new().write(true).open(&file).unwrap().set_len(last.at + 10).unwrap();
    assert!(fs::metadata(&file).unwrap().len() < bytes.len() as u64);
    clear_cache();
    let back = whole(&path);
    let mut lost = 0;
    for (id, m) in &back.messages {
        let was = mb.message(*id).unwrap();
        if m.all_headers() == was.all_headers() {
            continue;
        }
        lost += 1;
        assert_eq!(m.all_headers(), was.headers);
    }
    assert_eq!(lost, 1);
}

#[test]
fn a_block_whose_bytes_are_not_what_its_name_says_is_unavailable() {
    let mb = with_exchange_headers(3);
    let tmp = Temp::new("pack-wrong-hash");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    let dir = blob_dir(&path);
    let (id, m) = mb.messages.iter().next().unwrap();
    let r = Ref::of(&m.header_block.data);
    let loc = pack::find(&dir, &r.hash).unwrap();
    assert_eq!(pack::read_at(&dir, loc, &r).as_deref(), Some(m.header_block.data.as_slice()));
    // Every hostile number is no block, never a panic.
    for (at, len) in [(u64::MAX, r.len), (u64::MAX - 3, 8), (loc.at + 1, r.len), (loc.at, r.len + 1), (loc.at, u64::MAX), (0, 0)] {
        assert_eq!(pack::read_at(&dir, pack::Loc { pack: loc.pack, at }, &Ref { hash: r.hash, len }), None, "{at} {len}");
    }
    assert_eq!(pack::read_at(&dir, pack::Loc { pack: 0, at: 0 }, &r), None);
    assert_eq!(pack::read_at(&dir, pack::Loc { pack: 77, at: loc.at }, &r), None);
    // The same length of other bytes where it was.
    let (file, mut bytes) = the_pack(&path);
    bytes[loc.at as usize + 5] ^= 1;
    fs::write(&file, bytes).unwrap();
    clear_cache();
    assert_eq!(whole(&path).message(*id).unwrap().all_headers(), m.headers);
}

#[test]
fn bytes_a_save_cut_short_left_at_the_end_of_the_pack_are_never_read() {
    let mut mb = with_exchange_headers(4);
    let tmp = Temp::new("pack-torn-tail");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    let (file, before) = the_pack(&path);
    // A save that wrote half a block and stopped before the mailbox file named it.
    let torn = vec![0x5a; 777];
    fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(&torn).unwrap();
    clear_cache();
    assert_eq!(lines_of(&path), lines(&mb));
    // The next save writes after them, and both what was and what is new read back.
    let opened = open(&path).unwrap();
    mb = opened.mailbox.clone();
    let id: Id = 9_000;
    let mut m = Message::clone(mb.messages.values().next().unwrap());
    m.id = id;
    m.headers = exchange_headers(9_000);
    m.header_block = Default::default();
    m.split_headers();
    let block = m.header_block.data.clone();
    let wanted = m.all_headers();
    mb.messages.insert(id, Arc::new(m));
    save(&path, &mb).unwrap();
    let loc = pack::find(&blob_dir(&path), &Digest::of(&block)).unwrap();
    assert_eq!(loc.at, before.len() as u64 + torn.len() as u64);
    clear_cache();
    let back = whole(&path);
    assert_eq!(back.message(id).unwrap().all_headers(), wanted);
    assert_eq!(back.messages.len(), 5);
}

#[test]
fn the_whole_rewrite_gives_back_what_nothing_names_and_survives_a_stop_before_it_is_done() {
    let all = with_exchange_headers(60);
    let tmp = Temp::new("pack-compact");
    let path = tmp.file();
    save(&path, &all).unwrap();
    let (old_pack, old_bytes) = the_pack(&path);
    assert_eq!(old_bytes.len() as u64, block_bytes(&all));
    let old_file = fs::read(&path).unwrap();
    // Most of the mail goes: its blocks are dead weight in the pack, and the save that finds it
    // mostly dead writes everything anew, into a fresh pack of only what is named.
    let mut few = all.clone();
    let ids: Vec<Id> = few.messages.keys().copied().collect();
    for id in &ids[5..] {
        few.messages.remove(id);
    }
    assert!(block_bytes(&all) - block_bytes(&few) > pack::PACK_SLACK);
    let saved = save(&path, &few).unwrap();
    assert!(saved.compacted);
    let (new_pack, new_bytes) = the_pack(&path);
    assert_ne!(new_pack, old_pack, "a fresh pack, under a new name, the old one gone");
    assert_eq!(new_bytes.len() as u64, block_bytes(&few));
    clear_cache();
    assert_eq!(lines_of(&path), lines(&few));
    // Had it stopped before the new file was renamed over the old one: the old file, the old pack,
    // and the fresh pack nothing names. The old file reads as it did.
    fs::write(&path, &old_file).unwrap();
    fs::write(&old_pack, &old_bytes).unwrap();
    clear_cache();
    assert_eq!(lines_of(&path), lines(&all));
    // The sweep takes the pack nothing names, and leaves the one named.
    a_day_old_packs(&path);
    sweep(&path, Duration::ZERO).unwrap();
    assert_eq!(the_pack(&path).0, old_pack);
    clear_cache();
    assert_eq!(lines_of(&path), lines(&all));
}

/// Every pack beside `path`, made a day old.
fn a_day_old_packs(path: &Path) {
    for file in packs(path) {
        fs::OpenOptions::new().write(true).open(&file).unwrap().set_modified(SystemTime::now() - Duration::from_secs(86_400)).unwrap();
    }
}

/// A mailbox file of format 4 as a development build wrote it before #1376: each header block a
/// content file of its own, named `[hash, length]`.
fn dev_format_4(path: &Path, mb: &Mailbox) {
    let dir = blob_dir(path);
    let _ = make_dir(&dir);
    let mut out = index::header().to_vec();
    index::rest(&mut out, &rest_json(mb).unwrap()).unwrap();
    let mut records = 1;
    for (id, m) in &mb.messages {
        let p = prepare(*id, m, &mut HashMap::new(), None).unwrap();
        for (r, bytes) in p.parts.content(m) {
            put(&dir, &r, bytes).unwrap();
        }
        index::put(&mut out, (Coll::Message, *id), &p.json, &p.refs).unwrap();
        records += 1;
    }
    index::commit(&mut out, records).unwrap();
    fs::write(path, out).unwrap();
}

#[test]
fn blocks_a_development_build_kept_a_file_each_are_read_and_moved_into_a_pack() {
    let mb = with_exchange_headers(5);
    let tmp = Temp::new("pack-dev-format-4");
    let path = tmp.file();
    dev_format_4(&path, &mb);
    assert_eq!(crate::store_before::format_on_disk(&path), Some(INDEX_VERSION));
    let loose: Vec<PathBuf> = mb.messages.values().map(|m| blob_file(&path, &m.header_block.data)).collect();
    assert!(loose.iter().all(|f| f.is_file()) && packs(&path).is_empty());
    clear_cache();
    assert_eq!(lines_of(&path), lines(&mb), "read from where they are");
    // A save appended to the file leaves them where they are.
    let opened = open(&path).unwrap();
    let mut changed = opened.mailbox.clone();
    let id = *changed.messages.keys().next().unwrap();
    let mut m = Message::clone(changed.message(id).unwrap());
    m.read = !m.read;
    changed.messages.insert(id, Arc::new(m));
    assert!(!save(&path, &changed).unwrap().compacted);
    assert!(loose.iter().all(|f| f.is_file()));
    // The whole rewrite moves them into a pack, and the files go.
    rewrite(&path, &changed);
    assert!(loose.iter().all(|f| !f.exists()));
    assert_eq!(pack_bytes(&path), block_bytes(&mb));
    clear_cache();
    assert_eq!(lines_of(&path), lines(&mb));
    assert_eq!(sweep(&path, Duration::ZERO).unwrap().removed, 0);
    assert_eq!(lines_of(&path), lines(&mb));
}

#[test]
fn a_file_of_format_3_is_upgraded_into_a_pack_end_to_end() {
    let tmp = Temp::new("pack-format-3");
    let path = tmp.file();
    // As MailCraft 0.12.0 wrote it: every header line in its message's record.
    let mut mb = super::mailbox(8);
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    for (n, id) in ids.into_iter().enumerate() {
        let mut m = Message::clone(mb.message(id).unwrap());
        m.headers = exchange_headers(n);
        mb.messages.insert(id, Arc::new(m));
    }
    save(&path, &mb).unwrap();
    let mut bytes = fs::read(&path).unwrap();
    bytes.splice(16..20, 3u32.to_le_bytes());
    fs::write(&path, &bytes).unwrap();
    for f in packs(&path) {
        fs::remove_file(f).unwrap();
    }
    clear_cache();
    open_for_window(&path, &OpenProgress::default()).unwrap();
    assert_eq!(crate::store_before::format_on_disk(&path), Some(INDEX_VERSION));
    // The file as it was is kept for the MailCraft before this one, as it was.
    let before = fs::read_dir(&tmp.0).unwrap().flatten().find(|e| e.file_name().to_string_lossy().contains(".before-")).unwrap();
    assert_eq!(fs::read(before.path()).unwrap(), bytes);
    assert_eq!(packs(&path).len(), 1);
    assert_eq!(pack_bytes(&path), block_bytes(&mb));
    clear_cache();
    for (id, m) in &whole(&path).messages {
        assert_eq!(m.all_headers(), mb.message(*id).unwrap().headers);
    }
    a_day_old_packs(&path);
    sweep(&path, Duration::ZERO).unwrap();
    assert_eq!(packs(&path).len(), 1);
}

#[test]
fn a_header_block_name_is_two_or_four_numbers_and_nothing_else() {
    let hash = Digest::of(b"x").hex();
    let read = |json: Value| load_limit::read_json::<WireParts>(&serde_json::to_vec(&json).unwrap(), &open_budget());
    let loose = read(json!({"headers": [hash, 5]})).unwrap();
    assert_eq!(loose.headers.as_ref().unwrap().at, None);
    let packed = read(json!({"headers": [hash, 5, 2, 9]})).unwrap();
    assert_eq!(packed.headers.as_ref().unwrap().at, Some(pack::Loc { pack: 2, at: 9 }));
    assert_eq!(packed.pack(), Some(2));
    assert!(packed.hashes().next().is_none(), "a block in a pack is no content file");
    for bad in [
        json!({"headers": [hash]}),
        json!({"headers": [hash, 5, 2]}),
        json!({"headers": [hash, 5, 2, 9, 1]}),
        json!({"headers": [hash, 5, -1, 9]}),
        json!({"headers": [hash, 5, 4_294_967_296u64, 9]}),
        json!({"headers": [hash, 5, 2, "9"]}),
        json!({"headers": {"a": 1}}),
    ] {
        assert!(read(bad.clone()).is_err(), "{bad}");
    }
}

#[test]
fn pack_names_are_digits_and_nothing_else() {
    assert_eq!(pack::number("headers-1.pack"), Some(1));
    assert_eq!(pack::number("headers-4294967295.pack"), Some(u32::MAX));
    for bad in
        ["headers-0.pack", "headers-01.pack", "headers-.pack", "headers-4294967296.pack", "headers--1.pack", "headers-1.pack.tmp", "x/headers-1.pack"]
    {
        assert_eq!(pack::number(bad), None, "{bad}");
    }
    assert_eq!(pack::name(0), None);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    /// Whatever a pack holds and whatever a record says of it, a read gives the bytes named or
    /// nothing.
    #[test]
    fn any_pack_and_any_location_read_as_the_block_or_nothing(
        pack_bytes in proptest::collection::vec(any::<u8>(), 0..4096),
        at in any::<u64>(),
        len in any::<u64>(),
        small_at in 0u64..4200,
        small_len in 0u64..4200,
    ) {
        let tmp = Temp::new("pack-prop");
        let dir = tmp.0.join("x.blobs");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("headers-1.pack"), &pack_bytes).unwrap();
        for (at, len) in [(at, len), (small_at, small_len)] {
            let hash = pack_bytes.get(at as usize..(at as usize).saturating_add(len as usize)).map_or(Digest::of(b""), Digest::of);
            if let Some(bytes) = pack::read_at(&dir, pack::Loc { pack: 1, at }, &Ref { hash, len }) {
                prop_assert_eq!(bytes.len() as u64, len);
                prop_assert_eq!(Digest::of(&bytes), hash);
            }
        }
    }
}

/// #177 and #143: the header lines of mail let go of (an account removed) leave the disk once a
/// state without it is saved, though they are in a pack with others'.
#[test]
fn the_header_lines_of_mail_let_go_of_leave_the_pack() {
    let mut mb = with_exchange_headers(3);
    let tmp = Temp::new("pack-forget");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    let id = *mb.messages.keys().next().unwrap();
    let block = mb.message(id).unwrap().header_block.data.clone();
    let seen = |bytes: &[u8]| bytes.windows(19).any(|w| w == b"a0.prod.example.org");
    assert!(seen(&the_pack(&path).1));
    forget_when_saved(&path, 2, content_names(id, mb.messages.get(&id).unwrap()));
    // Not before a state without it is saved.
    assert_eq!(forget_due(&path, 1).removed, 0);
    mb.messages.remove(&id);
    save(&path, &mb).unwrap();
    assert!(forget_due(&path, 2).removed >= 1);
    let (_, bytes) = the_pack(&path);
    assert!(!seen(&bytes), "its lines are gone from the pack");
    assert!(pack::find(&blob_dir(&path), &Digest::of(&block)).is_none());
    clear_cache();
    assert_eq!(lines_of(&path), lines(&mb), "the others' are as they were");
    // The same message coming again is written anew, not named where its lines were.
    let mut again = mb.clone();
    let mut m = Message::clone(with_exchange_headers(3).message(id).unwrap());
    m.id = id;
    again.messages.insert(id, Arc::new(m));
    save(&path, &again).unwrap();
    clear_cache();
    assert_eq!(lines_of(&path), lines(&again));
}

/// The mailbox at `path` as an opening leaves it: header blocks on disk, not in memory.
fn opened_lazily(path: &Path) -> Mailbox {
    clear_cache();
    let mb = open(path).unwrap().mailbox;
    assert!(mb.messages.values().all(|m| m.header_block.data.is_empty() && m.header_block.kept.is_some()));
    mb
}

/// `mb` with most of its mail gone: the first `keep` messages stay.
fn only_first(mb: &Mailbox, keep: usize) -> Mailbox {
    let mut few = mb.clone();
    let ids: Vec<Id> = few.messages.keys().copied().collect();
    for id in &ids[keep..] {
        few.messages.remove(id);
    }
    few
}

/// `mb` with its first message marked read or unread.
fn toggled(mb: &Mailbox) -> Mailbox {
    let mut out = mb.clone();
    let id = *out.messages.keys().next().unwrap();
    let mut m = Message::clone(out.message(id).unwrap());
    m.read = !m.read;
    out.messages.insert(id, Arc::new(m));
    out
}

/// Review of #1376, P1: a program without the window's lock (the command line, an agent) wrote
/// the file, into a fresh pack, and removed the pack this one knew. This one's next save names
/// where that file says the blocks are, never a place in the pack that is gone.
#[test]
fn a_save_after_another_program_wrote_a_fresh_pack_names_where_the_blocks_are_now() {
    let all = with_exchange_headers(60);
    let tmp = Temp::new("pack-other-writer");
    let path = tmp.file();
    save(&path, &all).unwrap();
    let mine = opened_lazily(&path);
    // The other program, with its own copy of the files (so nothing it learns is known here),
    // takes most of the mail out and writes a fresh pack; what it leaves is put in their place.
    let other = Temp::new("pack-other-writer-b");
    let theirs = other.file();
    fs::copy(&path, &theirs).unwrap();
    fs::create_dir_all(blob_dir(&theirs)).unwrap();
    for f in packs(&path) {
        fs::copy(&f, blob_dir(&theirs).join(f.file_name().unwrap())).unwrap();
    }
    let few = only_first(&opened_lazily(&theirs), 5);
    assert!(save(&theirs, &few).unwrap().compacted);
    for f in packs(&path) {
        fs::remove_file(f).unwrap();
    }
    for f in packs(&theirs) {
        fs::copy(&f, blob_dir(&path).join(f.file_name().unwrap())).unwrap();
    }
    fs::copy(&theirs, &path).unwrap();
    assert_eq!(packs(&path).len(), 1);
    // This one saves what it has: the five both kept have their every line.
    save(&path, &toggled(&mine)).unwrap();
    clear_cache();
    let back = whole(&path);
    for id in few.messages.keys() {
        assert_eq!(back.message(*id).unwrap().all_headers(), all.message(*id).unwrap().all_headers(), "message {id}");
    }
    // And what it names is in a pack that is there.
    let on_disk: Vec<u32> = pack::on_disk(&blob_dir(&path)).into_iter().map(|(n, _)| n).collect();
    let (_, named) = named_by(&path).unwrap().unwrap();
    assert!(named.iter().all(|n| on_disk.contains(n)), "{named:?} {on_disk:?}");
}

/// Review of #1376, P1 and P2: a block known to be in the pack that can't be read when a fresh
/// pack would be written is never dropped with the old pack, nor stops the saves: the file is
/// written whole without a fresh pack, the old one kept, and every later save goes through.
#[cfg(unix)]
#[test]
fn a_block_that_cant_be_read_keeps_the_old_pack_and_the_saves_go_on() {
    use std::os::unix::fs::PermissionsExt as _;
    let all = with_exchange_headers(60);
    let tmp = Temp::new("pack-unreadable");
    let path = tmp.file();
    save(&path, &all).unwrap();
    let mine = opened_lazily(&path);
    let (old_pack, old_bytes) = the_pack(&path);
    fs::set_permissions(&old_pack, fs::Permissions::from_mode(0o000)).unwrap();
    // (Whoever reads every file anyway, root, can't test this.)
    if fs::read(&old_pack).is_ok() {
        fs::set_permissions(&old_pack, fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }
    // (Nothing held of what is taken out: the pack is mostly dead, and a fresh one is written.)
    let mut few = only_first(&mine, 5);
    drop(mine);
    save(&path, &few).unwrap();
    assert_eq!(packs(&path), std::slice::from_ref(&old_pack), "no fresh pack, the old one kept");
    // An edit after it is saved too, while the pack still can't be read.
    for _ in 0..2 {
        few = toggled(&few);
        save(&path, &few).unwrap();
    }
    assert_eq!(packs(&path), std::slice::from_ref(&old_pack));
    fs::set_permissions(&old_pack, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(fs::read(&old_pack).unwrap(), old_bytes, "the old pack is there, whole");
    clear_cache();
    let back = whole(&path);
    assert_eq!(back.messages.len(), 5);
    assert_eq!(lines_of(&path), lines(&only_first(&all, 5)), "every line of what was kept");
    assert_eq!(back.messages.values().next().unwrap().read, few.messages.values().next().unwrap().read, "the edit is in the file");
    // Readable again, the next save gives the space back.
    assert!(save(&path, &toggled(&few)).unwrap().compacted);
    assert_ne!(packs(&path), [old_pack]);
    clear_cache();
    assert_eq!(lines_of(&path), lines(&only_first(&all, 5)));
}

/// Review of #1376, P3: a pack an opening wrote into unflushed stays the one a save flushes when
/// another program's file is read over what is known, until it is flushed.
#[cfg(unix)]
#[test]
fn a_pack_an_opening_left_unflushed_is_flushed_before_another_programs_file_takes_over() {
    use std::os::unix::fs::PermissionsExt as _;
    let tmp = Temp::new("pack-refresh-unsynced");
    let dir = blob_dir(&tmp.file());
    fs::create_dir_all(&dir).unwrap();
    let stash = dir.join(pack::name(1).unwrap());
    fs::write(&stash, b"blocks").unwrap();
    // Can't be opened to be flushed: it stays the one to flush, and stays current.
    pack::install(&dir, Some(1), HashMap::new(), true);
    fs::set_permissions(&stash, fs::Permissions::from_mode(0o000)).unwrap();
    let blocked = fs::OpenOptions::new().append(true).open(&stash).is_err();
    if blocked {
        pack::refresh(&dir, Some(2), HashMap::new());
        assert!(pack::unsynced(&dir));
        assert_eq!(pack::current(&dir), Some(1));
    }
    fs::set_permissions(&stash, fs::Permissions::from_mode(0o600)).unwrap();
    // Flushed, the other file's pack takes over.
    pack::refresh(&dir, Some(2), HashMap::new());
    assert!(!pack::unsynced(&dir));
    assert_eq!(pack::current(&dir), Some(2));
}

/// Review of #1376, P2: blocks an Undo step holds are copied into the fresh pack, and count as
/// live there: the saves after it append, rather than each writing everything again.
#[test]
fn blocks_an_undo_step_holds_count_as_live_in_the_fresh_pack() {
    let all = with_exchange_headers(60);
    let tmp = Temp::new("pack-undo-live");
    let path = tmp.file();
    save(&path, &all).unwrap();
    // Of the mail taken out, twenty messages are still held (an Undo step), more than the slack
    // and more than the five left; the rest isn't, and makes the pack mostly dead.
    let mine = opened_lazily(&path);
    let ids: Vec<Id> = mine.messages.keys().copied().collect();
    let undo: Vec<(Id, Arc<Message>)> = ids[5..25].iter().map(|id| (*id, mine.messages.get(id).unwrap().clone())).collect();
    let held: u64 = ids[5..25].iter().map(|id| all.message(*id).unwrap().header_block.data.len() as u64).sum();
    assert!(held > pack::PACK_SLACK);
    let mut few = only_first(&mine, 5);
    drop(mine);
    assert!(save(&path, &few).unwrap().compacted);
    let (pack, _) = the_pack(&path);
    for _ in 0..4 {
        few = toggled(&few);
        let saved = save(&path, &few).unwrap();
        assert!(!saved.compacted, "an edit is appended");
    }
    assert_eq!(the_pack(&path).0, pack);
    // Going back finds every line of what was held.
    for (id, m) in undo {
        few.messages.insert(id, m);
    }
    save(&path, &few).unwrap();
    clear_cache();
    let back = lines_of(&path);
    let want = lines(&all);
    for id in &ids[..25] {
        assert_eq!(back.get(id), want.get(id), "message {id}");
    }
}

/// Review of #1376, P2: the lines of many messages let go of at once are written over a pack at a
/// time (one handle, one flush), every one of them.
#[test]
fn the_header_lines_of_many_messages_let_go_of_leave_the_pack_together() {
    let mut mb = with_exchange_headers(40);
    let tmp = Temp::new("pack-forget-many");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    for id in &ids[..30] {
        forget_when_saved(&path, 2, content_names(*id, mb.messages.get(id).unwrap()));
    }
    let gone: Vec<Arc<Vec<u8>>> = ids[..30].iter().map(|id| mb.message(*id).unwrap().header_block.data.clone()).collect();
    for id in &ids[..30] {
        mb.messages.remove(id);
    }
    save(&path, &mb).unwrap();
    assert!(forget_due(&path, 2).removed >= 30);
    let (_, bytes) = the_pack(&path);
    for block in &gone {
        assert!(!bytes.windows(block.len()).any(|w| w == block.as_slice()));
        assert!(pack::find(&blob_dir(&path), &Digest::of(block)).is_none());
    }
    clear_cache();
    assert_eq!(lines_of(&path), lines(&mb));
}

/// Review of #1376, P3: a pack a file kept aside names (here the file from before a newer format)
/// is never swept, nor removed by the whole rewrite that gives it up.
#[test]
fn a_pack_a_file_kept_aside_names_stays() {
    let all = with_exchange_headers(60);
    let tmp = Temp::new("pack-kept-aside");
    let path = tmp.file();
    save(&path, &all).unwrap();
    let (old_pack, old_bytes) = the_pack(&path);
    let kept = path.with_file_name(format!("{}.{}0.0.1", path.file_name().unwrap().to_string_lossy(), crate::store_before::BEFORE));
    fs::copy(&path, &kept).unwrap();
    // A fresh pack takes the place of the old one, which the kept file names.
    assert!(save(&path, &only_first(&all, 5)).unwrap().compacted);
    assert_eq!(fs::read(&old_pack).unwrap(), old_bytes);
    a_day_old_packs(&path);
    sweep(&path, Duration::ZERO).unwrap();
    assert_eq!(fs::read(&old_pack).unwrap(), old_bytes);
    // Gone once nothing names it.
    fs::remove_file(&kept).unwrap();
    sweep(&path, Duration::ZERO).unwrap();
    assert!(!old_pack.exists());
}

#[test]
fn a_header_blocks_place_after_its_name_names_its_pack_wherever_it_falls() {
    let hash = Digest::of(b"x");
    for (after, pack) in [(",12,3,456]", Some(3)), (",12,0,4]", None), (",12,3]", None), (",12,4294967296,0]", None), (",1,2,3", None), ("]", None)] {
        let mut bytes = noise(3, (1 << 20) - 70);
        bytes.extend_from_slice(format!("\"{}\"{after}", hash.hex()).as_bytes());
        let (mut names, mut packs) = (HashSet::new(), HashSet::new());
        names_in(&bytes[..], &mut names, &mut packs).unwrap();
        assert!(names.contains(&hash), "{after}");
        assert_eq!(packs.iter().copied().next(), pack, "{after}");
    }
}

/// Review of #1376, P2: another program appending to the same pack between two blocks of a save
/// moves where the next one goes; the place named is where it went, asked of the file.
#[test]
fn a_block_is_named_where_it_went_though_another_program_appended_meanwhile() {
    let mb = with_exchange_headers(2);
    let tmp = Temp::new("pack-two-appenders");
    let path = tmp.file();
    save(&path, &mb).unwrap();
    let dir = blob_dir(&path);
    let (file, _) = the_pack(&path);
    let mut writer = pack::Writer::appending(&dir);
    let one = Ref::of(b"one block");
    let at = writer.place(&one, Some(b"one block")).unwrap().unwrap();
    fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(b"another program's bytes").unwrap();
    let two = Ref::of(b"two block");
    let then = writer.place(&two, Some(b"two block")).unwrap().unwrap();
    assert_eq!(pack::read_at(&dir, at, &one).as_deref(), Some(&b"one block"[..]));
    assert_eq!(pack::read_at(&dir, then, &two).as_deref(), Some(&b"two block"[..]));
}

/// What 20,000 messages with Exchange-like headers and short bodies take beside the mailbox file:
/// how many files, and how much of the disk.
///
/// ```text
/// cargo test --release -p mailcraft-engine header_pack_bench -- --ignored --nocapture
/// ```
#[test]
#[ignore = "a benchmark: writes a mailbox of 20,000 messages"]
fn header_pack_bench() {
    let mut mb = super::headers::with_exchange_headers(20_000);
    for m in mb.messages.values_mut() {
        let m = Arc::make_mut(m);
        m.body.text = "A short note.".into();
        m.body.html = None;
        m.attachments.clear();
    }
    let tmp = Temp::new("header-pack-bench");
    let path = tmp.file();
    let started = Instant::now();
    save(&path, &mb).unwrap();
    let took = started.elapsed();
    let all = files(&blob_dir(&path));
    #[cfg(unix)]
    let disk: u64 = all.iter().map(|(f, _)| std::os::unix::fs::MetadataExt::blocks(&fs::metadata(f).unwrap()) * 512).sum();
    #[cfg(not(unix))]
    let disk: u64 = all.iter().map(|(_, len)| *len).sum();
    let apparent: u64 = all.iter().map(|(_, len)| *len).sum();
    println!(
        "20,000 messages: mailbox file {} KB; content folder {} files, {} KB apparent, {} KB on disk; saved in {took:?}",
        fs::metadata(&path).unwrap().len() >> 10,
        all.len(),
        apparent >> 10,
        disk >> 10
    );
}
