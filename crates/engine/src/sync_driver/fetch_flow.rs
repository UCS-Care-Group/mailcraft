// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fetching messages whole, the flow of it with no I/O: a command that needs its message whole
//! waits for it ([`Session::fetch_first_plan`]), the answer of a fetch for the owner says what the
//! waiting command does next ([`Session::fetch_now_answered`]: runs, stops, or waits for more of
//! a forward's messages), the message the reading pane shows is asked for at once
//! ([`Session::fetch_shown`]) and the accounts the background looks at are named in order
//! ([`Session::fetch_background_accounts`]). The host starts the fetches, holds the commands that
//! wait for a form to be closed, and words what is said.

use mailcraft_model::sync::GraphFetchResult;
use mailcraft_model::{AccountKind, Id};
use serde_json::{Value, json};

use super::fetch::{Came, apply_fetch, forward_of, forwarded, needs_whole, still_to_fetch, to_fetch};
use crate::cmd::compose;
use crate::{CmdError, Session};

/// A command that waits for messages to be fetched whole.
#[derive(Clone, Debug, PartialEq)]
pub struct Waiting {
    pub command: String,
    pub params: Value,
    /// The mailbox opening it was asked under ([`Session::openings`]): in another it never runs.
    pub opening: u64,
    /// For a forward of several messages, every message fetched for it so far.
    pub tried: Option<Vec<Id>>,
    /// The messages it waits for (of those under way it may wait for some only: it took the place
    /// of a forward, or joined its batch).
    pub waits: Vec<Id>,
}

/// What a command that needs its message whole is to wait for.
#[derive(Clone, Debug)]
pub struct FetchFirst {
    /// The account of the first message.
    pub account: Id,
    /// The messages to fetch now: one, or a batch of a forward's.
    pub messages: Vec<Id>,
    /// The first of them.
    pub first: Id,
    several: bool,
    wait: Waiting,
}

/// What the owner is to be told when a command begins to wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstNote {
    /// A forward of several that waited was called off: this took its place.
    ForwardCalledOff,
    /// A command that waited for a form to be closed was called off: this took its place.
    HeldCalledOff,
    /// Downloading the messages first.
    Several,
    /// Downloading the message first.
    One,
}

/// What the owner is to be told of an answer that has no command to go on with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NowNote {
    /// The server's own word on why it stopped.
    Why(String),
    /// The server didn't send them.
    NotSent,
    /// The message isn't on this computer yet.
    NotHere,
}

/// What the waiting command does once an answer is taken in.
#[derive(Debug)]
pub enum NowNext {
    /// Nothing waits (or it was asked in another mailbox).
    Nothing,
    /// It goes on: run it again, with `tried` for a forward of several.
    Run(Waiting),
    /// It stops, for this reason.
    Stopped { command: String, error: CmdError },
    /// It doesn't go on yet.
    Note(NowNote),
}

/// What taking in an answer to a fetch for the owner came to.
#[derive(Debug)]
pub struct NowAnswered {
    /// What came wasn't taken in: said in the status bar.
    pub refused: Option<String>,
    pub next: NowNext,
}

impl Session {
    /// What command `id` with `params` waits for before it runs: `Ok(None)` when it needs no
    /// message fetched. A forward of several messages (#360) is judged first (as many as one
    /// message carries, none that stands in for a message for good) and waits for each of them,
    /// an account's at a time and a batch at a time, judged again as each batch comes; once none
    /// is left to fetch it runs.
    pub fn fetch_first_plan(&mut self, id: &str, params: &Value) -> Result<Option<FetchFirst>, CmdError> {
        let tried = self.driver.fetch.tried.take();
        let forward = forwarded(self, id, params);
        let several = forward.as_ref().is_some_and(|(ids, _)| ids.len() > 1);
        let messages = match &forward {
            Some((ids, _)) if several => {
                compose::forward_check(self, ids, true)?;
                still_to_fetch(self, ids, tried.as_deref().unwrap_or_default())
            }
            Some((_, focus)) => to_fetch(self, id, *focus).into_iter().collect(),
            None => needs_whole(self, id, params).into_iter().collect(),
        };
        let Some(first) = messages.first().copied() else { return Ok(None) };
        let Some(account) = self.mailbox.message(first).map(|m| m.account) else { return Ok(None) };
        // Named, for the selection may have moved on by the time they are here. Another command's
        // `id`, if the call gave one, stays as it was, for the command to judge; a forward's
        // messages are named by `ids` instead, as the engine took them, the one in focus last.
        let mut then = params.as_object().cloned().unwrap_or_default();
        match &forward {
            Some((ids, focus)) => {
                let named: Vec<Id> = ids.iter().copied().filter(|m| m != focus).chain([*focus]).collect();
                then.remove("id");
                then.insert("ids".into(), json!(named));
            }
            None if then.get("id").is_none_or(Value::is_null) => {
                then.insert("id".into(), json!(first));
            }
            None => {}
        }
        let waits = forward.as_ref().map_or_else(|| vec![first], |(ids, _)| ids.clone());
        let wait = Waiting {
            command: id.to_string(),
            params: Value::Object(then),
            opening: self.openings(),
            tried: several.then(|| tried.unwrap_or_default()),
            waits,
        };
        Ok(Some(FetchFirst { account, messages, first, several, wait }))
    }

