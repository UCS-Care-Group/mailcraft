// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The online archive of an Exchange server account (#1818): one each, only an Exchange server
//! account's, gone with its owner, and nothing of its folders or mail written to the file.

use std::sync::Arc;

use crate::*;

fn exchange(mb: &mut Mailbox, email: &str) -> Id {
    let server = EwsServer { url: "https://mail.example.org/EWS/Exchange.asmx".into(), ..Default::default() };
    mb.add_account(Account { email: email.into(), kind: AccountKind::Exchange, ews: Some(server), ..Default::default() })
}

fn mailbox() -> (Mailbox, Id) {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let owner = exchange(&mut mb, "ada@corp.example.org");
    (mb, owner)
}

/// A folder of the archive's with a message in it, as a run would bring them. The folder's id.
fn archived(mb: &mut Mailbox, archive: Id) -> Id {
    let folder = mb.add_folder(archive, None, "2019").unwrap();
    let id = mb.alloc_id();
    mb.messages.insert(id, Arc::new(Message { id, account: archive, folder, subject: "Budget 2019".into(), ..Default::default() }));
    folder
}

#[test]
fn an_exchange_server_account_has_one_archive_with_no_folders_of_its_own() {
    let (mut mb, owner) = mailbox();
    let archive = mb.add_archive(owner).unwrap();
    assert_eq!(mb.add_archive(owner).unwrap(), archive, "one each: asking again finds it");
    let a = mb.account(archive).unwrap();
    assert!(a.is_archive() && !a.is_shared() && a.kind == AccountKind::Exchange);
    assert_eq!(a.email, "ada@corp.example.org");
    assert!(a.name.contains("Online Archive") && a.name.contains("ada@corp.example.org"));
    assert_eq!(a.ews, Some(EwsServer::default()), "no address and no sign-in of its own");
    assert!(mb.folders.iter().all(|f| f.account != archive), "its folders are the server's");
    assert_eq!(mb.archive_of(owner).map(|a| a.id), Some(archive));
    assert_eq!(mb.archive_owner(archive).map(|a| a.id), Some(owner));
    assert!(mb.archive_owner(owner).is_none());
}

#[test]
fn only_an_exchange_server_account_has_one() {
    let (mut mb, owner) = mailbox();
    let local = mb.accounts[0].id;
    assert!(mb.add_archive(local).is_err());
    assert!(mb.add_archive(9_999).is_err());
    let archive = mb.add_archive(owner).unwrap();
    assert!(mb.add_archive(archive).is_err(), "an archive has no archive");
    let microsoft = mb.add_account(Account { email: "ada@example.com".into(), kind: AccountKind::Microsoft, ..Default::default() });
    assert!(mb.add_archive(microsoft).is_err());
}

#[test]
fn the_archive_goes_with_its_owner_and_by_itself_alone() {
    let (mut mb, owner) = mailbox();
    let archive = mb.add_archive(owner).unwrap();
    archived(&mut mb, archive);
    mb.remove_account(owner).unwrap();
    assert!(mb.account(archive).is_none() && mb.messages.values().all(|m| m.account != archive));
    let (mut mb, owner) = mailbox();
    let archive = mb.add_archive(owner).unwrap();
    assert!(mb.remove_archive(owner).is_err(), "only an archive is removed as one");
    mb.remove_archive(archive).unwrap();
    assert!(mb.account(owner).is_some() && mb.archive_of(owner).is_none());
}

