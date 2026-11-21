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