    /// Why a fetch for `plan` can't be asked for, when the engine names it: of several messages,
    /// the one that can't come.
    pub fn fetch_unavailable(&self, plan: &FetchFirst) -> Option<CmdError> {
        if plan.several { compose::forward_check(self, &plan.messages, false).err() } else { None }
    }

    /// The fetch for `plan` was handed to the host: the command waits for it. What the owner is
    /// to be told.
    pub fn fetch_waiting(&mut self, plan: FetchFirst) -> FirstNote {
        let opening = self.openings();
        let FetchFirst { several, wait, .. } = plan;
        let forward = forward_of(&wait.command, &wait.params);
        // One command waits at a time: a forward of several still waiting in this mailbox is
        // called off, and said to be, for its download can take a while (#360). The same forward
        // asked for again goes on.
        let called_off = self.driver.fetch.then.as_ref().is_some_and(|before| {
            before.tried.is_some() && before.opening == opening && forward_of(&before.command, &before.params).is_none_or(|f| forward != Some(f))
        });
        // So is one whose messages came and that waits for a form to be closed, and said to be.
        let held_off = self.driver.fetch.held.take().is_some_and(|held| held.opening == opening);
        self.driver.fetch.then = Some(wait);
        if called_off {
            FirstNote::ForwardCalledOff
        } else if held_off {
            FirstNote::HeldCalledOff
        } else if several {
            FirstNote::Several
        } else {
            FirstNote::One
        }
    }

    /// The answer to a fetch of `messages` for the owner, asked under `opening`: take in what
    /// came and say what the command that waits does next. A forward of several stops when what
    /// came wasn't taken in, saying why, and at the first, in the order asked, that its server
    /// didn't send or that came but wasn't kept here, and when a batch brought none of its
    /// messages; else it goes on with the rest, if there are any, and runs once none is left to
    /// fetch. Another command runs once its own message is here, whatever else of the batch it
    /// came in didn't come.
    pub fn fetch_now_answered(&mut self, messages: &[Id], opening: u64, result: GraphFetchResult) -> NowAnswered {
        let nothing = |refused| NowAnswered { refused, next: NowNext::Nothing };
        // Asked for in a mailbox that another has taken the place of: it is nobody's here (nor is
        // a command that waits for it).
        if opening != self.openings() {
            return nothing(None);
        }
        // What came is judged for the messages the command that waits waits for.
        let judged: Vec<Id> = match &self.driver.fetch.then {
            Some(then) => messages.iter().copied().filter(|m| then.waits.contains(m)).collect(),
            None => messages.to_vec(),
        };
        let came = Came::of(self, &judged, &result);
        // Whether the batch brought any of its messages: one cut short before it came to those
        // the command waits for is asked for again, as for its own.
        let brought = !Came::of(self, messages, &result).answered.is_empty();
        self.driver.fetch.offline = result.offline;
        let account = result.account;
        let mut refused = if account != 0 { apply_fetch(self, result).err() } else { None };
        let said = refused.as_ref().map(ToString::to_string);
        let whole = |s: &Session, m: &Id| s.mailbox.message(*m).is_some_and(|m| !m.unfetched);
        let Some(then) = self.driver.fetch.then.take() else { return nothing(said) };
        // Asked of a mailbox that another has taken the place of: the message it names is not
        // this one's, whatever has that id here.
        if then.opening != self.openings() {
            return nothing(said);
        }
        let Waiting { command, params, opening, tried, waits } = then;
        let next = if let Some(mut tried) = tried {
            // Those of this batch it waits for may have come by other means meanwhile (the
            // background): then it goes on.
            let here = !judged.is_empty() && judged.iter().all(|m| whole(self, m));
            let not_kept = came.sent.iter().find(|m| self.mailbox.message(**m).is_some_and(|m| m.unfetched)).copied();
            let at = |m: Id| judged.iter().position(|x| *x == m);
            let stop = match (came.not_sent, not_kept) {
                // Not taken in as a whole: that is why, whatever it said of each.
                _ if refused.is_some() => refused.take(),
                (Some((m, gone)), k) if k.is_none_or(|k| at(m) < at(k)) => Some(compose::not_sent(self, m, gone)),
                (_, Some(k)) => Some(compose::not_kept(self, k)),
                _ if came.answered.is_empty() && self.driver.fetch.offline => compose::forward_check(self, &judged, false).err(),
                _ => None,
            };
            if let Some(error) = stop {
                NowNext::Stopped { command, error }
            } else if !brought && !here {
                NowNext::Note(came.why.map_or(NowNote::NotSent, NowNote::Why))
            } else {
                tried.extend(came.answered);
                NowNext::Run(Waiting { command, params, opening, tried: Some(tried), waits })
            }
        } else if waits.iter().all(|m| whole(self, m)) {
            NowNext::Run(Waiting { command, params, opening, tried: None, waits })
        } else {
            NowNext::Note(NowNote::NotHere)
        };
        NowAnswered { refused: said, next }
    }

