//! What is here and not on the server yet goes up (drafts, copies, mail brought in from files),
//! categories go up and come down as keywords, and a connection waits on the Inbox for news.

use mailcraft_model::sync::{CategoriesPushed, RemoteCategories, Upload, Uploaded};

use super::*;
use crate::idle::{IdleEnd, IdleOptions, watch_with};

const DRAFTS: Id = 101;
const INBOX: Id = 100;

/// A message written here, by its id and the folder it is in.
fn written(id: Id, folder: Id, subject: &str, message_id: &str) -> Message {
    Message {
        id,
        folder,
        message_id: message_id.into(),
        from: Address::new("Ada Lovelace", "ada@example.org"),
        to: vec![Address::new("Bob", "bob@example.org")],
        subject: subject.into(),
        date: SENT_AT,
        body: mailcraft_model::Body { text: format!("The text of {subject}."), html: None },
        read: true,
        ..Default::default()
    }
}

fn draft(id: Id, subject: &str, message_id: &str) -> Message {
    Message { draft: true, ..written(id, DRAFTS, subject, message_id) }
}

fn upload(message: Message, to: &str) -> Upload {
    Upload { message, to: to.into(), copy_from: String::new(), copy_uid: 0, copy_only: false }
}

fn uploading(imap: &FakeImap, folders: &[FolderState], uploads: Vec<Upload>, ops: Vec<RemoteOp>) -> SyncRequest {
    SyncRequest { uploads, ops, ..request(account(Some(imap), None), folders.to_vec()) }
}

/// The local folder that follows the server's drafts once the first run is applied.
fn drafts_of(folders: &[FolderState]) -> Id {
    folders.iter().find(|f| f.remote == "Entw&APw-rfe" || f.remote == "Entwürfe").map(|f| f.id).unwrap()
}

fn drafts_remote(folders: &[FolderState]) -> String {
    folders.iter().find(|f| f.id == drafts_of(folders)).unwrap().remote.clone()
}

#[test]
fn a_draft_written_here_goes_to_the_servers_drafts_and_is_replaced_when_saved_again() {
    for uidplus in [true, false] {
        let imap = two_way(true, uidplus);
        let folders = synced_folders(&imap);
        assert_eq!(drafts_of(&folders), DRAFTS);
        let remote = drafts_remote(&folders);
        let first = draft(801, "Plans", "plans-1@example.org");
        let result = sync(&uploading(&imap, &folders, vec![upload(first, &remote)], Vec::new()));
        assert_eq!(result.errors, Vec::<String>::new(), "UIDPLUS {uidplus}");
        // Marked as a draft that is read, and tied to the message here by its UID when the server says.
        assert_eq!(on_server(&imap, "Entw&APw-rfe"), [(1, "\\Seen \\Draft".into())]);
        let uid = if uidplus { 1 } else { 0 };
        assert_eq!(
            result.uploaded,
            [Uploaded {
                message: 801,
                folder: DRAFTS,
                to: remote.clone(),
                uid,
                uid_validity: if uidplus { 41 } else { 0 },
                message_id: "plans-1@example.org".into(),
                flags: RemoteFlags { uid, read: true, draft: true, ..Default::default() },
                copied: false,
            }]
        );
        // With its UID known it isn't downloaded again; without, it comes down to be matched by its Message-ID.
        let came: Vec<&str> = delta(&result, &remote).new.iter().map(|m| m.message_id.as_str()).collect();
        assert_eq!(came, if uidplus { vec![] } else { vec!["plans-1@example.org"] });
        // Saved again here: the new one goes up first, then the old one is taken away by its UID,
        // and nothing else in the mailbox is.
        let mut folders = applied(&folders, &result);
        if !uidplus {
            folders.iter_mut().find(|f| f.id == DRAFTS).unwrap().known_uids = vec![1];
        }
        imap.world().mailbox("Entw&APw-rfe").messages.push(message(7, "\\Deleted", "Someone else's"));
        let second = draft(801, "Plans, better", "plans-2@example.org");
        let again = uploading(&imap, &without(folders.clone(), &remote, &[1]), vec![upload(second, &remote)], vec![deleting(&remote, 1)]);
        let result = sync(&again);
        assert_eq!(result.errors, Vec::<String>::new());
        assert_eq!(result.deleted, [deleted(&remote, 1)]);
        let subjects: Vec<(u32, String)> = imap
            .world()
            .mailbox("Entw&APw-rfe")
            .messages
            .iter()
            .map(|m| (m.uid, String::from_utf8_lossy(&m.body).lines().find(|l| l.starts_with("Subject:")).unwrap().to_string()))
            .collect();
        if uidplus {
            assert_eq!(subjects, [(7, "Subject: Someone else's".into()), (8, "Subject: Plans, better".into())]);
        } else {
            // Without UID EXPUNGE the old one is only marked: no program shows it.
            assert_eq!(on_server(&imap, "Entw&APw-rfe").iter().filter(|(_, f)| !f.contains("\\Deleted")).count(), 1);
            assert_eq!(subjects.len(), 3);
        }
        let order: Vec<String> =
            changes(&imap).into_iter().filter(|c| c.starts_with("APPEND") || c.contains("EXPUNGE") || c.contains("Deleted")).collect();
        assert!(order[0].starts_with("APPEND \"Entw&APw-rfe\" (\\Seen \\Draft)"), "{order:?}");
        assert!(
            order.iter().position(|c| c.contains("plans") || c.starts_with("APPEND")).unwrap()
                < order.iter().position(|c| c.contains("Deleted")).unwrap()
        );
        assert!(never_swept(&imap));
    }
}

