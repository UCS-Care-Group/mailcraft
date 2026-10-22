// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #305: a run whose folder listing is cut short removes no message, from end to end: the
//! engine's `graph_mail_request` and `apply_graph_mail` with `mailcraft_net::graph::sync_mail` in
//! between, against the strict Graph stand-in. A message moved into a folder the listing missed
//! waits for a run with the whole listing, which finds it where it went. Nothing here leaves
//! this machine; every message is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::AtomicBool;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::graph_mail::{apply_graph_mail, graph_mail_request};
use mailcraft_model::{Id, Mailbox};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use serde_json::{Value, json};

/// Friday 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000;

/// A session with one Microsoft account kept in step through Graph, and a stand-in for its server.
struct Rig {
    server: StandIn,
    s: Session,
    account: Id,
}

impl Rig {
    fn new(address: &str) -> Rig {
        let server = StandIn::start().unwrap();
        let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
        let account = s.run("account.add", &json!({"email": address, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        Rig { server, s, account }
    }

    /// One whole run, each piece taken in as it comes: what taking in each piece answered.
    fn run(&mut self) -> Vec<Value> {
        let request = graph_mail_request(&self.s, self.account).unwrap();
        let _routed = self.server.route();
        let cancel = AtomicBool::new(false);
        let s = &mut self.s;
        let mut answers = Vec::new();
        mailcraft_net::graph::sync_mail(TOKEN, &request, &cancel, &mut |piece| answers.push(apply_graph_mail(s, piece).unwrap()));
        answers
    }

    /// The name of the folder the message with this subject is in here, if it is here.
    fn folder_of(&self, subject: &str) -> Option<String> {
        let m = self.s.mailbox.messages.values().find(|m| m.account == self.account && m.subject == subject)?;
        self.s.mailbox.folder(m.folder).map(|f| f.name.clone())
    }
}

#[test]
fn mail_moved_to_a_folder_a_short_listing_missed_waits_for_a_whole_listing() {
    let mut rig = Rig::new("listing-short@example.org");
    let (message, projects, apollo) = {
        let mut state = rig.server.state();
        let inbox = state.well_known("inbox");
        let projects = state.add_folder("Projects", None);
        let apollo = state.add_folder("Apollo", Some(&projects));
        (state.mailbox.add(&inbox, "Moved away", NOW - 3_600), projects, apollo)
    };
    rig.run();
    assert_eq!(rig.folder_of("Moved away").as_deref(), Some("Inbox"));
    assert!(rig.s.mailbox.folders.iter().any(|f| f.account == rig.account && f.name == "Apollo"));

    // In Outlook: moved into Apollo. The next listing can't read what lies in Projects, so
    // Apollo is missing from it and isn't read; the Inbox says the message left.
    {
        let mut state = rig.server.state();
        assert!(state.mailbox.move_to(&message, &apollo));
        state.broken_lists.push(projects.clone());
    }
    rig.run();
    assert_eq!(rig.folder_of("Moved away").as_deref(), Some("Inbox"), "a run with a listing cut short removes nothing");

    // The listing is whole again: the message is found where it went.
    rig.server.state().broken_lists.clear();
    rig.run();
    assert_eq!(rig.folder_of("Moved away").as_deref(), Some("Apollo"));

    // And a message truly gone is removed by a run with the whole listing.
    {
        let mut state = rig.server.state();
        assert!(state.mailbox.delete(&message));
    }
    rig.run();
    assert_eq!(rig.folder_of("Moved away"), None);
}
