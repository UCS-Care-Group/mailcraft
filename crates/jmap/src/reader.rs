// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the readers of an account's contacts ([`crate::contacts`]) and of its calendars
//! ([`crate::calendars`]) share: whose data is read, a request of one call with its answer, why a run can not
//! go on, how a text from a server is cut, and the reading of one kind of data ([`Kind`]) itself, which is
//! the same for every kind ([`Calls::read_all`]):
//!
//! 1. `<Type>/query` for the ids: a page of [`PAGE`] at a time by `position`, until the server lists no
//!    more (or as many as it said there are), [`MAX_PAGES`] pages and [`Kind::most`] ids at the most. Only
//!    what is an id of the protocol is kept, each once; a page that brings nothing new ends the list, so
//!    that a list that repeats itself is not followed for ever.
//! 2. `<Type>/get` of those ids, [`MAX_CHUNK`] to a request (or as many as the server takes in a `get`).
//!    When the server answers an error to such a call, or its answer is larger than [`Kind::get_bytes`],
//!    the call is halved until the item that breaks it is found ([`MAX_SPLITS`] times at the most); that
//!    one is not read, the rest are. After [`MAX_FAILURES`] calls that failed one after the other the
//!    rest is given up. Only what was asked for is read, each once.
//!
//! What came of it is an [`Outcome`]: the items, and whether they are all the server has.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value, json};

use crate::connection::{busy_seconds, unusable};
use crate::mail::{said, secrets_of};
use crate::{ApiRequest, BuildError, CallFailure, Connection, Error, HttpExchange, Id, Redactor, TransportError, Unusable};

/// How long a busy server is left alone when it does not say, in seconds.
pub(crate) const BUSY_FOR: u32 = 30;
/// Ids asked for in one page of a list, and the most pages read of one list.
pub(crate) const PAGE: usize = 500;
pub(crate) const MAX_PAGES: usize = 400;
/// Items asked for in one request.
pub(crate) const MAX_CHUNK: usize = 200;
/// Most times, for one list, that a call for items is halved to find the one that breaks it.
pub(crate) const MAX_SPLITS: usize = 64;
/// Calls for items that fail one after the other before the rest of a list is given up.
pub(crate) const MAX_FAILURES: usize = 3;
/// The most a page of ids may be, in bytes.
pub(crate) const QUERY_BYTES: usize = 1 << 20;

/// Why a run can not go on, with what there is to say of it, fit for a line.
pub(crate) enum Stop {
    /// The owner stopped the run.
    Cancelled,
    /// The credentials, or the access, were refused.
    Auth,
    /// The server is busy: for this many seconds.
    Busy(u32, String),
    /// The server can not be reached, or answers nonsense, or says it is unavailable.
    Down(String),
    /// The server answered, and this step failed: an error for the call, an answer that is too large or lacks
    /// what was asked.
    Failed(String),
}

/// The account whose data of `capability` is read: the one the session names for it, else the one for mail,
/// whichever offers it. None when the session does not advertise the capability, or neither account does.
pub(crate) fn account_for<H: HttpExchange>(connection: &Connection<H>, capability: &str) -> Option<Id> {
    let client = connection.client();
    let session = client.session();
    if !session.has_capability(capability) {
        return None;
    }
    let offers = |id: &&Id| session.account(id).is_some_and(|account| account.has_capability(capability));
    session.primary_account(capability).filter(offers).or_else(|| Some(client.account_id()).filter(offers)).cloned()
}

/// What a sign-in that failed comes to: refused, or busy for so many seconds, and the line to tell it with
/// (none for a refusal, which is told by itself).
pub(crate) struct SignIn {
    pub(crate) refused: bool,
    pub(crate) busy_for: Option<u32>,
    pub(crate) line: Option<String>,
}

/// What a run that could not begin says: signing in for the account `email` at `session` failed with `error`.
pub(crate) fn sign_in_failed(email: &str, session: &str, error: &Error) -> SignIn {
    let secrets = secrets_of(email, session, &Redactor::new(), "");
    let line = || Some(format!("Couldn't reach the server: {}", said(&secrets, error)));
    match error {
        Error::CredentialsRefused | Error::Forbidden { .. } => SignIn { refused: true, busy_for: None, line: None },
        Error::Busy { .. } => SignIn { refused: false, busy_for: Some(busy_seconds(error).unwrap_or(BUSY_FOR)), line: line() },
        _ => SignIn { refused: false, busy_for: None, line: line() },
    }
}