#[test]
fn nothing_goes_up_twice() {
    // A run that put the message there and was cut off before the mailbox heard: the next one
    // finds it by its Message-ID instead of putting it there again.
    let imap = two_way(true, false);
    let folders = synced_folders(&imap);
    let remote = drafts_remote(&folders);
    let request = uploading(&imap, &folders, vec![upload(draft(801, "Plans", "plans-1@example.org"), &remote)], Vec::new());
    let first = sync(&request);
    assert_eq!((first.errors.len(), first.uploaded[0].uid, sent(&imap, "APPEND")), (0, 0, 1));
    let second = sync(&request);
    assert_eq!(second.errors, Vec::<String>::new());
    assert_eq!((second.uploaded[0].uid, second.uploaded[0].uid_validity, sent(&imap, "APPEND")), (1, 41, 1));
    assert_eq!(server_uids(&imap, "Entw&APw-rfe"), [1]);
    // Mail brought in from a file that the server has already is the server's copy too; but
    // never one that a message here already is.
    let inbox = uploading(&imap, &folders, vec![upload(written(802, INBOX, "One again", "one@example.org"), "INBOX")], Vec::new());
    let result = sync(&inbox);
    assert_eq!((result.errors.len(), sent(&imap, "APPEND")), (0, 2));
    assert_eq!(server_uids(&imap, "INBOX"), [1, 2, 5, 6, 7]);
}

#[test]
fn mail_brought_in_goes_up_and_a_message_without_an_id_gets_one() {
    let imap = two_way(true, true);
    let folders = synced_folders(&imap);
    let imported = Message { read: false, flag: FlagStatus::Flagged, ..written(803, PROJECTS, "From a file", "") };
    let result = sync(&uploading(&imap, &folders, vec![upload(imported, "Projects")], Vec::new()));
    assert_eq!(result.errors, Vec::<String>::new());
    let up = &result.uploaded[0];
    assert!(up.message_id.starts_with("mc.") && up.message_id.ends_with("@example.org"), "{}", up.message_id);
    assert_eq!((up.uid, up.flags.read, up.flags.flagged), (2, false, true));
    assert_eq!(on_server(&imap, "Projects"), [(1, String::new()), (2, "\\Flagged".into())]);
    let body = String::from_utf8_lossy(&imap.world().mailbox("Projects").messages[1].body).into_owned();
    assert!(body.contains(&format!("Message-ID: <{}>", up.message_id)), "{body}");
    // Too large: said, and the rest goes on. Into a folder the server doesn't have: it waits, without a word.
    let huge = Message {
        body: mailcraft_model::Body { text: "x".repeat(26 * 1024 * 1024), html: None },
        ..written(804, PROJECTS, "Huge", "huge@example.org")
    };
    let nowhere = written(805, 999, "Nowhere", "nowhere@example.org");
    let fine = written(806, PROJECTS, "Fine", "fine@example.org");
    let result = sync(&uploading(&imap, &folders, vec![upload(huge, "Projects"), upload(nowhere, "Gone"), upload(fine, "Projects")], Vec::new()));
    assert_eq!(result.uploaded.iter().map(|u| u.message).collect::<Vec<_>>(), [806]);
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
}

