// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Choosing how to sign in (#252, E4) from what a server offers.
#![allow(clippy::unwrap_used)]

use proptest::prelude::*;

use super::*;

fn offer(headers: &[&str]) -> Vec<String> {
    headers.iter().map(|h| (*h).to_string()).collect()
}

#[test]
fn ntlm_comes_first_where_both_are_offered() {
    assert_eq!(choose(&offer(&["Basic realm=\"mail\"", "NTLM"])), Ok(Scheme::Ntlm));
    assert_eq!(choose(&offer(&["Negotiate", "NTLM", "Basic realm=\"mail\""])), Ok(Scheme::Ntlm));
    assert_eq!(choose(&offer(&["Basic realm=\"mail\", NTLM"])), Ok(Scheme::Ntlm));
    assert_eq!(choose(&offer(&["negotiate, ntlm"])), Ok(Scheme::Ntlm));
}

#[test]
fn basic_is_chosen_when_it_is_all_there_is() {
    assert_eq!(choose(&offer(&["Basic realm=\"mail\""])), Ok(Scheme::Basic));
    assert_eq!(choose(&offer(&["Negotiate", "Basic realm=\"a, NTLM\""])), Ok(Scheme::Basic));
    assert_eq!(choose(&offer(&["BASIC"])), Ok(Scheme::Basic));
}

#[test]
fn only_negotiate_is_a_typed_error() {
    assert_eq!(choose(&offer(&["Negotiate"])), Err(AuthError::NegotiateOnly));
    assert_eq!(choose(&offer(&["Negotiate YIIB"])), Err(AuthError::NegotiateOnly));
    assert!(AuthError::NegotiateOnly.to_string().contains("Kerberos"));
}

#[test]
fn nothing_usable_is_a_typed_error() {
    assert_eq!(choose(&[]), Err(AuthError::NothingSupported));
    assert_eq!(choose(&offer(&["Bearer realm=\"x\"", "Digest realm=\"y\", nonce=\"z\""])), Err(AuthError::NothingSupported));
    // A parameter or a quoted word is no scheme.
    assert_eq!(choose(&offer(&["Digest realm=\"NTLM\", qop=\"auth\""])), Err(AuthError::NothingSupported));
    assert_eq!(choose(&offer(&["Bearer error=\"basic\""])), Err(AuthError::NothingSupported));
}

#[test]
fn the_token_of_the_challenge_is_found() {
    assert_eq!(ntlm_token(&offer(&["Negotiate", "NTLM TlRMTVNTUAACAAAA=="])), Some("TlRMTVNTUAACAAAA=="));
    assert_eq!(ntlm_token(&offer(&["NTLM"])), None);
    assert_eq!(ntlm_token(&offer(&["Basic realm=\"x\""])), None);
    assert_eq!(ntlm_token(&[]), None);
}

proptest! {
    #[test]
    fn any_header_is_read_without_a_panic(headers in proptest::collection::vec(".{0,300}", 0..6)) {
        let _ = choose(&headers);
        let _ = ntlm_token(&headers);
    }
}