/// `text` cut to `most` bytes at the most, at a character's edge, without the white space around it.
pub(crate) fn cut(text: &str, most: usize) -> &str {
    if text.len() <= most {
        return text.trim();
    }
    let end = (0..=most).rev().find(|at| text.is_char_boundary(*at)).unwrap_or(0);
    text.get(..end).unwrap_or_default().trim()
}

/// The calls of one run: the connection, the account, and what is worked out once.
pub(crate) struct Calls<'a, H> {
    pub(crate) connection: &'a Connection<H>,
    pub(crate) cancel: &'a AtomicBool,
    /// What an error line must not show.
    pub(crate) secrets: Redactor,
    /// The account whose data is read.
    pub(crate) account: Id,
    /// The capability every request says it uses.
    pub(crate) capability: &'static str,
}

impl<'a, H: HttpExchange> Calls<'a, H> {
    /// The calls of a run for the account `account` of the connection, on behalf of the account `email` at
    /// `session`: neither of those, nor the credentials, nor a long id of the account will show in a line.
    pub(crate) fn new(
        connection: &'a Connection<H>,
        cancel: &'a AtomicBool,
        email: &str,
        session: &str,
        account: Id,
        capability: &'static str,
    ) -> Self {
        let client = connection.client();
        let mut secrets = secrets_of(email, session, client.redactor(), client.account_id().as_str());
        if account.as_str().len() >= 8 {
            secrets.add_personal(account.as_str());
        }
        Self { connection, cancel, secrets, account, capability }
    }

    /// What `error` says, fit for a line: no credential and no address in it.
    pub(crate) fn said(&self, error: &dyn fmt::Display) -> String {
        said(&self.secrets, error)
    }

