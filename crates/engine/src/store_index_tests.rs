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
        let opened = open(&path).unwrap();
        assert!(opened.mailbox == mb && opened.unavailable == 0 && opened.unfinished == 0, "form {form}");
        assert_eq!(opened.split, form == 2);
        // Unchanged, it stays as it is.
        assert!(save(&path, &opened.mailbox).unwrap().unchanged);
        assert_eq!(fs::read(&path).unwrap(), before);

        // Stopped while the new file was being written: the old one is the mailbox, and what was
        // written of the new one is a temporary file nobody reads.
        let half = tmp.0.join(format!("mailbox.mailcraft.{}-2a-9.tmp", std::process::id()));
        fs::write(&half, index::header()).unwrap();
        assert!(open(&path).unwrap().mailbox == mb, "form {form}");

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
        let back = open(&path).unwrap();
        assert!(back.mailbox == changed && back.unavailable == 0 && back.split, "form {form}");
        if form == 1 {
            assert!(crate::io::read_mailbox(&before).unwrap() == mb);
        }
        // From then on a save appends, and nothing more is kept aside.
        let mut more = back.mailbox.clone();
        more.message_mut(first).unwrap().read = true;
        let saved = save(&path, &more).unwrap();
        assert!(!saved.compacted && saved.bytes > 0 && saved.bytes < 2_000, "{saved:?}");
        assert_eq!(kept(&path, "format").len(), 1);
        assert!(open(&path).unwrap().mailbox == more);
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
    mb.signatures.push(mailcraft_model::Signature { name: "Short".into(), ..Default::default() });
    let many = save(&path, &mb).unwrap();
    assert!(!many.compacted && many.blobs == 1 && many.bytes < 20_000, "{many:?}");
    assert!(peek(&path) == mb);
    assert!(open(&path).unwrap().mailbox == mb);

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
    assert!(open(&path).unwrap().mailbox == mb);
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
        let opened = open(&path).unwrap();
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
    assert!(open(&path).unwrap().mailbox == mb && tail_of(&path) == Tail::Clean);
    // Nothing is swept while it is kept: it may name content the mailbox doesn't any more.
    let mut fewer = mb.clone();
    fewer.messages.retain(|_, m| m.attachments.is_empty());
    save(&path, &fewer).unwrap();
    for (file, _) in files(&blob_dir(&path)) {
        age(&file, Duration::from_secs(86_400));
    }
    assert_eq!(sweep(&path, GRACE).unwrap(), Swept::default());
    fs::remove_file(&aside[0]).unwrap();
    assert!(sweep(&path, GRACE).unwrap().removed > 0);
    assert!(open(&path).unwrap().mailbox == fewer);
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
    let opened = open(&path).unwrap();
    assert!(opened.mailbox == mb && opened.unfinished == 0);
    assert_eq!(tail_of(&path), Tail::Zeros);
    // The next save can't append after them: it writes the file whole, and keeps nothing.
    mb.message_mut(ids[1]).unwrap().read = true;
    assert!(save(&path, &mb).unwrap().compacted);
    assert!(kept(&path, "damaged-").is_empty() && open(&path).unwrap().mailbox == mb);

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
    assert!(open(&path).unwrap().mailbox == edited);
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
    assert!(open(&path).unwrap().mailbox == opened.mailbox);
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
            prop_assert!(open(&path).unwrap().mailbox == next);
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
