// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding an account's calendar and contacts servers (#209, D7): the host's side of
//! `mailcraft_dav::discovery`, once the owner pressed Find in File › Info › Calendars and Contacts
//! and not before.
//!
//! The machine decides everything about where the password may go; this only does what it asks:
//! the request it made (no redirect followed here), the SRV and TXT look-up of the domain over
//! DNS-over-HTTPS ([`crate::lookup_dav`], only the domain goes out), and the question for the
//! owner, which waits for the window. An address the owner typed is the only place asked for that
//! kind. What comes back is homes and counts, or a kind of failure ([`DavFindFailure`]), never a
//! server's words. Nothing here is logged but the outcome's kind.

use mailcraft_dav::discovery::{ConsentReason, Discovery, Failure, Outcome, SrvAnswer, SrvName, Step};
use mailcraft_dav::http::{HttpExchange, Secret};
use mailcraft_dav::request::Kind;
use mailcraft_model::{DavFindFailure, DavFindQuestion, DavFound};

use crate::dav::Wire;
use crate::dns_srv::{DavRecords, DavService, lookup_dav};

/// The most rounds a search is driven: each is a request, a look-up or a question, and the
/// machine bounds its own requests well below this.
const ROUNDS: usize = 512;

/// What the search starts from.
#[derive(Clone, Copy)]
pub struct DavFindAsk<'a> {
    /// The account's address: its domain is where the search looks when nothing is typed.
    pub email: &'a str,
    /// The account's mail server (its IMAP host), whose name is the owner's; empty for none.
    pub mail_host: &'a str,
    /// The name that signs in; empty means the address.
    pub username: &'a str,
    /// The password, sent only where the machine allows it.
    pub password: &'a str,
    /// The calendars' address the owner typed; empty to search.
    pub calendars: &'a str,
    /// The contacts' address the owner typed; empty to search.
    pub contacts: &'a str,
    /// The one certificate trusted besides the system's (a SHA-256 fingerprint); empty for none.
    pub trusted: &'a str,
}

/// The window's kind for a way the search failed.
fn failure_of(failure: Failure) -> DavFindFailure {
    match failure {
        Failure::Unreachable => DavFindFailure::Unreachable,
        Failure::NotFound => DavFindFailure::NotFound,
        Failure::NotDavServer => DavFindFailure::NotDavServer,
        Failure::SecureConnection => DavFindFailure::SecureConnection,
        Failure::Unreadable | Failure::InsecureRedirect | Failure::PrivateAddress => DavFindFailure::Unreadable,
        Failure::ServerRefused => DavFindFailure::ServerRefused,
        Failure::NoPrincipal | Failure::NoHome => DavFindFailure::NoHome,
        Failure::SignInNotSupported => DavFindFailure::SignInNotSupported,
        Failure::Declined => DavFindFailure::Declined,
        Failure::SignInRefused => DavFindFailure::SignInRefused,
        _ => DavFindFailure::TooLong,
    }
}

/// The look-up the machine asked for, by the fixed services only (`_caldavs._tcp.<domain>`).
fn look_up(name: &SrvName, srv: &mut dyn FnMut(DavService, &str) -> Option<DavRecords>) -> Option<SrvAnswer> {
    let name = name.as_str();
    let (service, domain) = [DavService::CalDav, DavService::CardDav]
        .into_iter()
        .find_map(|service| name.strip_prefix(service.label()).and_then(|rest| rest.strip_prefix('.')).map(|domain| (service, domain)))?;
    let records = srv(service, domain)?;
    Some(SrvAnswer { targets: records.targets.into_iter().map(|t| (t.host, t.port)).collect(), path: records.path })
}

/// The search, start to end, through `exchange` and `srv`; `question` asks the owner about a host
/// (`None` when the owner gave up: the search ends, declined), and `stopped` is asked before every
/// round (true: the owner gave up, and nothing more is sent).
pub fn find_with(
    ask: &DavFindAsk<'_>,
    exchange: &dyn HttpExchange,
    srv: &mut dyn FnMut(DavService, &str) -> Option<DavRecords>,
    question: &mut dyn FnMut(&str, DavFindQuestion) -> Option<bool>,
    stopped: &dyn Fn() -> bool,
) -> Result<DavFound, DavFindFailure> {
    let user = if ask.username.trim().is_empty() { ask.email.trim() } else { ask.username.trim() };
    let secret = Secret::basic(user, ask.password).map_err(|_| DavFindFailure::BadSettings)?;
    let mut search = Discovery::new(ask.email.trim(), secret, true, true).map_err(|_| DavFindFailure::BadSettings)?;
    if !ask.mail_host.trim().is_empty() {
        search = search.mail_host(ask.mail_host);
    }
    for (kind, typed) in [(Kind::Calendar, ask.calendars), (Kind::AddressBook, ask.contacts)] {
        if !typed.trim().is_empty() {
            search = search.typed(kind, typed).map_err(|_| DavFindFailure::BadSettings)?;
        }
    }
    let mut outcome = Outcome::Failed(Failure::TooManyRedirects);
    for _ in 0..ROUNDS {
        if stopped() {
            return Err(DavFindFailure::Declined);
        }
        match search.step() {
            Step::Request(request) => {
                let result = exchange.exchange(&request);
                search.response(result);
            }
            Step::LookupSrv(name) => {
                let answer = look_up(&name, srv);
                search.srv(answer);
            }
            Step::NeedsYes(consent) => {
                let why = match consent.reason() {
                    ConsentReason::OwnDomain => DavFindQuestion::OwnDomain,
                    ConsentReason::Elsewhere => DavFindQuestion::Elsewhere,
                };
                match question(consent.host(), why) {
                    Some(true) => search.agree(),
                    Some(false) => search.decline(),
                    None => return Err(DavFindFailure::Declined),
                }
            }
            Step::Done(done) => {
                outcome = done;
                break;
            }
        }
    }
    match outcome {
        Outcome::Found(found) => {
            let mut out = DavFound::default();
            if let Some(cal) = found.calendars() {
                out.calendar_homes = cal.homes().to_vec();
                out.calendars = cal.collections().calendars.len();
                out.passed_over += cal.collections().passed_over;
            }
            if let Some(card) = found.contacts() {
                out.contact_homes = card.homes().to_vec();
                out.address_books = card.collections().address_books.len();
                out.passed_over += card.collections().passed_over;
            }
            Ok(out)
        }
        Outcome::Microsoft => Err(DavFindFailure::Microsoft),
        Outcome::Google => Err(DavFindFailure::Google),
        Outcome::Failed(failure) => Err(failure_of(failure)),
    }
}

/// The search over the real network: the system's certificates and the one the owner trusts for
/// these servers (any other is a [`DavFindFailure::SecureConnection`]), the domain's SRV and TXT records over
/// DNS-over-HTTPS. Blocks: the host runs it on a thread of its own.
pub fn find(
    ask: &DavFindAsk<'_>,
    question: &mut dyn FnMut(&str, DavFindQuestion) -> Option<bool>,
    stopped: &dyn Fn() -> bool,
) -> Result<DavFound, DavFindFailure> {
    let wire = Wire::new(ask.trusted);
    let found = find_with(ask, &wire, &mut |service, domain| lookup_dav(service, domain).ok(), question, stopped);
    log::info!("calendar server search: {:?}", found.as_ref().map(|f| (f.calendars, f.address_books)));
    found
}

#[cfg(test)]
#[path = "dav_find_tests.rs"]
mod tests;
