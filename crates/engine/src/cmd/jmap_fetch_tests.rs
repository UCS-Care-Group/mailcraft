//! A JMAP account's messages fetched whole: which ones a host is asked for (the one opened at once, the
//! Inbox's newest in the background), what is taken in, and what is passed over.

use mailcraft_model::sync::{FolderListing, GraphFetched, GraphFolder, GraphMessage, JmapMailResult, MessagePage, ServerMessage};
use mailcraft_model::{Address, DownloadInFull};
use serde_json::json;

use super::super::jmap_mail::apply_jmap_mail;
use super::*;

const ME: &str = "ada@example.org";
const SESSION: &str = "https://mail.example.org/jmap/session?account=ada@example.org";

/// A session with one JMAP account whose Inbox and Projects folder are the server's; its id.
fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": ME, "kind": "jmap", "jmap": {"session": SESSION, "bearer": true}})).unwrap();
    let account = added["id"].as_u64().unwrap();
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    let folders = vec![folder("Min", "INBOX", FolderKind::Inbox), folder("Mpr", "Projects", FolderKind::Custom)];
    let result = JmapMailResult { account, email: ME.into(), listing: Some(FolderListing { folders, complete: true }), ..Default::default() };
    apply_jmap_mail(&mut s, result).unwrap();
    (s, account)
}

fn mail(id: &str, received: i64, size: u64) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), size, ..Default::default() },
        message_id: format!("<{id}@mail.example.org>"),
        subject: format!("Subject of {id}"),
        from: Address { name: "Grace".into(), email: "grace@example.org".into() },
        sent: received,
        received,
        preview: format!("Preview of {id}"),
        ..Default::default()
    }
}

/// `n` messages in the mailbox `mailbox`, `prefix1` (oldest) to `prefix<n>` (newest), as one complete window.
fn fill(s: &mut Session, account: Id, mailbox: &str, prefix: &str, n: i64) {
    let messages = (1..=n).map(|i| mail(&format!("{prefix}{i}"), 1_790_000_000 + i, 1_000)).collect();
    let page = MessagePage { folder: mailbox.into(), messages, afresh: true, ..Default::default() };
    apply_jmap_mail(s, JmapMailResult { account, email: ME.into(), pages: vec![page], whole: true, ..Default::default() }).unwrap();
}

/// `message` joins the mailbox `mailbox` as a window cut short, which removes nothing.
fn add(s: &mut Session, account: Id, mailbox: &str, message: GraphMessage) {
    let page = MessagePage { folder: mailbox.into(), messages: vec![message], ..Default::default() };
    apply_jmap_mail(s, JmapMailResult { account, email: ME.into(), pages: vec![page], ..Default::default() }).unwrap();
}

fn mime(id: &str, text: &str) -> Vec<u8> {
    format!(
        "From: Grace <grace@example.org>\r\nTo: {ME}\r\nSubject: Whole {id}\r\nMessage-ID: <{id}@mail.example.org>\r\nDate: Wed, 14 Oct 2026 09:00:00 +0000\r\n\
MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n"
    )
    .into_bytes()
}

fn fetched(account: Id, ids: &[&str]) -> GraphFetchResult {
    let fetched = ids.iter().map(|id| GraphFetched { id: (*id).into(), mime: mime(id, &format!("The words of {id}.")) }).collect();
    GraphFetchResult { account, email: ME.into(), fetched, ..Default::default() }
}

fn message<'s>(s: &'s Session, id: &str) -> &'s Message {
    s.mailbox.messages.values().map(|m| m.as_ref()).find(|m| m.server.as_ref().is_some_and(|server| server.id == id)).unwrap()
}

fn background(s: &Session, account: Id, most: usize) -> Vec<String> {
    jmap_fetch_request(s, account, None, most).unwrap().ids
}