    /// The owner's fetch of `now` took the place of `before`, which is called off: the message
    /// open in the reading pane, if it was of it, is asked for again once these are here.
    pub fn fetch_replaced(&mut self, before: &[Id], now: &[Id]) {
        if self.driver.fetch.opened.is_some_and(|m| before.contains(&m) && !now.contains(&m)) {
            self.driver.fetch.opened = None;
        }
    }

    /// A command that has its messages goes on at once, or waits for a form to be closed: the
    /// host says which (`hold`). A command held goes on, in the mailbox it was asked of, when
    /// [`Session::fetch_take_held`] gives it. With `tried` for a forward of several to run again.
    pub fn fetch_hold(&mut self, waiting: Waiting) {
        self.driver.fetch.held = Some(waiting);
    }

    /// The command that waited for a form to be closed, in the mailbox it was asked of.
    pub fn fetch_take_held(&mut self) -> Option<Waiting> {
        let held = self.driver.fetch.held.take()?;
        (held.opening == self.openings()).then_some(held)
    }

    /// Run `waiting` now: what a forward of several has fetched for it so far is kept for the run
    /// ([`Session::fetch_ran`] drops it after).
    pub fn fetch_running(&mut self, tried: Option<Vec<Id>>) {
        self.driver.fetch.tried = tried;
    }

    /// The command that waited has run.
    pub fn fetch_ran(&mut self) {
        self.driver.fetch.tried = None;
    }

    /// Whether `message`, the one the reading pane shows, is fetched once the batch under way
    /// (`batch`: a fetch for the owner) has come: one at a time, and a forward's can take a while.
    pub fn fetch_after_batch(&self, message: Id, batch: bool) -> bool {
        batch && self.driver.fetch.opened != Some(message) && self.sel.messages == [message]
    }

    /// The message the reading pane shows, when it is to be fetched at once: its account and the
    /// message. `batch`: a fetch for the owner is under way. Remembers what was looked at.
    pub fn fetch_shown(&mut self, batch: bool) -> Option<(Id, Id)> {
        let shown = match self.sel.messages.as_slice() {
            [one] => Some(*one),
            _ => None,
        };
        if shown == self.driver.fetch.opened {
            return None;
        }
        let to_fetch = shown.and_then(|m| self.mailbox.message(m)).filter(|m| m.unfetched && m.server.is_some()).map(|m| (m.account, m.id));
        match to_fetch {
            Some((_, m)) if self.fetch_after_batch(m, batch) => None,
            Some((account, m)) => {
                self.driver.fetch.opened = shown;
                Some((account, m))
            }
            None => {
                self.driver.fetch.opened = shown;
                None
            }
        }
    }

    /// The accounts whose messages the host fetches (`fetched`, by kind), in the order the
    /// background looks at them.
    pub fn fetch_accounts(&self, fetched: impl Fn(AccountKind) -> bool) -> Vec<Id> {
        self.mailbox.accounts.iter().filter(|a| fetched(a.kind)).map(|a| a.id).collect()
    }

    /// Whether the background looks for something to fetch at `now`: not stopped by the owner,
    /// and not resting after finding nothing.
    pub fn fetch_background_due(&self, now: f64) -> bool {
        !self.sync_progress.full_paused && !self.driver.fetch.resting(now)
    }

    /// The accounts the background may fetch from at `now`, in order: those whose batches haven't
    /// stopped lately.
    pub fn fetch_background_accounts(&self, fetched: impl Fn(AccountKind) -> bool, now: f64) -> Vec<Id> {
        self.fetch_accounts(fetched).into_iter().filter(|a| !self.driver.fetch.account_waits(*a, now)).collect()
    }
}
