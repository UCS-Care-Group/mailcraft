// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #991: an IMAP account with thousands of folders has them all read, up to 10,000, in their
//! tree, and is told how many more there were.

use mailcraft_model::sync::ListedFolder;
use mailcraft_model::{AccountKind, Server};

use super::*;
use crate::fake::{Fake, FakeImap, FakeMailbox, FakeMessage, ImapWorld};

const PASSWORD: &str = "hunter2";

fn mailbox(raw: &str, flags: &str) -> FakeMailbox {
    FakeMailbox { raw: raw.into(), flags: flags.into(), uid_validity: 7, messages: Vec::new(), refuse_open: false, next_uid: 0 }
}

/// An Inbox with a message, a Sent folder, and `count` more folders in a tree: `Projects/<n>`
/// each with a child `Projects/<n>/Notes` (so half are a level deeper), and the first under
/// the Inbox as the server spells it.
fn world(count: usize) -> ImapWorld {
    let mut inbox = mailbox("INBOX", "\\HasChildren");
    inbox.messages.push(FakeMessage {
        uid: 1,
        flags: String::new(),
        internal_date: " 1-Jul-2003 09:52:37 +0000".into(),
        body: b"From: Bob <bob@example.org>\r\nSubject: Hello\r\nMessage-ID: <hello@example.org>\r\n\r\nHi.\r\n".to_vec(),
        claimed_size: None,
    });
    let mut mailboxes = vec![inbox, mailbox("Sent", "\\Sent"), mailbox("INBOX/Receipts", "\\HasNoChildren"), mailbox("Projects", "\\HasChildren")];
    // The rest: a child after its parent, and the parents' order of the server's choosing.
    let mut n = 0;
    while mailboxes.len() < count {
        mailboxes.push(mailbox(&format!("Projects/{n:05}"), "\\HasChildren"));
        mailboxes.push(mailbox(&format!("Projects/{n:05}/Notes"), "\\HasNoChildren"));
        n += 1;
    }
    mailboxes.truncate(count);
    ImapWorld { user: "ada".into(), password: PASSWORD.into(), mailboxes, ..Default::default() }
}

fn request(imap: &FakeImap) -> SyncRequest {
    let account = Account {
        id: 7,
        email: "ada@example.org".into(),
        kind: AccountKind::Imap,
        incoming: Some(Server { host: "127.0.0.1".into(), port: imap.port, security: Security::None }),
        username: "ada".into(),
        ..Default::default()
    };
    SyncRequest { account, password: PASSWORD.into(), ..Default::default() }
}

fn proto_mailbox(name: &str) -> Mailbox {
    Mailbox { name: name.into(), raw: name.into(), delimiter: Some('/'), flags: Vec::new() }
}

/// The parent each listed folder was given, by its path.
fn parents(listed: &[ListedFolder]) -> HashMap<&str, &str> {
    listed.iter().map(|f| (f.remote.as_str(), f.parent.as_str())).collect()
}

/// Every folder of 10,000 is listed, in its place in the tree, and a run goes through them all.
#[test]
fn ten_thousand_folders_are_all_read_in_their_tree() {
    let imap = Fake::imap(world(MAX_FOLDERS));
    let result = sync(&request(&imap));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.listed.len(), MAX_FOLDERS);
    let parents = parents(&result.listed);
    assert_eq!(parents.get("INBOX/Receipts"), Some(&"INBOX"));
    assert_eq!(parents.get("Projects/00000"), Some(&"Projects"));
    assert_eq!(parents.get("Projects/04997/Notes"), Some(&"Projects/04997"));
    // Every one of them whose parent is a folder has it, named by its last part.
    for f in result.listed.iter().filter(|f| f.remote.starts_with("Projects/")) {
        assert!(!f.parent.is_empty() && parents.contains_key(f.parent.as_str()), "{}", f.remote);
        assert!(!f.name.contains('/'), "{}", f.name);
    }
    // And every one was opened: the Inbox's message came, the others are there, empty.
    assert_eq!(result.folders.len(), MAX_FOLDERS);
    assert_eq!(result.new_count(), 1);
}

/// A server with more folders than a run reads has the first 10,000 read, and the owner is told
/// how many more there were and where they begin.
#[test]
fn past_ten_thousand_the_rest_are_left_out_and_said() {
    let imap = Fake::imap(world(MAX_FOLDERS + 3));
    let result = sync(&request(&imap));
    assert_eq!(result.listed.len(), MAX_FOLDERS);
    assert_eq!(result.folders.len(), MAX_FOLDERS);
    let left = imap.world().mailboxes.get(MAX_FOLDERS).map(|m| m.raw.clone()).unwrap_or_default();
    assert_eq!(
        result.errors,
        [format!("The server has 10003 folders: one run reads the first 10000, and leaves out the other 3, from \"{left}\" on")]
    );
}

