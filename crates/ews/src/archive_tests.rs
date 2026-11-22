// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The personal archive (#1818): the requests name the archive's top of the mail when they are
//! about the archive and the mailbox's otherwise, a run of the archive reads its folders and none
//! of the mailbox's, and asking whether there is an archive never fails, whatever the server says.

use std::sync::atomic::AtomicBool;

use mailcraft_model::FolderKind;
use mailcraft_model::sync::{ChangeOutcome, EwsMailRequest, EwsMailResult, FolderChange};

use crate::changes::{CreateFolder, MoveFolder};
use crate::run::sync_store;
use crate::standin::{AUTHORIZATION, Hostile, StandIn, block_on};
use crate::*;

const URL: &str = "https://mail.example.org/EWS/Exchange.asmx";

fn client() -> Client {
    Client::new(URL, Secret::new(AUTHORIZATION)).unwrap()
}

fn request() -> EwsMailRequest {
    EwsMailRequest { email: "ada@example.org".into(), url: URL.into(), list_folders: true, ..Default::default() }
}

fn run(standin: &StandIn, store: Store, request: &EwsMailRequest) -> EwsMailResult {
    let mut all = EwsMailResult::default();
    block_on(sync_store(store, &client(), standin, request, &AtomicBool::new(false), &mut |_| true, &mut |piece| all.absorb(piece))).unwrap();
    all
}

fn probe(standin: &StandIn) -> ArchiveProbe {
    block_on(probe_archive(&client(), standin, &AtomicBool::new(false), &mut |_| true)).unwrap()
}

// ---- the requests ----

#[test]
fn the_folder_list_is_read_from_the_top_of_the_store_asked_for() {
    let mailbox = SyncFolderHierarchy::new("").element().write();
    assert!(mailbox.contains("Id=\"msgfolderroot\"") && !mailbox.contains("archive"), "{mailbox}");
    let archive = SyncFolderHierarchy::new("").in_store(Store::Archive).element().write();
    assert!(archive.contains("Id=\"archivemsgfolderroot\""), "{archive}");
    // (The state is the server's for that store, and goes as it came.)
    let later = SyncFolderHierarchy::new("H4sIAAAA").in_store(Store::Archive).element().write();
    assert!(later.contains("<m:SyncState>H4sIAAAA</m:SyncState>"));
}

#[test]
fn a_folder_made_or_moved_to_the_top_goes_to_the_top_of_its_store() {
    let top = CreateFolder::new("", "Projects").in_store(Store::Archive).element().write();
    assert!(top.contains("<m:ParentFolderId><t:DistinguishedFolderId Id=\"archivemsgfolderroot\"/></m:ParentFolderId>"), "{top}");
    assert!(CreateFolder::new("", "Projects").element().write().contains("Id=\"msgfolderroot\""));
    let moved = MoveFolder::new("AAMkFolder1", "").in_store(Store::Archive).element().write();
    assert!(moved.contains("<m:ToFolderId><t:DistinguishedFolderId Id=\"archivemsgfolderroot\"/></m:ToFolderId>"), "{moved}");
    // A folder named by id needs no store: the id is the server's either way.
    let under = CreateFolder::new("AAMkFolder2", "Projects").in_store(Store::Archive).element().write();
    assert!(under.contains("<t:FolderId Id=\"AAMkFolder2\"/>") && !under.contains("Distinguished"), "{under}");
}

#[test]
fn the_archive_is_asked_for_in_a_version_that_has_it() {
    assert_eq!(Store::Archive.since(), Version::Exchange2010Sp1);
    // (The client asks for 2013 and never steps down below its fallback.)
    assert!(Version::default() >= Store::Archive.since() && Version::FALLBACK >= Store::Archive.since());
    assert_eq!(Store::Archive.message_root().as_str(), "archivemsgfolderroot");
    assert_eq!(Store::Archive.deleted_items().as_str(), "archivedeleteditems");
    assert_eq!(DistinguishedFolder::ArchiveRoot.as_str(), "archiveroot");
    assert_eq!(DistinguishedFolder::ArchiveInbox.as_str(), "archiveinbox");
    assert_eq!(Store::default(), Store::Primary);
}

// ---- the probe ----