#[test]
fn nothing_of_the_archive_is_written_to_the_file() {
    let (mut mb, owner) = mailbox();
    assert!(matches!(mb.for_disk(), std::borrow::Cow::Borrowed(_)), "nothing to leave out");
    let archive = mb.add_archive(owner).unwrap();
    assert!(matches!(mb.for_disk(), std::borrow::Cow::Borrowed(_)), "an archive with nothing held of it");
    let folder = archived(&mut mb, archive);
    mb.account_mut(archive).unwrap().ews.as_mut().unwrap().hierarchy = "H4sI-state".into();
    let disk = mb.for_disk();
    assert!(disk.account(archive).is_some_and(Account::is_archive), "the link to its owner is kept");
    assert!(disk.folder(folder).is_none() && disk.messages.values().all(|m| m.account != archive));
    assert_eq!(disk.account(archive).unwrap().ews, Some(EwsServer::default()), "nor where its folder list went on from");
    let written = serde_json::to_string(&*disk).unwrap();
    assert!(!written.contains("Budget 2019") && !written.contains("H4sI-state"));
    // The mailbox in hand is as it was.
    assert!(mb.folder(folder).is_some() && mb.messages.values().any(|m| m.subject == "Budget 2019"));
    // And the owner's mail is all there.
    assert_eq!(disk.folders.iter().filter(|f| f.account == owner).count(), mb.folders.iter().filter(|f| f.account == owner).count());
}

#[test]
fn a_file_that_holds_archive_mail_all_the_same_loses_it_when_read() {
    let (mut mb, owner) = mailbox();
    let archive = mb.add_archive(owner).unwrap();
    archived(&mut mb, archive);
    let written = serde_json::to_string(&mb).unwrap();
    let mut read: Mailbox = serde_json::from_str(&written).unwrap();
    let fixed = read.repair();
    assert!(fixed.iter().any(|f| f.contains("online archive")), "{fixed:?}");
    assert!(read.account(archive).is_some_and(Account::is_archive));
    assert!(read.folders.iter().all(|f| f.account != archive), "no folders, and no standard ones made for it");
    assert!(read.messages.values().all(|m| m.account != archive));
    assert!(read.repair().is_empty(), "once is enough: {:?}", read.repair());
}

#[test]
fn an_archive_whose_owner_is_gone_or_cant_be_one_is_taken_away() {
    let (mut mb, owner) = mailbox();
    let archive = mb.add_archive(owner).unwrap();
    let local = mb.accounts[0].id;
    mb.account_mut(archive).unwrap().archive_of = Some(local);
    assert!(mb.archive_owner(archive).is_none());
    mb.repair();
    assert!(mb.account(archive).is_none());
    let (mut mb, owner) = mailbox();
    let archive = mb.add_archive(owner).unwrap();
    mb.account_mut(archive).unwrap().archive_of = Some(archive);
    mb.repair();
    assert!(mb.account(archive).is_none(), "its own archive");
}

#[test]
fn a_file_from_before_or_one_with_nonsense_opens() {
    let (mb, owner) = mailbox();
    let written = serde_json::to_value(&mb).unwrap();
    assert!(!written.to_string().contains("archiveOf"), "nothing written for an account that is none");
    let mut odd = written.clone();
    for account in odd["accounts"].as_array_mut().unwrap() {
        account["archiveOf"] = serde_json::json!("not an id");
    }
    let read: Mailbox = serde_json::from_value(odd).unwrap();
    assert!(read.accounts.iter().all(|a| a.archive_of.is_none()));
    assert!(read.account(owner).is_some());
}

#[test]
fn archive_mail_is_no_new_mail() {
    let (mut mb, owner) = mailbox();
    let archive = mb.add_archive(owner).unwrap();
    assert!(mb.counts_as_new_mail(owner) && !mb.counts_as_new_mail(archive));
}

