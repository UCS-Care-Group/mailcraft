// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The contacts part of a calendar job (#1917) when the sign-in doesn't cover changing contacts:
//! nothing goes up, each change waits, and the window is told to ask. (What goes up when it does
//! is in mailcraft-net's graph/contacts_tests.rs, against a stand-in for Graph.)

use std::cell::Cell;

use mailcraft_model::contact_sync::{ContactChangeKind, ContactOp, ContactResult, ContactsUpload, FolderOwner};
use mailcraft_net::NetError;
use mailcraft_net::oauth::GraphTokens;

use super::contacts_part;
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
    assert!(got.consent_needed && got.downloads.is_empty() && got.refresh.is_none());
    let uploaded = got.uploaded.unwrap();
    assert_eq!(uploaded.outcomes.len(), 1);
    assert_eq!(uploaded.outcomes[0].result, ContactResult::Failed);
    assert!(!uploaded.token_refused);
}

#[test]
fn a_renewal_that_fails_otherwise_holds_the_changes_without_asking_the_owner() {
    let busy = |_: bool| -> Result<GraphTokens, NetError> { Err(NetError::Config("no sign-in to renew".into())) };
    let got = contacts_part(&ask(&[]), "access", &busy, None);
    assert!(!got.consent_needed);
    let outcome = &got.uploaded.unwrap().outcomes[0];
    assert_eq!(outcome.result, ContactResult::Failed);
    assert!(!outcome.error.is_empty());
}

#[test]
fn nothing_waiting_and_nothing_to_read_asks_for_no_token() {
    let never = |_: bool| -> Result<GraphTokens, NetError> { panic!("no token is wanted") };
    let idle = ContactsAsk { upload: ContactsUpload::default(), ..ask(&[]) };
    let got = contacts_part(&idle, "access", &never, None);
    assert!(got.uploaded.is_none() && got.downloads.is_empty() && !got.consent_needed);
}
