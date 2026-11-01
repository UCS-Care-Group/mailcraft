// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The word index on its own: what it finds, and its file read back, cut short, damaged, of
//! another version, or junk (R6.2).

use std::path::PathBuf;
use std::sync::Arc;

use mailcraft_model::{Address, Attachment, Body, Message, words};

use super::*;

/// A folder of its own under the system's temporary folder, removed afterwards.
pub(crate) struct Temp(PathBuf);

impl Temp {
    pub(crate) fn new(name: &str) -> Temp {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mailcraft-words-{}-{n}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    pub(crate) fn file(&self) -> PathBuf {
        self.0.join("mailbox.mailcraft.words")
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn message(id: u64, subject: &str, from: &str, text: &str) -> Message {
    Message {
        id,
        subject: subject.into(),
        from: Address::new(from, format!("{}@example.org", from.to_lowercase())),
        to: vec![Address::new("Avery Quinn", "avery@mail.example")],
        body: Body { text: text.into(), html: None },
        ..Default::default()
    }
}

fn messages() -> Vec<Message> {
    let mut with_file = message(3, "Figures", "Kit", "See the attached sheet.");
    with_file.attachments = vec![Attachment::new("estimate-Q4.xlsx", "application/octet-stream", vec![1, 2, 3])];
    vec![
        message(1, "Meeting on Tuesday", "Ada", "The budget review meets at noon in the Gvožđar room."),
        message(2, "Re: Budget", "Bram", "Numbers attached. Café at three?"),
        with_file,
    ]
}

fn filled(file: Option<PathBuf>) -> Index {
    let mut index = Index::new(file);
    for m in messages() {
        index.insert(Doc::of(&m, &m.body.text), None);
    }
    index
}

/// The message ids `index` finds for `query` (its words) in `fields`.
fn found(index: &Index, query: &str, fields: u8) -> Vec<Id> {
    let bits = index.matching(&words::words(query), fields);
    let mut ids: Vec<Id> = index.ids().filter(|id| index.slot_of(*id).is_some_and(|s| bits.has(s))).collect();
    ids.sort_unstable();
    ids
}

#[test]
fn it_finds_word_starts_in_the_parts_asked_for() {
    let index = filled(None);
    assert_eq!(index.len(), 3);
    assert_eq!(found(&index, "meet", words::ALL), [1]);
    assert_eq!(found(&index, "budget", words::ALL), [1, 2]);
    assert_eq!(found(&index, "budget", words::SUBJECT), [2]);
    assert_eq!(found(&index, "budget", words::BODY), [1]);
    assert_eq!(found(&index, "gvozdar", words::ALL), [1]);
    assert_eq!(found(&index, "cafe", words::BODY), [2]);
    assert_eq!(found(&index, "ada", words::FROM), [1]);
    assert_eq!(found(&index, "avery", words::TO), [1, 2, 3]);
    assert_eq!(found(&index, "avery", words::FROM), Vec::<Id>::new());
    assert_eq!(found(&index, "estimate q4", words::ATTACHMENT), [3]);
    assert_eq!(found(&index, "udget", words::ALL), Vec::<Id>::new());
    assert_eq!(found(&index, "zyxwvut", words::ALL), Vec::<Id>::new());
    assert_eq!(found(&index, "", words::ALL), [1, 2, 3]);
}

#[test]
fn a_message_put_in_again_replaces_what_it_held_and_one_taken_out_goes() {
    let mut index = filled(None);
    let mut m = message(2, "Re: Invoice", "Bram", "Paid.");
    let doc = Doc::of(&m, &m.body.text);
    assert!(!index.current(2, fingerprint(&m)));
    index.insert(doc, None);
    assert!(index.current(2, fingerprint(&m)));
    assert_eq!(found(&index, "budget", words::ALL), [1]);
    assert_eq!(found(&index, "invoice", words::ALL), [2]);
    index.remove(1);
    assert_eq!(found(&index, "budget", words::ALL), Vec::<Id>::new());
    assert_eq!(index.len(), 2);
    // A flag or a folder isn't read: the same fingerprint.
    let before = fingerprint(&m);
    (m.read, m.folder, m.flag) = (true, 9, mailcraft_model::FlagStatus::Flagged);
    assert_eq!(fingerprint(&m), before);
    m.to.push(Address::new("Kit", "kit@example.org"));
    assert_ne!(fingerprint(&m), before);
}

#[test]
fn a_body_kept_on_disk_has_the_fingerprint_it_has_in_memory() {
    let m = message(4, "Long", "Ada", "A long text that lives in the content folder.");
    let r = mailcraft_model::ContentRef { sha256: sha256(m.body.text.as_bytes()), len: m.body.text.len() as u64 };
    let mut lean = m.clone();
    lean.body.text.clear();
    lean.kept = Some(Box::new(mailcraft_model::KeptBody { text: Some(r), ..Default::default() }));
    assert_eq!(fingerprint(&lean), fingerprint(&m));
    assert!(body_in_memory(&lean).is_none() && body_in_memory(&m).is_some());
    // Its reading failed and its text says so: still the file it is kept in.
    lean.kept.as_mut().unwrap().failed = true;
    lean.body.text = crate::store_placeholder().into();
    assert_eq!(fingerprint(&lean), fingerprint(&m));
}

#[test]
fn the_file_reads_back_as_it_was_written() {
    let tmp = Temp::new("round-trip");
    let file = tmp.file();
    let mut index = filled(Some(file.clone()));
    index.flush();
    index.remove(3);
    index.flush();
    let again = Index::open(&file).unwrap();
    assert_eq!((again.len(), again.word_count()), (2, index.word_count()));
    for (query, fields) in [("budget", words::ALL), ("meet", words::ALL), ("ada", words::FROM), ("estimate", words::ALL), ("cafe", words::BODY)] {
        assert_eq!(found(&again, query, fields), found(&index, query, fields), "{query}");
    }
    let m = &messages()[0];
    assert!(again.current(1, fingerprint(m)));
    // Written whole again: what was taken out is gone from the file too.
    let mut again = again;
    again.remove(2);
    again.compact();
    let bytes = std::fs::read(&file).unwrap();
    assert!(!bytes.windows(4).any(|w| w == b"cafe"), "a word only the removed message had");
    let third = Index::open(&file).unwrap();
    assert_eq!((third.len(), found(&third, "budget", words::ALL)), (1, vec![1]));
    // And goes on from there.
    let mut third = third;
    let m = message(7, "Picnic", "Oona", "Bring sandwiches.");
    third.insert(Doc::of(&m, &m.body.text), None);
    third.flush();
    let fourth = Index::open(&file).unwrap();
    assert_eq!((fourth.len(), found(&fourth, "sandwich", words::ALL)), (2, vec![7]));
    assert_eq!(found(&fourth, "meet", words::ALL), [1]);
}

#[test]
fn a_file_cut_short_keeps_what_was_whole_and_goes_on_after_it() {
    let tmp = Temp::new("cut-short");
    let file = tmp.file();
    let mut index = Index::new(Some(file.clone()));
    let ms = messages();
    index.insert(Doc::of(&ms[0], &ms[0].body.text), None);
    index.flush();
    let whole = std::fs::metadata(&file).unwrap().len();
    index.insert(Doc::of(&ms[1], &ms[1].body.text), None);
    index.flush();
    let len = std::fs::metadata(&file).unwrap().len();
    // The program stopped while the second message was being written.
    let f = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
    f.set_len(len - 3).unwrap();
    drop(f);
    let mut again = Index::open(&file).unwrap();
    assert_eq!((again.len(), found(&again, "meet", words::ALL)), (1, vec![1]));
    assert!(again.file_len >= whole && again.file_len < len - 3, "{} {whole} {len}", again.file_len);
    again.insert(Doc::of(&ms[2], &ms[2].body.text), None);
    again.flush();
    let third = Index::open(&file).unwrap();
    assert_eq!(found(&third, "figures", words::ALL), [3]);
    assert_eq!(third.len(), 2);
}

#[test]
fn a_damaged_file_or_one_of_another_version_is_not_believed() {
    let tmp = Temp::new("damaged");
    let file = tmp.file();
    let mut index = filled(Some(file.clone()));
    index.flush();
    let bytes = std::fs::read(&file).unwrap();
    // A byte changed in the middle of a record.
    let mut damaged = bytes.clone();
    let at = damaged.len() / 2;
    damaged[at] ^= 0x55;
    std::fs::write(&file, &damaged).unwrap();
    assert!(Index::open(&file).is_err());
    // Another version.
    let mut other = bytes.clone();
    other[8] = 9;
    std::fs::write(&file, &other).unwrap();
    assert!(Index::open(&file).unwrap_err().contains("version"));
    // Not an index at all.
    std::fs::write(&file, b"{\"messages\": []}                   ").unwrap();
    assert!(Index::open(&file).is_err());
    // Nothing there: an empty index.
    std::fs::remove_file(&file).unwrap();
    assert!(Index::open(&file).unwrap().is_empty());
}

/// Records whose checksums are right but whose contents lie: never believed, never a panic.
#[test]
fn records_that_lie_are_not_believed() {
    let tmp = Temp::new("lying");
    let file = tmp.file();
    let with = |records: &[(u8, Vec<u8>)]| {
        let mut out = header().to_vec();
        for (kind, p) in records {
            record(&mut out, *kind, p);
        }
        std::fs::write(&file, out).unwrap();
        Index::open(&file)
    };
    let words_rec = |ws: &[&str]| {
        let mut p = Vec::new();
        put_varint(&mut p, ws.len() as u64);
        for w in ws {
            put_varint(&mut p, w.len() as u64);
            p.extend_from_slice(w.as_bytes());
        }
        (WORDS, p)
    };
    let doc = |id: u64, entries: &[(u64, u8)]| {
        let mut p = Vec::new();
        p.extend_from_slice(&id.to_le_bytes());
        p.extend_from_slice(&0u64.to_le_bytes());
        put_varint(&mut p, entries.len() as u64);
        for (w, mask) in entries {
            put_varint(&mut p, *w);
            p.push(*mask);
        }
        (DOC, p)
    };
    assert!(with(&[words_rec(&["one", "two"]), doc(1, &[(0, 1), (1, 16)])]).is_ok());
    // A word number past the words.
    assert!(with(&[words_rec(&["one"]), doc(1, &[(5, 1)])]).is_err());
    // The same word twice in one message, a mask of no part, a part that doesn't exist.
    assert!(with(&[words_rec(&["one"]), doc(1, &[(0, 1), (0, 2)])]).is_err());
    assert!(with(&[words_rec(&["one"]), doc(1, &[(0, 0)])]).is_err());
    assert!(with(&[words_rec(&["one"]), doc(1, &[(0, 64)])]).is_err());
    // A word twice, an empty word, one too long, bytes that aren't text.
    assert!(with(&[words_rec(&["one", "one"])]).is_err());
    assert!(with(&[words_rec(&[""])]).is_err());
    assert!(with(&[words_rec(&[&"x".repeat(words::MAX_WORD + 1)])]).is_err());
    assert!(with(&[(WORDS, vec![1, 2, 0xff, 0xfe])]).is_err());
    // A message taken out that isn't there; postings past the messages, or not rising.
    let mut kill = Vec::new();
    put_varint(&mut kill, 1);
    put_varint(&mut kill, 4);
    assert!(with(&[(KILL, kill)]).is_err());
    let postings = |bytes: &[u8]| {
        let mut p = Vec::new();
        put_varint(&mut p, 1);
        put_varint(&mut p, 0);
        put_varint(&mut p, bytes.len() as u64);
        p.extend_from_slice(bytes);
        (POSTINGS, p)
    };
    let mut slots = Vec::new();
    put_varint(&mut slots, 2);
    for id in [1u64, 2] {
        slots.extend_from_slice(&id.to_le_bytes());
        slots.extend_from_slice(&0u64.to_le_bytes());
        slots.push(1);
    }
    let index = with(&[words_rec(&["one"]), (SLOTS, slots.clone()), postings(&[0, 1, 1, 16])]).unwrap();
    assert_eq!(found(&index, "one", words::ALL), [1, 2]);
    assert_eq!(found(&index, "one", words::BODY), [2]);
    assert!(with(&[words_rec(&["one"]), (SLOTS, slots.clone()), postings(&[0, 1, 5, 1])]).is_err());
    assert!(with(&[words_rec(&["one"]), (SLOTS, slots.clone()), postings(&[1, 1, 0, 1])]).is_err());
    // An unknown kind of record, and bytes left over after one.
    assert!(with(&[(42, vec![])]).is_err());
    let (kind, mut p) = words_rec(&["one"]);
    p.push(0);
    assert!(with(&[(kind, p)]).is_err());
}

#[test]
fn a_message_counts_its_words_up_to_the_limit() {
    let text: String = (0..MAX_DOC_WORDS + 500).map(|n| format!("w{n} ")).collect();
    let m = message(1, "Many", "Ada", &text);
    let doc = Doc::of(&m, &m.body.text);
    assert_eq!(doc.words.len(), MAX_DOC_WORDS);
}

#[test]
fn a_message_seen_is_known_by_itself_until_it_is_replaced() {
    let mut index = Index::new(None);
    let m = Arc::new(messages().remove(0));
    index.insert(Doc::of(&m, &m.body.text), Some(&m));
    assert!(index.seen(&m));
    let copy = Arc::new(Message::clone(&m));
    assert!(!index.seen(&copy));
    index.note_seen(&copy);
    assert!(index.seen(&copy) && !index.seen(&m));
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig { cases: 256, ..Default::default() })]

    /// Whatever the file holds, opening it says what is wrong or reads what is right: it never
    /// panics.
    #[test]
    fn junk_never_panics(junk in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..2048), keep_header in proptest::prelude::any::<bool>()) {
        let tmp = Temp::new("junk");
        let file = tmp.file();
        let mut bytes = if keep_header { header().to_vec() } else { Vec::new() };
        bytes.extend_from_slice(&junk);
        std::fs::write(&file, &bytes).unwrap();
        if let Ok(mut index) = Index::open(&file) {
            let _ = index.matching(&words::words("a b"), words::ALL);
            let m = message(1, "x", "y", "z");
            index.insert(Doc::of(&m, &m.body.text), None);
            index.flush();
            index.compact();
        }
    }
}