/// An archive is written with a kind of its own, so that a MailCraft without archives keeps it
/// whole as an account of a kind it doesn't know, rather than reading it as a second Exchange
/// server account and giving it folders; read back here, it is the archive it was.
#[test]
fn an_archive_is_written_as_a_kind_of_its_own_and_read_back_as_it_was() {
    let (mut mb, owner) = mailbox();
    let archive = mb.add_archive(owner).unwrap();
    let written = serde_json::to_value(&*mb.for_disk()).unwrap();
    let accounts = written["accounts"].as_array().unwrap();
    let kind_of = |id: Id| accounts.iter().find(|a| a["id"] == serde_json::json!(id)).unwrap()["kind"].clone();
    assert_eq!(kind_of(archive), "exchangeArchive");
    assert_eq!(kind_of(owner), "exchange", "its owner is written as it always was");
    // A kind a MailCraft without archives doesn't know: kept whole there.
    assert_eq!(serde_json::from_value::<AccountKind>(serde_json::json!("exchangeArchive")).unwrap(), AccountKind::Unknown);
    let mut read: Mailbox = serde_json::from_value(written).unwrap();
    assert!(read.repair().is_empty(), "{:?}", read.repair());
    let a = read.account(archive).unwrap();
    assert!(a.is_archive() && a.kind == AccountKind::Exchange && a.archive_of == Some(owner));
    assert!(read.folders.iter().all(|f| f.account != archive));
    assert_eq!(read.archive_of(owner).map(|a| a.id), Some(archive));
}

/// A message of the owner's mailbox as a run brings it: in its Inbox, by the server's id.
fn on_owners_server(mb: &mut Mailbox, owner: Id) -> (Id, Id) {
    let inbox = mb.folder_of_kind(owner, FolderKind::Inbox).unwrap();
    mb.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "INBOX-ID".into(), ..Default::default() });
    let id = mb.alloc_id();
    let server = crate::sync::ServerMessage { id: "ITEM-1".into(), folder: "INBOX-ID".into(), ..Default::default() };
    mb.messages
        .insert(id, Arc::new(Message { id, account: owner, folder: inbox, subject: "Kickoff".into(), server: Some(server), ..Default::default() }));
    (id, inbox)
}

/// Mail no server would put in the archive doesn't go there: nothing of the archive is kept on
/// this computer, so it would be lost when the program closes (#1818 review).
#[test]
fn mail_from_elsewhere_is_neither_moved_nor_copied_into_the_archive() {
    let (mut mb, owner) = mailbox();
    let archive = mb.add_archive(owner).unwrap();
    let year = archived(&mut mb, archive);
    let local_inbox = mb.folder_of_kind(mb.accounts[0].id, FolderKind::Inbox).unwrap();
    let id = mb.alloc_id();
    mb.messages.insert(id, Arc::new(Message { id, account: mb.accounts[0].id, folder: local_inbox, subject: "Mine".into(), ..Default::default() }));
    assert_eq!(mb.move_messages(&[id], year).unwrap_err().to_string(), ModelError::Invalid(INTO_ARCHIVE.into()).to_string());
    assert_eq!(mb.message(id).unwrap().folder, local_inbox, "left where it was");
    assert!(matches!(mb.copy_message(id, year), Err(ModelError::Invalid(why)) if why == COPY_INTO_ARCHIVE));
    // Nor a message of the owner's that no server has (only here).
    let inbox = mb.folder_of_kind(owner, FolderKind::Inbox).unwrap();
    let draft = mb.alloc_id();
    mb.messages.insert(draft, Arc::new(Message { id: draft, account: owner, folder: inbox, subject: "Here only".into(), ..Default::default() }));
    assert!(mb.move_messages(&[draft], year).is_err());
    // A copy from the owner's mailbox neither: its server makes none there.
    let (kickoff, _) = on_owners_server(&mut mb, owner);
    assert!(mb.copy_message(kickoff, year).is_err());
    // What the owner's server has moves in; mail within the archive moves and copies as ever.
    mb.move_messages(&[kickoff], year).unwrap();
    assert_eq!(mb.message(kickoff).unwrap().account, archive);
    let other = mb.add_folder(archive, None, "2020").unwrap();
    let budget = mb.messages.values().find(|m| m.subject == "Budget 2019").unwrap().id;
    mb.copy_message(budget, other).unwrap();
    mb.move_messages(&[budget], other).unwrap();
}

