// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mailbox file in format 3: what a save appends, how it reads back after a crash or
//! damage, and how the earlier forms become it.

use super::*;

/// [`mailbox`] with contacts, events and tasks too.
fn full_mailbox(messages: usize) -> Mailbox {
    let mut mb = mailbox(messages);
    let calendar = mb.default_calendar().unwrap();
    for i in 0..6 {
        mb.add_contact(mailcraft_model::Contact { display_name: format!("Person {i}"), first: format!("P{i}"), ..Default::default() });
        mb.add_event(mailcraft_model::Event { calendar, title: format!("Meeting {i}"), ..Default::default() }).unwrap();
        mb.add_task(mailcraft_model::Task { title: format!("Task {i}"), ..Default::default() });
    }
    mb
}

/// What follows the last commit of the file at `path`.
fn tail_of(path: &Path) -> Tail {
    let bytes = fs::read(path).unwrap();
    index::replay(&bytes[..], bytes.len() as u64).unwrap().tail
}

/// Every file beside the mailbox file whose name begins `mailbox.mailcraft.<what>`.
fn kept(path: &Path, what: &str) -> Vec<PathBuf> {
    let prefix = format!("mailbox.mailcraft.{what}");
    files(path.parent().unwrap()).into_iter().map(|(f, _)| f).filter(|f| f.file_name().unwrap().to_str().unwrap().starts_with(&prefix)).collect()
}

#[test]
fn a_mailbox_of_either_earlier_form_becomes_this_one_without_loss() {
    for form in [1, 2] {
        let tmp = Temp::new(&format!("migrate-{form}"));
        let path = tmp.file();
        let mb = full_mailbox(30);
        match form {
            1 => fs::write(&path, crate::io::write_mailbox(&mb).unwrap()).unwrap(),
            _ => save_v2(&path, &mb),
        }
        let before = fs::read(&path).unwrap();
        let mut opened = open(&path).unwrap();
        assert!(opened.unavailable == 0 && opened.unfinished == 0, "form {form}");
        assert_eq!(opened.split, form == 2);
        // Unchanged, it stays as it is.
        assert!(save(&path, &opened.mailbox).unwrap().unchanged);
        assert_eq!(fs::read(&path).unwrap(), before);
        load(&path, &mut opened);
        assert!(opened.mailbox == mb, "form {form}");

        // Stopped while the new file was being written: the old one is the mailbox, and what was
        // written of the new one is a temporary file nobody reads.
        let half = tmp.0.join(format!("mailbox.mailcraft.{}-2a-9.tmp", std::process::id()));
        fs::write(&half, index::header()).unwrap();
        assert!(whole(&path) == mb, "form {form}");

        // The first change saved writes this form whole, keeps the old file beside it, and the
        // mailbox reads back exactly.
        let opened = open(&path).unwrap();
        let mut changed = opened.mailbox.clone();
        let first = *changed.messages.keys().next().unwrap();
        changed.message_mut(first).unwrap().flag = mailcraft_model::FlagStatus::Flagged;
        let saved = save(&path, &changed).unwrap();
        assert!(saved.compacted && !saved.unchanged, "form {form}");
        assert_eq!(fs::read(tmp.0.join(format!("mailbox.mailcraft.format{form}"))).unwrap(), before);
        assert!(fs::read(&path).unwrap().starts_with(crate::io::INDEX_MAGIC));
        let mut back = open(&path).unwrap();
        assert!(back.unavailable == 0 && back.split, "form {form}");
        load(&path, &mut back);
        let mut expect = mb.clone();
        expect.message_mut(first).unwrap().flag = mailcraft_model::FlagStatus::Flagged;
        assert!(back.mailbox == expect, "form {form}");
        if form == 1 {
            assert!(crate::io::read_mailbox(&before).unwrap() == mb);
        }
        // From then on a save appends, and nothing more is kept aside.
        let mut more = back.mailbox.clone();
        more.message_mut(first).unwrap().read = true;
        let saved = save(&path, &more).unwrap();
        assert!(!saved.compacted && saved.bytes > 0 && saved.bytes < 2_000, "{saved:?}");
        assert_eq!(kept(&path, "format").len(), 1);
        assert!(whole(&path) == more);
    }
}

#[test]
fn a_save_appends_what_changed_and_nothing_else() {
    let tmp = Temp::new("append");
    let path = tmp.file();
    let mut mb = full_mailbox(300);
    let first = save(&path, &mb).unwrap();
    assert!(first.compacted);
    let ids: Vec<Id> = mb.messages.keys().copied().collect();

    // One message read: one record and a commit, a few hundred bytes.
    let size = fs::metadata(&path).unwrap().len();
    mb.message_mut(ids[7]).unwrap().read = true;
    let one = save(&path, &mb).unwrap();
    assert!(!one.compacted && one.bytes < 1_000 && one.blobs == 0, "{one:?}");
    assert_eq!(fs::metadata(&path).unwrap().len(), size + one.bytes);
    assert!(peek(&path) == mb);

    // Of every kind of item a change, a new one and one gone; and the rest of the mailbox.
    let contact = *mb.contacts.keys().next().unwrap();
    mb.contact_mut(contact).unwrap().title = "Engineer".into();
    let event = *mb.events.keys().next().unwrap();
    mb.events.remove(&event);
    let task = *mb.tasks.keys().last().unwrap();
    mb.task_mut(task).unwrap().title = "Done soon".into();
    mb.add_contact(mailcraft_model::Contact { display_name: "Newcomer".into(), ..Default::default() });
    mb.messages.remove(&ids[3]);
    let inbox = mb.inbox().unwrap();
    mb.add_message(Message {
        folder: inbox,
        subject: "New".into(),
        attachments: vec![Attachment::new("n.bin", "application/octet-stream", noise(404, 30_000))],
        ..Default::default()
    })
    .unwrap();
    mb.signatures.push(mailcraft_model::Signature::plain_text("Short", ""));
    let many = save(&path, &mb).unwrap();
    assert!(!many.compacted && many.blobs == 1 && many.bytes < 20_000, "{many:?}");
    assert!(peek(&path) == mb);
    assert!(whole(&path) == mb);

    // Saved again, the same state writes nothing; a change written and then undone is two.
    assert!(save(&path, &mb).unwrap().unchanged);
    let was = mb.clone();
    mb.message_mut(ids[9]).unwrap().subject = "Changed".into();
    assert!(!save(&path, &mb).unwrap().unchanged);
    let len = fs::metadata(&path).unwrap().len();
    assert!(save(&path, &mb).unwrap().unchanged);
    assert_eq!(fs::metadata(&path).unwrap().len(), len);
    assert!(!save(&path, &was).unwrap().unchanged);
    assert!(peek(&path) == was);
}

