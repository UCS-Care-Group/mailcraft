// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_jmap::{Error, TransportError};
use mailcraft_model::sync::{JmapMailRequest, JmapMailResult};

use crate::jmap_rules::*;

const SESSION: &str = "https://mail.example.org/jmap/session";

fn page() -> Vec<PageAccount> {
    vec![PageAccount { email: "ada@example.org".into(), session: SESSION.into(), bearer: true, name: String::new() }]
}

#[test]
fn the_page_holds_an_account_only_at_the_server_it_named() {
    let accounts = page();
    assert!(named(&accounts, "ada@example.org", SESSION));
    // The address in any case, and either one as typed with spaces around it.
    assert!(named(&accounts, " Ada@Example.ORG ", &format!(" {SESSION} ")));
    // Another server, on the same host or not, is not the one the page named; nor another address.
    for session in ["https://mail.example.org/jmap/other", "https://mail.example.org", "https://other.example/jmap/session", ""] {
        assert!(!named(&accounts, "ada@example.org", session), "{session}");
    }
    assert!(!named(&accounts, "bo@example.org", SESSION));
    assert!(!named(&[], "ada@example.org", SESSION));
}

fn refused() -> JmapMailResult {
    JmapMailResult { auth_refused: true, ..Default::default() }
}

#[test]
fn a_run_turned_down_goes_once_more_only_with_a_newer_page_credential() {
    // The first try: whatever is at hand goes, and nothing at all ends the run.
    assert!(matches!(next(Some("one".into()), None), Next::Go(s) if s == "one"));
    assert!(matches!(next(None, None), Next::NoSecret));
    // The page's turned down: tried once more.
    assert!(again(&refused(), true, false));
    // With a newer one it goes; with the same one, or none, it ends as the server answered.
    assert!(matches!(next(Some("two".into()), Some(("one".into(), refused()))), Next::Go(s) if s == "two"));
    assert!(matches!(next(Some("one".into()), Some(("one".into(), refused()))), Next::End(last) if last.auth_refused));
    assert!(matches!(next(None, Some(("one".into(), refused()))), Next::End(last) if last.auth_refused));
    // Never a third time; never for a secret the owner typed (the window asks for it); never for
    // a run that wasn't turned down.
    assert!(!again(&refused(), true, true));
    assert!(!again(&refused(), false, false));
    assert!(!again(&JmapMailResult::default(), true, false));
}

#[test]
fn a_run_stopped_before_it_read_anything_says_it_was_stopped_and_nothing_else() {
    let request = JmapMailRequest { account: 7, email: "ada@example.org".into(), session: SESSION.into(), ..Default::default() };
    let last = stopped(&request);
    assert!(last.stopped && !last.partial && !last.auth_refused && last.errors.is_empty());
    assert_eq!((last.account, last.email.as_str(), last.session.as_str()), (7, "ada@example.org", SESSION));
}

#[test]
fn a_redirect_the_browser_does_not_follow_says_which_address_to_give() {
    let redirect = Error::Transport(TransportError::RedirectRefused);
    for downloading in [false, true] {
        let line = redirect_line(&redirect, downloading).unwrap();
        assert!(line.contains("address of the JMAP session itself") && line.contains("/.well-known/jmap"), "{line}");
    }
    assert!(redirect_line(&redirect, true).unwrap().starts_with("Couldn't download"));
    for other in [Error::Transport(TransportError::Unreachable), Error::Transport(TransportError::Aborted), Error::CredentialsRefused] {
        assert_eq!(redirect_line(&other, false), None, "{other:?}");
    }
}