/// Mail moved into the archive that its server hasn't moved yet goes back where the server still
/// has it, when the archive is hidden or goes and in the mailbox file: never lost (#1818 review).
#[test]
fn a_move_into_the_archive_not_made_yet_is_put_back_not_lost() {
    let (mut mb, owner) = mailbox();
    let archive = mb.add_archive(owner).unwrap();
    let year = archived(&mut mb, archive);
    let (kickoff, inbox) = on_owners_server(&mut mb, owner);
    mb.move_messages(&[kickoff], year).unwrap();
    // Written to the file: where the server has it, in the owner's Inbox.
    let written = mb.for_disk();
    let m = written.message(kickoff).expect("written, not left out");
    assert_eq!((m.account, m.folder), (owner, inbox));
    assert!(written.messages.values().all(|m| m.subject != "Budget 2019"), "the archive's own mail stays out");
    drop(written);
    assert_eq!(mb.message(kickoff).unwrap().account, archive, "the mailbox in memory is as it was");
    // Hidden (or gone from the server): put back the same way.
    mb.remove_archive(archive).unwrap();
    let m = mb.message(kickoff).expect("kept");
    assert_eq!((m.account, m.folder), (owner, inbox));
    // Once the server has moved it, it is the archive's own and goes with it.
    let archive = mb.add_archive(owner).unwrap();
    let year = archived(&mut mb, archive);
    mb.move_messages(&[kickoff], year).unwrap();
    let folder_id = "YEAR-ID".to_string();
    mb.folder_mut(year).unwrap().server = Some(ServerFolder { id: folder_id.clone(), ..Default::default() });
    Arc::make_mut(mb.messages.get_mut(&kickoff).unwrap()).server.as_mut().unwrap().folder = folder_id;
    mb.remove_archive(archive).unwrap();
    assert!(mb.message(kickoff).is_none());
}

/// Taking one archive away puts back only the moves into that one: another's go on; and a
/// mailbox holding archive mail without folders still leaves it out of the file (#1834 review).
#[test]
fn only_the_archive_taken_away_has_its_moves_put_back() {
    let (mut mb, owner) = mailbox();
    let second = exchange(&mut mb, "bob@corp.example.org");
    let a = mb.add_archive(owner).unwrap();
    let b = mb.add_archive(second).unwrap();
    let (kickoff, _) = on_owners_server(&mut mb, owner);
    let year_a = archived(&mut mb, a);
    mb.move_messages(&[kickoff], year_a).unwrap();
    // Bob's: a message of his server moved into his archive.
    let inbox_b = mb.folder_of_kind(second, FolderKind::Inbox).unwrap();
    mb.folder_mut(inbox_b).unwrap().server = Some(ServerFolder { id: "BOB-INBOX".into(), ..Default::default() });
    let id = mb.alloc_id();
    let server = crate::sync::ServerMessage { id: "BOB-1".into(), folder: "BOB-INBOX".into(), ..Default::default() };
    mb.messages.insert(id, Arc::new(Message { id, account: second, folder: inbox_b, server: Some(server), ..Default::default() }));
    let year_b = archived(&mut mb, b);
    mb.move_messages(&[id], year_b).unwrap();
    mb.remove_archive(a).unwrap();
    assert_eq!(mb.message(kickoff).unwrap().account, owner, "a's move put back");
    assert_eq!(mb.message(id).unwrap().account, b, "b's goes on");
    // Archive mail with no folder of its own here is still left out.
    mb.folders.retain(|f| f.account != b);
    let written = mb.for_disk();
    assert!(written.messages.values().all(|m| m.account != b || m.id == id), "{:?}", written.messages.len());
}

#[test]
fn an_archive_is_never_put_before_or_moved_by_itself() {
    let (mut mb, owner) = mailbox();
    let archive = mb.add_archive(owner).unwrap();
    assert!(mb.move_account(archive, None).is_err());
    assert!(mb.move_account(owner, Some(archive)).is_err());
}
