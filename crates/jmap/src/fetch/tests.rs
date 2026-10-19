//! Whole messages: a scripted server for what is asked and how a hostile answer is read, the
//! in-process server for the round trips, and servers made to fail for what a batch does when
//! something goes wrong.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use proptest::prelude::*;

use super::*;
use crate::standin::{self, StandIn};
use crate::{Auth, Response, block_on};

const SESSION: &str = "https://jmap.example.org/jmap/session?account=ada%40example.org";
const TOKEN: &str = "tk-3f9a7c";
const CAP: usize = 1 << 20;

// ------------------------------------------------------------------------------------------
// A scripted server

/// An `Http` that answers what it is told to, in order, and keeps what it was sent.
#[derive(Clone, Default)]
struct Script {
    replies: Rc<RefCell<VecDeque<Result<Response>>>>,
    sent: Rc<RefCell<Vec<Request>>>,
}

impl Script {
    fn sent(&self) -> Vec<Request> {
        self.sent.borrow().clone()
    }
}

impl Http for Script {
    async fn send(&self, req: Request) -> Result<Response> {
        self.sent.borrow_mut().push(req);
        self.replies.borrow_mut().pop_front().unwrap_or_else(|| Err(Error::Io("nothing was scripted".into())))
    }
}

fn reply(status: u16, body: &Value) -> Result<Response> {
    Ok(Response { status, body: serde_json::to_vec(body).unwrap(), retry_after: None })
}

fn octets(status: u16, body: &[u8]) -> Result<Response> {
    Ok(Response { status, body: body.to_vec(), retry_after: None })
}

/// What `Email/get` answers, as the one answer of a request.
fn email_get(answer: Value) -> Result<Response> {
    reply(200, &json!({"methodResponses": [["Email/get", answer, "c0"]], "sessionState": "1"}))
}

fn session_doc(download: &str) -> Value {
    json!({
        "capabilities": {crate::CORE: {"maxCallsInRequest": 16, "maxObjectsInGet": 500}, crate::MAIL: {}},
        "accounts": {"A1": {"name": "ada@example.org"}},
        "primaryAccounts": {crate::MAIL: "A1"},
        "apiUrl": "/jmap/api",
        "downloadUrl": download,
        "uploadUrl": "/jmap/upload/{accountId}/",
        "state": "1",
    })
}

const PATH_STYLE: &str = "/jmap/download/{accountId}/{blobId}/{name}?type={type}";

/// A connected client, and the script it speaks over; `replies` follow the session's.
fn connected_with(download: &str, replies: Vec<Result<Response>>) -> (Client<Script>, Script) {
    let script = Script::default();
    script.replies.borrow_mut().push_back(reply(200, &session_doc(download)));
    script.replies.borrow_mut().extend(replies);
    let client = block_on(Client::connect(script.clone(), SESSION, &Auth::bearer(TOKEN))).unwrap();
    (client, script)
}

fn connected(replies: Vec<Result<Response>>) -> (Client<Script>, Script) {
    connected_with(PATH_STYLE, replies)
}

fn fetch(client: &Client<Script>, email: &str, blob: Option<&str>) -> Result<Fetched> {
    block_on(fetch_message(client, email, blob, CAP))
}

// ------------------------------------------------------------------------------------------
// One message

#[test]
fn an_email_is_found_by_its_id_and_its_blob_is_downloaded() {
    let message = b"Subject: hi\r\n\r\nHello.\r\n";
    let (client, script) =
        connected(vec![email_get(json!({"list": [{"id": "E1", "blobId": "B1", "size": message.len()}], "notFound": []})), octets(200, message)]);
    assert_eq!(fetch(&client, "E1", None), Ok(Fetched::Whole(message.to_vec())));
    let sent = script.sent();
    assert_eq!(sent.len(), 3, "the session, the blob's name, the blob");
    // The blob and its size are asked for, and nothing else.
    let call: Value = serde_json::from_slice(&sent[1].body).unwrap();
    assert_eq!(call["using"], json!([crate::CORE, crate::MAIL]));
    assert_eq!(call["methodCalls"], json!([["Email/get", {"accountId": "A1", "ids": ["E1"], "properties": ["blobId", "size"]}, "c0"]]));
    // The blob is downloaded from the session's address for it, signed in, and held to the cap.
    let get = &sent[2];
    assert_eq!(
        (get.method.as_str(), get.url.as_str(), get.max),
        ("GET", "https://jmap.example.org/jmap/download/A1/B1/message.eml?type=message%2Frfc822", CAP)
    );
    assert!(get.headers.contains(&("Authorization".into(), format!("Bearer {TOKEN}"))));
    assert!(get.body.is_empty());
}

#[test]
fn an_address_template_of_the_query_kind_is_filled_in_too() {
    let download = "/api/method/app.download?accountId={accountId}&blobId={blobId}&name={name}&type={type}";
    let (client, script) = connected_with(download, vec![octets(200, b"X: y\r\n\r\n")]);
    assert_eq!(fetch(&client, "E1", Some("B-1_x")), Ok(Fetched::Whole(b"X: y\r\n\r\n".to_vec())));
    assert_eq!(
        script.sent()[1].url,
        "https://jmap.example.org/api/method/app.download?accountId=A1&blobId=B-1_x&name=message.eml&type=message%2Frfc822"
    );
}