// ---- kept in step with a session's mailbox ----

mod in_step {
    use mailcraft_model::{Account, FolderKind, Mailbox};
    use serde_json::json;

    use super::*;
    use crate::Session;

    /// A mailbox of two accounts, each with an Inbox of messages whose texts are long enough to be
    /// kept outside the mailbox file; saved in `tmp`.
    pub(crate) fn kept(tmp: &Temp) -> (PathBuf, Id, Id) {
        let path = tmp.0.join("mailbox.mailcraft");
        let mut mb = Mailbox::with_local_account("Avery", "avery@mail.example");
        let work = mb.add_account(Account { name: "Work".into(), email: "avery@work.example".into(), ..Default::default() });
        let inbox = |mb: &Mailbox, account: Id| mb.folders.iter().find(|f| f.account == account && f.kind == FolderKind::Inbox).unwrap().id;
        let first = mb.accounts[0].id;
        let filler = " Nothing else of note this week, all is well with the project.".repeat(30);
        for (n, (subject, word)) in [("Meeting", "zymurgy"), ("Lunch", "quokka"), ("Figures", "marmalade")].into_iter().enumerate() {
            let folder = inbox(&mb, first);
            mb.add_message(Message { folder, account: first, date: n as i64, ..message(0, subject, "Ada", &format!("About the {word}.{filler}")) })
                .unwrap();
        }
        let folder = inbox(&mb, work);
        mb.add_message(Message { folder, account: work, ..message(0, "Rota", "Kit", &format!("The nightingale rota.{filler}")) }).unwrap();
        crate::store::save(&path, &mb).unwrap();
        (path, first, work)
    }

