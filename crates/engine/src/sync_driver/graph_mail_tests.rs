// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_model::{Account, AccountKind, Mailbox, OAuth, OAuthProvider};

use super::graph_mail::*;
use crate::Session;

fn session(kind: AccountKind, oauth: bool) -> (Session, u64) {
    let mut mb = Mailbox::with_local_account("Me", "me@example.org");
    let id = mb.accounts[0].id;
    let a: &mut Account = &mut mb.accounts[0];
    a.kind = kind;
    a.oauth = oauth.then(|| OAuth { provider: OAuthProvider::Microsoft, ..Default::default() });
    (Session::new(mb), id)
}

#[test]
fn no_account_or_no_sign_in_is_a_refusal_not_a_panic() {
    let (s, id) = session(AccountKind::Microsoft, false);
    assert_eq!(plan_graph_mail(&s, id + 100, true, "r".into()).unwrap_err(), Refusal::AccountGone);
    assert_eq!(plan_graph_mail(&s, id, true, "r".into()).unwrap_err(), Refusal::NoSignIn);
}

#[test]
fn without_a_refresh_token_a_quiet_run_asks_nobody_and_an_asked_for_one_signs_in() {
    let (s, id) = session(AccountKind::Microsoft, true);
    assert!(matches!(plan_graph_mail(&s, id, true, String::new()), Ok(MailStart::Quiet { address }) if address == "me@example.org"));
    assert!(matches!(plan_graph_mail(&s, id, false, String::new()), Ok(MailStart::SignIn { address, .. }) if address == "me@example.org"));
}

#[test]
fn with_a_refresh_token_the_run_goes_ahead_with_the_mailboxs_request() {
    let (s, id) = session(AccountKind::Microsoft, true);
    match plan_graph_mail(&s, id, true, "refresh".into()) {
        Ok(MailStart::Run(plan)) => assert_eq!((plan.mail.account, plan.refresh.as_str()), (id, "refresh")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_account_not_kept_in_step_through_graph_is_refused() {
    let (s, id) = session(AccountKind::Imap, true);
    assert!(matches!(plan_graph_mail(&s, id, true, "refresh".into()), Err(Refusal::NotGraph(why)) if why.contains("Graph")));
}

#[test]
fn a_plans_debug_text_never_holds_the_refresh_token() {
    let (s, id) = session(AccountKind::Microsoft, true);
    let start = plan_graph_mail(&s, id, true, "fake-refresh-token-123".into()).unwrap();
    assert!(!format!("{start:?}").contains("fake-refresh-token-123"));
}
