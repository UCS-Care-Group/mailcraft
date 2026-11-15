// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzzes DAV discovery (#209, D3): the state machine fed arbitrary answers, redirects, challenges,
//! SRV targets and TXT paths, as a hostile server would, with an owner who never agrees to anything.
//!
//! Invariants: nothing panics; the search ends within its request cap; every request is HTTPS;
//! no request goes to a Microsoft or Google host; a request carries the password only after the
//! same origin answered a request without it with a 401; and without the owner's yes the password
//! goes only to the account's mail server or to a host the SRV answer named.
#![no_main]

use mailcraft_dav::discovery::{Discovery, MAX_REQUESTS, Outcome, SrvAnswer, Step};
use mailcraft_dav::http::{HttpResponse, Secret, TransportError};
use mailcraft_model::{is_exchange_online, is_google_host};

/// Statuses a hostile server uses, picked by a byte: most are the ones the machine acts on.
const STATUSES: [u16; 12] = [207, 207, 207, 301, 302, 303, 307, 401, 401, 401, 403, 404];

/// A valid answer to start from, which the input then spoils in places.
const LISTING: &str = "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/dav/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/dav/p/</d:href></d:current-user-principal><c:calendar-home-set><d:href>https://other.example.net/dav/cal/</d:href></c:calendar-home-set><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>";

fn origin_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    format!("{}://{}", url.split("://").next().unwrap_or(""), rest.split(['/', '?', '#']).next().unwrap_or("")).to_ascii_lowercase()
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(mut search) = Discovery::new("ada@example.org", Secret::new("Basic ZnV6ejpmdXp6"), true, true) else { return };
    search = search.mail_host("imap.example.org");
    let text = String::from_utf8_lossy(data).into_owned();
    let named: Vec<String> = text.split_whitespace().map(|word| word.trim_end_matches('.').to_ascii_lowercase()).collect();
    let mut pieces = data.split(|byte| *byte == 0xff);
    // Origins that answered a request without the password with a 401.
    let mut challenged: Vec<String> = Vec::new();
    let mut requests = 0usize;
    for _ in 0..1024 {
        match search.step() {
            Step::Request(request) => {
                requests += 1;
                let url = request.url().to_ascii_lowercase();
                let origin = origin_of(&url);
                assert!(url.starts_with("https://"), "{url}");
                let host = origin.trim_start_matches("https://").split(':').next().unwrap_or("").to_owned();
                assert!(!is_exchange_online(&host) && !is_google_host(&host), "{host}");
                if request.field("authorization").is_some() {
                    assert!(challenged.contains(&origin), "a password went before it was asked for");
                    assert!(host == "imap.example.org" || named.contains(&host), "{host}");
                }
                let piece = pieces.next().unwrap_or(&[]);
                let kind = piece.first().copied().unwrap_or(0);
                let status = STATUSES[usize::from(kind) % STATUSES.len()];
                let body = if kind % 3 != 1 {
                    // The valid answer with a byte spoiled.
                    let mut valid = LISTING.as_bytes().to_vec();
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
                response = response.with_field("WWW-Authenticate", if kind % 2 == 0 { "Basic realm=\"x\"" } else { rest.as_str() });
                response = response.with_field("DAV", if kind % 5 < 3 { "1, calendar-access, addressbook" } else { rest.as_str() });
                if status == 401 && request.field("authorization").is_none() {
                    challenged.push(origin);
                }
                search.response(if kind == 0xfe { Err(TransportError::Failed) } else { Ok(response) });
            }
            Step::LookupSrv(_) => {
                let targets = named.iter().enumerate().map(|(n, word)| (word.clone(), if n % 2 == 0 { 443 } else { 8443 })).collect();
                search.srv(Some(SrvAnswer { targets, path: Some(text.chars().take(40).collect()) }));
            }
            Step::NeedsYes(_) => search.decline(),
            Step::Done(outcome) => {
                assert!(requests <= usize::from(MAX_REQUESTS));
                if let Outcome::Found(found) = outcome {
                    for located in [found.calendars(), found.contacts()].into_iter().flatten() {
                        assert!(located.home().starts_with("https://"));
                    }
                }
                return;
            }
        }
    }
    panic!("the search did not end");
});