    /// An answer that is not what was asked for.
    pub(crate) fn shape(&self, what: &'static str) -> Stop {
        Stop::Failed(self.said(&unusable(what)))
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Why a request failed as a whole (as for mail): the credentials or the access refused, the server
    /// busy, the exchange called off because the owner stopped the run; a request or an answer that is too
    /// large is for the step; anything else says the server can not be had.
    fn call_stop(&self, error: &Error) -> Stop {
        match error {
            Error::CredentialsRefused | Error::Forbidden { .. } => Stop::Auth,
            Error::Transport(TransportError::Aborted) if self.cancelled() => Stop::Cancelled,
            Error::Busy { .. } => Stop::Busy(busy_seconds(error).unwrap_or(BUSY_FOR), self.said(error)),
            Error::TooLarge { .. } | Error::Build(_) | Error::Unusable(Unusable::BodyTooLarge | Unusable::TooComplex) => {
                Stop::Failed(self.said(error))
            }
            other => Stop::Down(self.said(other)),
        }
    }

    /// A request of the one call `method` with `args`, and its answer, held to `most` bytes. The owner's
    /// stop is looked at first.
    pub(crate) async fn one_call(&self, method: &str, args: Value, most: usize) -> Result<Map<String, Value>, Stop> {
        if self.cancelled() {
            return Err(Stop::Cancelled);
        }
        let built = |error: BuildError| Stop::Failed(self.said(&error));
        let mut request = ApiRequest::new();
        request.using(self.capability).map_err(built)?;
        request.set_response_limit(most);
        let args = match args {
            Value::Object(args) => args,
            _ => Map::new(),
        };
        let call = request.add_call(method, args).map_err(built)?;
        let mut answers = self.connection.call(&request, &self.secrets).await.map_err(|error| self.call_stop(&error))?;
        answers.take_named(&call, method).map_err(|failure| match &failure {
            CallFailure::Method(error) if error.kind().may_succeed_later() => Stop::Down(self.said(&failure)),
            _ => Stop::Failed(self.said(&failure)),
        })
    }
}

/// One kind of data as a run reads it: what is asked, how much, and the words for what goes wrong.
pub(crate) struct Kind {
    /// The methods: `<Type>/query` and `<Type>/get`.
    pub(crate) query: &'static str,
    pub(crate) get: &'static str,
    /// What the list is asked besides the account and the page: a filter, say. Empty for all there is.
    pub(crate) listing: Map<String, Value>,
    /// What a `get` is asked besides the account and the ids: the properties, say.
    pub(crate) reading: Map<String, Value>,
    /// The most ids a list is read to.
    pub(crate) most: usize,
    /// The most an answer to a `get` may be, in bytes.
    pub(crate) get_bytes: usize,
    /// An answer without the list that was asked for, a list that repeats itself, and one that has too many
    /// pages, in an error's words.
    pub(crate) no_list: &'static str,
    pub(crate) repeats: &'static str,
    pub(crate) too_long: &'static str,
}

/// What one object of an answer to a `get` comes to.
pub(crate) enum Made<T> {
    /// What it was read as.
    Item(T),
    /// Answered for, and nothing that is kept: a group among the cards, say.
    Other,
}

/// What was read of one list, and what could not be.
pub(crate) struct Outcome<T> {
    pub(crate) items: Vec<T>,
    /// The server answered the first page of the list: there is something to hand over, if nothing.
    pub(crate) listed: bool,
    /// The first trouble that did not end the reading.
    pub(crate) why: Option<String>,
    /// How many ids the list named that got no answer: no item, and no word that it is gone.
    pub(crate) unread: usize,
    /// The list could not be read to its end, or named what is no id.
    pub(crate) short: bool,
    /// There is more than a run reads.
    pub(crate) capped: bool,
}

impl<T> Default for Outcome<T> {
    fn default() -> Self {
        Self { items: Vec::new(), listed: false, why: None, unread: 0, short: false, capped: false }
    }
}

impl<T> Outcome<T> {
    /// Everything the server has in the list was read.
    pub(crate) fn clean(&self) -> bool {
        self.listed && self.why.is_none() && self.unread == 0 && !self.short && !self.capped
    }

    fn trouble(&mut self, why: String) {
        self.why.get_or_insert(why);
    }
}

impl<H: HttpExchange> Calls<'_, H> {
    /// All of `kind`: the list, then the items, each made by `make` from its id and its object. An error is
    /// what ends the run, or a list that could not be begun; anything less is in `out`, with what was read.
    pub(crate) async fn read_all<T>(&self, kind: &Kind, out: &mut Outcome<T>, make: impl Fn(&str, &Value) -> Made<T>) -> Result<(), Stop> {
        let ids = self.list(kind, out).await?;
        let take = usize::try_from(self.connection.client().limits().max_objects_in_get()).unwrap_or(usize::MAX);
        let most = MAX_CHUNK.min(take).max(1);
        log::debug!("jmap {}: {} listed, {most} to a request", kind.get, ids.len());
        let mut work: VecDeque<&[String]> = ids.chunks(most).collect();
        let (mut failures, mut splits) = (0usize, 0usize);
        while let Some(chunk) = work.pop_front() {
            match self.get(kind, chunk, out, &make).await {
                Ok(()) => failures = 0,
                Err(Stop::Failed(why)) => {
                    // One of them, perhaps: halve, and ask again.
                    if chunk.len() > 1
                        && splits < MAX_SPLITS
                        && let Some((first, second)) = chunk.split_at_checked(chunk.len() / 2)
                    {
                        splits = splits.saturating_add(1);
                        work.push_front(second);
                        work.push_front(first);
                        continue;
                    }
                    out.unread = out.unread.saturating_add(chunk.len());
                    out.trouble(why);
                    failures = failures.saturating_add(1);
                    if failures >= MAX_FAILURES {
                        out.short = true;
                        break;
                    }
                }
                Err(stop) => return Err(stop),
            }
        }
        Ok(())
    }