#[test]
fn the_file_is_written_whole_again_when_what_was_appended_outgrows_it() {
    let tmp = Temp::new("compact");
    let path = tmp.file();
    let mut mb = mailbox(200);
    save(&path, &mb).unwrap();
    let whole = fs::metadata(&path).unwrap().len();
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    let (mut appended, mut compacted) = (0, 0);
    for round in 0..400 {
        for id in ids.iter().skip(round % 7).step_by(7) {
            let m = mb.message_mut(*id).unwrap();
            m.read = !m.read;
        }
        let saved = save(&path, &mb).unwrap();
        if saved.compacted {
            compacted += 1;
        } else {
            appended += 1;
        }
        // Never much more than twice the mailbox and some room.
        assert!(fs::metadata(&path).unwrap().len() <= 2 * whole + COMPACT_SLACK + (64 << 10));
    }
    assert!(appended > 300 && compacted > 2, "{appended} appended, {compacted} compacted");
    assert!(super::whole(&path) == mb);
    // A compacted file is one commit: every record in it says what the mailbox is.
    save_single(&path, &mb).unwrap();
    open(&path).unwrap();
    mb.message_mut(ids[0]).unwrap().read ^= true;
    assert!(save(&path, &mb).unwrap().compacted);
    let bytes = fs::read(&path).unwrap();
    let replayed = index::replay(&bytes[..], bytes.len() as u64).unwrap();
    assert_eq!(replayed.items.len(), mb.messages.len());
    assert_eq!(bytes.len() as u64, replayed.good_len);
}

#[test]
fn a_save_cut_short_leaves_the_mailbox_as_the_last_finished_save_left_it() {
    let tmp = Temp::new("torn");
    let path = tmp.file();
    let mut mb = full_mailbox(40);
    save(&path, &mb).unwrap();
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    mb.message_mut(ids[0]).unwrap().read = true;
    save(&path, &mb).unwrap();
    let finished = mb.clone();
    let good = fs::read(&path).unwrap();
    // The next save, cut short at every byte.
    mb.message_mut(ids[1]).unwrap().subject = "Renamed".into();
    mb.events.clear();
    let saved = save(&path, &mb).unwrap();
    assert!(!saved.compacted);
    let full = fs::read(&path).unwrap();
    assert_eq!(full.len() as u64, good.len() as u64 + saved.bytes);
    for cut in good.len()..full.len() {
        fs::write(&path, &full[..cut]).unwrap();
        let mut opened = open(&path).unwrap();
        load(&path, &mut opened);
        assert!(opened.mailbox == finished, "cut at {cut}");
        assert_eq!(opened.unfinished, (cut - good.len()) as u64);
    }
    // Saved again from there, the file is kept as it was and written whole.
    fs::write(&path, &full[..full.len() - 3]).unwrap();
    let opened = open(&path).unwrap();
    assert!(opened.unfinished > 0);
    let saved = save(&path, &mb).unwrap();
    assert!(saved.compacted);
    let aside = kept(&path, "damaged-");
    assert_eq!(aside.len(), 1);
    assert_eq!(fs::read(&aside[0]).unwrap(), &full[..full.len() - 3]);
    assert!(whole(&path) == mb && tail_of(&path) == Tail::Clean);
    // Nothing it names is swept while it is kept: it may name content the mailbox doesn't any more.
    let mut fewer = mb.clone();
    fewer.messages.retain(|_, m| m.attachments.is_empty());
    save(&path, &fewer).unwrap();
    for (file, _) in files(&blob_dir(&path)) {
        age(&file, Duration::from_secs(86_400));
    }
    assert_eq!(sweep(&path, GRACE).unwrap().removed, 0);
    fs::remove_file(&aside[0]).unwrap();
    assert!(sweep(&path, GRACE).unwrap().removed > 0);
    assert!(whole(&path) == fewer);
}

#[test]
fn zeros_after_the_last_save_are_no_save_and_damage_is_kept() {
    let tmp = Temp::new("zeros");
    let path = tmp.file();
    let mut mb = mailbox(20);
    save(&path, &mb).unwrap();
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    mb.message_mut(ids[0]).unwrap().read = true;
    save(&path, &mb).unwrap();
    let good = fs::read(&path).unwrap();

    // A file system that made room for a write it never did.
    let mut zeros = good.clone();
    zeros.extend(vec![0u8; 5000]);
    fs::write(&path, &zeros).unwrap();
    let mut opened = open(&path).unwrap();
    load(&path, &mut opened);
    assert!(opened.mailbox == mb && opened.unfinished == 0);
    assert_eq!(tail_of(&path), Tail::Zeros);
    // The next save can't append after them: it writes the file whole, and keeps nothing.
    mb.message_mut(ids[1]).unwrap().read = true;
    assert!(save(&path, &mb).unwrap().compacted);
    assert!(kept(&path, "damaged-").is_empty() && whole(&path) == mb);

    // A byte changed near the end: the mailbox opens as far as the damage, and the file is kept
    // aside before it is written again, never thrown away.
    mb.message_mut(ids[2]).unwrap().read = true;
    save(&path, &mb).unwrap();
    mb.message_mut(ids[3]).unwrap().read = true;
    save(&path, &mb).unwrap();
    let mut bytes = fs::read(&path).unwrap();
    let at = bytes.len() - 100;
    bytes[at] ^= 0x40;
    fs::write(&path, &bytes).unwrap();
    let opened = open(&path).unwrap();
    assert!(opened.unfinished > 0 && opened.mailbox != mb);
    assert!(matches!(tail_of(&path), Tail::Bytes(_)));
    // The sweep doesn't trust it either.
    assert_eq!(sweep(&path, Duration::ZERO).unwrap(), Swept::default());
    let mut edited = opened.mailbox.clone();
    edited.message_mut(ids[5]).unwrap().read = true;
    assert!(save(&path, &edited).unwrap().compacted);
    assert_eq!(fs::read(&kept(&path, "damaged-")[0]).unwrap(), bytes);
    assert!(whole(&path) == loaded(&path, &edited));
}