#[test]
fn a_blob_the_caller_has_is_downloaded_without_asking_for_it() {
    let (client, script) = connected(vec![octets(200, b"Subject: x\r\n\r\n")]);
    assert_eq!(fetch(&client, "E1", Some("B7")), Ok(Fetched::Whole(b"Subject: x\r\n\r\n".to_vec())));
    let sent = script.sent();
    assert_eq!(sent.len(), 2, "the session and the blob");
    assert_eq!(sent[1].url, "https://jmap.example.org/jmap/download/A1/B7/message.eml?type=message%2Frfc822");
    // A blob id that isn't one goes nowhere.
    for bad in ["", "a b", "a/b", "../x", "B1?x", &"b".repeat(crate::MAX_ID + 1)] {
        assert!(matches!(fetch(&client, "E1", Some(bad)), Err(Error::Protocol(_))), "{bad}");
    }
    assert_eq!(script.sent().len(), 2);
}

#[test]
fn a_message_the_server_no_longer_has_is_missing_and_no_error() {
    // Not found when its blob is asked for...
    let (client, script) = connected(vec![email_get(json!({"list": [], "notFound": ["E1"]}))]);
    assert_eq!(fetch(&client, "E1", None), Ok(Fetched::Missing));
    assert_eq!(script.sent().len(), 2, "nothing is downloaded");
    // ... and when the blob is: a 404, whatever the server says with it.
    let problem = reply(404, &json!({"type": "about:blank", "status": 404, "detail": "No such blob."}));
    let (client, _) = connected(vec![email_get(json!({"list": [{"id": "E1", "blobId": "B1", "size": 5}]})), problem]);
    assert_eq!(fetch(&client, "E1", None), Ok(Fetched::Missing));
    let (client, _) = connected(vec![octets(404, b"")]);
    assert_eq!(fetch(&client, "E1", Some("B1")), Ok(Fetched::Missing));
    let (client, _) = connected(vec![octets(404, &vec![b'x'; 5_000])]);
    assert_eq!(fetch(&client, "E1", Some("B1")), Ok(Fetched::Missing));
}

#[test]
fn a_message_over_the_cap_is_refused_and_never_cut() {
    // The server says how big it is: the blob is not even asked for.
    let (client, script) = connected(vec![email_get(json!({"list": [{"id": "E1", "blobId": "B1", "size": 1001}]}))]);
    let refused = block_on(fetch_message(&client, "E1", None, 1000));
    assert!(matches!(refused, Err(Error::TooLarge(_))), "{refused:?}");
    assert_eq!(script.sent().len(), 2);
    // At the cap exactly, it is taken.
    let (client, _) = connected(vec![email_get(json!({"list": [{"id": "E1", "blobId": "B1", "size": 1000}]})), octets(200, &[b'x'; 1000])]);
    assert_eq!(block_on(fetch_message(&client, "E1", None, 1000)).map(|f| f == Fetched::Whole(vec![b'x'; 1000])), Ok(true));
    // A size no number can hold is too big as well.
    let (client, _) = connected(vec![email_get(json!({"list": [{"id": "E1", "blobId": "B1", "size": u64::MAX}]}))]);
    assert!(matches!(block_on(fetch_message(&client, "E1", None, usize::MAX)), Err(Error::TooLarge(_))));
    // A server that says nothing of the size, understates it or says what is no size is held to the cap while
    // the answer comes.
    for size in [json!(null), json!(5), json!("big"), json!(-1), json!(1.5)] {
        let entry = json!({"id": "E1", "blobId": "B1", "size": size});
        let (client, script) = connected(vec![email_get(json!({"list": [entry]})), octets(200, &[b'x'; 1001])]);
        let refused = block_on(fetch_message(&client, "E1", None, 1000));
        assert!(matches!(refused, Err(Error::TooLarge(_))), "{size}: {refused:?}");
        assert_eq!(script.sent().len(), 3, "{size}: the blob was asked for");
    }
    let (client, _) = connected(vec![octets(200, &[b'x'; 1001])]);
    assert!(matches!(block_on(fetch_message(&client, "E1", Some("B1"), 1000)), Err(Error::TooLarge(_))));
    // Whatever is asked for, no more than MAX_MESSAGE is: the transport is told so.
    let (client, script) = connected(vec![octets(200, b"x")]);
    assert!(block_on(fetch_message(&client, "E1", Some("B1"), usize::MAX)).is_ok());
    assert_eq!(script.sent()[1].max, MAX_MESSAGE);
}

#[test]
fn the_statuses_become_the_errors_the_caller_acts_on() {
    let busy = |status, retry_after| Ok(Response { status, body: Vec::new(), retry_after });
    let cases: Vec<(Result<Response>, Error)> = vec![
        (octets(401, b""), Error::Auth("the server refused the credentials".into())),
        (octets(403, b""), Error::Auth("the server refused the credentials".into())),
        (busy(429, Some(120)), Error::Busy { status: 429, retry_after: Some(120) }),
        (busy(503, None), Error::Busy { status: 503, retry_after: None }),
        (octets(500, b""), Error::Bad("HTTP 500".into())),
        (octets(400, b""), Error::Bad("HTTP 400".into())),
        (octets(302, b""), Error::Bad("HTTP 302".into())),
        (reply(500, &json!({"detail": "the\nmail  server broke"})), Error::Bad("HTTP 500 the mail server broke".into())),
        (octets(200, b""), Error::Protocol("the server sent no message".into())),
        (Err(Error::Io("offline".into())), Error::Io("offline".into())),
        (Err(Error::TooLarge("big".into())), Error::TooLarge("big".into())),
    ];
    for (answer, expected) in cases {
        let (client, _) = connected(vec![answer]);
        assert_eq!(fetch(&client, "E1", Some("B1")), Err(expected));
    }
}

