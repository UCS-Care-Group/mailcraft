// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The sign-ins a phone makes through a custom scheme and the words they end in: a fake token
//! endpoint and a hand-back that plays the system. Nothing leaves this machine.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::*;
use crate::fake::{FakeHttp, json_answer};

const SCHEME: &str = "org.example.mail";
const PATH: &str = "/oauth2redirect";
const TOKENS: &str = r#"{"token_type":"Bearer","expires_in":3599,"access_token":"acc-ess.tok_en","refresh_token":"re-fresh.tok_en"}"#;

fn oauth() -> OAuth {
    OAuth { provider: OAuthProvider::Microsoft, client_id: "client-1".into(), tenant: String::new(), ..Default::default() }
}

fn endpoints_of(server: &FakeHttp, permission: Option<Permission>) -> Endpoints {
    let real = match permission {
        Some(permission) => permission_endpoints(&oauth(), permission),
        None => Endpoints::for_resource(&oauth(), Resource::GraphMail),
    };
    Endpoints { authorize: server.url("/authorize"), token: server.url("/token"), ..real }
}

type Seen = Arc<Mutex<String>>;

fn asked_of(seen: &Seen) -> HashMap<String, String> {
    let url = seen.lock().unwrap().clone();
    http::pairs(url.split_once('?').map_or("", |(_, query)| query)).into_iter().collect()
}

/// Run `go` with a hand-back that gives `template` (with `{state}` filled in).
fn back<R>(template: &str, go: impl FnOnce(&RedirectStrategy, &dyn Fn(&str)) -> R) -> (R, Seen) {
    let seen = Seen::default();
    let (for_handback, template) = (seen.clone(), template.to_string());
    let handback = move |_: &AtomicBool, _: Duration| -> Result<String, NetError> {
        let state = asked_of(&for_handback).get("state").cloned().unwrap_or_default();
        Ok(template.replace("{state}", &state))
    };
    let via = RedirectStrategy::CustomScheme(CustomScheme::new(SCHEME, PATH, &handback).unwrap());
    let noting = seen.clone();
    let open = move |url: &str| *noting.lock().unwrap() = url.to_string();
    (go(&via, &open), seen)
}

#[test]
fn each_permission_is_asked_for_by_itself_and_never_with_the_rest() {
    let scope = |p| permission_endpoints(&oauth(), p).scope;
    let tasks = scope(Permission::Tasks);
    assert!(tasks.contains("Tasks.ReadWrite") && !tasks.contains("Shared") && !tasks.contains("Mail.ReadWrite"), "{tasks}");
    let shared = scope(Permission::SharedMail);
    assert!(shared.contains("Mail.ReadWrite.Shared") && !shared.contains("Send.Shared") && !shared.contains("Tasks"), "{shared}");
    let send = scope(Permission::SharedSend);
    assert!(send.contains("Mail.ReadWrite.Shared") && send.contains("Mail.Send.Shared") && !send.contains("Tasks"), "{send}");
    // Nothing else is put to the owner on the page beside it.
    for p in [Permission::Tasks, Permission::SharedMail, Permission::SharedSend] {
        let e = permission_endpoints(&oauth(), p);
        assert!(e.consent.is_empty() && e.lesser.is_empty());
    }
}

#[test]
fn a_permission_brings_tokens_through_the_custom_scheme() {
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let (result, seen) = back(&format!("{SCHEME}:{PATH}?code=the-code&state={{state}}"), |via, open| {
        sign_in_for_permission_at(&endpoints_of(&server, Some(Permission::Tasks)), &oauth(), "ada@example.org", open, &AtomicBool::new(false), via)
    });
    let tokens = result.unwrap();
    assert_eq!(tokens.refresh, "re-fresh.tok_en");
    let page = asked_of(&seen);
    assert!(page["scope"].contains("Tasks.ReadWrite"));
    assert_eq!(page["redirect_uri"], format!("{SCHEME}:{PATH}"));
    assert_eq!(page["login_hint"], "ada@example.org");
}

#[test]
fn a_permissions_no_says_whose_it_is() {
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let no = |code: &str| format!("{SCHEME}:{PATH}?error=access_denied&error_description=AADSTS{code}%3A+No.&state={{state}}");
    for (code, expected) in [("90094", FavoritesRefusal::Approval), ("53003", FavoritesRefusal::Organisation)] {
        let (result, _) = back(&no(code), |via, open| {
            sign_in_for_permission_at(&endpoints_of(&server, Some(Permission::SharedMail)), &oauth(), "", open, &AtomicBool::new(false), via)
        });
        assert_eq!(result.unwrap_err(), expected, "{code}");
    }
    assert_eq!(server.asked(), 0);
}

#[test]
fn a_permission_is_not_asked_of_google() {
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let google = OAuth { provider: OAuthProvider::Google, ..oauth() };
    let (result, seen) = back("", |via, open| {
        sign_in_for_permission_at(&endpoints_of(&server, Some(Permission::Tasks)), &google, "", open, &AtomicBool::new(false), via)
    });
    assert!(matches!(result, Err(FavoritesRefusal::Other(_))));
    assert!(seen.lock().unwrap().is_empty(), "no page was opened");
}

#[test]
fn a_sign_in_says_how_it_ended() {
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let ending = |address: String| {
        back(&address, |via, open| sign_in_ending_via(&endpoints_of(&server, None), &oauth(), "", open, &AtomicBool::new(false), via)).0
    };
    let denied = |code: &str| format!("{SCHEME}:{PATH}?error=access_denied&error_description=AADSTS{code}%3A+No.&state={{state}}");
    assert_eq!(ending(denied("90094")).unwrap_err(), Ending::Approval);
    // Conditional Access: the organisation's own rules refuse the sign-in.
    assert_eq!(ending(denied("53003")).unwrap_err(), Ending::Blocked);
    assert_eq!(ending(denied("7000112")).unwrap_err(), Ending::Blocked);
    assert_eq!(ending(denied("65004")).unwrap_err(), Ending::Declined);
    assert!(matches!(ending(format!("{SCHEME}:{PATH}?code=c&state=not-it")).unwrap_err(), Ending::Other(_)));
    assert!(ending(format!("{SCHEME}:{PATH}?code=c&state={{state}}")).is_ok());
}

#[test]
fn the_host_ending_the_wait_is_a_timeout_or_a_call_off() {
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let run = |error: NetError| {
        let handback = move |_: &AtomicBool, _: Duration| -> Result<String, NetError> { Err(error.clone()) };
        let via = RedirectStrategy::CustomScheme(CustomScheme::new(SCHEME, PATH, &handback).unwrap());
        sign_in_ending_via(&endpoints_of(&server, None), &oauth(), "", &|_| {}, &AtomicBool::new(false), &via)
    };
    assert_eq!(run(NetError::SignInTimedOut).unwrap_err(), Ending::TimedOut);
    assert_eq!(run(NetError::SignInCancelled).unwrap_err(), Ending::Cancelled);
}

#[test]
fn google_is_not_signed_in_through_a_custom_scheme() {
    let server = FakeHttp::answering(json_answer("200 OK", TOKENS));
    let google = OAuth { provider: OAuthProvider::Google, ..oauth() };
    let (result, seen) = back("", |via, open| sign_in_ending_via(&endpoints_of(&server, None), &google, "", open, &AtomicBool::new(false), via));
    assert!(matches!(result, Err(Ending::Other(_))) && seen.lock().unwrap().is_empty());
}