#[test]
fn a_retired_favorites_choice_in_this_form_is_written_away_at_the_next_save() {
    let tmp = Temp::new("retired");
    let path = tmp.file();
    let mb = mailbox(5);
    save(&path, &mb).unwrap();
    // A save by a program that still wrote how an account's Favorites were read.
    let mut rest: Value = serde_json::from_slice(&rest_json(&mb).unwrap()).unwrap();
    rest["accounts"][0]["favoritesSource"] = json!("ews");
    let mut bytes = fs::read(&path).unwrap();
    index::rest(&mut bytes, &serde_json::to_vec(&rest).unwrap()).unwrap();
    index::commit(&mut bytes, 1).unwrap();
    fs::write(&path, &bytes).unwrap();
    let mut opened = open(&path).unwrap();
    assert!(opened.mailbox.accounts[0].retired_favorites_choice && opened.notice().is_none());
    assert_eq!(opened.mailbox.take_retired_favorites_choices(), 1);
    // Nothing else changed, and still the next save writes the rest of the mailbox without it.
    let saved = save(&path, &opened.mailbox).unwrap();
    assert!(!saved.unchanged && !saved.compacted, "{saved:?}");
    let again = open(&path).unwrap();
    assert!(!again.mailbox.accounts[0].retired_favorites_choice && whole(&path) == mb);
    assert!(save(&path, &again.mailbox).unwrap().unchanged);
}

/// Where each record of a format-3 file begins, its bytes and its kind.
fn records(bytes: &[u8]) -> Vec<(usize, usize, u8)> {
    let mut out = Vec::new();
    let mut at = index::HEADER_LEN as usize;
    while at + 5 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let size = index::FRAME as usize + len;
        out.push((at, size, bytes[at + 4]));
        at += size;
    }
    out
}

/// The bytes of `bytes` with the one at `at` changed.
fn flipped(bytes: &[u8], at: usize) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out[at] ^= 0x40;
    out
}