#[test]
fn the_error_of_a_method_or_an_email_with_no_blob_fails_the_fetch_with_its_reason() {
    let refused = reply(200, &json!({"methodResponses": [["error", {"type": "requestTooLarge", "description": "too many"}, "c0"]]}));
    let (client, _) = connected(vec![refused]);
    assert_eq!(fetch(&client, "E1", None), Err(Error::Refused { kind: "requestTooLarge".into(), description: "too many".into() }));
    let (client, _) = connected(vec![octets(400, b"")]);
    assert_eq!(fetch(&client, "E1", None), Err(Error::Bad("HTTP 400".into())));
    // Nothing for the email in either list, or no blob for it.
    for answer in [json!({"list": [], "notFound": []}), json!({}), json!({"list": [{"id": "E1"}]}), json!({"list": [{"id": "E1", "blobId": "a b"}]})]
    {
        let (client, script) = connected(vec![email_get(answer.clone())]);
        assert!(matches!(fetch(&client, "E1", None), Err(Error::Protocol(_))), "{answer}");
        assert_eq!(script.sent().len(), 2, "nothing is downloaded");
    }
    // An id that is none isn't sent at all, and is said to be none.
    let (client, script) = connected(vec![]);
    for bad in ["", "a b", "a/b", &"e".repeat(crate::MAX_ID + 1)] {
        assert_eq!(fetch(&client, bad, None), Err(Error::Protocol("not an email id".into())), "{bad}");
    }
    assert_eq!(script.sent().len(), 1);
}

#[test]
fn what_an_email_get_answers_is_read_by_name_and_type() {
    // Each case: what the server answered, and what that comes to for the email asked about (a blob
    // is downloaded and given back whole when there is one).
    let blob = |extra: Value| {
        let mut entry = json!({"id": "E1", "blobId": "B1"});
        entry.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        entry
    };
    let downloads: Vec<(&str, Value)> = vec![
        ("a size that is text", json!({"list": [blob(json!({"size": "7"}))]})),
        ("a size with a fraction", json!({"list": [blob(json!({"size": 7.5}))]})),
        ("a size below zero", json!({"list": [blob(json!({"size": -7}))]})),
        ("a size that is null", json!({"list": [blob(json!({"size": null}))]})),
        ("other properties", json!({"list": [blob(json!({"mailboxIds": {"M1": true}, "threadId": "T1"}))], "state": 7})),
        ("entries for others first", json!({"list": [{"id": "E2", "blobId": "B2"}, 5, null, "E1", {"blobId": "B3"}, {"id": 1}, blob(json!({}))]})),
        ("the first entry counts", json!({"list": [blob(json!({})), {"id": "E1", "blobId": "B9"}]})),
        ("a list and the not found list", json!({"list": [blob(json!({}))], "notFound": ["E1"]})),
    ];
    for (what, answer) in downloads {
        let (client, script) = connected(vec![email_get(answer), octets(200, b"Subject: x\r\n\r\n")]);
        assert_eq!(fetch(&client, "E1", None), Ok(Fetched::Whole(b"Subject: x\r\n\r\n".to_vec())), "{what}");
        assert!(script.sent()[2].url.contains("/B1/"), "{what}");
    }
    // Gone, whichever way it is said.
    let gone: Vec<(&str, Value)> = vec![
        ("not found", json!({"list": [], "notFound": ["E1"]})),
        ("not found twice", json!({"notFound": ["E1", "E1"]})),
        ("a list that is a map", json!({"list": {"E1": blob(json!({}))}, "notFound": ["E1"]})),
        ("others found, this one not", json!({"list": [{"id": "E2", "blobId": "B2"}], "notFound": ["E1", 5, null]})),
    ];
    for (what, answer) in gone {
        let (client, script) = connected(vec![email_get(answer)]);
        assert_eq!(fetch(&client, "E1", None), Ok(Fetched::Missing), "{what}");
        assert_eq!(script.sent().len(), 2, "{what}: nothing is downloaded");
    }
    // Not the blob asked about, or none at all.
    let none: Vec<(&str, Value)> = vec![
        ("no list", json!({"notFound": []})),
        ("a list that is text", json!({"list": "E1"})),
        ("entries for others only", json!({"list": [{"id": "E2", "blobId": "B2"}, {"id": "e1", "blobId": "B1"}]})),
        ("a blob that is a number", json!({"list": [blob(json!({"blobId": 7}))]})),
        ("a blob that is null", json!({"list": [blob(json!({"blobId": null}))]})),
        ("a blob that is empty", json!({"list": [blob(json!({"blobId": ""}))]})),
        ("a blob with a slash", json!({"list": [blob(json!({"blobId": "a/b"}))]})),
        ("a blob with an accent", json!({"list": [blob(json!({"blobId": "é"}))]})),
        ("a blob that is too long", json!({"list": [blob(json!({"blobId": "b".repeat(crate::MAX_ID + 1)}))]})),
        ("not found that are numbers", json!({"notFound": [1, null, ["E1"]]})),
    ];
    for (what, answer) in none {
        let (client, script) = connected(vec![email_get(answer)]);
        assert!(matches!(fetch(&client, "E1", None), Err(Error::Protocol(_))), "{what}");
        assert_eq!(script.sent().len(), 2, "{what}: nothing is downloaded");
    }
}

