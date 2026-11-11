// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What this host's categories job does (#661): a sign-in that doesn't cover changing the list
//! sends nothing and says the list is read-only; one Microsoft no longer takes, or one that fails
//! otherwise, says which; with a token, what waits is told and the list read back, and a change
//! the server won't let be made, or a token it refuses on the way, is said as the window takes
//! it. Graph is the tests' own: nothing here leaves this machine.

use std::cell::RefCell;

use mailcraft_model::outlook_colors::OUTLOOK_COLORS;
use mailcraft_model::remote::{
    CategoriesDownload, CategoriesUpload, CategoriesUploaded, CategoryChange, CategoryChangeKind, CategoryOutcome, CategoryResult,
};
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

/// Graph, for a job that must not reach it.
fn unasked() -> CategoriesGraph<'static> {
    CategoriesGraph {
        push: &|_: &str, _: &CategoriesUpload| -> CategoriesUploaded { panic!("nothing is told without a token") },
        read: &|_: &str| -> Result<CategoriesDownload, NetError> { panic!("nothing is read without a token") },
    }
}

fn given(_: &mailcraft_model::OAuth, _: &str) -> Result<GraphTokens, NetError> {
    let tokens = mailcraft_model::sync::Tokens { access: "acc-ess".into(), refresh: "re-fresh-2".into(), ..Default::default() };
    Ok(GraphTokens { tokens, extras_missing: false, covers_favorites: false })
}

/// What Graph made of the change: `result` for each, or the token refused before any was told.
fn told(result: Option<CategoryResult>) -> impl Fn(&str, &CategoriesUpload) -> CategoriesUploaded {
    move |token: &str, upload: &CategoriesUpload| {
        assert_eq!(token, "acc-ess");
        let outcomes = result
            .map(|result| {
                let o = |c: &CategoryChange| CategoryOutcome {
                    kind: c.kind,
                    name: c.name.clone(),
                    color: c.color,
                    result,
                    error: String::new(),
                    found: false,
                };
                upload.changes.iter().map(o).collect()
            })
            .unwrap_or_default();
        CategoriesUploaded { account: upload.account, email: upload.email.clone(), outcomes, token_refused: result.is_none() }
    }
}

#[test]
fn with_a_token_what_waits_is_told_and_the_list_read_back() {
    let reads = RefCell::new(0);
    let read = |token: &str| -> Result<CategoriesDownload, NetError> {
        assert_eq!(token, "acc-ess");
        *reads.borrow_mut() += 1;
        let travel = mailcraft_model::Category { name: "Travel".into(), color: OUTLOOK_COLORS[4] };
        Ok(CategoriesDownload { account: 0, email: String::new(), categories: vec![travel], complete: true })
    };
    let push = told(Some(CategoryResult::Done));
    let out = categories_job_with(job(vec![create()]), &given, &CategoriesGraph { push: &push, read: &read });
    assert_eq!(out.covered, Some(true));
    assert_eq!(out.tokens.as_ref().map(|t| t.refresh.as_str()), Some("re-fresh-2"));
    let uploaded = out.uploaded.unwrap();
    assert_eq!((uploaded.account, uploaded.outcomes.len(), uploaded.outcomes[0].result), (3, 1, CategoryResult::Done));
    assert!(!out.denied && !out.sign_in_refused && out.errors.is_empty());
    let download = out.download.unwrap();
    assert_eq!((download.account, download.email.as_str(), download.categories.len()), (3, "ada@example.org", 1));
    assert_eq!(*reads.borrow(), 1);
}

#[test]
fn a_change_the_server_wont_let_be_made_is_said_denied_and_the_list_still_read() {
    let read = |_: &str| -> Result<CategoriesDownload, NetError> { Ok(CategoriesDownload { complete: true, ..Default::default() }) };
    let push = told(Some(CategoryResult::Denied));
    let out = categories_job_with(job(vec![create()]), &given, &CategoriesGraph { push: &push, read: &read });
    assert!(out.denied && out.download.is_some() && !out.sign_in_refused);
}

#[test]
fn a_token_refused_while_the_changes_go_is_said_and_nothing_more_is_asked() {
    let push = told(None);
    let read = |_: &str| -> Result<CategoriesDownload, NetError> { panic!("not read with a refused token") };
    let out = categories_job_with(job(vec![create()]), &given, &CategoriesGraph { push: &push, read: &read });
    assert!(out.sign_in_refused && out.uploaded.is_some_and(|u| u.token_refused) && out.download.is_none());
}

#[test]
fn with_nothing_waiting_nothing_is_told_and_the_list_is_read() {
    let push = |_: &str, _: &CategoriesUpload| -> CategoriesUploaded { panic!("nothing waits") };
    let read = |_: &str| -> Result<CategoriesDownload, NetError> { Ok(CategoriesDownload { complete: true, ..Default::default() }) };
    let out = categories_job_with(job(Vec::new()), &given, &CategoriesGraph { push: &push, read: &read });
    assert!(out.uploaded.is_none() && out.download.is_some() && out.covered == Some(true));
}

#[test]
fn a_sign_in_that_doesnt_cover_changing_the_list_sends_nothing() {
    let renew = |_: &mailcraft_model::OAuth, _: &str| -> Result<GraphTokens, NetError> { Err(NetError::ConsentNeeded("not covered".into())) };
    let out = categories_job_with(job(vec![create()]), &renew, &unasked());
    assert_eq!(out.covered, Some(false));
    assert!(out.uploaded.is_none() && out.download.is_none() && out.errors.is_empty() && !out.sign_in_refused);
}

#[test]
fn a_sign_in_microsoft_no_longer_takes_is_told_apart_from_a_failure() {
    let over = |_: &mailcraft_model::OAuth, _: &str| -> Result<GraphTokens, NetError> {
        Err(NetError::Proto(mailcraft_proto::ProtoError::Auth("the sign-in is over".into())))
    };
    let out = categories_job_with(job(vec![create()]), &over, &unasked());
    assert!(out.sign_in_refused && out.covered.is_none() && out.errors.is_empty());

    let down = |_: &mailcraft_model::OAuth, _: &str| -> Result<GraphTokens, NetError> {
        Err(NetError::Proto(mailcraft_proto::ProtoError::Io("no connection".into())))
    };
    let out = categories_job_with(job(vec![create()]), &down, &unasked());
    assert!(!out.sign_in_refused && out.covered.is_none() && out.uploaded.is_none());
    assert_eq!(out.errors.len(), 1);
    assert!(!out.errors[0].contains("re-fresh"), "{}", out.errors[0]);
}

/// A read of the list that fails (no connection, a server error) is a failure of the job, said
/// as one; the calendar's permission not covering it says nothing here (its job says it).
#[test]
fn a_read_of_the_list_that_fails_is_a_failure() {
    let read = |_: &str| -> Result<CategoriesDownload, NetError> { Err(NetError::Proto(mailcraft_proto::ProtoError::Io("no connection".into()))) };
    for changes in [Vec::new(), vec![create()]] {
        let push = told(Some(CategoryResult::Done));
        let out = categories_job_with(job(changes), &given, &CategoriesGraph { push: &push, read: &read });
        assert!(out.download.is_none() && out.errors.len() == 1, "{:?}", out.errors);
    }
    let read = |_: &str| -> Result<CategoriesDownload, NetError> { Err(NetError::Forbidden("calendar".into())) };
    let push = |_: &str, _: &CategoriesUpload| -> CategoriesUploaded { panic!("nothing waits") };
    let out = categories_job_with(job(Vec::new()), &given, &CategoriesGraph { push: &push, read: &read });
    assert!(out.errors.is_empty());
}