#[test]
fn a_damaged_save_stops_the_reading_there_so_no_later_save_says_mail_is_here_that_isnt() {
    let tmp = Temp::new("damaged-middle");
    let path = tmp.file();
    let mut mb = mailbox(20);
    let inbox = mb.inbox().unwrap();
    mb.folder_mut(inbox).unwrap().uid_seen = 100;
    save(&path, &mb).unwrap();
    let before = mb.clone();
    let at_n = fs::metadata(&path).unwrap().len() as usize;
    // Save N brings new mail: a message, and the Inbox read up to 110.
    let fresh = mb.add_message(Message { folder: inbox, subject: "New mail".into(), ..Default::default() }).unwrap();
    mb.folder_mut(inbox).unwrap().uid_seen = 110;
    assert!(!save(&path, &mb).unwrap().compacted);
    let at_n1 = fs::metadata(&path).unwrap().len() as usize;
    // Save N+1 only makes a folder a favourite: its rest record says 110 too.
    let sent = mb.folders.iter().find(|f| f.id != inbox).unwrap().id;
    mb.folder_mut(sent).unwrap().favorite = true;
    assert!(!save(&path, &mb).unwrap().compacted);
    let good = fs::read(&path).unwrap();

    // One byte of the new message's record in save N changes on the disk.
    let (put_at, _, kind) = records(&good).into_iter().find(|(at, _, _)| *at == at_n).unwrap();
    assert_eq!(kind, 2);
    let bytes = flipped(&good, put_at + 30);
    fs::write(&path, &bytes).unwrap();
    let opened = open(&path).unwrap();
    // The reading stops at save N: the mailbox is as before it, the Inbox read up to 100, so the
    // new mail is fetched again; save N+1 is left out with it, and both are said.
    assert!(loaded(&path, &opened.mailbox) == before, "the mailbox is the one before the damaged save");
    assert!(opened.mailbox.message(fresh).is_none());
    assert_eq!(opened.mailbox.folder(inbox).unwrap().uid_seen, 100);
    assert_eq!((opened.lost_saves, opened.lost_records, opened.unfinished), (2, 0, (bytes.len() - at_n) as u64));
    let notice = opened.notice().unwrap();
    assert!(notice.starts_with("2 saves of the mailbox file were damaged, or came after a damaged one, and were left out"), "{notice}");
    let mut s = crate::Session::empty();
    let said = s.run("file.open", &json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!((said["lostSaves"].as_u64(), said["unfinished"].as_u64()), (Some(2), Some((bytes.len() - at_n) as u64)));
    assert_eq!(said["notice"].as_str(), Some(notice.as_str()));
    // The next save writes the file whole and keeps the damaged one aside, as it was.
    let mut edited = open(&path).unwrap().mailbox;
    let first = *edited.messages.keys().next().unwrap();
    edited.message_mut(first).unwrap().read = true;
    assert!(save(&path, &edited).unwrap().compacted);
    assert_eq!(fs::read(&kept(&path, "damaged-")[0]).unwrap(), bytes);
    let back = open(&path).unwrap();
    assert!(back.notice().is_none() && back.mailbox == edited);
    assert!(at_n1 > at_n);
}

#[test]
fn a_damaged_commit_counts_every_save_it_costs() {
    let tmp = Temp::new("damaged-commit");
    let path = tmp.file();
    let mut mb = mailbox(10);
    save(&path, &mb).unwrap();
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    let mut ends = vec![fs::metadata(&path).unwrap().len() as usize];
    for id in &ids[..3] {
        mb.message_mut(*id).unwrap().read = true;
        assert!(!save(&path, &mb).unwrap().compacted);
        ends.push(fs::metadata(&path).unwrap().len() as usize);
    }
    let good = fs::read(&path).unwrap();
    let lost = |bytes: &[u8]| {
        let replayed = index::replay(bytes, bytes.len() as u64).unwrap();
        (replayed.lost_saves, replayed.good_len as usize)
    };
    let commit_of = |end: usize| records(&good).into_iter().find(|(at, size, _)| at + size == end).unwrap();
    // The first of three saves damaged in a record of its own, or in its commit (whose count
    // then seems to close the save after it): three saves lost either way, read up to the first.
    assert_eq!(lost(&flipped(&good, ends[0] + 30)), (3, ends[0]));
    let (commit, size, kind) = commit_of(ends[1]);
    assert_eq!((kind, size), (4, 25));
    assert_eq!(lost(&flipped(&good, commit + 8)), (3, ends[0]));
    assert_eq!(lost(&flipped(&good, commit + 20)), (3, ends[0]));
    // The second's commit: two.
    let (commit, _, _) = commit_of(ends[2]);
    assert_eq!(lost(&flipped(&good, commit + 20)), (2, ends[1]));
    // The last save's commit: one.
    let (commit, _, _) = commit_of(ends[3]);
    assert_eq!(lost(&flipped(&good, commit + 20)), (1, ends[2]));
    // A save cut short after a damaged one isn't counted: it was never finished.
    let mut torn = flipped(&good, ends[1] + 30);
    torn.truncate(ends[3] - 3);
    assert_eq!(lost(&torn), (1, ends[1]));
}

#[test]
fn a_damaged_item_of_the_first_save_costs_that_item_and_the_mail_is_fetched_again() {
    let tmp = Temp::new("damaged-base");
    let path = tmp.file();
    let mut mb = full_mailbox(12);
    let inbox = mb.inbox().unwrap();
    {
        let f = mb.folder_mut(inbox).unwrap();
        f.uid_seen = 300;
        f.server = Some(mailcraft_model::ServerFolder {
            id: "in".into(),
            name: "Inbox".into(),
            delta: "https://graph.example/delta?token=1".into(),
            caught_up: true,
            ..Default::default()
        });
    }
    assert!(save(&path, &mb).unwrap().compacted);
    let good = fs::read(&path).unwrap();
    let recs = records(&good);
    // One message's record in the first (and only) save.
    let (put_at, _, _) = recs.iter().copied().filter(|r| r.2 == 2).nth(3).unwrap();
    let id = u64::from_le_bytes(good[put_at + 6..put_at + 14].try_into().unwrap());
    let bytes = flipped(&good, put_at + 40);
    fs::write(&path, &bytes).unwrap();
    let opened = open(&path).unwrap();
    // Everything else is read; the folders' marks are forgotten, so the message comes again.
    let mut expect = mb.clone();
    expect.messages.remove(&id);
    for f in &mut expect.folders {
        f.uid_seen = 0;
        // Fetched again, it is a first download, which the junk lists leave alone (#504).
        f.whole = Some(false);
        if let Some(server) = &mut f.server {
            server.delta.clear();
            server.caught_up = false;
        }
    }
    let mut back = opened.mailbox.clone();
    load_all(&path, &mut back).unwrap();
    assert!(back == expect, "the rest of the mailbox is read");
    assert_eq!((opened.lost_records, opened.lost_saves), (1, 0));
    assert!(opened.unfinished > 0);
    let notice = opened.notice().unwrap();
    assert!(notice.starts_with("1 item of the mailbox file (a message, contact, event or task) was damaged and left out"), "{notice}");
    assert!(notice.contains("fetched again") && !notice.contains("no save"), "{notice}");
    let mut s = crate::Session::empty();
    assert_eq!(s.run("file.open", &json!({"path": path.to_string_lossy()})).unwrap()["lostRecords"].as_u64(), Some(1));
    // The next save writes it whole and keeps the damaged file aside.
    let mut edited = open(&path).unwrap().mailbox;
    edited.message_mut(*edited.messages.keys().next().unwrap()).unwrap().read = true;
    assert!(save(&path, &edited).unwrap().compacted);
    assert_eq!(fs::read(&kept(&path, "damaged-")[0]).unwrap(), bytes);
    assert!(whole(&path) == loaded(&path, &edited));

    // Its commit damaged, the first save is still read whole, with later saves after it: no
    // item is lost (none is said to be, and no folder's mail is fetched again), and the file is
    // kept aside before it is written again.
    fs::write(&path, &good).unwrap();
    let mut later = open(&path).unwrap().mailbox;
    later.message_mut(id).unwrap().subject = "Later".into();
    assert!(!save(&path, &later).unwrap().compacted);
    let two = fs::read(&path).unwrap();
    let (commit, _, kind) = records(&two).into_iter().find(|(at, size, _)| at + size == good.len()).unwrap();
    assert_eq!(kind, 4);
    for at in [commit + 6, commit + 20] {
        for damaged in [flipped(&good, at), flipped(&two, at)] {
            let replayed = index::replay(&damaged[..], damaged.len() as u64).unwrap();
            assert_eq!(replayed.items.len(), mb.messages.len() + 18);
            assert_eq!((replayed.lost_records, replayed.lost_saves), (0, 0));
            assert!(matches!(replayed.tail, Tail::Bytes(_)));
        }
    }
    fs::write(&path, flipped(&two, commit + 20)).unwrap();
    let opened = open(&path).unwrap();
    assert_eq!(opened.mailbox.message(id).unwrap().subject, "Later");
    assert_eq!(opened.mailbox.folder(inbox).unwrap().uid_seen, 300);
    let notice = opened.notice().unwrap();
    assert!(!notice.contains("item") && !notice.contains("fetched again"), "{notice}");
    // And a later save after it that loses the way: the first save is read whole, nothing of it
    // is said to be lost, and the later save's bytes are left out (never taken in half).
    let mut torn = flipped(&two, commit + 20);
    let later_at = good.len();
    torn[later_at + 4] = 0x7f;
    let replayed = index::replay(&torn[..], torn.len() as u64).unwrap();
    assert_eq!(replayed.items.len(), mb.messages.len() + 18);
    assert_eq!((replayed.lost_records, replayed.good_len as usize), (0, good.len()));
    assert_eq!(replayed.tail, Tail::Bytes((two.len() - good.len()) as u64));
    fs::write(&path, &torn).unwrap();
    assert_ne!(open(&path).unwrap().mailbox.message(id).unwrap().subject, "Later");

    // Its rest record damaged, there is no mailbox: that is said, not "no save was finished".
    let (rest_at, _, kind) = recs[0];
    assert_eq!(kind, 1);
    fs::write(&path, flipped(&good, rest_at + 40)).unwrap();
    let error = open(&path).unwrap_err();
    assert!(error.contains("damaged") && !error.contains("no save"), "{error}");
    // A record that loses the way, too; the file cut short in its first save is that.
    fs::write(&path, flipped(&good, put_at + 4)).unwrap();
    let error = open(&path).unwrap_err();
    assert!(error.contains("damaged where it begins"), "{error}");
    fs::write(&path, &good[..put_at + 7]).unwrap();
    let error = open(&path).unwrap_err();
    assert!(error.contains("no save in it was finished"), "{error}");
}

#[test]
fn a_file_of_a_newer_form_is_said_to_be_from_a_newer_mailcraft() {
    let tmp = Temp::new("newer");
    let path = tmp.file();
    save(&path, &mailbox(3)).unwrap();
    let good = fs::read(&path).unwrap();
    // This form with a higher number, and another form of ours altogether.
    let higher = [&good[..16], &[4, 0, 0, 0][..], &good[20..]].concat();
    for (bytes, says) in [(higher, "newer MailCraft (format 4)"), (b"MailCraft journal\n\x05\0\0\0".to_vec(), "newer MailCraft")] {
        fs::write(&path, &bytes).unwrap();
        let error = open(&path).unwrap_err();
        assert!(error.contains(says), "{error}");
        assert!(crate::io::read_mailbox(&bytes).is_err());
        assert!(sweep(&path, Duration::ZERO).is_err());
    }
    assert!(crate::io::read_mailbox(b"MailCraft journal\n").unwrap_err().contains("newer MailCraft"));
    // JSON that isn't ours still isn't.
    fs::write(&path, b"{\"hello\": 1, \"version\": \"x\"}").unwrap();
    assert!(open(&path).unwrap_err().contains("isn't a MailCraft mailbox"));
}

#[test]
fn files_kept_aside_keep_what_they_name_and_none_is_removed() {
    // A file of an earlier form kept aside keeps its content from the sweep, as a damaged one does.
    let tmp = Temp::new("kept-format");
    let path = tmp.file();
    let mb = mailbox(12);
    save_v2(&path, &mb);
    let mut changed = whole(&path);
    let gone = first_with(&changed, |m| m.attachments.iter().any(|a| a.data.len() > INLINE_MAX));
    let gone_file = blob_file(&path, &changed.message(gone).unwrap().attachments[0].data);
    changed.remove_message(gone).unwrap();
    assert!(save(&path, &changed).unwrap().compacted);
    assert_eq!(kept(&path, "format2").len(), 1);
    for (file, _) in files(&blob_dir(&path)) {
        age(&file, Duration::from_secs(86_400));
    }
    assert_eq!(sweep(&path, GRACE).unwrap().removed, 0);
    assert!(gone_file.exists());
    fs::remove_file(&kept(&path, "format2")[0]).unwrap();
    assert!(sweep(&path, GRACE).unwrap().removed > 0 && !gone_file.exists());

    // Files kept aside are never removed by a save, however many there are: each may be the
    // only copy of what it holds.
    let tmp = Temp::new("kept-all");
    let path = tmp.file();
    let mut mb = mailbox(4);
    save(&path, &mb).unwrap();
    let before = [
        "damaged-100",
        "damaged-300",
        "damaged-200-2",
        "damaged-200",
        "damaged-50",
        "damaged-x",
        "changed-10",
        "changed-20",
        "changed-30",
        "changed-40",
        "changed-50",
        "changed-60",
        "format1",
    ];
    for name in before {
        fs::write(tmp.0.join(format!("mailbox.mailcraft.{name}")), name).unwrap();
    }
    for round in 0..2 {
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(b"not a save");
        fs::write(&path, &bytes).unwrap();
        open(&path).unwrap();
        let first = *mb.messages.keys().next().unwrap();
        mb.message_mut(first).unwrap().read ^= true;
        assert!(save(&path, &mb).unwrap().compacted);
        // The one kept now is there, with the bytes it had.
        assert!(kept(&path, "damaged-").iter().any(|p| fs::read(p).unwrap() == bytes), "round {round}");
    }
    let mut left: Vec<String> =
        kept(&path, "").iter().map(|p| p.file_name().unwrap().to_str().unwrap().trim_start_matches("mailbox.mailcraft.").to_string()).collect();
    left.retain(|n| n != "blobs");
    for name in before {
        assert!(left.contains(&name.to_string()), "{name} is still there: {left:?}");
        assert_eq!(fs::read(tmp.0.join(format!("mailbox.mailcraft.{name}"))).unwrap(), name.as_bytes());
    }
    assert_eq!(left.len(), before.len() + 2, "{left:?}");
}

#[test]
fn a_file_another_writer_changed_is_kept_aside_before_it_is_written_over() {
    let tmp = Temp::new("other-writer");
    let path = tmp.file();
    let mut mb = mailbox(6);
    save(&path, &mb).unwrap();
    // Another program writes the file meanwhile.
    let other = tmp.0.join("other.mailcraft");
    save(&other, &mailbox(2)).unwrap();
    let theirs = fs::read(&other).unwrap();
    fs::write(&path, &theirs).unwrap();
    let first = *mb.messages.keys().next().unwrap();
    mb.message_mut(first).unwrap().read = true;
    assert!(save(&path, &mb).unwrap().compacted);
    let aside = kept(&path, "changed-");
    assert_eq!(aside.len(), 1);
    assert_eq!(fs::read(&aside[0]).unwrap(), theirs);
    assert!(whole(&path) == mb);
    // From then on the file is this program's again: saves append, and nothing more is kept.
    mb.message_mut(first).unwrap().read = false;
    assert!(!save(&path, &mb).unwrap().compacted);
    assert_eq!(kept(&path, "changed-").len(), 1);
}

#[test]
fn a_file_whose_time_changed_but_not_its_bytes_is_still_ours_and_saves_append() {
    let tmp = Temp::new("touched");
    let path = tmp.file();
    let mut mb = mailbox(8);
    assert!(save(&path, &mb).unwrap().compacted);
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    // Written whole, appended to, and opened afresh: each time the file's time then changes
    // without its bytes (as on a share whose times come late), and the next save still appends,
    // keeping nothing aside.
    for (i, id) in ids.iter().take(4).enumerate() {
        if i == 2 {
            *lock(&shared(&path).state) = State::default();
            mb = open(&path).unwrap().mailbox;
        }
        age(&path, Duration::from_secs(3_600 + i as u64));
        mb.message_mut(*id).unwrap().read ^= true;
        assert!(!save(&path, &mb).unwrap().compacted, "save {i}");
    }
    assert!(kept(&path, "changed-").is_empty());
    *lock(&shared(&path).state) = State::default();
    assert!(open(&path).unwrap().mailbox == mb);
    // Bytes that did change, the same length and time aside, are another writer's: kept aside.
    let mut theirs = fs::read(&path).unwrap();
    let last = theirs.len() - 30;
    theirs[last] ^= 1;
    open(&path).unwrap();
    fs::write(&path, &theirs).unwrap();
    age(&path, Duration::from_secs(60));
    mb.message_mut(ids[5]).unwrap().read ^= true;
    assert!(save(&path, &mb).unwrap().compacted);
    let aside = kept(&path, "changed-");
    assert_eq!(aside.len(), 1);
    assert_eq!(fs::read(&aside[0]).unwrap(), theirs);
}

/// Open the file at `path` with the sum of its bytes held back until the returned sender lets
/// go: the opening is over while the sum is still being worked out (#519).
fn open_with_sum_held(path: &Path) -> (Mailbox, std::sync::mpsc::Sender<()>) {
    let (go, hold) = std::sync::mpsc::channel();
    *lock(&HASH_HOLD) = Some((path.to_path_buf(), hold));
    *lock(&shared(path).state) = State::default();
    let mb = open(path).unwrap().mailbox;
    assert!(matches!(lock(&shared(path).state).stamp.as_ref().map(|s| &s.sum), Some(Sum::Pending(..))), "the sum is still being worked out");
    (mb, go)
}

/// Let the sum go after a moment, on a thread of its own; the flag says it has.
fn let_go_later(go: std::sync::mpsc::Sender<()>) -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let gone = Arc::new(AtomicBool::new(false));
    let flag = gone.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        flag.store(true, Ordering::SeqCst);
        drop(go);
    });
    (gone, thread)
}