    /// A session on the mailbox at `path` with no body in memory, keeping a word index.
    pub(crate) fn session(path: &std::path::Path) -> Session {
        crate::store::clear_cache();
        let opened = crate::store::with_budget(1, || crate::store::open(path)).unwrap();
        let mut s = Session::new(opened.mailbox);
        s.path = Some(path.to_path_buf());
        s.index_words = true;
        s
    }

    /// What the session's index finds for `query` (anywhere), after a search brought it in step.
    fn finds(s: &mut Session, query: &str) -> Vec<Id> {
        s.view.search = format!("{query} ");
        s.view.scope = crate::Scope::Everywhere;
        s.wait_for_word_index();
        let _ = s.rows();
        let words = s.words().unwrap();
        let guard = words.lock();
        found(guard.as_ref().unwrap(), query, words::ALL)
    }

    /// #301: a whole address after `from:` finds that sender's mail alone, answered from the word
    /// index: its words narrow the messages, and the address is looked at on each.
    #[test]
    fn a_whole_address_finds_one_sender_from_the_index() {
        let tmp = Temp::new("address");
        let path = tmp.0.join("mailbox.mailcraft");
        let mut mb = Mailbox::with_local_account("Avery", "avery@mail.example");
        let account = mb.accounts[0].id;
        let folder = mb.folders.iter().find(|f| f.account == account && f.kind == FolderKind::Inbox).unwrap().id;
        let senders = [
            ("One", "ada@example.org"),
            ("Two", "ADA@Example.org"),
            ("Three", "adam@example.org"),
            ("Four", "ada.smith@example.org"),
            ("Five", "ada@example.org.example"),
        ];
        for (subject, email) in senders {
            mb.add_message(Message { folder, account, from: Address::new("Ada", email), ..message(0, subject, "Ada", "Hello.") }).unwrap();
        }
        crate::store::save(&path, &mb).unwrap();
        let mut s = session(&path);
        let subjects = |s: &mut Session, query: &str| {
            s.view.search = query.into();
            s.view.scope = crate::Scope::Everywhere;
            s.wait_for_word_index();
            let mut out: Vec<String> = s.rows().iter().map(|r| s.mailbox.message(r.id).unwrap().subject.clone()).collect();
            out.sort();
            out
        };
        assert_eq!(subjects(&mut s, "from:ada@example.org"), ["One", "Two"]);
        assert!(s.words().unwrap().complete(), "answered from the index");
        // Words of it find more, as before.
        assert_eq!(subjects(&mut s, "from:ada"), ["Five", "Four", "One", "Three", "Two"]);
        assert_eq!(subjects(&mut s, "from:ada@example.org -subject:two"), ["One"]);
    }