#[test]
fn a_jmap_account_has_a_fetch_request_of_its_own() {
    let (mut s, account) = session();
    fill(&mut s, account, "Min", "E", 3);
    let opened = message(&s, "E2").id;
    let request = jmap_fetch_request(&s, account, Some(opened), 1).unwrap();
    // The message, and where the server is: all a host needs to connect, and no secret.
    assert_eq!((request.account, request.email.as_str(), request.session.as_str(), request.bearer), (account, ME, SESSION, true));
    assert_eq!((request.ids, request.background), (vec!["E2".to_string()], false));
    // The same through the command, as a host that has only the commands asks.
    let answer = s.run("sync.jmapFetchRequest", &json!({"message": opened})).unwrap();
    assert_eq!((answer["ids"].clone(), answer["session"].as_str(), answer["bearer"].as_bool()), (json!(["E2"]), Some(SESSION), Some(true)));
    assert_eq!(answer.get("background"), None, "what is false is not said");
    let answer = s.run("sync.jmapFetchRequest", &json!({"account": account, "most": 2})).unwrap();
    assert_eq!((answer["ids"].clone(), answer["background"].as_bool()), (json!(["E3", "E2"]), Some(true)));
    // One that is here whole is nothing to fetch; one with no id the server can be asked about, neither.
    s.run("sync.applyJmapFetch", &json!({"result": fetched(account, &["E2"])})).unwrap();
    assert!(jmap_fetch_request(&s, account, Some(opened), 1).unwrap().ids.is_empty());
    let nameless = message(&s, "E3").id;
    s.mailbox.message_mut(nameless).unwrap().server.as_mut().unwrap().id = String::new();
    assert!(jmap_fetch_request(&s, account, Some(nameless), 1).unwrap().ids.is_empty());
    s.mailbox.message_mut(nameless).unwrap().server.as_mut().unwrap().id = "a b/c".into();
    assert!(jmap_fetch_request(&s, account, Some(nameless), 1).unwrap().ids.is_empty());
    // Not for the address, the server or the ids the request is made of.
    let shown = format!("{:?} {:#?}", jmap_fetch_request(&s, account, None, 5).unwrap(), jmap_fetch_request(&s, account, Some(opened), 1).unwrap());
    assert!(!shown.contains('@') && !shown.contains("example.org") && !shown.contains("E1"), "{shown}");
}