#[test]
fn what_a_server_echoes_never_reaches_an_error() {
    // A server that names the account by its address (the account id holds it) and echoes the address
    // it was asked for, the header it was given and the password.
    let download = "/api/method/app.download?accountId={accountId}&blobId={blobId}&name={name}&type={type}";
    let mut doc = session_doc(download);
    doc["primaryAccounts"][crate::MAIL] = json!("AYWRhQGV4YW1wbGUub3Jn");
    let url = "https://jmap.example.org/api/method/app.download?accountId=AYWRhQGV4YW1wbGUub3Jn&blobId=B1&name=message.eml&type=message%2Frfc822";
    let header = Auth::basic("ada@example.org", "hunter2").header().unwrap();
    let echoes = [
        reply(500, &json!({"detail": format!("could not give {url} to {header} (hunter2)")})),
        Err(Error::Io(format!("error sending request for url ({url})"))),
        Err(Error::Io(format!("refused {header}"))),
    ];
    for echoed in echoes {
        let script = Script::default();
        script.replies.borrow_mut().extend([reply(200, &doc), echoed]);
        let client = block_on(Client::connect(script.clone(), SESSION, &Auth::basic("ada@example.org", "hunter2"))).unwrap();
        let shown = fetch(&client, "E1", Some("B1")).unwrap_err().to_string();
        assert!(shown.contains("***"), "{shown}");
        for secret in ["hunter2", "AYWRhQ", "ada@", "blobId", "app.download", "Basic"] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
    }
}

#[test]
fn what_is_shown_of_a_fetched_message_is_not_the_message() {
    let private = b"Subject: private\r\n\r\nHello Ada.".to_vec();
    assert_eq!(format!("{:?}", Fetched::Whole(private.clone())), format!("Fetched::Whole({} bytes)", private.len()));
    assert_eq!(format!("{:?}", Fetched::Missing), "Fetched::Missing");
}

// ------------------------------------------------------------------------------------------
// The in-process server, and servers made to fail

fn message(subject: &str) -> Vec<u8> {
    format!(
        "From: Zoe Park <zoe@example.org>\r\nTo: {}\r\nSubject: {subject}\r\nMessage-ID: <{subject}@example.org>\r\n\r\nHello about {subject}.\r\n",
        standin::USER
    )
    .into_bytes()
}

/// A stand-in with `n` emails in the Inbox (`s0`, `s1`…), oldest first: their ids.
fn filled(n: usize) -> (StandIn, Vec<String>) {
    let stand_in = StandIn::new();
    let ids = stand_in.with(|d| {
        let inbox = d.mailbox("Inbox", None, Some("inbox"));
        (0..n).map(|i| d.deliver(&inbox, &message(&format!("s{i}")), &[], &format!("2026-10-14T09:{:02}:{:02}Z", i / 60, i % 60))).collect()
    });
    (stand_in, ids)
}

fn connect<H: Http>(http: H) -> Client<H> {
    block_on(Client::connect(http, standin::SESSION, &Auth::bearer(standin::TOKEN))).unwrap()
}

fn request(ids: &[String]) -> JmapFetchRequest {
    JmapFetchRequest { account: 7, email: standin::USER.into(), session: standin::SESSION.into(), bearer: true, ids: ids.to_vec(), background: true }
}

fn batch(client: &Client<impl Http>, ids: &[String]) -> GraphFetchResult {
    block_on(fetch_messages(client, &request(ids), &AtomicBool::new(false)))
}

fn ids_of(fetched: &[GraphFetched]) -> Vec<&str> {
    fetched.iter().map(|f| f.id.as_str()).collect()
}

fn names(list: &[String]) -> Vec<&str> {
    list.iter().map(String::as_str).collect()
}

/// A server that does what a stand-in does, until a download is asked for: then `on_download` has its say.
struct Troubled {
    inner: StandIn,
    downloads: Rc<Cell<usize>>,
    on_download: Box<dyn Fn(usize, &Request) -> Option<Result<Response>>>,
}

impl Troubled {
    fn new(inner: &StandIn, on_download: impl Fn(usize, &Request) -> Option<Result<Response>> + 'static) -> Troubled {
        Troubled { inner: inner.clone(), downloads: Rc::default(), on_download: Box::new(on_download) }
    }
}

impl Http for Troubled {
    async fn send(&self, req: Request) -> Result<Response> {
        if req.method == "GET" && req.url.contains("/download/") {
            self.downloads.set(self.downloads.get() + 1);
            if let Some(answer) = (self.on_download)(self.downloads.get(), &req) {
                return answer;
            }
        }
        self.inner.send(req).await
    }
}

fn downloads_seen(stand_in: &StandIn) -> usize {
    stand_in.seen().iter().filter(|s| s.path.starts_with("/jmap/download/")).count()
}

