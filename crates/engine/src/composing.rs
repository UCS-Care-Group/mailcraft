// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Several messages being written at once (#286), each named by an id.
//!
//! The current one is [`Session::compose`]: the one a command acts on when it names none (the
//! reading pane's; later the window in front, which the host sets with
//! [`Session::show_compose`]). The others wait in [`Session::composing`]. A command that acts on
//! a message being written ([`crate::CommandSpec::writing`]) takes an optional `compose` id: the
//! message it names is made current while the command runs, and the one that was current is
//! current again after it. An agent names it whenever more than one is open.

use serde_json::Value;

use crate::{CmdError, Compose, Session, p};

/// Most messages being written at once: twelve windows of their own and the reading pane's.
pub const MAX_COMPOSING: usize = 13;

impl Session {
    /// Every message being written, the current one first.
    pub fn all_composing(&self) -> impl Iterator<Item = &Compose> {
        self.compose.iter().chain(self.composing.iter())
    }

    /// How many messages are being written.
    pub fn composing_count(&self) -> usize {
        self.all_composing().count()
    }

    /// The id of the current message being written.
    pub fn compose_id(&self) -> Option<u64> {
        self.compose.as_ref().map(|c| c.id)
    }

    /// The message being written with this id.
    pub fn composing_by_id(&self, id: u64) -> Option<&Compose> {
        self.all_composing().find(|c| c.id == id)
    }

    /// The message being written that a call with `params` acts on: the one its `compose` id
    /// names, else the current one. A `compose` that names none open is none.
    pub fn compose_named(&self, params: &Value) -> Option<&Compose> {
        match params.get("compose") {
            None => self.compose.as_ref(),
            Some(v) => v.as_u64().and_then(|id| self.composing_by_id(id)),
        }
    }

    /// Make the message being written with `id` the current one (the host: the reading pane's, or
    /// the window in front). `false` when none is open with that id.
    pub fn show_compose(&mut self, id: u64) -> bool {
        if self.compose_id() == Some(id) {
            return true;
        }
        let Some(at) = self.composing.iter().position(|c| c.id == id) else { return false };
        let c = self.composing.remove(at);
        if let Some(was) = self.compose.replace(c) {
            self.composing.push(was);
        }
        true
    }

    /// Close every message being written, without saving any (all the mailbox's data is removed).
    pub fn close_all_composing(&mut self) {
        self.compose = None;
        self.composing.clear();
    }

    /// Open `c` as another message being written, with an id of its own, and answer the id. It is
    /// the current one, but for one an agent opens while the owner is writing in the current
    /// one: that one waits behind it, and the owner's stays where they are writing (#286). More
    /// than [`MAX_COMPOSING`] are refused.
    pub(crate) fn open_compose(&mut self, mut c: Compose) -> Result<u64, CmdError> {
        if self.composing_count() >= MAX_COMPOSING {
            return Err(failed!(
                "{n} messages are being written already, the most there can be at once; send, save or close one first",
                n = MAX_COMPOSING
            ));
        }
        self.compose_seq = self.compose_seq.saturating_add(1);
        c.id = self.compose_seq;
        let behind = self.agent.is_some() && self.compose.as_ref().is_some_and(|c| c.owner_changed);
        if behind {
            self.composing.push(c);
        } else if let Some(was) = self.compose.replace(c) {
            self.composing.push(was);
        }
        Ok(self.compose_seq)
    }

    /// Give an id to a message being written that has none (one a host put there itself), and
    /// make one current when there is none: the one put aside last.
    fn number_composing(&mut self) {
        if self.compose.is_none() {
            self.compose = self.composing.pop();
        }
        let mut seq = self.compose_seq;
        for c in self.compose.iter_mut().chain(self.composing.iter_mut()) {
            if c.id == 0 {
                seq = seq.saturating_add(1);
                c.id = seq;
            }
        }
        self.compose_seq = seq;
    }

    /// [`Session::run`]: on the message being written that `params` names (`compose`), else on the
    /// current one.
    pub(crate) fn run_on_message(&mut self, id: &str, params: &Value) -> Result<Value, CmdError> {
        // A command run by another (Save Draft first) acts on the message the outer one acts on.
        let outer = self.in_command();
        if !outer {
            self.number_composing();
        }
        let writing = self.registry.get(id).is_some_and(|spec| spec.writing);
        let named = if writing && params.is_object() { p::id_strict(params, "compose")? } else { None };
        if writing && named.is_none() && !outer && self.agent.is_some() && self.composing_count() > 1 {
            return Err(failed!(
                "{n} messages are being written: name the one `{command}` is for with `compose`, its id (`ui.inspect` lists them under `beingWritten`)",
                n = self.composing_count(),
                command = id
            ));
        }
        let result = match named {
            None => self.run_here(id, params),
            Some(target) => {
                if self.composing_by_id(target).is_none() {
                    return Err(failed!("no message being written has the id {id}", id = target));
                }
                let mut bare = params.clone();
                if let Some(map) = bare.as_object_mut() {
                    map.remove("compose");
                }
                self.run_named(target, id, &bare)
            }
        };
        // The current one closed, the one put aside last is current again.
        if !outer {
            self.number_composing();
        }
        result
    }

    /// Run `id` with the message being written `target` current, and the one that was current
    /// current again after it, whatever became of `target` (closed, sent, or still open).
    fn run_named(&mut self, target: u64, id: &str, params: &Value) -> Result<Value, CmdError> {
        let was = self.compose_id();
        if was == Some(target) {
            return self.run_here(id, params);
        }
        let Some(at) = self.composing.iter().position(|c| c.id == target) else {
            return Err(failed!("no message being written has the id {id}", id = target));
        };
        // The one current waits with the others meanwhile, so that the command sees every one.
        let c = self.composing.remove(at);
        if let Some(current) = self.compose.replace(c) {
            self.composing.push(current);
        }
        let result = self.run_here(id, params);
        // The one that was current is current again; `target`, unless the command closed it,
        // waits where it waited.
        if let Some(was) = was
            && let Some(i) = self.composing.iter().position(|c| c.id == was)
        {
            let back = self.composing.remove(i);
            if let Some(acted) = self.compose.replace(back) {
                let at = at.min(self.composing.len());
                self.composing.insert(at, acted);
            }
        }
        result
    }
}