#[test]
fn a_save_waits_for_the_sum_of_the_file_as_opened_and_still_knows_its_own_bytes() {
    let tmp = Temp::new("sum-later-ours");
    let path = tmp.file();
    let mut mb = mailbox(8);
    save(&path, &mb).unwrap();
    let (opened, go) = open_with_sum_held(&path);
    assert!(loaded(&path, &opened) == mb);
    // Its time changes, not its bytes (a share whose times come late): the save waits for the
    // sum, finds the bytes its own and appends, keeping nothing aside.
    age(&path, Duration::from_secs(3_600));
    let (gone, thread) = let_go_later(go);
    let first = *mb.messages.keys().next().unwrap();
    mb.message_mut(first).unwrap().read ^= true;
    assert!(!save(&path, &mb).unwrap().compacted);
    assert!(gone.load(Ordering::SeqCst), "the save waited for the sum");
    thread.join().unwrap();
    assert!(kept(&path, "changed-").is_empty());
    // And the sum goes on from there: the next time the file's time alone changes, it is still ours.
    age(&path, Duration::from_secs(7_200));
    mb.message_mut(first).unwrap().read ^= true;
    assert!(!save(&path, &mb).unwrap().compacted);
    assert!(kept(&path, "changed-").is_empty());
    *lock(&shared(&path).state) = State::default();
    assert!(whole(&path) == mb);
}