#[test]
fn a_message_makes_the_round_trip_through_the_server() {
    let (stand_in, ids) = filled(3);
    let client = connect(stand_in.clone());
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(block_on(fetch_message(&client, id, None, CAP)), Ok(Fetched::Whole(message(&format!("s{i}")))));
    }
    // One for the session, then for each message the name of its blob and the blob.
    let seen = stand_in.seen();
    assert_eq!(seen.len(), 1 + 3 * 2);
    assert!(seen.iter().all(|s| s.status == 200));
    assert_eq!(stand_in.calls(), ["Email/get"; 3]);
    // The server deletes one: its id is not found, and its blob is a 404.
    let blob = stand_in.with(|d| d.email(&ids[1]).map(|e| e.blob.clone())).unwrap();
    assert!(stand_in.with(|d| d.remove(&ids[1])));
    assert_eq!(block_on(fetch_message(&client, &ids[1], None, CAP)), Ok(Fetched::Missing));
    assert_eq!(block_on(fetch_message(&client, &ids[1], Some(&blob), CAP)), Ok(Fetched::Missing));
    // The one beyond the cap is refused without a download, and an email it never had is missing.
    let big = stand_in.with(|d| {
        let inbox = d.mailbox_named("Inbox").map(str::to_string).unwrap();
        d.deliver(&inbox, &vec![b'x'; 4096], &[], "2026-10-15T00:00:00Z")
    });
    let before = downloads_seen(&stand_in);
    assert!(matches!(block_on(fetch_message(&client, &big, None, 4095)), Err(Error::TooLarge(_))));
    assert_eq!(downloads_seen(&stand_in), before);
    assert!(block_on(fetch_message(&client, &big, None, 4096)).is_ok());
    assert_eq!(block_on(fetch_message(&client, "E999", None, CAP)), Ok(Fetched::Missing));
    // The credentials the server doesn't hold are an error of their own.
    stand_in.with(|d| d.token = Some("rotated".into()));
    assert!(matches!(block_on(fetch_message(&client, &ids[0], None, CAP)), Err(Error::Auth(_))));
    assert!(matches!(block_on(fetch_message(&client, &ids[0], Some("B1"), CAP)), Err(Error::Auth(_))));
}

#[test]
fn a_batch_comes_in_the_order_asked_and_names_what_the_server_no_longer_has() {
    let (stand_in, ids) = filled(4);
    let client = connect(stand_in.clone());
    stand_in.with(|d| d.remove(&ids[1]));
    // Newest first, one the server lost, one it never had, one that is no id, and one twice.
    let asked = vec![ids[3].clone(), ids[1].clone(), ids[0].clone(), "E999".into(), "not an id".into(), ids[3].clone(), ids[2].clone()];
    let got = batch(&client, &asked);
    assert_eq!((got.account, got.email.as_str()), (7, standin::USER));
    assert_eq!(ids_of(&got.fetched), [ids[3].as_str(), ids[0].as_str(), ids[2].as_str()]);
    assert_eq!(got.fetched[0].mime, message("s3"));
    assert_eq!(got.fetched[1].mime, message("s0"));
    assert_eq!(names(&got.missing), [ids[1].as_str(), "E999"]);
    assert_eq!(names(&got.failed), ["not an id"]);
    assert!(got.errors.is_empty() && !got.offline && got.busy_for.is_none(), "{:?}", got.errors);
    // Their names in one request, then a download each of the messages the server has.
    assert_eq!(stand_in.calls(), ["Email/get"]);
    assert_eq!(downloads_seen(&stand_in), 3);
}

#[test]
fn a_batch_is_the_first_hundred_in_requests_of_what_the_server_takes() {
    let (stand_in, ids) = filled(130);
    let client = connect(stand_in.clone());
    let got = batch(&client, &ids);
    assert_eq!(got.fetched.len(), 100);
    assert_eq!(ids_of(&got.fetched), ids.iter().take(100).map(String::as_str).collect::<Vec<_>>());
    assert_eq!(stand_in.calls(), ["Email/get"]);
    // A server that takes three ids at a time is asked three at a time.
    let (stand_in, ids) = filled(8);
    stand_in.with(|d| d.max_get = 3);
    let client = connect(stand_in.clone());
    let got = batch(&client, &ids);
    assert_eq!(got.fetched.len(), 8);
    assert_eq!(stand_in.calls(), ["Email/get"; 3]);
    // Nothing to fetch asks nobody.
    let before = stand_in.seen().len();
    let empty = batch(&client, &[]);
    assert!(empty.fetched.is_empty() && empty.errors.is_empty());
    let nothing_valid = batch(&client, &["a b".to_string(), String::new()]);
    assert_eq!(names(&nothing_valid.failed), ["a b", ""]);
    assert_eq!(stand_in.seen().len(), before);
}

