// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail of a Microsoft account kept with characters that couldn't be read (#185), from end to
//! end: a mailbox from before Windows-1250 and the rest were read has U+FFFD where đ, ’ and –
//! were. Opened, those messages are fetched again from the strict Graph stand-in, once, and read
//! as they are; what the owner did to them stays, nothing comes twice, and a message whose own
//! bytes are broken isn't fetched at every opening. Nothing here leaves this machine; every
//! message is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::AtomicBool;

use mailcraft_charset::base64::STANDARD;
use mailcraft_engine::Session;
use mailcraft_engine::cmd::graph_mail::{apply_graph_fetch, apply_graph_mail, graph_fetch_request, graph_mail_request};
use mailcraft_model::{FlagStatus, Id, Mailbox};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use serde_json::{Value, json};

/// Friday 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000;

/// A message in Windows-1250: đ and ž, which Windows-1252 hasn't, and ’ and –, which it has
/// elsewhere.
const CENTRAL_EUROPEAN: &str = "From: =?windows-1250?Q?Tarik_Gvo=9E=F0ar?= <tarik@example.org>\r\n\
To: Avery <charsets@example.org>\r\nSubject: =?windows-1250?Q?Introduction_=96_Josh?=\r\nMessage-ID: <cp1250@example.org>\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b\"\r\n\r\n--b\r\n\
Content-Type: text/plain; charset=windows-1250\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nHi Avery, we=92ve met.\r\n--b\r\n\
Content-Type: text/html; charset=windows-1250\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n\
<html><head><meta http-equiv=3D\"Content-Type\" content=3D\"text/html; charset=3Dwindows-1250\"></head><body><p>Hi Avery, we=92ve met.</p></body></html>\r\n--b--\r\n";

struct Rig {
    server: StandIn,
    s: Session,
    account: Id,
}

impl Rig {
    fn run(&mut self) {
        let request = graph_mail_request(&self.s, self.account).unwrap();
        let _routed = self.server.route();
        let s = &mut self.s;
        mailcraft_net::graph::sync_mail(TOKEN, &request, &AtomicBool::new(false), &mut |piece| {
            apply_graph_mail(s, piece).unwrap();
        });
    }

    fn fetch(&mut self) -> Value {
        let request = graph_fetch_request(&self.s, self.account, None, 100).unwrap();
        let _routed = self.server.route();
        let result = mailcraft_net::graph::fetch_messages(TOKEN, &request, &AtomicBool::new(false));
        apply_graph_fetch(&mut self.s, result).unwrap()
    }

    /// How many times the server was asked for the MIME of the message `id` there.
    fn downloads(&self, id: &str) -> usize {
        // The id's closing `=` is written `%3D` in the address.
        let id = id.trim_end_matches('=');
        self.server.state().requests.iter().filter(|r| r.target.contains(id) && r.target.ends_with("/$value")).count()
    }

    fn here(&self, id: &str) -> &mailcraft_model::Message {
        self.s.mailbox.messages.values().find(|m| m.server.as_ref().is_some_and(|s| s.id == id)).unwrap()
    }
}

#[test]
fn mail_kept_with_unreadable_characters_is_fetched_again_once_and_read_as_it_is() {
    let server = StandIn::start().unwrap();
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let account = s.run("account.add", &json!({"email": "charsets@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let (good, broken) = {
        let mut state = server.state();
        let inbox = state.well_known("inbox");
        let good = state.mailbox.add(&inbox, "Introduction – Josh", NOW - 3_600);
        state.mailbox.update(&good, |m| {
            m.mime = Some(CENTRAL_EUROPEAN.into());
            (m.read, m.flag, m.categories) = (true, "flagged".into(), vec!["Blue category".into()]);
        });
        // Broken at the source: a byte that isn't UTF-8 in a message that says it is.
        let broken = state.mailbox.add(&inbox, "Broken", NOW - 7_200);
        let text = STANDARD.encode(b"Broken \xff text.\r\n");
        state.mailbox.update(&broken, |m| {
            m.mime = Some(format!(
                "From: Kit <kit@example.org>\r\nSubject: Broken\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{text}\r\n"
            ))
        });
        (good, broken)
    };
    let mut rig = Rig { server, s, account };
    rig.run();
    rig.fetch();
    assert_eq!((rig.downloads(&good), rig.downloads(&broken)), (1, 1));

    // As a version from before Windows-1250 was read kept it.
    let id = rig.here(&good).id;
    {
        let m = rig.s.mailbox.message_mut(id).unwrap();
        m.from.name = "Tarik Gvo\u{FFFD}\u{FFFD}ar".into();
        m.subject = "Introduction \u{FFFD} Josh".into();
        m.body.text = "Hi Avery, we\u{FFFD}ve met.".into();
        m.body.html = Some("<p>Hi Avery, we\u{FFFD}ve met.</p>".into());
    }
    assert!(rig.here(&broken).body.text.contains('\u{FFFD}'));
    let count = rig.s.mailbox.messages.len();

    let dir = std::env::temp_dir().join(format!("mailcraft-185-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("charsets.mailcraft");
    let path = path.to_string_lossy();
    rig.s.run("file.save", &json!({"path": path})).unwrap();

    // Opened: both are fetched again, and read as they are.
    let opened = rig.s.run("file.open", &json!({"path": path})).unwrap();
    assert_eq!(opened["fetchedAgain"], 2, "{opened}");
    assert!(rig.here(&good).unfetched && rig.here(&good).subject.starts_with("Introduction"), "nothing disappears meanwhile");
    rig.fetch();
    assert_eq!((rig.downloads(&good), rig.downloads(&broken)), (2, 2));
    let m = rig.here(&good);
    assert_eq!((m.from.name.as_str(), m.subject.as_str(), m.body.text.trim()), ("Tarik Gvožđar", "Introduction – Josh", "Hi Avery, we’ve met."));
    assert!(m.body.html.as_deref().unwrap().contains("we’ve met"));
    assert!(!m.unfetched && m.read && m.flag == FlagStatus::Flagged && m.categories == ["Blue category"]);
    let inbox = rig.s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    assert_eq!(m.folder, inbox);
    rig.run();
    assert_eq!(rig.s.mailbox.messages.len(), count, "nothing twice");

    // Saved and opened again: the broken one isn't fetched again.
    rig.s.run("file.save", &json!({})).unwrap();
    let opened = rig.s.run("file.open", &json!({"path": path})).unwrap();
    assert_eq!(opened["fetchedAgain"], 0, "{opened}");
    assert!(rig.here(&broken).body.text.contains('\u{FFFD}') && !rig.here(&broken).unfetched);
    rig.fetch();
    assert_eq!((rig.downloads(&good), rig.downloads(&broken)), (2, 2));
    assert_eq!(rig.here(&good).subject, "Introduction – Josh");
    let _ = std::fs::remove_dir_all(&dir);
}