#[test]
fn a_mailbox_with_an_archive_has_one_and_one_without_has_none() {
    let standin = StandIn::new();
    assert_eq!(probe(&standin), ArchiveProbe::Absent);
    standin.state().add_archive(true);
    assert_eq!(probe(&standin), ArchiveProbe::Present);
    let state = standin.state();
    let asked = state.requests.last().unwrap();
    assert!(asked.operation == "GetFolder" && asked.body.contains("Id=\"archivemsgfolderroot\""));
    assert!(state.writes.is_empty(), "asking writes nothing");
}

#[test]
fn a_server_whose_schema_hasnt_the_name_has_no_archive() {
    let standin = StandIn::new();
    standin.state().add_archive(true);
    standin.state().unknown_names = vec!["archivemsgfolderroot".into()];
    assert_eq!(probe(&standin), ArchiveProbe::Absent);
}

#[test]
fn what_says_nothing_is_unknown_not_absent() {
    // A refused sign-in, a policy block, a server failing without a fault: nothing is said of the
    // archive, so what was known before stays.
    for status in [401, 403, 500, 503] {
        let standin = StandIn::new();
        standin.state().add_archive(true);
        standin.state().refuse_names = Some((vec!["archivemsgfolderroot".into()], status));
        assert_eq!(probe(&standin), ArchiveProbe::Unknown, "{status}");
    }
    let standin = StandIn::new();
    standin.state().add_archive(true);
    standin.state().busy = (10, 2_000);
    assert_eq!(probe(&standin), ArchiveProbe::Unknown, "still busy after the waits");
}

#[test]
fn a_hostile_answer_is_never_an_archive() {
    for how in [
        Hostile::Dtd,
        Hostile::CutShort,
        Hostile::NotXml,
        Hostile::Deep,
        Hostile::Huge,
        Hostile::Fault,
        Hostile::TooFew,
        Hostile::BadId,
        Hostile::BadItems,
    ] {
        let standin = StandIn::new();
        standin.state().add_archive(true);
        standin.state().hostile = Some(how);
        let found = probe(&standin);
        if how != Hostile::BadItems {
            assert_ne!(found, ArchiveProbe::Present, "{how:?}");
        }
    }
}

#[test]
fn a_cancelled_probe_asks_nothing() {
    let standin = StandIn::new();
    standin.state().add_archive(true);
    let found = block_on(probe_archive(&client(), &standin, &AtomicBool::new(true), &mut |_| true)).unwrap();
    assert_eq!(found, ArchiveProbe::Unknown);
    assert!(standin.state().requests.is_empty());
}

// ---- a run of the archive ----

/// A stand-in with a folder of the owner's in the mailbox, and an archive with a folder of its own
/// and a message in it. The ids of the mailbox's folder and the archive's.
fn both(standin: &StandIn, inbox: bool) -> (String, String) {
    let mut s = standin.state();
    let mine = s.add_folder("Projects");
    s.add_archive(inbox);
    let old = s.add_own_folder("", "2019", "archivemsgfolderroot");
    s.deliver(&old, "Budget 2019", true);
    (mine, old)
}

#[test]
fn a_run_of_the_archive_reads_its_folders_and_none_of_the_mailboxs() {
    let standin = StandIn::new();
    let (mine, old) = both(&standin, true);
    let result = run(&standin, Store::Archive, &request());
    assert!(result.errors.is_empty() && result.whole, "{:?}", result.errors);
    let folders = result.folders.as_ref().unwrap();
    let ids: Vec<&str> = folders.changed.iter().map(|f| f.id.as_str()).collect();
    assert!(ids.contains(&old.as_str()) && !ids.contains(&mine.as_str()), "the archive's own folders only");
    let inbox = standin.state().folder_id("inbox").unwrap();
    assert!(!ids.contains(&inbox.as_str()));
    let kind = |name: &str| folders.changed.iter().find(|f| f.name == name).map(|f| f.kind);
    assert_eq!(kind("Inbox"), Some(FolderKind::Inbox));
    assert_eq!(kind("Deleted Items"), Some(FolderKind::Deleted));
    assert_eq!(kind("2019"), Some(FolderKind::Custom));
    assert!(folders.changed.iter().all(|f| f.parent.is_empty()), "the archive's top is the top here");
    assert!(result.pages.iter().flat_map(|p| &p.messages).any(|m| m.subject == "Budget 2019"), "its mail is read");
    // (The archive has none of the server's own folders of the mailbox: they aren't asked for.)
    let state = standin.state();
    assert!(state.requests.iter().all(|r| !r.body.contains("conversationhistory")));
    assert!(state.writes.is_empty());
}

