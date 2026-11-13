// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The desktop app's JMAP code against a real server on this computer (`cargo xtask live`, which
//! starts a throwaway one in Docker and says where it is; `docs/live-tests.md`).
//!
//! Every test here is `#[ignore]`d: none runs in `cargo test` or in `cargo xtask ci`. Each puts
//! something into the test mailbox the plain way a client of the protocol would (a JMAP `set`
//! call), then reads it back through the very functions the window's jobs run
//! (`jmap_runner::run_*_with`) and looks at what they made of it. That is the part the scripted
//! exchanges of the unit tests cannot show: that a real server names things the way the readers
//! expect.
//!
//! The mailbox is the test server's own (`MAILCRAFT_LIVE_EMAIL`, an `example.org` address), made
//! for the run and gone with it. Never point these at a real mailbox: they write to it. Without
//! `MAILCRAFT_LIVE_SESSION` a test says so and fails, rather than pass having tested nothing. The
//! session must be on this computer (loopback): anything else is refused before a byte is sent.
//! A server whose session names no account for contacts or for calendars has that test say so and
//! end there: mail is the one thing every server here must have.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use mailcraft_jmap::{Address, session_request};
use mailcraft_model::sync::{GraphMessage, JmapCalendarRequest, JmapContactsRequest, JmapFetchRequest, JmapMailRequest, JmapMailResult, JmapScope};
use mailcraft_model::{FolderKind, Rights};
use mailcraft_net::https;
use serde_json::{Value, json};

use crate::jmap_runner;

/// How long the server is given to come up.
const PATIENCE: Duration = Duration::from_secs(120);
const MAX_ANSWER: usize = 4 << 20;

/// Where the test server is, and how to sign in to it.
struct Live {
    session: String,
    email: String,
    secret: String,
    /// The `Authorization` field the app's own code would send.
    authorization: String,
    /// The session object, once the server has answered.
    told: Value,
}

impl Live {
    /// The server of this run, up and signed in to. Panics with what is missing: a live test that
    /// cannot reach its server has not passed.
    fn get() -> Live {
        let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set: run these through `cargo xtask live`"));
        let (session, email, secret) = (var("MAILCRAFT_LIVE_SESSION"), var("MAILCRAFT_LIVE_EMAIL"), var("MAILCRAFT_LIVE_SECRET"));
        let home = Address::parse_secure(session.trim()).expect("the live session address is one a credential may go to");
        let host = home.host().to_string();
        assert!(
            host == "localhost" || host == "127.0.0.1" || host == "[::1]" || host == "::1",
            "the live tests write to the mailbox they are given: only a server on this computer is taken"
        );
        let creds = jmap_runner::credentials(&email, false, &secret).expect("the live credentials can be sent");
        let signed = session_request(&home, &creds).expect("a session request can be made");
        let authorization = signed.fields().find(|(name, _)| name.eq_ignore_ascii_case("Authorization")).expect("signed").1.to_string();
        let mut live = Live { session, email, secret, authorization, told: Value::Null };
        live.told = live.wait_for_session();
        live
    }

    fn send(&self, method: &str, url: &str, body: &[u8]) -> Result<https::Response, String> {
        let fields = [("Authorization", self.authorization.as_str()), ("Content-Type", "application/json")];
        let ask = https::Request { method, url, fields: &fields, body, max_response: MAX_ANSWER, trusted: "" };
        https::send(&ask).map_err(|_| "the server could not be reached".to_string())
    }