#[test]
fn a_batch_holds_only_so_much_and_the_rest_wait_for_the_next() {
    let stand_in = StandIn::new();
    let ids: Vec<String> = stand_in.with(|d| {
        let inbox = d.mailbox("Inbox", None, Some("inbox"));
        (0..6u8).map(|i| d.deliver(&inbox, &[b'a' + i; 1000], &[], &format!("2026-10-14T09:00:0{i}Z"))).collect()
    });
    let client = connect(stand_in.clone());
    let flag = AtomicBool::new(false);
    // Room for 2,500 octets: two messages of a thousand come, and the third would take the batch over.
    let first = block_on(fetch_batch(&client, &request(&ids), &flag, 2000, 2500));
    assert_eq!(ids_of(&first.fetched), names(&ids[..2]));
    assert!(first.missing.is_empty() && first.failed.is_empty() && first.errors.is_empty() && !first.offline);
    assert_eq!(downloads_seen(&stand_in), 2, "the third was not even downloaded");
    // The next request names the rest, and the batches go on from there until none is left.
    let mut left: Vec<String> = ids[2..].to_vec();
    let mut batches = 1;
    while !left.is_empty() {
        let got = block_on(fetch_batch(&client, &request(&left), &flag, 2000, 2500));
        assert!(!got.fetched.is_empty() && got.errors.is_empty(), "each batch brings something: {got:?}");
        left.retain(|id| !got.fetched.iter().any(|f| &f.id == id));
        batches += 1;
    }
    assert_eq!(batches, 3);
    assert_eq!(downloads_seen(&stand_in), 6, "each message was downloaded once");
    // One message is always taken, however large; the others wait.
    let one = block_on(fetch_batch(&client, &request(&ids[..3]), &flag, 2000, 10));
    assert_eq!(ids_of(&one.fetched), names(&ids[..1]));
    assert!(one.errors.is_empty());
}

#[test]
fn a_message_whose_size_is_not_stated_is_counted_when_it_has_come() {
    let answer = json!({"list": [{"id": "E1", "blobId": "B1"}, {"id": "E2", "blobId": "B2"}, {"id": "E3", "blobId": "B3"}]});
    let replies = vec![email_get(answer), octets(200, &[b'x'; 1000]), octets(200, &[b'y'; 1000]), octets(200, &[b'z'; 1000])];
    let (client, script) = connected(replies);
    let ids: Vec<String> = ["E1", "E2", "E3"].map(String::from).to_vec();
    let got = block_on(fetch_batch(&client, &request(&ids), &AtomicBool::new(false), CAP, 1500));
    // 1,000 is within the budget, so the second is asked for; 2,000 is over, so the third waits.
    assert_eq!(ids_of(&got.fetched), ["E1", "E2"]);
    assert!(got.missing.is_empty() && got.failed.is_empty() && got.errors.is_empty());
    assert_eq!(script.sent().len(), 4, "the session, the blobs' names, two blobs");
}

#[test]
fn cancel_is_looked_at_before_the_batch_and_before_each_message() {
    let (stand_in, ids) = filled(4);
    let cancel = Arc::new(AtomicBool::new(true));
    let client = connect(stand_in.clone());
    // Cancelled before anything: the server is not asked.
    let before = stand_in.seen().len();
    let got = block_on(fetch_messages(&client, &request(&ids), &cancel));
    assert!(got.fetched.is_empty() && got.missing.is_empty() && got.failed.is_empty() && got.errors.is_empty());
    assert_eq!(stand_in.seen().len(), before);
    // Cancelled while the first message comes: that one is in the answer, and no other is asked for.
    cancel.store(false, Ordering::Relaxed);
    let flag = cancel.clone();
    let troubled = Troubled::new(&stand_in, move |_, _| {
        flag.store(true, Ordering::Relaxed);
        None
    });
    let client = connect(troubled);
    let got = block_on(fetch_messages(&client, &request(&ids), &cancel));
    assert_eq!(ids_of(&got.fetched), [ids[0].as_str()]);
    assert!(got.errors.is_empty() && !got.offline);
    assert_eq!(downloads_seen(&stand_in), 1);
}

#[test]
fn three_refused_one_after_another_stop_the_batch_and_a_good_one_in_between_does_not() {
    // The first three: refused. The rest wait.
    let (stand_in, ids) = filled(6);
    let troubled = Troubled::new(&stand_in, |_, _| Some(octets(500, b"")));
    let downloads = troubled.downloads.clone();
    let client = connect(troubled);
    let got = batch(&client, &ids);
    assert!(got.fetched.is_empty() && got.missing.is_empty());
    assert_eq!(names(&got.failed), names(&ids[..3]));
    assert_eq!(got.errors.len(), 1, "{:?}", got.errors);
    assert!(got.errors[0].contains("HTTP 500"), "{}", got.errors[0]);
    assert!(!got.offline && got.busy_for.is_none());
    assert_eq!(downloads.get(), 3, "the rest wait");
    // Refused, refused, one that comes, refused, refused: no three in a row.
    let (stand_in, ids) = filled(5);
    let troubled = Troubled::new(&stand_in, |n, _| (n != 3).then(|| octets(500, b"")));
    let client = connect(troubled);
    let got = batch(&client, &ids);
    assert_eq!(ids_of(&got.fetched), [ids[2].as_str()]);
    assert_eq!(names(&got.failed), [ids[0].as_str(), ids[1].as_str(), ids[3].as_str(), ids[4].as_str()]);
    assert!(got.errors.is_empty(), "{:?}", got.errors);
    // A message the server no longer has is no refusal: it breaks the run of them, too.
    let (stand_in, ids) = filled(5);
    stand_in.with(|d| d.remove(&ids[2]));
    let troubled = Troubled::new(&stand_in, |_, _| Some(octets(500, b"")));
    let client = connect(troubled);
    let got = batch(&client, &ids);
    assert_eq!(
        (names(&got.failed), names(&got.missing)),
        (vec![ids[0].as_str(), ids[1].as_str(), ids[3].as_str(), ids[4].as_str()], vec![ids[2].as_str()])
    );
    assert!(got.errors.is_empty());
}

