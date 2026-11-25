// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The contacts part of a calendar job (#1917) when the sign-in doesn't cover changing contacts:
//! nothing goes up, each change waits, and the window is told to ask. (What goes up when it does
//! is in mailcraft-net's graph/contacts_tests.rs, against a stand-in for Graph.)

use std::cell::Cell;

use mailcraft_model::contact_sync::{ContactChangeKind, ContactOp, ContactResult, ContactsUpload, FolderOwner};
use mailcraft_net::NetError;
use mailcraft_net::oauth::GraphTokens;

use super::{contacts_part, keep_newest, refused_folders};
use crate::graph::ContactsAsk;

fn ask(mailboxes: &[&str]) -> ContactsAsk {
    let op = ContactOp { kind: ContactChangeKind::Delete, contact: 3, remote: "c-bo".into(), owner: FolderOwner::Own, ..Default::default() };
    ContactsAsk {
        upload: ContactsUpload { account: 1, email: "ada@example.org".into(), ops: vec![op] },
        // (Not read here: that would ask Graph itself.)
        read: false,
        folders: Vec::new(),
        mailboxes: mailboxes.iter().map(|m| m.to_string()).collect(),
    }
}

#[test]
fn without_the_permission_nothing_goes_up_every_change_waits_and_the_owner_is_to_be_asked() {
    let asked = Cell::new(None);
    let refuse = |shared: bool| -> Result<GraphTokens, NetError> {
        asked.set(Some(shared));
        Err(NetError::ConsentNeeded("AADSTS65001".into()))
    };
    let got = contacts_part(&ask(&["sales@example.org"]), "access", &refuse, None);
    assert_eq!(asked.get(), Some(true), "the shared mailboxes' permission is asked for with it");
    assert!(got.consent_needed && got.downloads.is_empty() && got.tokens.is_none());
    // Nothing answered: not one may have reached the server (a new one isn't taken for sent).
    assert!(got.uploaded.is_none() && !got.token_refused && !got.sign_in_refused);
}

#[test]
fn a_renewal_that_fails_otherwise_holds_the_changes_without_asking_the_owner() {
    let busy = |_: bool| -> Result<GraphTokens, NetError> { Err(NetError::Config("no sign-in to renew".into())) };
    let got = contacts_part(&ask(&[]), "access", &busy, None);
    assert!(!got.consent_needed && !got.sign_in_refused && got.uploaded.is_none());
    // Said as the job's error, so that the next job waits out the back-off.
    assert_eq!(got.errors.len(), 1);
}

#[test]
fn nothing_waiting_and_nothing_to_read_asks_for_no_token() {
    let never = |_: bool| -> Result<GraphTokens, NetError> { panic!("no token is wanted") };
    let idle = ContactsAsk { upload: ContactsUpload::default(), ..ask(&[]) };
    let got = contacts_part(&idle, "access", &never, None);
    assert!(got.uploaded.is_none() && got.downloads.is_empty() && !got.consent_needed);
}

#[test]
fn the_contacts_refresh_token_is_never_dropped() {
    use mailcraft_model::sync::Tokens;
    let newer = || Some(Tokens { access: "c-access".into(), refresh: "re-fresh-9".into(), ..Default::default() });
    // A run that used the token it was handed has none of its own: the contacts' go back whole.
    let mut kept = None;
    keep_newest(&mut kept, newer());
    assert_eq!(kept.as_ref().map(|t| t.refresh.as_str()), Some("re-fresh-9"));
    // A run with its own keeps its access token, and takes the newer refresh token.
    let mut kept = Some(Tokens { access: "run-access".into(), refresh: "re-fresh-1".into(), ..Default::default() });
    keep_newest(&mut kept, newer());
    assert_eq!(kept.as_ref().map(|t| (t.access.as_str(), t.refresh.as_str())), Some(("run-access", "re-fresh-9")));
    keep_newest(&mut kept, None);
    assert_eq!(kept.map(|t| t.refresh), Some("re-fresh-9".into()));
}

#[test]
fn a_shared_folder_that_refused_a_change_is_read_as_one_that_can_only_be_read() {
    use mailcraft_model::contact_sync::{ContactFolder, ContactOutcome, ContactsUploaded};
    let sales = FolderOwner::Mailbox("sales@example.org".into());
    let folder =
        |remote: &str, owner: &FolderOwner| ContactFolder { remote: remote.into(), owner: owner.clone(), writable: true, ..Default::default() };
    let folders = [folder("f-leads", &sales), folder("f-other", &sales), folder("f-mine", &FolderOwner::Own)];
    let denied = |remote: &str, owner: &FolderOwner| ContactOutcome {
        op: ContactOp { folder: remote.into(), owner: owner.clone(), ..Default::default() },
        result: ContactResult::Denied,
        ..Default::default()
    };
    let uploaded = ContactsUploaded { outcomes: vec![denied("f-leads", &sales), denied("f-mine", &FolderOwner::Own)], ..Default::default() };
    let read = refused_folders(&folders, Some(&uploaded));
    assert_eq!(read.iter().map(|f| f.writable).collect::<Vec<_>>(), [false, true, true]);
    assert!(refused_folders(&folders, None).iter().all(|f| f.writable));
}
