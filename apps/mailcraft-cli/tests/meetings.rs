//! Meetings by mail from end to end, for an account whose calendar no server keeps: the
//! invitation a meeting on this computer sends goes out through the fake SMTP server as an
//! iCalendar request (RFC 5546, RFC 6047), the answer that comes back to the fake IMAP server is
//! taken into the meeting, and the cancellation goes the same way. The engine queues, the
//! network crate sends: this is where the two meet, as in `two_way.rs`.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]

#[path = "../../../crates/net/src/fake.rs"]
mod fake;

use fake::{Fake, FakeImap, FakeMailbox, FakeMessage, FakeSmtp, ImapWorld, SmtpWorld};
use mailcraft_engine::Session;
use mailcraft_engine::cmd::account::{apply_sync, sync_request};
use mailcraft_model::{FolderKind, Id, Mailbox, Response};
use serde_json::{Value, json};

const PASSWORD: &str = "hunter2";

fn mailbox(raw: &str, flags: &str, uid_validity: u32) -> FakeMailbox {
    FakeMailbox { raw: raw.into(), flags: flags.into(), uid_validity, messages: Vec::new(), refuse_open: false, next_uid: 0 }
}

struct Rig {
    imap: FakeImap,
    smtp: FakeSmtp,
    s: Session,
    account: Id,
}

impl Rig {
    fn new() -> Rig {
        let imap = Fake::imap(ImapWorld {
            user: "ada@example.org".into(),
            password: PASSWORD.into(),
            can_move: true,
            uidplus: true,
            mailboxes: vec![
                mailbox("INBOX", "\\HasNoChildren", 11),
                mailbox("Sent Items", "\\HasNoChildren \\Sent", 31),
                mailbox("Deleted Items", "\\HasNoChildren \\Trash", 41),
            ],
            ..Default::default()
        });
        let smtp = Fake::smtp(SmtpWorld { user: "ada@example.org".into(), password: PASSWORD.into(), auth: true, ..Default::default() });
        // Friday 2027-01-15, 08:00 UTC.
        let mut s = Session::at(Mailbox::with_local_account("", ""), 1_800_000_000, 0);
        let servers = json!({
            "email": "ada@example.org", "displayName": "Ada",
            "incoming": {"host": "127.0.0.1", "port": imap.port, "security": "none"},
            "outgoing": {"host": "127.0.0.1", "port": smtp.port, "security": "none"},
        });
        let account = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
        Rig { imap, smtp, s, account }
    }

    fn run(&mut self, id: &str, params: Value) -> Value {
        self.s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
    }

    fn sync(&mut self) -> Value {
        let mut request = sync_request(&self.s, self.account).unwrap();
        request.password = PASSWORD.into();
        let result = mailcraft_net::sync(&request);
        assert_eq!(result.errors, Vec::<String>::new());
        apply_sync(&mut self.s, result).unwrap()
    }

    /// A message arrives in the server's Inbox.
    fn arrives(&self, raw: String) {
        let mut world = self.imap.world();
        let inbox = world.mailboxes.iter_mut().find(|m| m.raw == "INBOX").unwrap();
        let uid = inbox.messages.iter().map(|m| m.uid).max().unwrap_or(0) + 1;
        inbox.messages.push(FakeMessage {
            uid,
            flags: String::new(),
            internal_date: "15-Jan-2027 09:00:00 +0000".into(),
            body: raw.into_bytes(),
            claimed_size: None,
        });
    }

    /// The messages the SMTP server took, as they were sent.
    fn delivered(&self) -> Vec<(Vec<String>, String)> {
        self.smtp.world().delivered.iter().map(|d| (d.recipients.clone(), d.data.replace("\r\n ", ""))).collect()
    }
}

#[test]
fn an_invitation_goes_out_the_answer_comes_back_and_the_cancellation_follows() {
    let mut r = Rig::new();
    r.sync();
    // A meeting in the calendar on this computer, sent: it waits in the Outbox for Send/Receive.
    let made = r.run(
        "calendar.newMeeting",
        json!({"title": "Design review", "start": "2027-01-20T10:00", "end": "2027-01-20T11:00", "attendees": "Bob <bob@example.org>", "send": true}),
    );
    let id = made["id"].as_u64().unwrap();
    assert!(r.delivered().is_empty());
    r.sync();
    let sent = r.delivered();
    assert_eq!(sent.len(), 1);
    let (to, data) = &sent[0];
    assert_eq!(to, &["bob@example.org".to_string()]);
    assert!(data.contains("Content-Type: text/calendar; method=REQUEST; charset=utf-8"), "{data}");
    assert!(data.contains("METHOD:REQUEST") && data.contains("ORGANIZER;CN=Ada:mailto:ada@example.org"), "{data}");
    let uid = r.s.mailbox.event(id).unwrap().uid.clone();
    assert!(data.contains(&format!("UID:{uid}")), "{data}");
    // Filed as sent, and gone from the Outbox.
    let outbox = r.s.mailbox.folder_of_kind(r.account, FolderKind::Outbox).unwrap();
    assert_eq!(r.s.mailbox.count(outbox), 0);

    // Bob accepts: his program's answer comes to the Inbox, and is taken into the meeting.
    r.arrives(format!(
        "From: Bob <bob@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: Accepted: Design review\r\nDate: Fri, 15 Jan 2027 09:00:00 +0000\r\n\
         Message-ID: <reply-1@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b1\"\r\n\r\n\
         --b1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nBob has accepted.\r\n\
         --b1\r\nContent-Type: text/calendar; method=REPLY; charset=utf-8\r\n\r\n\
         BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//Calendar//EN\r\nMETHOD:REPLY\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20270115T090000Z\r\n\
         DTSTART:20270120T100000Z\r\nDTEND:20270120T110000Z\r\nORGANIZER:mailto:ada@example.org\r\nATTENDEE;PARTSTAT=ACCEPTED:mailto:bob@example.org\r\n\
         END:VEVENT\r\nEND:VCALENDAR\r\n--b1--\r\n"
    ));
    r.sync();
    let reply = r.s.mailbox.messages.values().find(|m| m.subject == "Accepted: Design review").map(|m| m.id).expect("the answer came");
    let said = r.run("calendar.invitation", json!({"message": reply}));
    assert_eq!((said["kind"].as_str(), said["trackable"].as_bool(), said["tracked"].as_bool()), (Some("reply"), Some(true), Some(false)));
    r.run("calendar.takeReply", json!({"message": reply}));
    assert_eq!(r.s.mailbox.event(id).unwrap().attendees[0].response, Response::Accepted);

    // Called off: the cancellation goes the same way, with the next sequence.
    r.run("calendar.cancel", json!({"id": id, "send": true}));
    r.sync();
    let sent = r.delivered();
    assert_eq!(sent.len(), 2);
    let data = &sent[1].1;
    assert!(data.contains("method=CANCEL") && data.contains("METHOD:CANCEL") && data.contains("SEQUENCE:1"), "{data}");
    assert!(data.contains("STATUS:CANCELLED") && data.contains(&format!("UID:{uid}")), "{data}");
    assert!(r.s.mailbox.event(id).is_none());
}