    /// #329: a word answered from the index beside a day in words, matched on the session's day.
    #[test]
    fn a_word_from_the_index_and_a_day_in_words() {
        let tmp = Temp::new("dated");
        let path = tmp.0.join("mailbox.mailcraft");
        let mut mb = Mailbox::with_local_account("Avery", "avery@mail.example");
        let account = mb.accounts[0].id;
        let folder = mb.folders.iter().find(|f| f.account == account && f.kind == FolderKind::Inbox).unwrap().id;
        let noon = |d: u8| mailcraft_time::DateTime::new(mailcraft_time::Date::new(2026, 10, d).unwrap(), 12, 0, 0).unwrap().civil_seconds();
        for (subject, d) in [("Old", 22), ("Yesterday", 25), ("Today", 26)] {
            mb.add_message(Message { folder, account, date: noon(d), received: noon(d), ..message(0, subject, "Ada", "About the zymurgy.") })
                .unwrap();
        }
        crate::store::save(&path, &mb).unwrap();
        let mut s = session(&path);
        s.clock = crate::Clock { now: Some(noon(26) + 3600), offset: Some(0), zone: None };
        let subjects = |s: &mut Session, query: &str| {
            s.view.search = query.into();
            s.view.scope = crate::Scope::Everywhere;
            s.wait_for_word_index();
            let mut out: Vec<String> = s.rows().iter().map(|r| s.mailbox.message(r.id).unwrap().subject.clone()).collect();
            out.sort();
            out
        };
        assert_eq!(subjects(&mut s, "zymurgy received:today"), ["Today"]);
        assert!(s.words().unwrap().complete(), "answered from the index");
        assert_eq!(subjects(&mut s, "zymurgy received:10/25..10/26"), ["Today", "Yesterday"]);
        assert_eq!(subjects(&mut s, "zymurgy -received:\"this week\""), ["Old"]);
    }