#[test]
fn a_run_of_the_mailbox_reads_none_of_the_archives_folders() {
    let standin = StandIn::new();
    let (mine, old) = both(&standin, true);
    let result = run(&standin, Store::Primary, &request());
    let folders = result.folders.as_ref().unwrap();
    assert!(folders.changed.iter().any(|f| f.id == mine));
    assert!(!folders.changed.iter().any(|f| f.id == old || f.name == "Top of Archive"), "the archive stays out of the mailbox");
    assert!(!result.pages.iter().flat_map(|p| &p.messages).any(|m| m.subject == "Budget 2019"));
}

#[test]
fn an_archive_without_an_inbox_or_a_name_for_it_is_read_all_the_same() {
    let standin = StandIn::new();
    let (_, old) = both(&standin, false);
    let result = run(&standin, Store::Archive, &request());
    assert!(result.errors.is_empty() && result.whole, "{:?}", result.errors);
    assert!(result.folders.as_ref().unwrap().changed.iter().any(|f| f.id == old));
    let standin = StandIn::new();
    let (_, old) = both(&standin, true);
    standin.state().unknown_names = vec!["archiveinbox".into()];
    let result = run(&standin, Store::Archive, &request());
    assert!(result.errors.is_empty() && result.whole, "{:?}", result.errors);
    assert!(result.folders.as_ref().unwrap().changed.iter().any(|f| f.id == old));
}

#[test]
fn a_run_of_an_archive_the_server_doesnt_have_says_so_and_reads_nothing() {
    let standin = StandIn::new();
    standin.state().deliver("inbox", "Kickoff", false);
    let result = run(&standin, Store::Archive, &request());
    assert!(!result.whole && result.errors.iter().any(|e| e.contains("online archive")), "{:?}", result.errors);
    assert!(result.pages.is_empty(), "nothing of the mailbox is read in its place");
}

#[test]
fn a_folder_made_at_the_top_of_the_archive_is_made_there() {
    let standin = StandIn::new();
    both(&standin, true);
    let first = run(&standin, Store::Archive, &request());
    let changes = vec![FolderChange::Create { folder: 7, name: "Taxes".into(), parent: String::new(), parent_folder: None }];
    let result = run(&standin, Store::Archive, &EwsMailRequest { changes, hierarchy: first.hierarchy.clone(), ..request() });
    assert!(matches!(&result.changes[0].outcome, ChangeOutcome::Done { .. }), "{:?}", result.changes[0].outcome);
    let state = standin.state();
    let top = state.folder_id("archivemsgfolderroot").unwrap();
    assert!(state.folders.iter().any(|f| f.name == "Taxes" && f.parent == top), "under the archive's top, not the mailbox's");
}

#[test]
fn only_a_definite_no_says_there_is_no_archive() {
    use crate::error::ResponseCode;
    // The server has no such folder, or its schema doesn't know the name or the version.
    for code in [ResponseCode::FolderNotFound, ResponseCode::SchemaValidation, ResponseCode::InvalidRequest, ResponseCode::InvalidServerVersion] {
        assert!(crate::run::says_no_archive(&code), "{code}");
    }
    // Access denied, a mailbox being moved, a server failing for now, anything else: nothing is
    // said, and two of them don't take an archive that is there off the pane.
    for code in [
        ResponseCode::AccessDenied,
        ResponseCode::MailboxUnavailable,
        ResponseCode::ServerBusy,
        ResponseCode::InternalServerError,
        ResponseCode::TimeoutExpired,
        ResponseCode::NonExistentMailbox,
        ResponseCode::ItemNotFound,
        ResponseCode::Other("ErrorMailboxMoveInProgress".into()),
        ResponseCode::Other("ErrorMailboxStoreUnavailable".into()),
    ] {
        assert!(!crate::run::says_no_archive(&code), "{code}");
    }
}