#[test]
fn a_copy_of_a_message_on_the_server_is_copied_there() {
    let imap = two_way(true, true);
    let folders = synced_folders(&imap);
    let copy = |id: Id, uid: u32, only: bool| Upload {
        message: written(id, PROJECTS, "One", "one@example.org"),
        to: "Projects".into(),
        copy_from: "INBOX".into(),
        copy_uid: uid,
        copy_only: only,
    };
    let result = sync(&uploading(&imap, &folders, vec![copy(810, 1, false)], Vec::new()));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!((result.uploaded[0].uid, result.uploaded[0].uid_validity, result.uploaded[0].copied), (2, 31, true));
    assert_eq!(changes(&imap), ["UID COPY 1 \"Projects\""]);
    assert_eq!(server_uids(&imap, "INBOX"), [1, 2, 5, 6]);
    // What it was copied from is gone: put there from here, unless what is here isn't all of it.
    let result = sync(&uploading(&imap, &folders, vec![copy(811, 3, false), copy(812, 4, true)], Vec::new()));
    assert_eq!(result.uploaded.iter().map(|u| (u.message, u.copied)).collect::<Vec<_>>(), [(811, false)]);
    assert_eq!(result.errors, ["Couldn't copy \"One\" on the server: the message it was copied from is no longer there"]);
}

#[test]
fn categories_go_up_as_keywords_and_come_down_as_categories() {
    let imap = two_way(true, true);
    imap.world().mailbox("INBOX").messages[0].flags = "\\Seen Work $Forwarded Junk Red_category".into();
    let first = sync(&request(account(Some(&imap), None), local_folders()));
    let one = delta(&first, "INBOX").new.iter().find(|m| m.uid == 1).unwrap();
    assert_eq!(
        (one.categories.clone(), one.synced_categories.clone()),
        (vec!["Work".to_string(), "Red category".into()], vec!["Work".to_string(), "Red category".into()])
    );
    let folders = applied(&local_folders(), &first);
    // Given and taken here.
    let ops = vec![
        RemoteOp::Categories { folder: "INBOX".into(), uid: 1, add: vec!["Blue category".into()], remove: vec!["Work".into()] },
        RemoteOp::Categories { folder: "INBOX".into(), uid: 2, add: vec!["Blue category".into(), "Größe".into()], remove: vec![] },
    ];
    let result = sync(&pushing(&imap, &folders, ops.clone()));
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(
        result.categorized,
        [
            CategoriesPushed { folder: "INBOX".into(), uid: 1, add: vec!["Blue category".into()], remove: vec!["Work".into()] },
            CategoriesPushed { folder: "INBOX".into(), uid: 2, add: vec!["Blue category".into()], remove: vec![] },
        ]
    );
    assert_eq!(changes(&imap), ["UID STORE 1:2 +FLAGS.SILENT (Blue_category)", "UID STORE 1 -FLAGS.SILENT (Work)"]);
    assert_eq!(on_server(&imap, "INBOX")[0].1, "\\Seen $Forwarded Junk Red_category Blue_category");
    // What is read afterwards shows them: for the messages here, only those that have any.
    assert_eq!(
        delta(&result, "INBOX").categories,
        [
            RemoteCategories { uid: 1, names: vec!["Red category".into(), "Blue category".into()] },
            RemoteCategories { uid: 2, names: vec!["Blue category".into()] }
        ]
    );
    // A server that keeps no keywords of the owner's (Exchange Online) is told nothing, and the
    // answer says so without a word of complaint.
    imap.world().permanent_flags = Some("\\Seen \\Answered \\Flagged \\Deleted \\Draft $MDNSent".into());
    let stores = sent(&imap, "STORE");
    let result = sync(&pushing(&imap, &folders, ops));
    assert_eq!((result.errors.len(), result.categorized.len(), result.keywords_refused.as_slice()), (0, 0, &["INBOX".to_string()][..]));
    assert_eq!(sent(&imap, "STORE"), stores);
}

/// A watch on the Inbox of a server that can IDLE, on a thread of its own: what it says, and
/// how many times it had news.
struct Watch {
    stop: std::sync::Arc<AtomicBool>,
    news: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    end: std::thread::JoinHandle<IdleEnd>,
}