    pub(crate) fn id_of(s: &Session, subject: &str) -> Id {
        s.mailbox.messages.values().find(|m| m.subject == subject).unwrap().id
    }

    #[test]
    fn the_index_follows_the_mailbox_through_commands_undo_and_mail_that_arrives() {
        let tmp = Temp::new("in-step");
        let (path, _, _) = kept(&tmp);
        let mut s = session(&path);
        assert!(s.mailbox.messages.values().all(|m| m.body_is_kept()), "every text on disk");
        let meeting = id_of(&s, "Meeting");
        assert_eq!(finds(&mut s, "zymurgy"), [meeting]);
        assert!(s.words().unwrap().complete());
        // Deleted for good: gone from the index; undone: back.
        s.run("mail.deletePermanently", &json!({"ids": [meeting]})).unwrap();
        assert_eq!(finds(&mut s, "zymurgy"), Vec::<Id>::new());
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(finds(&mut s, "zymurgy"), [meeting]);
        // A command that fails changes nothing.
        assert!(s.run("mail.move", &json!({"ids": [meeting], "folder": 999_999})).is_err());
        assert_eq!(finds(&mut s, "zymurgy"), [meeting]);
        // Moved: the same words.
        let archive = s.mailbox.folders.iter().find(|f| f.kind == FolderKind::Archive).unwrap().id;
        s.run("mail.move", &json!({"ids": [meeting], "folder": archive})).unwrap();
        assert_eq!(finds(&mut s, "zymurgy"), [meeting]);
        // Changed outside a command (mail that came down from a server): as it is now.
        let lunch = id_of(&s, "Lunch");
        s.mailbox.message_mut(lunch).unwrap().subject = "Supper".into();
        s.mailbox_changed();
        assert_eq!(finds(&mut s, "supper"), [lunch]);
        assert_eq!(finds(&mut s, "lunch"), Vec::<Id>::new());
        assert_eq!(finds(&mut s, "quokka"), [lunch], "its text read again from disk");
        let folder = s.mailbox.inbox().unwrap();
        let new = s.mailbox.add_message(Message { folder, ..message(0, "Picnic", "Oona", "Bring sandwiches.") }).unwrap();
        s.mailbox_changed();
        assert_eq!(finds(&mut s, "sandwich"), [new]);
    }