#[test]
fn a_fetch_request_is_for_a_jmap_account_and_a_message_of_it() {
    let (mut s, account) = session();
    fill(&mut s, account, "Min", "E", 2);
    let mine = message(&s, "E1").id;
    let imap = s.run("account.add", &json!({"email": "bob@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    let theirs = s
        .mailbox
        .add_message(Message { account: imap, folder: s.mailbox.folder_of_kind(imap, FolderKind::Inbox).unwrap(), ..Default::default() })
        .unwrap();
    assert!(jmap_fetch_request(&s, imap, None, 5).is_err(), "not a JMAP account");
    assert!(jmap_fetch_request(&s, 9_999, None, 5).is_err(), "no such account");
    assert!(jmap_fetch_request(&s, account, Some(theirs), 1).is_err(), "a message of another account");
    assert!(jmap_fetch_request(&s, account, Some(9_999), 1).is_err(), "no such message");
    assert!(jmap_fetch_request(&s, account, Some(mine), 1).is_ok());
    // The mailbox file may say anything of the server: it is looked at again.
    for bad in ["", "ftp://mail.example.org/s", "http://mail.example.org/s", "https://u:p@mail.example.org/s", "https://mail.example.org/\u{0}"] {
        s.mailbox.account_mut(account).unwrap().jmap.as_mut().unwrap().session = bad.into();
        assert!(jmap_fetch_request(&s, account, Some(mine), 1).is_err(), "{bad:?}");
    }
    s.mailbox.account_mut(account).unwrap().jmap = None;
    assert!(jmap_fetch_request(&s, account, Some(mine), 1).is_err());
    // The command is there for an account of the kind, and the other kinds' are not its.
    assert!(s.run("sync.jmapFetchRequest", &json!({"account": imap})).is_err());
    let mut local = Session::empty();
    assert!(!local.enabled("sync.jmapFetchRequest") && local.enabled("sync.applyJmapFetch"));
    assert!(local.run("sync.jmapFetchRequest", &json!({})).is_err());
}

#[test]
fn the_background_fetches_the_inboxs_newest_forty_and_nothing_else() {
    let (mut s, account) = session();
    fill(&mut s, account, "Min", "E", 60);
    fill(&mut s, account, "Mpr", "P", 5);
    // A message of another folder that is newer than every one of the Inbox's is not among the Inbox's newest.
    add(&mut s, account, "Mpr", mail("PNEW", 1_790_001_000, 1_000));
    // Newest first, a few at a time.
    let first = background(&s, account, 10);
    assert_eq!(first, (51..=60).rev().map(|i| format!("E{i}")).collect::<Vec<_>>());
    let ids: Vec<&str> = first.iter().map(String::as_str).collect();
    let answer = apply_jmap_fetch(&mut s, fetched(account, &ids)).unwrap();
    assert_eq!(answer["fetched"], 10);
    assert!(!message(&s, "E60").unfetched && message(&s, "E50").unfetched);
    // What is fetched is not asked for again; four batches make forty, and that is all.
    let mut asked = first.len();
    loop {
        let next = background(&s, account, 10);
        if next.is_empty() {
            break;
        }
        asked += next.len();
        assert!(asked <= PREFETCH_NEWEST, "{asked}");
        let ids: Vec<&str> = next.iter().map(String::as_str).collect();
        apply_jmap_fetch(&mut s, fetched(account, &ids)).unwrap();
    }
    assert_eq!(asked, PREFETCH_NEWEST);
    // The twenty older, and the other folder, wait until they are opened.
    assert!(!message(&s, "E21").unfetched && message(&s, "E20").unfetched && message(&s, "P5").unfetched && message(&s, "PNEW").unfetched);
    let older = message(&s, "E20").id;
    assert_eq!(jmap_fetch_request(&s, account, Some(older), 1).unwrap().ids, ["E20"]);
    let projects = message(&s, "P5").id;
    assert_eq!(jmap_fetch_request(&s, account, Some(projects), 1).unwrap().ids, ["P5"]);
    // A new message comes in front of the forty: it is the next, and the oldest of them leaves the scope.
    add(&mut s, account, "Min", mail("N1", 1_790_000_999, 1_000));
    assert_eq!(background(&s, account, 10), ["N1"]);
}

#[test]
fn how_many_the_background_asks_for_is_held_to_what_a_request_may_name() {
    let (mut s, account) = session();
    fill(&mut s, account, "Min", "E", 60);
    // Nothing asked for is one, and anything beyond is the scope: forty.
    assert_eq!(background(&s, account, 0).len(), 1);
    assert_eq!(background(&s, account, 1).len(), 1);
    assert_eq!(background(&s, account, 100).len(), PREFETCH_NEWEST);
    assert_eq!(background(&s, account, usize::MAX).len(), PREFETCH_NEWEST);
    let answer = s.run("sync.jmapFetchRequest", &json!({"most": 1e300})).unwrap();
    assert_eq!(answer["ids"].as_array().map(Vec::len), Some(10), "a number that is none is the usual ten");
    let answer = s.run("sync.jmapFetchRequest", &json!({"most": 5})).unwrap();
    assert_eq!(answer["ids"].as_array().map(Vec::len), Some(5));
    // No Inbox that the server has: nothing to look after.
    let (empty, account) = session();
    assert!(background(&empty, account, 10).is_empty());
}

#[test]
fn the_background_leaves_out_what_is_large_gone_incomplete_or_passed_over() {
    let (mut s, account) = session();
    let sizes = [("E1", 1_000), ("E2", 30 << 20), ("E3", 1_000), ("E4", 1_000), ("E5", 1_000), ("E6", 0)];
    let messages = sizes.iter().enumerate().map(|(i, (id, size))| mail(id, 1_790_000_000 + i as i64, *size)).collect();
    let page = MessagePage { folder: "Min".into(), messages, afresh: true, ..Default::default() };
    apply_jmap_mail(&mut s, JmapMailResult { account, email: ME.into(), pages: vec![page], whole: true, ..Default::default() }).unwrap();
    let (e3, e4) = (message(&s, "E3").id, message(&s, "E4").id);
    s.mailbox.message_mut(e3).unwrap().server.as_mut().unwrap().gone = true;
    s.mailbox.message_mut(e4).unwrap().incomplete = true;
    // A large one waits to be opened (the default is 10 MB), a gone one and an incomplete one are not there to fetch.
    assert_eq!(background(&s, account, 10), ["E6", "E5", "E1"]);
    // Everything: no limit.
    s.mailbox.account_mut(account).unwrap().download_in_full = DownloadInFull::Everything;
    assert_eq!(background(&s, account, 10), ["E6", "E5", "E2", "E1"]);
    // One the server wouldn't give is passed over for the session, and still asked for when it is opened.
    let refused = GraphFetchResult { account, email: ME.into(), failed: vec!["E5".into()], ..Default::default() };
    apply_jmap_fetch(&mut s, refused).unwrap();
    assert_eq!(background(&s, account, 10), ["E6", "E2", "E1"]);
    let e5 = message(&s, "E5").id;
    assert_eq!(jmap_fetch_request(&s, account, Some(e5), 1).unwrap().ids, ["E5"]);
}

#[test]
fn a_message_fetched_whole_is_taken_in_and_the_counts_are_those_of_what_the_background_looks_after() {
    let (mut s, account) = session();
    fill(&mut s, account, "Min", "E", 60);
    let large = message(&s, "E55").id;
    s.mailbox.message_mut(large).unwrap().server.as_mut().unwrap().size = 30 << 20;
    let answer = apply_jmap_fetch(&mut s, fetched(account, &["E60", "E59"])).unwrap();
    let m = message(&s, "E60");
    assert!(!m.unfetched && m.body.text.contains("The words of E60."), "{:?}", m.body.text);
    assert_eq!((m.subject.as_str(), m.read, m.folder), ("Whole E60", false, s.mailbox.folder_by_server(account, "Min").unwrap()));
    // Forty are looked after: two are here, thirty-eight wait and one of them is large; the sixty are not what is counted.
    assert_eq!(
        (answer["fetched"].clone(), answer["all"].clone(), answer["waiting"].clone(), answer["large"].clone()),
        (json!(2), json!(40), json!(38), json!(1))
    );
    assert_eq!((answer["missing"].clone(), answer["failed"].clone(), answer["offline"].clone()), (json!(0), json!(0), json!(false)));
    // What the host says went wrong is told back.
    let stopped =
        GraphFetchResult { account, email: ME.into(), errors: vec!["Couldn't".into()], offline: true, busy_for: Some(30), ..Default::default() };
    let answer = apply_jmap_fetch(&mut s, stopped).unwrap();
    assert_eq!((answer["errors"].clone(), answer["offline"].clone(), answer["busyFor"].clone()), (json!(["Couldn't"]), json!(true), json!(30)));
    // The message the owner opened and the others look the same to the rest of the program.
    assert!(s.mailbox.clone().repair().is_empty());
}

#[test]
fn what_the_server_no_longer_has_is_passed_over_in_the_background_and_asked_for_when_opened() {
    let (mut s, account) = session();
    fill(&mut s, account, "Min", "E", 5);
    assert_eq!(background(&s, account, 10), ["E5", "E4", "E3", "E2", "E1"]);
    let gone = GraphFetchResult {
        account,
        email: ME.into(),
        missing: vec!["E5".into(), "E3".into(), "not an id".into(), String::new()],
        ..Default::default()
    };
    let answer = apply_jmap_fetch(&mut s, gone).unwrap();
    assert_eq!(answer["missing"], 4);
    // Not asked for again by itself, and not counted among what the background still looks after...
    assert_eq!(background(&s, account, 10), ["E4", "E2", "E1"]);
    assert_eq!((answer["all"].clone(), answer["waiting"].clone()), (json!(3), json!(3)));
    // ... but still there, and still asked for when the owner opens one.
    let e5 = message(&s, "E5").id;
    assert!(message(&s, "E5").unfetched);
    assert_eq!(jmap_fetch_request(&s, account, Some(e5), 1).unwrap().ids, ["E5"]);
    // The next run lists the whole Inbox and finds the message under another id (some servers name a message by
    // where it is): it is the same message, and the new id is not one that was passed over.
    let renamed = GraphMessage { message_id: "<E5@mail.example.org>".into(), ..mail("F5", 1_790_000_005, 1_000) };
    let others = (1..=4).map(|i| mail(&format!("E{i}"), 1_790_000_000 + i, 1_000));
    let page = MessagePage { folder: "Min".into(), messages: others.chain([renamed]).collect(), afresh: true, ..Default::default() };
    apply_jmap_mail(&mut s, JmapMailResult { account, email: ME.into(), pages: vec![page], whole: true, ..Default::default() }).unwrap();
    assert_eq!(message(&s, "F5").id, e5, "the same message, named anew");
    assert!(background(&s, account, 10).contains(&"F5".to_string()));
    // However many a host names, only so many are remembered.
    let many = GraphFetchResult { account, email: ME.into(), missing: (0..5_000).map(|n| format!("X{n}")).collect(), ..Default::default() };
    let before = s.graph_unfetchable.len();
    apply_jmap_fetch(&mut s, many).unwrap();
    assert_eq!(s.graph_unfetchable.len() - before, MAX_PASSED_OVER);
}

#[test]
fn a_result_for_another_account_or_kind_is_refused_and_remembers_nothing() {
    let (mut s, account) = session();
    fill(&mut s, account, "Min", "E", 2);
    let stranger = GraphFetchResult {
        account,
        email: "eve@example.org".into(),
        fetched: vec![GraphFetched { id: "E1".into(), mime: mime("E1", "Hi") }],
        missing: vec!["E2".into()],
        ..Default::default()
    };
    assert!(apply_jmap_fetch(&mut s, stranger).is_err());
    assert!(message(&s, "E1").unfetched);
    assert!(s.graph_unfetchable.is_empty(), "nothing was passed over for an answer that was refused");
    let imap = s.run("account.add", &json!({"email": "bob@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    assert!(apply_jmap_fetch(&mut s, GraphFetchResult { account: imap, ..Default::default() }).is_err());
    assert!(apply_jmap_fetch(&mut s, GraphFetchResult { account: 9_999, ..Default::default() }).is_err());
    assert!(apply_jmap_fetch(&mut s, GraphFetchResult::default()).is_err());
    // Through the command: a result that is none is a mistake in the params, not a failure to take in.
    for junk in
        [json!({}), json!({"result": 5}), json!({"result": {"account": "x"}}), json!({"result": {"fetched": [{"id": 5}]}}), json!({"result": null})]
    {
        assert!(matches!(s.run("sync.applyJmapFetch", &junk), Err(CmdError::Params(_))), "{junk}");
    }
}

#[test]
fn junk_in_a_result_is_taken_in_without_harm() {
    let (mut s, account) = session();
    fill(&mut s, account, "Min", "E", 3);
    let before = s.mailbox.clone();
    let junk = GraphFetchResult {
        account,
        email: ME.into(),
        fetched: vec![
            GraphFetched { id: "X404".into(), mime: mime("X404", "Nobody's") },
            GraphFetched { id: String::new(), mime: vec![0xff; 64] },
            GraphFetched { id: "a b/c".into(), mime: Vec::new() },
        ],
        missing: vec!["\u{0}".into(), "x".repeat(100_000)],
        failed: vec!["\r\n".into()],
        errors: vec!["\u{0}".into(); 3],
        busy_for: Some(u32::MAX),
        ..Default::default()
    };
    apply_jmap_fetch(&mut s, junk).unwrap();
    assert!(s.mailbox.messages == before.messages, "no message of the mailbox was touched");
    // Octets that are no message at all, for a message that is: it is read as well as it can be, and nothing crashes.
    let garbage = GraphFetchResult {
        account,
        email: ME.into(),
        fetched: vec![GraphFetched { id: "E1".into(), mime: [0xff, 0x00, b'\r', 0xfe, b':', b'\n'].repeat(5_000) }],
        ..Default::default()
    };
    apply_jmap_fetch(&mut s, garbage).unwrap();
    assert!(!message(&s, "E1").unfetched);
    assert!(s.mailbox.clone().repair().is_empty());
}

#[test]
fn an_agent_is_never_the_one_that_fetches_or_takes_in() {
    use crate::agent::{Access, Class, Verdict, check, classify};
    let (mut s, account) = session();
    fill(&mut s, account, "Min", "E", 2);
    for (id, params) in
        [("sync.jmapFetchRequest", json!({"account": account})), ("sync.applyJmapFetch", json!({"result": fetched(account, &["E1"])}))]
    {
        assert_eq!(classify(&s, id, &params), Access::Never, "{id}");
        assert!(matches!(check(&s, id, &params, &Class::ALL), Verdict::Refuse(_)), "{id}");
    }
}