    /// `<Type>/query`, page by page: the ids of the list, each once, [`Kind::most`] at the most. A failure
    /// before the first page is the caller's; one after it is told, and what was listed until then is still
    /// read.
    async fn list<T>(&self, kind: &Kind, out: &mut Outcome<T>) -> Result<Vec<String>, Stop> {
        let mut ids: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut position = 0usize;
        for _ in 0..MAX_PAGES {
            let mut args = kind.listing.clone();
            args.insert("accountId".into(), json!(self.account.as_str()));
            args.insert("position".into(), json!(position));
            args.insert("limit".into(), json!(PAGE));
            args.insert("calculateTotal".into(), json!(true));
            let page = self.one_call(kind.query, Value::Object(args), QUERY_BYTES).await.and_then(|answer| match answer.get("ids") {
                Some(Value::Array(page)) => Ok((page.clone(), answer.get("total").and_then(Value::as_u64))),
                _ => Err(self.shape(kind.no_list)),
            });
            let (page, total) = match page {
                Ok(page) => page,
                // (The server answered, and not with a page: what was listed is read all the same.)
                Err(Stop::Failed(why)) if out.listed => {
                    out.short = true;
                    out.trouble(why);
                    return Ok(ids);
                }
                Err(stop) => return Err(stop),
            };
            out.listed = true;
            if page.is_empty() {
                return Ok(ids);
            }
            let mut fresh = 0usize;
            for id in page.iter().take(PAGE) {
                let Some(id) = id.as_str().filter(|id| Id::new(id).is_ok()) else {
                    // What is no id names nothing that can be asked for.
                    out.short = true;
                    continue;
                };
                if seen.contains(id) {
                    continue;
                }
                if ids.len() >= kind.most {
                    out.capped = true;
                    return Ok(ids);
                }
                seen.insert(id.to_string());
                ids.push(id.to_string());
                fresh = fresh.saturating_add(1);
            }
            position = position.saturating_add(page.len().min(PAGE));
            if total.is_some_and(|total| usize::try_from(total).is_ok_and(|total| position >= total)) {
                return Ok(ids);
            }
            // A page that brings nothing new is a list that does not go on: not one to follow for ever.
            if fresh == 0 {
                out.short = true;
                out.trouble(self.said(&unusable(kind.repeats)));
                return Ok(ids);
            }
        }
        out.short = true;
        out.trouble(self.said(&unusable(kind.too_long)));
        Ok(ids)
    }

    /// `<Type>/get` of `ids`. Only what was asked for is read, each once; an id the answer has no object for
    /// and does not say is gone counts as not read.
    async fn get<T>(&self, kind: &Kind, ids: &[String], out: &mut Outcome<T>, make: &impl Fn(&str, &Value) -> Made<T>) -> Result<(), Stop> {
        let mut args = kind.reading.clone();
        args.insert("accountId".into(), json!(self.account.as_str()));
        args.insert("ids".into(), json!(ids));
        let answer = self.one_call(kind.get, Value::Object(args), kind.get_bytes).await?;
        let list = answer.get("list").and_then(Value::as_array).ok_or_else(|| self.shape(kind.no_list))?;
        let asked: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let gone: HashSet<&str> = answer
            .get("notFound")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(ids.len())
            .filter_map(Value::as_str)
            .filter_map(|id| asked.get(id).copied())
            .collect();
        let mut found: HashSet<&str> = HashSet::new();
        let mut read: Vec<T> = Vec::new();
        for entry in list.iter().take(ids.len()) {
            let Some(id) = entry.get("id").and_then(Value::as_str).and_then(|id| asked.get(id).copied()) else { continue };
            if !found.insert(id) {
                continue;
            }
            match make(id, entry) {
                Made::Item(item) => read.push(item),
                Made::Other => {}
            }
        }
        // A server that says everything of a call is gone has more likely lost what holds it (the address
        // book, the calendar), or can not find again what it listed, than its owner removed all of it since
        // the list was read: none of it is taken for gone.
        let all_gone = ids.len() > 1 && found.is_empty() && ids.iter().all(|id| gone.contains(id.as_str()));
        let short = if all_gone { ids.len() } else { ids.iter().filter(|id| !found.contains(id.as_str()) && !gone.contains(id.as_str())).count() };
        out.unread = out.unread.saturating_add(short);
        out.items.append(&mut read);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