    #[test]
    fn the_index_is_kept_beside_the_mailbox_and_read_back_when_it_opens_again() {
        let tmp = Temp::new("reopen");
        let (path, _, _) = kept(&tmp);
        let mut s = session(&path);
        let meeting = id_of(&s, "Meeting");
        assert_eq!(finds(&mut s, "zymurgy"), [meeting]);
        let file = crate::store::words_file(&path);
        assert!(file.is_file());
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&file).unwrap().permissions()) & 0o077, 0, "for its owner only");
        let len = std::fs::metadata(&file).unwrap().len();
        drop(s);
        let mut again = session(&path);
        assert_eq!(finds(&mut again, "zymurgy"), [meeting]);
        assert_eq!(std::fs::metadata(&file).unwrap().len(), len, "nothing had to be read again");
        // Junk where the index was: built again, never a crash.
        drop(again);
        std::fs::write(&file, b"MCWORDS\0\x01\0\0\0\0\0\0\0junk junk junk junk").unwrap();
        let mut third = session(&path);
        assert_eq!(finds(&mut third, "zymurgy"), [meeting]);
        assert!(Index::open(&file).is_ok());
        // Off: no index.
        third.index_words = false;
        third.view.search = "anything".into();
        let _ = third.rows();
        assert!(third.words().is_none());
        assert!(super::super::remove(&path).is_ok() && !file.exists());
    }

    #[test]
    fn removing_an_account_takes_its_words_off_the_disk() {
        let tmp = Temp::new("account");
        let (path, _, work) = kept(&tmp);
        let mut s = session(&path);
        let rota = id_of(&s, "Rota");
        assert_eq!(finds(&mut s, "nightingale"), [rota]);
        let file = crate::store::words_file(&path);
        let holds = |file: &std::path::Path| std::fs::read(file).unwrap().windows(11).any(|w| w == b"nightingale");
        assert!(holds(&file));
        s.run("account.remove", &json!({"id": work})).unwrap();
        s.view.search.clear();
        let _ = s.rows();
        assert!(!holds(&file), "gone without a search");
        assert_eq!(finds(&mut s, "zymurgy").len(), 1);
    }

    /// #325: an account removed where no word index is kept (`mailcraft-cli`, an agent without
    /// the window) has its mail's words taken off the disk the first time the index is brought in
    /// step after it is opened again, not only once enough has been taken out.
    #[test]
    fn mail_gone_while_the_index_was_closed_takes_its_words_off_the_disk() {
        let tmp = Temp::new("closed");
        let (path, _, work) = kept(&tmp);
        let file = crate::store::words_file(&path);
        let holds = |file: &std::path::Path| std::fs::read(file).unwrap().windows(11).any(|w| w == b"nightingale");
        assert_eq!(finds(&mut session(&path), "nightingale").len(), 1);
        assert!(holds(&file));

        // Removed, and saved, by a session that keeps no index, as the command line does.
        let mut cli = session(&path);
        cli.index_words = false;
        cli.run("account.remove", &json!({"id": work})).unwrap();
        cli.run("file.save", &json!({})).unwrap();
        drop(cli);
        assert!(holds(&file), "nothing touched the index");

        let mut s = session(&path);
        s.wait_for_word_index();
        assert!(!holds(&file), "gone at the first step");
        assert_eq!(finds(&mut s, "zymurgy").len(), 1);
        assert!(finds(&mut s, "nightingale").is_empty());
    }
}

// ---- built in the background ----

mod background {
    use mailcraft_model::Mailbox;

    use super::in_step::session;
    use super::*;
    use crate::{Session, WordIndex};

    fn many(tmp: &Temp, n: usize) -> PathBuf {
        let path = tmp.0.join("mailbox.mailcraft");
        let mut mb = Mailbox::with_local_account("Avery", "avery@mail.example");
        let folder = mb.inbox().unwrap();
        for i in 0..n {
            let text = format!("Note {i}: the word{i} of the day.");
            mb.add_message(Message { folder, date: i as i64, ..message(0, &format!("Item {i}"), "Ada", &text) }).unwrap();
        }
        crate::store::save(&path, &mb).unwrap();
        path
    }

    fn count(s: &Session, query: &str) -> usize {
        let words = s.words().unwrap();
        let guard = words.lock();
        let index = guard.as_ref().unwrap();
        found(index, query, words::ALL).len()
    }

    #[test]
    fn the_index_is_built_in_the_background_and_says_how_far_it_has_got() {
        let tmp = Temp::new("background");
        let path = many(&tmp, 3_000);
        let mut s = session(&path);
        assert_eq!(s.word_index(), WordIndex::Off, "nothing until the list is first shown");
        let _ = s.rows();
        assert!(matches!(s.word_index(), WordIndex::Building { total: 3_000, .. } | WordIndex::Complete), "{:?}", s.word_index());
        s.wait_for_word_index();
        assert_eq!(s.word_index(), WordIndex::Complete);
        assert_eq!(count(&s, "note"), 3_000);
        assert_eq!(count(&s, "word2999"), 1);
        // Stopped part of the way (the window closed): the next time it goes on from there.
        drop(s);
        std::fs::remove_file(crate::store::words_file(&path)).unwrap();
        let mut s = session(&path);
        let _ = s.rows();
        drop(s);
        let mut s = session(&path);
        s.wait_for_word_index();
        assert_eq!((s.word_index(), count(&s, "note")), (WordIndex::Complete, 3_000));
    }