#[test]
fn a_file_changed_on_disk_before_the_sum_is_worked_out_is_still_kept_aside() {
    let tmp = Temp::new("sum-later-theirs");
    let path = tmp.file();
    let mut mb = mailbox(8);
    save(&path, &mb).unwrap();
    let (_, go) = open_with_sum_held(&path);
    // Another writer changes a byte, the length the same: the time and the bytes say so.
    let mut theirs = fs::read(&path).unwrap();
    let last = theirs.len() - 30;
    theirs[last] ^= 1;
    fs::write(&path, &theirs).unwrap();
    age(&path, Duration::from_secs(60));
    let (gone, thread) = let_go_later(go);
    let first = *mb.messages.keys().next().unwrap();
    mb.message_mut(first).unwrap().read ^= true;
    assert!(save(&path, &mb).unwrap().compacted);
    assert!(gone.load(Ordering::SeqCst), "the save waited for the sum");
    thread.join().unwrap();
    let aside = kept(&path, "changed-");
    assert_eq!(aside.len(), 1);
    assert_eq!(fs::read(&aside[0]).unwrap(), theirs);
    assert!(whole(&path) == mb);
}

#[test]
fn a_file_cut_short_has_no_sum_of_bytes_its_reading_never_took() {
    let tmp = Temp::new("sum-cut-short");
    let path = tmp.file();
    let mut mb = mailbox(8);
    save(&path, &mb).unwrap();
    let first = *mb.messages.keys().next().unwrap();
    mb.message_mut(first).unwrap().subject = "Changed".into();
    save(&path, &mb).unwrap();
    // A crash cut the last save short.
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, &bytes[..bytes.len() - 10]).unwrap();
    *lock(&shared(&path).state) = State::default();
    let mut mb = open(&path).unwrap().mailbox;
    // Only its time changes: with no sum of the bytes as read (none was taken whole), it is
    // another writer's, kept aside as such, as it always was.
    age(&path, Duration::from_secs(3_600));
    mb.message_mut(first).unwrap().read ^= true;
    save(&path, &mb).unwrap();
    assert_eq!(kept(&path, "changed-").len(), 1);
    assert_eq!(kept(&path, "damaged-").len(), 0);
}

#[test]
fn a_record_larger_than_a_piece_is_summed_in_pieces_and_the_file_is_still_known() {
    let tmp = Temp::new("sum-large-record");
    let path = tmp.file();
    let mut mb = mailbox(4);
    let first = *mb.messages.keys().next().unwrap();
    // A record of 3 MB: read past the buffer, straight into its own.
    mb.message_mut(first).unwrap().headers.push(("X-Large".into(), "x".repeat(3 * HASH_PIECE)));
    save(&path, &mb).unwrap();
    *lock(&shared(&path).state) = State::default();
    SUMMED.set((0, 0));
    let mut mb = open(&path).unwrap().mailbox;
    let (read, largest) = SUMMED.get();
    assert!(read > HASH_PIECE && largest == HASH_PIECE, "read {read}, pieces up to {largest}");
    age(&path, Duration::from_secs(3_600));
    mb.message_mut(first).unwrap().read ^= true;
    assert!(!save(&path, &mb).unwrap().compacted);
    assert!(kept(&path, "changed-").is_empty());
}

#[test]
fn no_memory_for_the_sum_leaves_none_and_the_file_is_kept_aside_when_its_time_changes() {
    let tmp = Temp::new("sum-no-memory");
    let path = tmp.file();
    let mut mb = mailbox(6);
    save(&path, &mb).unwrap();
    *lock(&shared(&path).state) = State::default();
    NO_MEMORY_FOR_SUM.set(true);
    let opened = open(&path);
    NO_MEMORY_FOR_SUM.set(false);
    assert!(loaded(&path, &opened.unwrap().mailbox) == mb);
    assert!(matches!(lock(&shared(&path).state).stamp.as_ref().map(|s| &s.sum), Some(Sum::None)));
    // Without a sum, a file whose time changed is taken for another writer's, as for any file
    // whose bytes weren't known: kept aside, nothing lost.
    age(&path, Duration::from_secs(3_600));
    let first = *mb.messages.keys().next().unwrap();
    mb.message_mut(first).unwrap().read ^= true;
    assert!(save(&path, &mb).unwrap().compacted);
    assert_eq!(kept(&path, "changed-").len(), 1);
    assert!(whole(&path) == mb);
}

#[test]
fn what_was_worked_out_of_attachments_gone_from_memory_is_let_go() {
    let tmp = Temp::new("hashes");
    let path = tmp.file();
    let mut mb = mailbox(16);
    save(&path, &mb).unwrap();
    let mut changed = mb.clone();
    for m in changed.messages.values_mut() {
        let seed = m.id;
        Arc::make_mut(m).attachments.push(Attachment::new("more.bin", "application/octet-stream", noise(seed, 3_000)));
    }
    save(&path, &changed).unwrap();
    let held = lock(&shared(&path).state).hashes.len();
    assert!(held >= 16, "{held}");
    // Those attachments gone from every copy in memory, a save lets go of what was known of them.
    drop(changed);
    mb.message_mut(*mb.messages.keys().next().unwrap()).unwrap().read = true;
    save(&path, &mb).unwrap();
    let now = lock(&shared(&path).state).hashes.len();
    let alive = mb.messages.values().flat_map(|m| &m.attachments).count();
    assert!(now <= alive && now < held, "{now} of {held}, {alive} attachments in memory");
}

