// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzzes DAV discovery (#209, D3): the state machine fed arbitrary answers, redirects, challenges,
//! SRV targets and TXT paths, as a hostile server would, with an owner who agrees or not as the
//! input says.
//!
//! Invariants: nothing panics; the search ends within its request cap; every request is HTTPS;
//! no request goes to a Microsoft or Google host or to a host given by number; a request goes only
//! to a host in the owner's mail domain, the typed host, the mail server (at 443), the first three
//! valid names the SRV answer gave, or a host the owner agreed to; and a request carries the
//! password only after the same origin answered a request without it with a 401 that said it is a
//! calendar server (a `DAV` header, on that answer or on its `OPTIONS`), except for the host the
//! owner typed, which a bare Basic 401 is enough for.
#![no_main]

use mailcraft_dav::discovery::{Discovery, MAX_REQUESTS, Outcome, SrvAnswer, Step};
use mailcraft_dav::http::{HttpResponse, Method, Secret, TransportError};
use mailcraft_dav::request::Kind;
use mailcraft_model::{is_exchange_online, is_google_host};

/// Statuses a hostile server uses, picked by a byte: most are the ones the machine acts on.
const STATUSES: [u16; 14] = [207, 207, 207, 301, 302, 303, 307, 308, 401, 401, 401, 403, 404, 200];

/// A valid answer to start from, which the input then spoils in places.
const LISTING: &str = "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/dav/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/dav/p/</d:href></d:current-user-principal><c:calendar-home-set><d:href>https://other.example.net/dav/cal/</d:href></c:calendar-home-set><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>";

const TYPED: &str = "https://typed.example.net";

/// `scheme://authority`, lower case, from an address.
fn origin_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    format!("{scheme}://{}", rest.split(['/', '?', '#']).next().unwrap_or("")).to_ascii_lowercase()
}

/// A host name the SRV answer may name, as the machine reads one: labels of letters, digits and
/// hyphens, a dot, and a last label that is not all digits.
fn valid_name(word: &str) -> bool {
    word.len() <= 253
        && word.contains('.')
        && !word.rsplit('.').next().is_some_and(|last| last.bytes().all(|b| b.is_ascii_digit()))
        && word.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(mut search) = Discovery::new("ada@example.org", Secret::new("Basic ZnV6ejpmdXp6"), true, true) else { return };
    search = search.mail_host("imap.example.org");
    let typed = data.first().is_some_and(|byte| byte % 2 == 1);
    if typed {
        let Ok(next) = search.typed(Kind::Calendar, &format!("{TYPED}/dav/")) else { return };
        search = next;
    }
    let agrees = data.get(1).is_some_and(|byte| byte % 3 == 0);
    let text = String::from_utf8_lossy(data).into_owned();
    let words: Vec<String> = text.split_whitespace().map(|word| word.trim_end_matches('.').to_ascii_lowercase()).collect();
    // What the machine takes of the SRV answer: the first three distinct valid names, at 443 or 8443.
    let answer: Vec<(String, u16)> = words.iter().enumerate().map(|(n, word)| (word.clone(), if n % 2 == 0 { 443 } else { 8443 })).collect();
    let mut srv_named: Vec<String> = Vec::new();
    let mut seen: Vec<(String, u16)> = Vec::new();
    for (host, port) in &answer {
        if valid_name(host) && !seen.contains(&(host.clone(), *port)) && seen.len() < 3 {
            seen.push((host.clone(), *port));
            srv_named.push(if *port == 443 { format!("https://{host}") } else { format!("https://{host}:{port}") });
        }
    }
    let mut pieces = data.split(|byte| *byte == 0xff);
    // Origins that answered a request without the password in a way that says they are calendar
    // servers; and the hosts the owner agreed to.
    let mut evidenced: Vec<String> = Vec::new();
    let mut agreed: Vec<String> = Vec::new();
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
                assert!(host.rsplit('.').next().is_some_and(|last| !last.bytes().all(|b| b.is_ascii_digit())) && !host.starts_with('['), "{host}");
                // Never this machine, nor a name written in hexadecimal: no owner typed one here.
                assert!(host != "localhost" && !host.ends_with(".localhost") && !host.starts_with("127.") && !host.rsplit('.').next().is_some_and(|l| l.starts_with("0x") && l.get(2..).is_some_and(|d| d.bytes().all(|b| b.is_ascii_hexdigit()))), "{host}");
                // Where a request may go at all: before the owner's yes, only to the owner's own.
                let known = host == "example.org"
                    || host.ends_with(".example.org")
                    || (typed && origin == TYPED)
                    || origin == "https://imap.example.org"
                    || srv_named.contains(&origin)
                    || agreed.contains(&origin);
                assert!(known, "a request went to {origin} before the owner said yes");
                let carries = request.field("authorization").is_some();
                if carries {
                    let proven = evidenced.contains(&origin) || (typed && origin == TYPED);
                    assert!(proven, "a password went to {origin} before it said it is a calendar server");
                    assert!(host == "imap.example.org" && origin == "https://imap.example.org" || host != "imap.example.org");
                    assert!(
                        (typed && origin == TYPED) || origin == "https://imap.example.org" || srv_named.contains(&origin) || agreed.contains(&origin),
                        "the password went to {origin}, which is nobody's"
                    );
                }
                let piece = pieces.next().unwrap_or(&[]);
                let kind = piece.first().copied().unwrap_or(0);
                let options = request.method() == Method::Options;
                let status = if options { [200, 204, 401, 404][usize::from(kind) % 4] } else { STATUSES[usize::from(kind) % STATUSES.len()] };
                let dav_ok = kind % 5 < 3;
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
                // Where a redirect leads: what the input says, or one of the places no server may send
                // the search to (this machine, by name or number, a private network).
                const PLACES: [&str; 11] = [
                    "https://127.0.0.1:631/x",
                    "https://localhost/x",
                    "https://foo.localhost/",
                    "https://localhost./x",
                    "https://foo.localhost./",
                    "https://LOCALHOST./",
                    "https://[::1]/",
                    "https://0x7f000001/",
                    "https://10.0.0.1./",
                    "https://2130706433/",
                    "https://169.254.169.254/",
                ];
                let place = if kind % 4 == 0 { PLACES[usize::from(kind / 4) % PLACES.len()] } else { rest.as_str() };
                let mut response = HttpResponse::new(status, body).with_field("Location", place);
                response = response.with_field("WWW-Authenticate", if kind % 2 == 0 { "Basic realm=\"x\"" } else { rest.as_str() });
                response = response.with_field("DAV", if dav_ok { "1, calendar-access, addressbook" } else { rest.as_str() });
                // (What the machine takes for evidence: an answer to a request without the
                // password that names the service, by a 401 or by an OPTIONS that answers.)
                if !carries && dav_ok && ((status == 401) || (options && matches!(status, 200 | 204))) {
                    evidenced.push(origin);
                }
                // A certificate that is not accepted, and a connection that fails.
                let result = match kind {
                    0xfe => Err(TransportError::Failed),
                    0xfd => Err(TransportError::Certificate),
                    _ => Ok(response),
                };
                search.response(result);
            }
            Step::LookupSrv(_) => {
                search.srv(Some(SrvAnswer { targets: answer.clone(), path: Some(text.chars().take(40).collect()) }));
            }
            Step::NeedsYes(consent) => {
                if agrees {
                    agreed.push(format!("https://{}", consent.host()));
                    search.agree();
                } else {
                    search.decline();
                }
            }
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