    #[test]
    fn a_whole_download_that_arrives_is_read_in_the_background_too() {
        let tmp = Temp::new("download");
        let path = many(&tmp, 10);
        let mut s = session(&path);
        s.wait_for_word_index();
        assert_eq!(s.word_index(), WordIndex::Complete);
        let folder = s.mailbox.inbox().unwrap();
        for i in 0..READ_NOW_MOST + 10 {
            s.mailbox.add_message(Message { folder, ..message(0, "Arrived", "Kit", &format!("Download {i}")) }).unwrap();
        }
        s.mailbox_changed();
        s.view.search = "download".into();
        let _ = s.rows();
        assert!(matches!(s.word_index(), WordIndex::Building { .. }), "left to the background: {:?}", s.word_index());
        s.wait_for_word_index();
        assert_eq!((s.word_index(), count(&s, "download")), (WordIndex::Complete, READ_NOW_MOST + 10));
    }
}

// ---- a search answered from it ----

mod answered {
    use std::time::Duration;

    use mailcraft_model::{FolderKind, Mailbox};
    use serde_json::json;

    use super::in_step::{id_of, session};
    use super::*;
    use crate::{Scope, Session, WordIndex};

    /// A mailbox whose texts are kept outside the mailbox file, with words to find in every part.
    fn mailbox(tmp: &Temp) -> PathBuf {
        let path = tmp.0.join("mailbox.mailcraft");
        let mut mb = Mailbox::with_local_account("Avery", "avery@mail.example");
        let inbox = mb.inbox().unwrap();
        let junk = mb.folders.iter().find(|f| f.kind == FolderKind::Junk).unwrap().id;
        let filler = " Nothing else of note this week, all is well with the project.".repeat(30);
        let mut add = |folder: Id, subject: &str, from: &str, text: &str, file: Option<&str>| {
            let mut m = Message { folder, date: mb.messages.len() as i64, ..message(0, subject, from, &format!("{text}{filler}")) };
            m.attachments = file.map(|f| vec![Attachment::new(f, "application/pdf", vec![7; 10])]).unwrap_or_default();
            m.read = subject.starts_with('R');
            mb.add_message(m).unwrap();
        };
        add(inbox, "Meeting on Tuesday", "Ada", "The budget review meets at noon in the Gvožđar room.", None);
        add(inbox, "Re: Budget", "Bram", "Numbers attached. Café at three? Review budget later.", Some("budget-Q4.pdf"));
        add(inbox, "Figures", "Kit", "See the attached estimate.", Some("estimate.xlsx"));
        add(inbox, "Picnic", "Đorđe", "Bring sandwiches and the meeting notes.", None);
        add(junk, "Cheap budget", "Spam", "A budget review you can't refuse.", None);
        add(inbox, "[IQA] Audit dates", "Stuart", "Dates for the visit.", None);
        add(inbox, "IQA-2026 sign-off", "Stuart", "Signed.", None);
        add(inbox, "RE: IQA", "Kit", "Fine by me.", None);
        crate::store::save(&path, &mb).unwrap();
        path
    }

    fn found(s: &mut Session, query: &str, scope: Scope) -> Vec<String> {
        s.view.search = query.into();
        s.view.scope = scope;
        let rows = s.rows();
        let mut subjects: Vec<String> = rows.iter().map(|r| s.mailbox.message(r.id).unwrap().subject.clone()).collect();
        subjects.sort();
        subjects
    }

    const QUERIES: &[&str] = &[
        "budget",
        "budg",
        "udget",
        "BUDGET review",
        "\"budget review\"",
        "\"review budget\"",
        "\"budget rev\"",
        "gvozdar",
        "dorde",
        "from:đorđe",
        "cafe",
        "café",
        "meet",
        "subject:meet",
        "body:meet",
        "-budget",
        "budget -meeting",
        "-\"budget review\"",
        "attachment:estimate",
        "attachment:q4",
        "has:attachment budget",
        "is:unread budget",
        "is:read",
        "to:avery",
        "from:\"ada\" to:avery",
        "zyxwvut",
        "category:red",
        "- @ ...",
        "on:1970-01-01 sandwiches",
        // #194: a space after a keyword's colon, and a keyword or a word half typed.
        "from: stuart",
        "from: stuart su",
        "from: stuart subject:",
        "from: stuart subject: iqa",
        "from: stuart subject: i",
        "subject:iqa",
        "subject: IQA -from: kit",
        "re: iqa",
        "from:",
    ];

