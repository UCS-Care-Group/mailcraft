// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What this host's categories job does before Graph would be asked (#661): a sign-in that
//! doesn't cover changing the list sends nothing and says the list is read-only; one Microsoft no
//! longer takes, or one that fails otherwise, says which. Nothing here leaves this machine.

use mailcraft_model::outlook_colors::OUTLOOK_COLORS;
use mailcraft_model::remote::{CategoriesUpload, CategoryChange, CategoryChangeKind};
use mailcraft_model::{Account, AccountKind};
use mailcraft_net::NetError;
use mailcraft_net::oauth::GraphTokens;
use mailcraft_ui_egui::category_sync::CategoriesJob;

use super::*;

fn job(changes: Vec<CategoryChange>) -> CategoriesJob {
    let account = Account { id: 3, kind: AccountKind::Microsoft, email: "ada@example.org".into(), ..Default::default() };
    CategoriesJob {
        upload: CategoriesUpload { account: 3, email: account.email.clone(), changes },
        account,
        oauth: mailcraft_model::OAuth::default(),
        refresh: "re-fresh".into(),
        cancel: Default::default(),
    }
}

fn create() -> CategoryChange {
    CategoryChange { kind: CategoryChangeKind::Create, name: "Travel".into(), color: OUTLOOK_COLORS[4] }
}

#[test]
fn a_sign_in_that_doesnt_cover_changing_the_list_sends_nothing() {
    let renew = |_: &mailcraft_model::OAuth, _: &str| -> Result<GraphTokens, NetError> { Err(NetError::ConsentNeeded("not covered".into())) };
    let out = categories_job_with(job(vec![create()]), &renew);
    assert_eq!(out.covered, Some(false));
    assert!(out.uploaded.is_none() && out.download.is_none() && out.errors.is_empty() && !out.sign_in_refused);
}

#[test]
fn a_sign_in_microsoft_no_longer_takes_is_told_apart_from_a_failure() {
    let over = |_: &mailcraft_model::OAuth, _: &str| -> Result<GraphTokens, NetError> {
        Err(NetError::Proto(mailcraft_proto::ProtoError::Auth("the sign-in is over".into())))
    };
    let out = categories_job_with(job(vec![create()]), &over);
    assert!(out.sign_in_refused && out.covered.is_none() && out.errors.is_empty());

    let down = |_: &mailcraft_model::OAuth, _: &str| -> Result<GraphTokens, NetError> {
        Err(NetError::Proto(mailcraft_proto::ProtoError::Io("no connection".into())))
    };
    let out = categories_job_with(job(vec![create()]), &down);
    assert!(!out.sign_in_refused && out.covered.is_none() && out.uploaded.is_none());
    assert_eq!(out.errors.len(), 1);
    assert!(!out.errors[0].contains("re-fresh"), "{}", out.errors[0]);
}