/// Looking up 10,000 folders' parents is one pass, not a search of all of them for each: well
/// under a second, even unoptimised.
#[test]
fn the_tree_of_ten_thousand_folders_is_found_at_once() {
    let mut names = vec!["INBOX".to_string(), "inbox/Late".to_string()];
    names.extend((0..MAX_FOLDERS).map(|n| if n % 2 == 0 { format!("Top{n}") } else { format!("Top{}/Child", n - 1) }));
    let mailboxes: Vec<Mailbox> = names.iter().map(|n| proto_mailbox(n)).collect();
    let plan = planned(&mailboxes, &[], false);
    let started = Instant::now();
    let listed = listed(&plan);
    let took = started.elapsed();
    assert_eq!(listed.len(), MAX_FOLDERS);
    let parents = parents(&listed);
    // The Inbox under any spelling.
    assert_eq!(parents.get("inbox/Late"), Some(&"INBOX"));
    assert_eq!(parents.get("Top4/Child"), Some(&"Top4"));
    // The last of them read are in their place too.
    let last = listed.iter().find(|f| f.remote == format!("Top{}/Child", MAX_FOLDERS - 4)).map(|f| (f.parent.as_str(), f.name.as_str()));
    assert_eq!(last, Some(("Top9996", "Child")));
    assert!(took < Duration::from_secs(1), "{took:?}");
}

/// The parent found is the first folder of that name, as a search from the start found it,
/// and a parent past what a run reads is no parent.
#[test]
fn the_first_of_a_name_is_the_parent_and_only_one_that_is_read() {
    let mailboxes = [proto_mailbox("A"), proto_mailbox(" A "), proto_mailbox("A/b"), proto_mailbox("Inbox"), proto_mailbox("INBOX/c")];
    let plan = planned(&mailboxes, &[], false);
    let small = listed(&plan);
    let got: Vec<(&str, &str, &str)> = small.iter().map(|f| (f.remote.as_str(), f.parent.as_str(), f.name.as_str())).collect();
    assert_eq!(got, [("INBOX", "", "Inbox"), ("A", "", "A"), (" A ", "", "A"), ("A/b", "A", "b"), ("INBOX/c", "INBOX", "c")]);

    // Past the cap: the folders read come first, the child's parent is left out of them.
    let mut names: Vec<String> = (0..MAX_FOLDERS).map(|n| format!("F{n}")).collect();
    names.insert(0, "Late/child".into());
    names.push("Late".into());
    let mailboxes: Vec<Mailbox> = names.iter().map(|n| proto_mailbox(n)).collect();
    let plan = planned(&mailboxes, &[], false);
    let tree = listed(&plan);
    assert_eq!(tree.len(), MAX_FOLDERS);
    let child = tree.iter().find(|f| f.remote == "Late/child").map(|f| (f.parent.as_str(), f.name.as_str()));
    assert_eq!(child, Some(("", "Late/child")));
    let said = left_out(&plan, false).map(|t| t.to_string()).unwrap_or_default();
    assert!(said.ends_with("leaves out the other 2, from \"F9999\" on"), "{said}");
    // Nothing left out, nothing said.
    assert_eq!(left_out(plan.get(..MAX_FOLDERS).unwrap(), false), None);
}

/// A server whose list of folders is too long to keep whole has the run say that there are at
/// least as many as were kept, past a run's 10,000 (#991) and short of it, when the names and
/// flags fill what one list keeps.
#[test]
fn a_list_cut_short_is_said_too() {
    let imap = Fake::imap(world(12_001));
    let result = sync(&request(&imap));
    assert_eq!((result.listed.len(), result.folders.len()), (MAX_FOLDERS, MAX_FOLDERS));
    let first = imap.world().mailboxes.get(MAX_FOLDERS).map(|m| m.raw.clone()).unwrap_or_default();
    assert_eq!(
        result.errors,
        [format!("The server has at least 12000 folders: one run reads the first 10000, and leaves out the rest, from \"{first}\" on")]
    );

    // 1,500 folders, each listed with 64 long flags: the list is full long before.
    let mut world = world(1_500);
    let flags: Vec<String> = (0..64).map(|n| format!("\\F{n}{}", "x".repeat(248))).collect();
    let flags = flags.join(" ");
    for mailbox in world.mailboxes.iter_mut().skip(2) {
        mailbox.flags = flags.clone();
    }
    let imap = Fake::imap(world);
    let result = sync(&request(&imap));
    assert!(result.listed.len() < 1_500 && result.listed.len() > 2, "{}", result.listed.len());
    let kept = result.listed.len();
    assert_eq!(
        result.errors,
        [format!("The server has at least {kept} folders: their list was too long to read whole, and the folders after them were left out")]
    );
}