    /// Every query finds through the index what reading every message finds.
    #[test]
    fn the_index_finds_what_reading_every_message_finds() {
        let tmp = Temp::new("same");
        let path = mailbox(&tmp);
        let mut indexed = session(&path);
        indexed.wait_for_word_index();
        assert_eq!(indexed.word_index(), WordIndex::Complete);
        let mut read = session(&path);
        read.index_words = false;
        read.search_time = Duration::MAX;
        for scope in [Scope::Folder, Scope::Everywhere] {
            for q in QUERIES {
                let want = found(&mut read, q, scope);
                assert_eq!(found(&mut indexed, q, scope), want, "{q} ({scope:?})");
                assert_eq!(indexed.unsearched, 0, "{q}");
            }
        }
        // A sample of what that means.
        assert_eq!(found(&mut indexed, "budg", Scope::Everywhere), ["Meeting on Tuesday", "Re: Budget"]);
        assert_eq!(found(&mut indexed, "\"budget review\"", Scope::Folder), ["Meeting on Tuesday"]);
        assert_eq!(found(&mut indexed, "gvozdar", Scope::Folder), ["Meeting on Tuesday"]);
        assert!(found(&mut indexed, "udget", Scope::Everywhere).is_empty());
        // #194: `from: stuart subject: iqa` is `from:stuart subject:iqa`, found by either way.
        for s in [&mut indexed, &mut read] {
            let stuart = ["IQA-2026 sign-off", "[IQA] Audit dates"];
            assert_eq!(found(s, "from: stuart subject: iqa", Scope::Everywhere), stuart);
            assert_eq!(found(s, "from: stuart subject:", Scope::Everywhere), stuart);
            assert_eq!(found(s, "from: stuart si", Scope::Everywhere), ["IQA-2026 sign-off"]);
            assert_eq!(found(s, "subject: iqa", Scope::Folder), ["IQA-2026 sign-off", "RE: IQA", "[IQA] Audit dates"]);
        }
    }

    /// Searching with a complete index reads no body into memory, and says nothing was left
    /// unsearched; while it is built, what it hasn't got is searched as before.
    #[test]
    fn a_complete_index_answers_without_reading_the_messages() {
        let tmp = Temp::new("no-reading");
        let path = mailbox(&tmp);
        let mut s = session(&path);
        s.wait_for_word_index();
        let loads = s.loads();
        assert_eq!(found(&mut s, "sandwiches", Scope::Everywhere), ["Picnic"]);
        assert_eq!((s.loads(), s.unsearched), (loads, 0), "no body brought in");
        assert!(s.mailbox.messages.values().all(|m| m.body_is_kept()));
        // A phrase in a text kept on disk is checked on that text, still without bringing it in.
        assert_eq!(found(&mut s, "\"meeting notes\"", Scope::Everywhere), ["Picnic"]);
        assert_eq!(s.loads(), loads);
        // Out of time for checking it: counted, not found.
        s.search_time = Duration::ZERO;
        assert!(found(&mut s, "\"the meeting\"", Scope::Everywhere).is_empty());
        assert_eq!(s.unsearched, 2, "two hold both words");
    }

    #[test]
    fn a_message_changed_since_the_index_was_built_is_found_as_it_is_now() {
        let tmp = Temp::new("changed");
        let path = mailbox(&tmp);
        let mut s = session(&path);
        s.wait_for_word_index();
        let picnic = id_of(&s, "Picnic");
        s.run("mail.deletePermanently", &json!({"ids": [picnic]})).unwrap();
        assert!(found(&mut s, "sandwiches", Scope::Everywhere).is_empty());
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(found(&mut s, "sandwiches", Scope::Everywhere), ["Picnic"]);
        let m = s.mailbox.message_mut(picnic).unwrap();
        m.subject = "Barbecue".into();
        s.mailbox_changed();
        assert_eq!(found(&mut s, "barbecue sandwiches", Scope::Everywhere), ["Barbecue"]);
        assert!(found(&mut s, "picnic", Scope::Everywhere).is_empty());
    }

    /// Hostile queries through the index: never a panic.
    #[test]
    fn hostile_queries_never_panic() {
        let tmp = Temp::new("hostile");
        let path = mailbox(&tmp);
        let mut s = session(&path);
        s.wait_for_word_index();
        for q in [
            "\"",
            "\"\"\"",
            "-",
            "--",
            "-\"\"",
            "from:",
            ":::",
            "\u{0}\u{301}\u{301}",
            &"a ".repeat(5_000),
            &"\"x y ".repeat(400),
            "会議 -会",
            "İİİ ß",
        ] {
            let _ = s.run("view.search", &json!({"query": q, "scope": "everywhere"}));
            let _ = s.rows();
        }
    }
}

/// What the sender hid is part of what the index read of a message (#599): a message that gains
/// or loses it is read again, and one that hid nothing has the fingerprint it always had.
#[test]
fn the_fingerprint_follows_what_the_sender_hid() {
    let plain = Message { subject: "s".into(), body: Body { text: "t".into(), html: None }, ..Default::default() };
    let hiding = Message { hidden_text: "preview line".into(), ..plain.clone() };
    let other = Message { hidden_text: "another line".into(), ..plain.clone() };
    assert_ne!(fingerprint(&plain), fingerprint(&hiding));
    assert_ne!(fingerprint(&hiding), fingerprint(&other));
    assert_eq!(fingerprint(&plain), fingerprint(&Message { hidden_text: String::new(), ..plain.clone() }));
}