#[test]
fn a_busy_server_the_network_and_the_sign_in_each_stop_the_batch_with_their_word() {
    let wait = |retry_after| Some(Ok(Response { status: 503, body: Vec::new(), retry_after }));
    let cases: Vec<(&str, Box<dyn Fn(usize, &Request) -> Option<Result<Response>>>, bool, Option<u32>)> = vec![
        ("busy, with a wait", Box::new(move |_, _| wait(Some(30))), false, Some(30)),
        ("busy, without", Box::new(move |_, _| wait(None)), false, Some(0)),
        ("the network", Box::new(|_, _| Some(Err(Error::Io("offline".into())))), true, None),
        ("the sign-in", Box::new(|_, _| Some(octets(401, b""))), false, None),
        ("a refusal of the way", Box::new(|_, _| Some(Err(Error::Tls("not https".into())))), false, None),
    ];
    for (what, on_download, offline, busy_for) in cases {
        // The second message is where it goes wrong: the first is kept, the others wait.
        let (stand_in, ids) = filled(5);
        let troubled = Troubled::new(&stand_in, move |n, request| if n == 2 { on_download(n, request) } else { None });
        let client = connect(troubled);
        let got = batch(&client, &ids);
        assert_eq!(ids_of(&got.fetched), [ids[0].as_str()], "{what}");
        assert!(got.missing.is_empty() && got.failed.is_empty(), "{what}");
        assert_eq!((got.offline, got.busy_for), (offline, busy_for), "{what}");
        assert_eq!(got.errors.len(), 1, "{what}: {:?}", got.errors);
        assert!(got.errors[0].starts_with("Couldn't download the messages from the server: "), "{}", got.errors[0]);
    }
    // The names of the blobs can't be had: nothing is downloaded, and the batch says why.
    let (stand_in, ids) = filled(3);
    let client = connect(stand_in.clone());
    stand_in.with(|d| d.token = Some("rotated".into()));
    let got = batch(&client, &ids);
    assert!(got.fetched.is_empty() && got.failed.is_empty() && got.errors.len() == 1 && !got.offline, "{:?}", got.errors);
    assert_eq!(downloads_seen(&stand_in), 0);
}

#[test]
fn a_message_over_the_cap_is_passed_over_and_the_rest_come() {
    let stand_in = StandIn::new();
    let ids: Vec<String> = stand_in.with(|d| {
        let inbox = d.mailbox("Inbox", None, Some("inbox"));
        [100usize, 5000, 100, 5000, 5000, 100, 5000]
            .iter()
            .enumerate()
            .map(|(i, n)| d.deliver(&inbox, &vec![b'x'; *n], &[], &format!("2026-10-14T09:00:0{i}Z")))
            .collect()
    });
    let client = connect(stand_in.clone());
    let got = block_on(fetch_batch(&client, &request(&ids), &AtomicBool::new(false), 1000, MAX_BATCH_OCTETS));
    // The big ones are not even downloaded; three big ones in a row would be too many, but a small one is between.
    assert_eq!(ids_of(&got.fetched), [ids[0].as_str(), ids[2].as_str(), ids[5].as_str()]);
    assert_eq!(names(&got.failed), [ids[1].as_str(), ids[3].as_str(), ids[4].as_str(), ids[6].as_str()]);
    assert!(got.errors.is_empty(), "{:?}", got.errors);
    assert_eq!(downloads_seen(&stand_in), 3);
    // Three of them one after another stop it, with a word for why.
    let got = block_on(fetch_batch(&client, &request(&ids[3..]), &AtomicBool::new(false), 1000, MAX_BATCH_OCTETS));
    assert_eq!((names(&got.failed), got.errors.len(), got.fetched.len()), (vec![ids[3].as_str(), ids[4].as_str(), ids[6].as_str()], 0, 1));
    let got = block_on(fetch_batch(
        &client,
        &request(&[ids[1].clone(), ids[3].clone(), ids[4].clone(), ids[0].clone()]),
        &AtomicBool::new(false),
        1000,
        MAX_BATCH_OCTETS,
    ));
    assert_eq!((got.failed.len(), got.errors.len(), got.fetched.len()), (3, 1, 0));
    assert!(got.errors[0].contains("too large"), "{}", got.errors[0]);
    // A blob the server sends that is bigger than it said is refused as it comes.
    let lying = Troubled::new(&stand_in, |_, _| Some(octets(200, &vec![b'y'; 2000])));
    let client = connect(lying);
    let got = block_on(fetch_batch(&client, &request(&ids[..1]), &AtomicBool::new(false), 1000, MAX_BATCH_OCTETS));
    assert!(got.fetched.is_empty() && got.failed.len() == 1, "{got:?}");
}

