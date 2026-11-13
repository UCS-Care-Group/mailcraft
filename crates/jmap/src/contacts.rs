// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A JMAP account's contacts (RFC 9610; the cards are JSContact, RFC 9553), read for the engine. Nothing
//! here writes to the server.
//!
//! [`sync_contacts`] takes the request the engine worked out ([`JmapContactsRequest`]) and hands back all
//! the contacts it could read ([`JmapContactsResult`]). Nothing is kept between runs.
//!
//! **Whose contacts.** The account the session names for contacts (`primaryAccounts`), when it offers them;
//! else the account for mail, when that one does. A session that does not advertise
//! `urn:ietf:params:jmap:contacts`, or no such account: [`JmapContactsResult::not_offered`], which is no
//! failure, and nothing is asked.
//!
//! **What is asked of the server**, each in a request of one call:
//!
//! 1. `ContactCard/query`, with no filter, for the ids of the cards of every address book: a page of 500
//!    at a time by `position`, until the server lists no more (or as many as it said there are), 400 pages
//!    and [`MAX_CONTACTS`] ids at the most.
//! 2. `ContactCard/get` of those ids, 200 to a request (or as many as the server takes in a `get`). When
//!    the server answers an error to such a call, or its answer is too large (cards may hold pictures), the
//!    call is halved until the card that breaks it is found; that one is not read, the rest are.
//!
//! The paging and the halving are what every reader of one kind of data does (`reader`): here is what is
//! asked for contacts ([`kind`]), and what a card becomes (`card`).
//!
//! A card's picture is not read: neither one written into the card, nor a blob of the account, nor an
//! address on the web, so nothing but the two calls above is ever asked of the server.
//!
//! **Complete** ([`ContactsDownload::complete`]) says that every contact the server has is in the answer,
//! and the engine then removes what the answer lacks: so it is true only when every page of the list was
//! read, every id listed was answered for (with a card, or as gone since), none was left out for the cap,
//! and the run was not stopped. Anything less adds and updates and removes nothing.
//!
//! **When something goes wrong.** `cancel` is looked at before every request: set, the run ends with
//! `stopped`. The credentials or the access refused (401, 403): `auth_refused`. A busy server (429, 503):
//! `busy_for`, as many seconds as it asked, or 30. Anything else is a line in `errors`, and what was read
//! until then is still handed over, not complete. The lines name no contact, and what a server says in one
//! is scrubbed of the session's address, the account's, the credentials and any word that holds an `@`.
//!
//! **What comes back is hostile.** An answer is held to the size its request can need; ids are held to the
//! JMAP alphabet before they go into a request; only cards that were asked for are read, each once; every
//! map, list and string of a card is bounded (`card`); nothing is indexed, unwrapped or recursed into.

mod card;
#[cfg(test)]
mod tests;

use std::sync::atomic::AtomicBool;

use mailcraft_model::remote::{ContactsDownload, MAX_CONTACTS, RemoteContact};
use mailcraft_model::sync::{JmapContactsRequest, JmapContactsResult};
use serde_json::Map;

use self::card::card;
use crate::reader::{Calls, Kind, Made, Outcome, Stop, account_for, sign_in_failed};
use crate::{Connection, Error, HttpExchange};

/// The capability of JMAP for Contacts (RFC 9610 §1.4).
pub const CAPABILITY_CONTACTS: &str = "urn:ietf:params:jmap:contacts";

/// The most an answer with cards may be, in bytes: 200 of them, which may hold their pictures.
const GET_BYTES: usize = 24 << 20;
const _: () = assert!(GET_BYTES <= crate::http::API_RESPONSE_LIMIT);

/// The cards as a run reads them: all of every address book, each whole.
fn kind() -> Kind {
    Kind {
        query: "ContactCard/query",
        get: "ContactCard/get",
        listing: Map::new(),
        reading: Map::new(),
        most: MAX_CONTACTS,
        get_bytes: GET_BYTES,
        no_list: "the answer has no list of contacts",
        repeats: "the list of contacts repeats itself",
        too_long: "the list of contacts has too many pages",
    }
}

/// Read all the contacts of a JMAP account, as `request` says (see the module documentation). `connection`
/// is signed in already ([`Connection::connect`]); a caller whose sign-in failed ends with
/// [`failed_contacts`]. `cancel` is read before every request: set, the run ends there, `stopped`.
///
/// Never an error as a whole: what went wrong is in the result, and what was read until then with it.
/// Nothing here waits: a busy server ends the run with the seconds it asked for.
pub async fn sync_contacts<H: HttpExchange>(connection: &Connection<H>, request: &JmapContactsRequest, cancel: &AtomicBool) -> JmapContactsResult {
    let mut result = answer_for(request);
    let Some(account) = account_for(connection, CAPABILITY_CONTACTS) else {
        log::debug!("jmap contacts: the server offers none for the account");
        result.not_offered = true;
        return result;
    };
    let calls = Calls::new(connection, cancel, &request.email, &request.session, account, CAPABILITY_CONTACTS);
    let mut out: Outcome<RemoteContact> = Outcome::default();
    // (A group, or what is no card, is answered for and is no contact.)
    let stop = calls.read_all(&kind(), &mut out, |id, entry| card(id, entry).map_or(Made::Other, Made::Item)).await.err();
    let complete = stop.is_none() && out.clean();
    let mut why = out.why.take();
    match stop {
        None => {}
        Some(Stop::Cancelled) => result.stopped = true,
        Some(Stop::Auth) => result.auth_refused = true,
        Some(Stop::Busy(seconds, line)) => {
            result.busy_for = Some(seconds);
            why = Some(line);
        }
        Some(Stop::Down(line) | Stop::Failed(line)) => why = Some(line),
    }
    if let Some(why) = why {
        result.errors.push(format!("Couldn't read the contacts from the server: {why}"));
    } else if out.unread > 0 {
        let contacts = if out.unread == 1 { "1 contact".to_string() } else { format!("{} contacts", out.unread) };
        result.errors.push(format!("Couldn't read {contacts} from the server: the answer left them out."));
    }
    if out.capped {
        result.errors.push(format!("The server has more contacts than are read: {MAX_CONTACTS} at the most."));
    }
    // (A list that named what is no id, say.)
    if out.listed && !complete && result.errors.is_empty() && !result.stopped && !result.auth_refused {
        result.errors.push("Couldn't read all the contacts from the server.".to_string());
    }
    log::debug!(
        "jmap contacts: {} read (listed: {}, complete: {complete}, stopped: {}, busy: {})",
        out.items.len(),
        out.listed,
        result.stopped,
        result.busy_for.is_some()
    );
    if out.listed {
        result.contacts = Some(ContactsDownload { account: request.account, email: request.email.trim().to_string(), complete, contacts: out.items });
    }
    result
}

/// The end of a run that could not begin: signing in to the server ([`Connection::connect`]) failed with
/// `error`. What [`sync_contacts`] would have said of the same trouble: the sign-in refused, the server
/// busy, or a line.
pub fn failed_contacts(request: &JmapContactsRequest, error: &Error) -> JmapContactsResult {
    let failed = sign_in_failed(&request.email, &request.session, error);
    JmapContactsResult { auth_refused: failed.refused, busy_for: failed.busy_for, errors: Vec::from_iter(failed.line), ..answer_for(request) }
}

/// An answer to `request`, with nothing in it yet.
fn answer_for(request: &JmapContactsRequest) -> JmapContactsResult {
    JmapContactsResult { account: request.account, email: request.email.clone(), session: request.session.clone(), ..Default::default() }
}
