// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The index of conversations ([`crate::threads::Threads`]) answers as the walk over every
//! message ([`Mailbox::conversation`]) does, through every kind of change to the mailbox.

use crate::threads::Threads;
use crate::*;

fn mail(folder: Id, subject: &str, date: i64) -> Message {
    Message { folder, subject: subject.into(), date, from: Address::new("Bob", "bob@example.org"), ..Default::default() }
}

/// Every message's conversation, from the index brought in step and from the walk.
fn agrees(threads: &mut Threads, mb: &Mailbox) {
    threads.update(mb);
    for id in mb.messages.keys() {
        assert_eq!(threads.conversation(mb, *id), mb.conversation(*id), "message {id}");
    }
    assert!(threads.conversation(mb, 999_999).is_empty(), "no such message");
}

#[test]
fn the_index_answers_as_the_walk_through_every_change() {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let account = mb.accounts[0].id;
    let other = mb.add_account(Account { name: "b@example.org".into(), email: "b@example.org".into(), ..Default::default() });
    let inbox = mb.inbox().unwrap();
    let deleted = mb.folder_of_kind(account, FolderKind::Deleted).unwrap();
    let other_inbox = mb.folder_of_kind(other, FolderKind::Inbox).unwrap();
    let mut ids = Vec::new();
    for (i, subject) in ["Lunch", "RE: Lunch", "Fwd: lunch", "Budget", "", "  ", "Re: Budget", "Plans"].iter().enumerate() {
        ids.push(mb.add_message(mail(inbox, subject, 100 + i as i64)).unwrap());
    }
    // The same topic in another account is another conversation.
    let elsewhere = mb.add_message(mail(other_inbox, "Re: Lunch", 50)).unwrap();
    let mut threads = Threads::default();
    agrees(&mut threads, &mb);
    assert_eq!(threads.conversation(&mb, ids[0]), vec![ids[0], ids[1], ids[2]]);
    assert_eq!(threads.conversation(&mb, elsewhere), vec![elsewhere]);

    // A read mark: the message is another allocation, of the same conversation.
    mb.message_mut(ids[1]).unwrap().read = true;
    agrees(&mut threads, &mb);
    // A subject changed: it moves to another conversation.
    mb.message_mut(ids[2]).unwrap().subject = "Budget".into();
    agrees(&mut threads, &mb);
    // Into Deleted Items: left out, but where it is read.
    mb.move_message(ids[3], deleted).unwrap();
    agrees(&mut threads, &mb);
    // Gone, the first and the last.
    mb.remove_message(ids[0]).unwrap();
    mb.remove_message(elsewhere).unwrap();
    agrees(&mut threads, &mb);
    // Come, with a subject now and none.
    let late = mb.add_message(mail(inbox, "re: budget", 300)).unwrap();
    mb.message_mut(ids[4]).unwrap().subject = "Plans".into();
    mb.message_mut(ids[7]).unwrap().subject = String::new();
    agrees(&mut threads, &mb);
    assert!(threads.conversation(&mb, late).contains(&ids[2]));

    // One gone and one come: as many messages as before, not the same ones.
    mb.remove_message(ids[6]).unwrap();
    mb.add_message(mail(inbox, "Re: Plans", 400)).unwrap();
    agrees(&mut threads, &mb);

    // Another mailbox in its place altogether (an Undo, another file), and an empty one.
    let mut older = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let older_inbox = older.inbox().unwrap();
    for i in 0..20 {
        older.add_message(mail(older_inbox, if i % 3 == 0 { "Plans" } else { "Re: Lunch" }, i)).unwrap();
    }
    agrees(&mut threads, &older);
    agrees(&mut threads, &mb);
    agrees(&mut threads, &Mailbox::default());
    agrees(&mut threads, &mb);
}

/// Subjects that fold alike are one conversation however they are written: letter case, a
/// long one, a sign that folds to ASCII, a Greek final sigma.
#[test]
fn subjects_folded_alike_are_one_conversation() {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let inbox = mb.inbox().unwrap();
    let long = "Quarterly Planning ".repeat(12);
    let subjects =
        ["Kiln", "RE: kiln", "\u{212A}iln", "ΣΑΣ", "Re: σας", "Fw[3]: ΣΑΣ", long.as_str(), &format!("FWD: {}", long.to_uppercase()), "re:", ""];
    for (i, subject) in subjects.iter().enumerate() {
        mb.add_message(mail(inbox, subject, i as i64)).unwrap();
    }
    let mut threads = Threads::default();
    agrees(&mut threads, &mb);
}

/// Built a chunk at a time, with changes in between, the index ends up as one built at once.
#[test]
fn an_index_built_a_chunk_at_a_time_is_the_one_built_at_once() {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let inbox = mb.inbox().unwrap();
    let subjects = ["Lunch", "Re: Lunch", "Budget", "Plans", ""];
    for i in 0..(3 * crate::messages::CHUNK as usize) {
        mb.add_message(mail(inbox, subjects[i % subjects.len()], i as i64)).unwrap();
    }
    let mut threads = Threads::default();
    let mut steps = 0;
    while !threads.update_some(&mb, 1) {
        steps += 1;
        if steps == 2 {
            // A change half way: a chunk already filed is filed again.
            let first = *mb.messages.keys().next().unwrap();
            mb.message_mut(first).unwrap().subject = "Plans".into();
        }
        assert!(steps < 20, "it gets there");
    }
    assert!(steps >= 3, "a chunk at a time");
    for id in mb.messages.keys() {
        assert_eq!(threads.conversation(&mb, *id), mb.conversation(*id), "message {id}");
    }
}