    /// The session object, asked for until the server is up.
    fn wait_for_session(&self) -> Value {
        let begun = Instant::now();
        let mut url = self.session.clone();
        loop {
            if let Ok(answer) = self.send("GET", &url, b"") {
                if answer.status == 200
                    && let Ok(told) = serde_json::from_slice::<Value>(&answer.body)
                    && told.get("apiUrl").is_some()
                {
                    return told;
                }
                // (The well-known address may send us on, on the same server.)
                if matches!(answer.status, 301 | 302 | 307 | 308)
                    && let Some(to) = answer.location.as_deref()
                {
                    url = self.on_origin(to);
                    continue;
                }
            }
            assert!(begun.elapsed() < PATIENCE, "the test server did not come up in {} seconds", PATIENCE.as_secs());
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// The origin of the session address: `http://127.0.0.1:port`.
    fn origin(&self) -> String {
        let after = self.session.find("://").map_or(0, |at| at + 3);
        let end = self.session[after..].find('/').map_or(self.session.len(), |at| after + at);
        self.session[..end].to_string()
    }

    /// `path` (or an address on this origin) as an address on the origin.
    fn on_origin(&self, path: &str) -> String {
        let origin = self.origin();
        if path.starts_with(&origin) {
            path.to_string()
        } else {
            assert!(path.starts_with('/'), "the test server pointed off itself");
            format!("{origin}{path}")
        }
    }

    /// The account the session names for `capability`, when the server offers it.
    fn offered(&self, capability: &str) -> Option<String> {
        self.told["primaryAccounts"][capability].as_str().map(str::to_string)
    }

    /// The account the session names for `capability`, which the server must offer.
    fn account_for(&self, capability: &str) -> String {
        self.offered(capability).unwrap_or_else(|| panic!("the server names no account for {capability}"))
    }

    /// Whether the server offers `capability`; when it does not, says so on the test's output, for
    /// the test to end there.
    fn offers(&self, capability: &str, what: &str) -> bool {
        let offered = self.offered(capability).is_some();
        if !offered {
            println!("live: skipped, the server offers no {what} over JMAP ({capability})");
        }
        offered
    }

    /// One JMAP method call, and its answer's arguments.
    fn call(&self, capability: &str, method: &str, args: Value) -> Value {
        let api = self.on_origin(self.told["apiUrl"].as_str().expect("the session has an apiUrl"));
        let body = json!({"using": ["urn:ietf:params:jmap:core", capability], "methodCalls": [[method, args, "c0"]]});
        let answer = self.send("POST", &api, body.to_string().as_bytes()).expect("the server answers a JMAP call");
        assert_eq!(answer.status, 200, "{method} was answered with a status that is no answer");
        let read: Value = serde_json::from_slice(&answer.body).expect("a JMAP answer is JSON");
        let first = &read["methodResponses"][0];
        assert_eq!(first[0], method, "{method} was answered with {}: {}", first[0], first[1]);
        first[1].clone()
    }

    /// Create one object with a `set` call, and say its id. Panics with why the server refused.
    fn create(&self, capability: &str, method: &str, object: Value) -> String {
        let args = json!({"accountId": self.account_for(capability), "create": {"n1": object}});
        let made = self.call(capability, method, args);
        made["created"]["n1"]["id"].as_str().unwrap_or_else(|| panic!("{method} did not make it: {}", made["notCreated"]["n1"])).to_string()
    }

    /// A whole Send/Receive of the mailbox, its pieces taken together.
    fn mail_run(&self) -> JmapMailResult {
        let request = JmapMailRequest {
            account: 7,
            email: self.email.clone(),
            session: self.session.clone(),
            window: 100,
            list_folders: true,
            scope: JmapScope::All,
            ..Default::default()
        };
        let mut whole = JmapMailResult::default();
        jmap_runner::run_mail_with(&request, &self.secret, &cancel(), &mut |piece| whole.absorb(piece));
        whole
    }
}

/// A mark that this run's objects carry, so that a server kept from an earlier run (`--keep`)
/// does not confuse this one.
fn mark() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    format!("live{now}")
}

/// A job's stop that nobody asks for.
fn cancel() -> jmap_runner::Stop {
    jmap_runner::Stop::new(Arc::new(AtomicBool::new(false)))
}

#[test]
#[ignore = "needs a mail server on this computer: cargo xtask live"]
fn live_mail_is_listed_and_a_message_comes_down_whole() {
    let live = Live::get();
    let subject = format!("Hello from {}", mark());
    // The folders, first: an account has an Inbox.
    let first = live.mail_run();
    assert!(first.errors.is_empty() && first.whole && !first.auth_refused, "{:?}", first.errors);
    let listing = first.listing.expect("the run lists the folders");
    let inbox = listing.folders.iter().find(|folder| folder.kind == FolderKind::Inbox).expect("the server has an Inbox").id.clone();
    // A message put into the Inbox, as another client would.
    let email = json!({
        "mailboxIds": {inbox.clone(): true},
        "from": [{"name": "Bo Example", "email": "bo@example.org"}],
        "to": [{"email": live.email}],
        "subject": subject,
        "bodyValues": {"b": {"value": "The body of the live message.\r\n"}},
        "textBody": [{"partId": "b", "type": "text/plain"}],
    });
    let id = live.create("urn:ietf:params:jmap:mail", "Email/set", email);
    // The next run lists it in the Inbox, with what a message list shows.
    let run = live.mail_run();
    assert!(run.errors.is_empty() && run.whole, "{:?}", run.errors);
    let listed: Vec<&GraphMessage> = run.pages.iter().filter(|page| page.folder == inbox).flat_map(|page| &page.messages).collect();
    let message = listed.iter().find(|message| message.subject == subject).expect("the new message is in the Inbox's window");
    assert_eq!(message.server.id, id);
    assert_eq!(message.from.email, "bo@example.org");
    // And it comes down whole, as the bytes of a message.
    let fetch =
        JmapFetchRequest { account: 7, email: live.email.clone(), session: live.session.clone(), ids: vec![id.clone()], ..Default::default() };
    let fetched = jmap_runner::run_fetch_with(&fetch, &live.secret, &cancel());
    assert!(fetched.errors.is_empty(), "{:?}", fetched.errors);
    let whole = fetched.fetched.iter().find(|one| one.id == id).expect("the message was fetched");
    let text = String::from_utf8_lossy(&whole.mime);
    assert!(text.contains(&subject) && text.contains("The body of the live message."));
}

#[test]
#[ignore = "needs a mail server on this computer: cargo xtask live"]
fn live_contacts_come_down_with_their_fields() {
    let live = Live::get();
    let capability = "urn:ietf:params:jmap:contacts";
    if !live.offers(capability, "contacts") {
        return;
    }
    let name = format!("Zoe {}", mark());
    let books = live.call(capability, "AddressBook/get", json!({"accountId": live.account_for(capability), "ids": null}));
    let book = books["list"][0]["id"].as_str().expect("the account has an address book").to_string();
    let card = json!({
        "@type": "Card",
        "version": "1.0",
        "addressBookIds": {book: true},
        "name": {"full": name, "components": [{"kind": "given", "value": "Zoe"}, {"kind": "surname", "value": "Example"}]},
        "organizations": {"o1": {"name": "Example Works"}},
        "emails": {"e1": {"address": "zoe@example.org", "contexts": {"work": true}}},
        "phones": {"p1": {"number": "+1 555 0100", "features": {"mobile": true}}},
        "notes": {"n1": {"note": "Met at the live test."}},
    });
    let id = live.create(capability, "ContactCard/set", card);
    let request = JmapContactsRequest { account: 7, email: live.email.clone(), session: live.session.clone(), bearer: false };
    let result = jmap_runner::run_contacts_with(&request, &live.secret, &cancel());
    assert!(result.errors.is_empty() && !result.not_offered && !result.auth_refused, "{:?}", result.errors);
    let download = result.contacts.expect("the contacts were read");
    assert!(download.complete && download.account == 7);
    let contact = download.contacts.iter().find(|contact| contact.id == id).expect("the new contact is among them");
    assert_eq!((contact.display_name.as_str(), contact.first.as_str(), contact.last.as_str()), (name.as_str(), "Zoe", "Example"));
    assert_eq!(contact.company, "Example Works");
    assert_eq!(contact.emails.first().map(|entry| entry.value.as_str()), Some("zoe@example.org"));
    assert_eq!(contact.phones.first().map(|entry| entry.value.as_str()), Some("+1 555 0100"));
    assert!(contact.notes.contains("Met at the live test."));
}

#[test]
#[ignore = "needs a mail server on this computer: cargo xtask live"]
fn live_calendars_and_their_events_come_down() {
    let live = Live::get();
    let capability = "urn:ietf:params:jmap:calendars";
    if !live.offers(capability, "calendars") {
        return;
    }
    let title = format!("Review {}", mark());
    let calendars = live.call(capability, "Calendar/get", json!({"accountId": live.account_for(capability), "ids": null}));
    let list = calendars["list"].as_array().expect("the account has calendars");
    let main = list.iter().find(|calendar| calendar["isDefault"] == true).or(list.first()).expect("a calendar");
    let calendar = main["id"].as_str().expect("a calendar has an id").to_string();
    // Tomorrow at ten, in UTC, for half an hour: well inside the stretch that is asked for.
    let now = mailcraft_time::now();
    let day = now - now.rem_euclid(86_400) + 86_400;
    let start = day + 10 * 3_600;
    let wall = mailcraft_time::DateTime::from_timestamp(start, 0);
    let local = format!("{:04}-{:02}-{:02}T{:02}:{:02}:00", wall.date.year, wall.date.month, wall.date.day, wall.hour, wall.minute);
    let event = json!({
        "@type": "Event",
        "calendarIds": {calendar.clone(): true},
        "title": title,
        "start": local,
        "timeZone": "Etc/UTC",
        "duration": "PT30M",
        "locations": {"l1": {"@type": "Location", "name": "Room 4"}},
    });
    let id = live.create(capability, "CalendarEvent/set", event);
    let request = JmapCalendarRequest {
        account: 7,
        email: live.email.clone(),
        session: live.session.clone(),
        bearer: false,
        from: now - 31 * 86_400,
        to: now + 366 * 86_400,
    };
    let result = jmap_runner::run_calendar_with(&request, &live.secret, &cancel());
    assert!(result.errors.is_empty() && !result.not_offered && !result.auth_refused, "{:?}", result.errors);
    let listed = result.calendars.expect("the calendars were listed");
    assert!(listed.complete);
    assert_eq!(listed.calendars.iter().filter(|calendar| calendar.main).count(), 1);
    assert!(listed.calendars.iter().all(|calendar| calendar.rights == Rights::Read), "a calendar is read and not written here");
    let events: Vec<_> = result.events.iter().flat_map(|download| &download.events).collect();
    let found = events.iter().find(|event| event.title == title).expect("the new event is in the stretch");
    assert!(found.id == id || found.id.contains(&id), "an occurrence is named after its event");
    assert_eq!((found.start, found.end, found.all_day), (start, start + 1_800, false));
    assert_eq!(found.location, "Room 4");
}