#[test]
fn a_file_with_more_records_than_can_be_held_is_refused() {
    let tmp = Temp::new("too-many");
    let path = tmp.file();
    save(&path, &full_mailbox(30)).unwrap();
    let bytes = fs::read(&path).unwrap();
    assert!(index::replay_capped(&bytes[..], bytes.len() as u64, 1_000).is_ok());
    let error = index::replay_capped(&bytes[..], bytes.len() as u64, 20).err().unwrap();
    assert!(error.contains("more records"), "{error}");
}

#[test]
fn a_hostile_file_of_this_form_is_refused_or_read_and_names_nothing_outside_its_folder() {
    let tmp = Temp::new("hostile3");
    let path = tmp.file();
    let mb = mailbox(4);
    save(&path, &mb).unwrap();
    let good = fs::read(&path).unwrap();
    let id = *mb.messages.keys().next().unwrap();
    let json = serde_json::to_vec(mb.message(id).unwrap()).unwrap();
    // The file with one more save of one record.
    let with = |record: &dyn Fn(&mut Vec<u8>)| {
        let mut bytes = good.clone();
        record(&mut bytes);
        index::commit(&mut bytes, 1).unwrap();
        bytes
    };
    let refused = [
        // A newer format, a header that isn't one, nothing after the header.
        [&good[..16], &[4, 0, 0, 0, 0, 0, 0, 0][..], &good[24..]].concat(),
        [&good[..20], &[1, 0, 0, 0][..], &good[24..]].concat(),
        index::header().to_vec(),
        // Records whose checksums are right and whose content isn't.
        with(&|w| {
            index::put(w, (Coll::Message, id), b"{not json", b"").unwrap();
        }),
        with(&|w| {
            index::put(w, (Coll::Message, id), &json, b"[1,2]").unwrap();
        }),
        with(&|w| {
            index::put(w, (Coll::Event, 9), b"7", b"").unwrap();
        }),
        with(&|w| {
            index::rest(w, b"\"a string\"").unwrap();
        }),
    ];
    for (n, bytes) in refused.iter().enumerate() {
        fs::write(&path, bytes).unwrap();
        let error = open(&path).unwrap_err();
        assert!(error.contains("damaged") || error.contains("newer MailCraft"), "{n}: {error}");
        assert!(sweep(&path, Duration::ZERO).is_err(), "{n}");
    }
    // Content names that are paths or lies cost that content and nothing else.
    let secret = noise(1, 4_000);
    fs::write(tmp.0.join("secret"), &secret).unwrap();
    for refs in [json!({"text": ["../secret", 4000]}), json!({"attachments": [["/etc/passwd", 10], null]}), json!({"text": ["", 1u64 << 62]})] {
        let refs = serde_json::to_vec(&refs).unwrap();
        fs::write(
            &path,
            with(&|w| {
                index::put(w, (Coll::Message, id), &json, &refs).unwrap();
            }),
        )
        .unwrap();
        let opened = open(&path).unwrap();
        let m = opened.mailbox.message(id).unwrap();
        assert!(m.body.text.as_bytes() != secret.as_slice());
        assert!(opened.mailbox.clone().repair().is_empty());
        let mut next = opened.mailbox.clone();
        next.message_mut(id).unwrap().read ^= true;
        save(&path, &next).unwrap();
        sweep(&path, Duration::ZERO).unwrap();
    }
    // Items the rest record brings along are no part of the mailbox.
    let bogus = serde_json::to_vec(&full_mailbox(9)).unwrap();
    fs::write(
        &path,
        with(&|w| {
            index::rest(w, &bogus).unwrap();
        }),
    )
    .unwrap();
    let opened = open(&path).unwrap();
    assert_eq!((opened.mailbox.messages.len(), opened.mailbox.contacts.len()), (mb.messages.len(), 0));
    // A key that isn't an id the model takes is repaired.
    fs::write(
        &path,
        with(&|w| {
            index::put(w, (Coll::Message, u64::MAX), &json, b"").unwrap();
        }),
    )
    .unwrap();
    let opened = open(&path).unwrap();
    assert!(opened.mailbox.messages.keys().all(|k| *k < u64::MAX) && opened.mailbox.clone().repair().is_empty());
    assert_eq!(opened.mailbox.messages.len(), mb.messages.len() + 1);
    // Saved, what was repaired is written.
    save(&path, &opened.mailbox).unwrap();
    assert!(whole(&path) == loaded(&path, &opened.mailbox));
    assert_eq!(fs::read(tmp.0.join("secret")).unwrap(), secret);
}

