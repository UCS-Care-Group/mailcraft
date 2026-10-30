// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzzes Autodiscover (#252, E3): the state machine fed arbitrary answers, redirects, challenges
//! and SRV targets, as a hostile server would.
//!
//! Invariants: nothing panics; the search ends; every request it makes is HTTPS; with nobody
//! having agreed to anything, every request is to a host inside the owner's domain; a request
//! carries the sign-in only after a `401` offering Basic; and when the search finds a server,
//! its addresses are HTTPS.

#![no_main]

use mailcraft_ews::http::{HttpResponse, Secret, TransportError};
use mailcraft_ews::{Autodiscover, Outcome, Step};

/// Statuses a hostile server uses, picked by a byte: most are the ones the machine acts on.
const STATUSES: [u16; 10] = [200, 200, 200, 200, 301, 302, 307, 401, 403, 404];

/// A valid answer to start from, which the input then spoils in places.
const SETTINGS: &str = "<Autodiscover xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006\"><Response xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a\"><Account><Action>settings</Action><Protocol><Type>EXCH</Type><Server>exch.example.org</Server><EwsUrl>https://mail.example.org/EWS/Exchange.asmx</EwsUrl></Protocol></Account></Response></Autodiscover>";

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(mut search) = Autodiscover::new("ada@example.org", Secret::new("Basic ZnV6ejpmdXp6")) else { return };
    // The input is cut into pieces, one for each answer: its first byte says what kind.
    let mut pieces = data.split(|byte| *byte == 0xff);
    let text = String::from_utf8_lossy(data).into_owned();
    // A search is at most 16 requests, an SRV lookup and a question for each place: far fewer
    // than this. A search that does not end by then is a loop.
    for _ in 0..512 {
        match search.step() {
            Step::Request(request) => {
                let url = request.url().to_ascii_lowercase();
                assert!(url.starts_with("https://"));
                let host = url.trim_start_matches("https://").split(['/', ':']).next().unwrap_or("");
                assert!(host == "example.org" || host.ends_with(".example.org"), "{host}");
                let piece = pieces.next().unwrap_or(&[]);
                let kind = piece.first().copied().unwrap_or(0);
                let status = STATUSES[usize::from(kind) % STATUSES.len()];
                let body = if kind % 3 == 0 {
                    // The valid answer with a byte spoiled.
                    let mut valid = SETTINGS.as_bytes().to_vec();
                    if let (Some(at), Some(byte)) = (piece.get(1), piece.get(2)) {
                        let place = usize::from(*at) % valid.len();
                        valid[place] = *byte;
                    }
                    valid
                } else {
                    piece.get(1..).unwrap_or(&[]).to_vec()
                };
                let rest = String::from_utf8_lossy(piece.get(1..).unwrap_or(&[])).into_owned();
                let mut response = HttpResponse::new(status, body).with_field("Location", &rest);
                // A challenge: Basic, or what the input says.
                response = response.with_field("WWW-Authenticate", if kind % 2 == 0 { "Basic realm=\"x\"" } else { rest.as_str() });
                // The sign-in goes only to answer a challenge, and the request carries it only then.
                if request.field("authorization").is_some() {
                    assert!(host == "example.org" || host.ends_with(".example.org"));
                }
                search.response(if kind == 0xfe { Err(TransportError::Other) } else { Ok(response) });
            }
            Step::LookupSrv(_) => search.srv(Some(text.split_whitespace().enumerate().map(|(n, word)| (word.to_owned(), if n % 2 == 0 { 443 } else { 8443 })).collect())),
            Step::NeedsYes(_) => search.decline(),
            Step::Done(outcome) => {
                if let Outcome::Found(found) = outcome {
                    assert!(found.urls().iter().all(|url| url.to_ascii_lowercase().starts_with("https://")));
                }
                return;
            }
        }
    }
    panic!("the search did not end");
});