#[test]
fn an_email_with_no_usable_blob_is_passed_over_and_three_in_a_row_stop_the_batch() {
    let ids: Vec<String> = ["E1", "E2", "E3", "E4", "E5"].map(String::from).to_vec();
    // E1 comes; E2's blob is no id and E3 is in neither list; E4 is gone; E5 comes.
    let answer = json!({"list": [{"id": "E1", "blobId": "B1"}, {"id": "E2", "blobId": "a b"}, {"id": "E5", "blobId": "B5"}], "notFound": ["E4"]});
    let (client, script) = connected(vec![email_get(answer), octets(200, b"One"), octets(200, b"Five")]);
    let got = block_on(fetch_messages(&client, &request(&ids), &AtomicBool::new(false)));
    assert_eq!(ids_of(&got.fetched), ["E1", "E5"]);
    assert_eq!((names(&got.failed), names(&got.missing)), (vec!["E2", "E3"], vec!["E4"]));
    assert!(got.errors.is_empty());
    assert_eq!(script.sent().len(), 4, "the session, the blobs' names, two blobs");
    // Three of them in a row.
    let answer = json!({"list": [{"id": "E1"}, {"id": "E2"}], "notFound": []});
    let (client, script) = connected(vec![email_get(answer)]);
    let got = block_on(fetch_messages(&client, &request(&ids), &AtomicBool::new(false)));
    assert_eq!((names(&got.failed), got.errors.len(), got.fetched.len()), (vec!["E1", "E2", "E3"], 1, 0));
    assert_eq!(script.sent().len(), 2);
}

#[test]
fn what_stops_the_names_of_the_blobs_stops_the_batch() {
    let ids: Vec<String> = ["E1", "E2"].map(String::from).to_vec();
    let cases: Vec<(Result<Response>, bool, Option<u32>)> = vec![
        (Err(Error::Io("offline".into())), true, None),
        (Ok(Response { status: 429, body: Vec::new(), retry_after: Some(9) }), false, Some(9)),
        (octets(401, b""), false, None),
        (octets(500, b""), false, None),
        (reply(200, &json!({"methodResponses": [["error", {"type": "serverFail"}, "c0"]]})), false, None),
        (octets(200, b"not json"), false, None),
    ];
    for (answer, offline, busy_for) in cases {
        let (client, script) = connected(vec![answer]);
        let got = block_on(fetch_messages(&client, &request(&ids), &AtomicBool::new(false)));
        assert!(got.fetched.is_empty() && got.missing.is_empty() && got.failed.is_empty());
        assert_eq!((got.offline, got.busy_for, got.errors.len()), (offline, busy_for, 1), "{:?}", got.errors);
        assert_eq!(script.sent().len(), 2, "nothing is downloaded");
    }
}

#[test]
fn a_batch_says_which_account_it_was_for() {
    let (stand_in, ids) = filled(1);
    let client = connect(stand_in);
    let got = block_on(fetch_messages(
        &client,
        &JmapFetchRequest { account: 12, email: "x@example.org".into(), ids, ..Default::default() },
        &AtomicBool::new(false),
    ));
    assert_eq!((got.account, got.email.as_str(), got.fetched.len()), (12, "x@example.org", 1));
}

// ------------------------------------------------------------------------------------------
// Whatever a server says

proptest! {
    #[test]
    fn any_answer_about_a_blob_is_read_or_refused_and_never_panics(
        answer in prop_oneof![crate::tests::any_json(), Just(json!({"list": [{"id": "E1", "blobId": "B1", "size": 3}], "notFound": ["E2"]}))],
        download in proptest::collection::vec(any::<u8>(), 0..64),
        status in prop_oneof![Just(200u16), Just(404u16), any::<u16>()],
        noise in proptest::collection::vec(any::<u8>(), 0..64),
    ) {
        for first in [email_get(answer.clone()), octets(200, &noise)] {
            let replies = vec![first, octets(status, &download), octets(status, &download), octets(status, &download)];
            let (single, _) = connected(replies.clone());
            let _ = fetch(&single, "E1", None);
            let (client, _) = connected(replies);
            let got = block_on(fetch_messages(&client, &request(&["E1".to_string(), "E2".to_string(), "E3".to_string()]), &AtomicBool::new(false)));
            // Each message is in one list at most, and only those that were asked for are.
            let mut seen = HashSet::new();
            for id in got.fetched.iter().map(|f| f.id.as_str()).chain(got.missing.iter().map(String::as_str)).chain(got.failed.iter().map(String::as_str)) {
                prop_assert!(["E1", "E2", "E3"].contains(&id) && seen.insert(id), "{id}");
            }
            prop_assert!(got.fetched.iter().all(|f| !f.mime.is_empty()));
        }
    }

    #[test]
    fn any_text_as_an_id_is_asked_about_or_passed_over(id in ".{0,40}", blob in ".{0,40}") {
        let (client, script) = connected(vec![email_get(json!({"list": [], "notFound": [id.clone()]}))]);
        let got = block_on(fetch_messages(&client, &request(std::slice::from_ref(&id)), &AtomicBool::new(false)));
        if valid_id(&id) {
            prop_assert_eq!(names(&got.missing), vec![id.as_str()]);
        } else {
            prop_assert_eq!(names(&got.failed), vec![id.as_str()]);
            prop_assert_eq!(script.sent().len(), 1, "an id that is none is not sent");
        }
        // And a blob the caller names is downloaded, or refused before anything is sent.
        let (client, script) = connected(vec![octets(200, b"x")]);
        let fetched = fetch(&client, "E1", Some(&blob));
        if valid_id(&blob) {
            prop_assert_eq!(fetched, Ok(Fetched::Whole(b"x".to_vec())));
        } else {
            prop_assert!(matches!(fetched, Err(Error::Protocol(_))));
            prop_assert_eq!(script.sent().len(), 1);
        }
    }
}