#[test]
fn the_rest_record_is_the_mailbox_without_its_items() {
    // Written without copying the mailbox, it is what a copy without its items comes to.
    let mut mb = full_mailbox(5);
    mb.zone = "Europe/Paris".into();
    let mut shell = mb.clone();
    shell.version = INDEX_VERSION;
    shell.messages.clear();
    shell.contacts.clear();
    shell.events.clear();
    shell.tasks.clear();
    assert_eq!(rest_json(&mb).unwrap(), serde_json::to_vec(&shell).unwrap());
    // Anything but a mailbox isn't written that way.
    assert!(5u8.serialize(Shell(&mut serde_json::Serializer::new(Vec::new()))).is_err());
    assert!(vec![1].serialize(Shell(&mut serde_json::Serializer::new(Vec::new()))).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever bytes follow the header, reading them ends in a mailbox file or an error.
    #[test]
    fn arbitrary_records_never_panic(body in proptest::collection::vec(any::<u8>(), 0..600), kind in 0u8..6, len in 0u32..700) {
        let mut bytes = index::header().to_vec();
        bytes.extend(&body);
        for claimed in [bytes.len() as u64, bytes.len() as u64 + 77, (bytes.len() as u64).saturating_sub(5), 0] {
            let _ = index::replay(&bytes[..], claimed);
        }
        // A frame that looks right around junk.
        let mut framed = index::header().to_vec();
        framed.extend(len.to_le_bytes());
        framed.push(kind);
        framed.extend(&body);
        let _ = index::replay(&framed[..], framed.len() as u64);
    }

    /// A real file, cut anywhere and with any byte changed, opens as some state it had or is
    /// refused; saved from there it opens as what was saved.
    #[test]
    fn a_damaged_file_never_panics(cut in any::<prop::sample::Index>(), flip in any::<prop::sample::Index>(), mask in 1u8..=255) {
        let tmp = Temp::new("damaged-prop");
        let path = tmp.file();
        let mut mb = full_mailbox(6);
        save(&path, &mb).unwrap();
        let ids: Vec<Id> = mb.messages.keys().copied().collect();
        mb.message_mut(ids[0]).unwrap().read = true;
        save(&path, &mb).unwrap();
        mb.tasks.clear();
        save(&path, &mb).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        let at = flip.index(bytes.len());
        bytes[at] ^= mask;
        bytes.truncate(cut.index(bytes.len() + 1).max(1));
        fs::write(&path, &bytes).unwrap();
        if let Ok(opened) = open(&path) {
            prop_assert!(opened.mailbox.clone().repair().is_empty());
            let mut next = opened.mailbox.clone();
            next.categories.clear();
            save(&path, &next).unwrap();
            prop_assert!(whole(&path) == loaded(&path, &next));
        }
        let _ = sweep(&path, Duration::ZERO);
    }
}

/// How quickly a large mailbox opens and saves. `cargo test --release -p mailcraft-engine --lib
/// large_mailbox -- --ignored --nocapture`; `MAILCRAFT_BENCH_MESSAGES` sets the size (100,000 by
/// default).
#[test]
#[ignore]
fn large_mailbox_opens_and_saves_quickly() {
    let n: usize = std::env::var("MAILCRAFT_BENCH_MESSAGES").ok().and_then(|v| v.parse().ok()).unwrap_or(100_000);
    let tmp = Temp::new("large");
    let path = tmp.file();
    let t = Instant::now();
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    let words = ["the", "report", "budget", "meeting", "please", "review", "attached", "quarter", "figures", "thanks"];
    for i in 0..n {
        // Mostly short notes kept in the mailbox file, one in five long enough to go outside.
        let len = if i % 5 == 0 { 1_500 + (i % 7) * 300 } else { 200 + (i % 11) * 60 };
        let mut text = String::with_capacity(len + 16);
        let mut w = i;
        while text.len() < len {
            text.push_str(words[w % words.len()]);
            text.push(' ');
            w = w.wrapping_mul(31).wrapping_add(7);
        }
        mb.add_message(Message {
            folder: inbox,
            subject: format!("Message {i} about the {}", words[i % words.len()]),
            from: Address::new(format!("Sender {}", i % 997), format!("sender{}@example.org", i % 997)),
            to: vec![Address::new("Avery Quinn", "avery@example.org")],
            message_id: format!("{i}@mail.example.org"),
            date: 1_600_000_000 + i as i64 * 60,
            body: mailcraft_model::Body { text, html: None },
            read: i % 3 != 0,
            ..Default::default()
        })
        .unwrap();
    }
    eprintln!("built {n} messages in {:?}", t.elapsed());
    let t = Instant::now();
    let first = save(&path, &mb).unwrap();
    eprintln!("first save (whole): {:?}, mailbox file {} MB, {} content files", t.elapsed(), first.bytes >> 20, first.blobs);
    drop(mb);
    *lock(&shared(&path).state) = State::default();

    let t = Instant::now();
    let opened = open(&path).unwrap();
    eprintln!("open: {:?} ({} messages)", t.elapsed(), opened.mailbox.messages.len());
    let mut mb = opened.mailbox;
    let saves: usize = std::env::var("MAILCRAFT_BENCH_SAVES").ok().and_then(|v| v.parse().ok()).unwrap_or(5);
    let ids: Vec<Id> = mb.messages.keys().copied().take(saves).collect();
    let mut quickest = Duration::MAX;
    let mut slowest = Duration::ZERO;
    for id in &ids {
        mb.message_mut(*id).unwrap().read ^= true;
        let t = Instant::now();
        let saved = save(&path, &mb).unwrap();
        quickest = quickest.min(t.elapsed());
        slowest = slowest.max(t.elapsed());
        assert!(!saved.compacted && saved.bytes < 2_000, "{saved:?}");
    }
    eprintln!("save after one change: {quickest:?} to {slowest:?}, a few hundred bytes appended");
    let held: u64 = mb.messages.values().map(|m| m.size()).sum();
    eprintln!("mailbox file {} MB; message text and attachments in memory {} MB", fs::metadata(&path).unwrap().len() >> 20, held >> 20);
    *lock(&shared(&path).state) = State::default();
    let t = Instant::now();
    let whole = save(&path, &mb).unwrap();
    eprintln!("written whole again (as after a crash or a migration): {:?} ({} MB)", t.elapsed(), whole.bytes >> 20);
}

/// A reader that gives a few bytes at a time, as a slow disk or a pipe may.
struct Trickle<'a>(&'a [u8], usize);

impl std::io::Read for Trickle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.1 = self.1 % 7 + 1;
        let n = self.1.min(buf.len()).min(self.0.len());
        buf[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        Ok(n)
    }
}

/// What a replay read, to compare.
fn replayed_as_text(r: Result<index::Replayed, String>) -> String {
    match r {
        Err(e) => format!("error {e}"),
        Ok(r) => {
            let mut items: Vec<String> =
                r.items.iter().map(|(k, i)| format!("{k:?} {:?} {:?} {:?} {}", i.json(), i.refs(), i.fp, i.size())).collect();
            items.sort();
            format!("{:?} {items:?} {} {:?} {} {}", r.rest, r.good_len, r.tail, r.lost_saves, r.lost_records)
        }
    }
}

#[test]
fn replay_reads_the_same_whatever_pieces_the_file_comes_in_damaged_or_not() {
    let tmp = Temp::new("replay-pieces");
    let path = tmp.file();
    let mut mb = full_mailbox(12);
    save(&path, &mb).unwrap();
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    for (i, id) in ids.iter().take(4).enumerate() {
        mb.message_mut(*id).unwrap().subject = format!("Changed {i}");
        save(&path, &mb).unwrap();
    }
    mb.messages.remove(&ids[6]);
    save(&path, &mb).unwrap();
    let good = fs::read(&path).unwrap();
    // The file as it is, cut short anywhere, and with a byte changed anywhere (every 97th).
    let mut variants = vec![good.clone()];
    for at in (0..good.len()).step_by(97) {
        variants.push(good[..at].to_vec());
        let mut bad = good.clone();
        bad[at] ^= 0x55;
        variants.push(bad);
    }
    variants.push([good.clone(), vec![0; 300]].concat());
    for (n, bytes) in variants.iter().enumerate() {
        let len = bytes.len() as u64;
        let whole = replayed_as_text(index::replay(&bytes[..], len));
        let pieces = replayed_as_text(index::replay(Trickle(bytes, n), len));
        assert_eq!(whole, pieces, "variant {n}");
    }
}