fn watch(imap: &FakeImap, options: IdleOptions) -> Watch {
    let request = request(account(Some(imap), None), Vec::new());
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let news = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (stopping, counting) = (stop.clone(), news.clone());
    let end = std::thread::spawn(move || {
        watch_with(&request, &stopping, options, &mut || {
            counting.fetch_add(1, Ordering::SeqCst);
        })
    });
    Watch { stop, news, end }
}

fn quick() -> IdleOptions {
    IdleOptions {
        renew: Duration::from_secs(60),
        tick: Duration::from_millis(20),
        first_retry: Duration::from_millis(50),
        last_retry: Duration::from_millis(200),
    }
}

/// Wait up to five seconds for `done`.
fn soon(done: impl Fn() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    done()
}

#[test]
fn news_on_the_inbox_asks_for_a_run() {
    let imap = Fake::imap(ImapWorld { idle: true, ..imap_world() });
    let w = watch(&imap, quick());
    assert!(soon(|| imap.world().idling == 1));
    // Looked at only.
    assert!(imap.world().log.iter().any(|line| line.ends_with("EXAMINE \"INBOX\"")));
    imap.world().idle_news = vec!["* 5 EXISTS".into()];
    assert!(soon(|| w.news.load(Ordering::SeqCst) == 1));
    // What isn't news of the Inbox asks for nothing.
    imap.world().idle_news = vec!["* OK still here".into()];
    std::thread::sleep(Duration::from_millis(100));
    imap.world().idle_news = vec!["* 2 EXPUNGE".into(), "* 3 FETCH (FLAGS (\\Seen))".into()];
    assert!(soon(|| w.news.load(Ordering::SeqCst) == 3));
    // Told to stop: it says DONE and goodbye.
    w.stop.store(true, Ordering::SeqCst);
    assert_eq!(w.end.join().unwrap(), IdleEnd::Stopped);
    let log = imap.world().log.clone();
    assert!(log.iter().any(|line| line == "DONE") && log.iter().any(|line| line.ends_with("LOGOUT")), "{log:?}");
    assert_eq!(imap.world().idling, 0);
    // Nothing on that connection changed anything.
    assert!(changes(&imap).is_empty());
}

#[test]
fn an_idle_is_begun_again_before_the_server_gives_up_on_it() {
    let imap = Fake::imap(ImapWorld { idle: true, ..imap_world() });
    let w = watch(&imap, IdleOptions { renew: Duration::from_millis(150), ..quick() });
    assert!(soon(|| imap.world().sent("IDLE") >= 3));
    assert!(imap.world().log.iter().filter(|line| *line == "DONE").count() >= 2);
    // One connection all along.
    assert_eq!(imap.world().connections, 1);
    w.stop.store(true, Ordering::SeqCst);
    assert_eq!(w.end.join().unwrap(), IdleEnd::Stopped);
    assert!(IdleOptions::default().renew <= Duration::from_secs(29 * 60));
}

#[test]
fn a_lost_connection_is_made_again_and_a_server_without_idle_is_left_alone() {
    // Cut off: made again, after a wait.
    let imap = Fake::imap(ImapWorld { idle: true, hang_up_on: Some("IDLE".into()), ..imap_world() });
    let w = watch(&imap, quick());
    assert!(soon(|| imap.world().connections >= 3));
    imap.world().hang_up_on = None;
    assert!(soon(|| imap.world().idling == 1));
    imap.world().idle_news = vec!["* 9 EXISTS".into()];
    assert!(soon(|| w.news.load(Ordering::SeqCst) == 1));
    w.stop.store(true, Ordering::SeqCst);
    assert_eq!(w.end.join().unwrap(), IdleEnd::Stopped);
    // No IDLE: nothing to wait on.
    let plain = Fake::imap(imap_world());
    assert_eq!(watch(&plain, quick()).end.join().unwrap(), IdleEnd::NotOffered);
    assert_eq!(plain.world().sent("IDLE"), 0);
    // A sign-in that is refused ends it, without the password.
    let refused = Fake::imap(ImapWorld { idle: true, password: "changed".into(), ..imap_world() });
    let IdleEnd::SignIn(why) = watch(&refused, quick()).end.join().unwrap() else { panic!("not a refused sign-in") };
    assert!(!why.contains(PASSWORD), "{why}");
    assert_eq!(refused.world().connections, 1);
}
