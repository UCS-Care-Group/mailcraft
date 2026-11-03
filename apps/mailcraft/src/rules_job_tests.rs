// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What this host's rules job asks for, and when (#566): a token that may change the rules only
//! when something waits to go, and nothing sent without it. Every case here ends before Graph
//! would be asked: nothing leaves this machine.

use std::cell::Cell;

use mailcraft_model::remote_rules::{RuleOp, RuleOpKind, RulesUpload};
use mailcraft_model::{Account, AccountKind};
use mailcraft_net::NetError;
use mailcraft_net::oauth::GraphTokens;
use mailcraft_ui_egui::rule_sync::RulesJob;

use super::*;

fn job(ops: Vec<RuleOp>) -> RulesJob {
    let account = Account { id: 3, kind: AccountKind::Microsoft, email: "ada@example.org".into(), ..Default::default() };
    RulesJob {
        upload: RulesUpload { account: 3, email: account.email.clone(), ops },
        account,
        oauth: mailcraft_model::OAuth::default(),
        refresh: "re-fresh".into(),
        folders: Vec::new(),
        cancel: Default::default(),
    }
}

fn delete() -> RuleOp {
    RuleOp { kind: RuleOpKind::Delete, id: "R1".into(), ..Default::default() }
}

fn over() -> NetError {
    NetError::Proto(mailcraft_proto::ProtoError::Auth("the sign-in is over".into()))
}

#[test]
fn nothing_waiting_never_asks_for_leave_to_change_the_rules() {
    let asked_write = Cell::new(false);
    let write = |_: &mailcraft_model::OAuth, _: &str| -> Result<GraphTokens, NetError> {
        asked_write.set(true);
        Err(over())
    };
    let read = |_: &mailcraft_model::OAuth, _: &str| -> Result<GraphTokens, NetError> { Err(NetError::ConsentNeeded("no mail".into())) };
    let out = rules_job_with(job(Vec::new()), &write, &read);
    assert!(!asked_write.get());
    assert!(out.unreadable && !out.consent_needed && out.uploaded.is_none());
}

#[test]
fn a_sign_in_without_leave_sends_nothing_and_says_the_owner_is_to_be_asked() {
    let write = |_: &mailcraft_model::OAuth, _: &str| -> Result<GraphTokens, NetError> { Err(NetError::ConsentNeeded("not covered".into())) };
    let asked_read = Cell::new(false);
    let read = |_: &mailcraft_model::OAuth, _: &str| -> Result<GraphTokens, NetError> {
        asked_read.set(true);
        Err(over())
    };
    let out = rules_job_with(job(vec![delete()]), &write, &read);
    // Nothing went; the server's rules were to be read with the ordinary sign-in all the same.
    assert!(out.consent_needed && out.uploaded.is_none());
    assert!(asked_read.get());
    assert!(out.sign_in_refused);
}

#[test]
fn a_sign_in_that_fails_otherwise_says_so_and_reads_nothing() {
    let write = |_: &mailcraft_model::OAuth, _: &str| -> Result<GraphTokens, NetError> {
        Err(NetError::Proto(mailcraft_proto::ProtoError::Io("no connection".into())))
    };
    let read = |_: &mailcraft_model::OAuth, _: &str| -> Result<GraphTokens, NetError> { panic!("not to be read") };
    let out = rules_job_with(job(vec![delete()]), &write, &read);
    assert!(!out.consent_needed && !out.sign_in_refused && out.uploaded.is_none());
    assert_eq!(out.errors.len(), 1);
    assert!(!out.errors[0].contains("re-fresh"));
}
