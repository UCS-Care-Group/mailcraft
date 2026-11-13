// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a Send/Receive sends is marked as being sent, and the mark is on disk, before the run is
//! handed to the host (AU3-26); and mail in a shared mailbox's Outbox waits for the owner's
//! permission to send from shared mailboxes (`Mail.Send.Shared`, #70). The decisions and the
//! state, with no I/O: whether a run may go now, what is marked as sending and when the mark is
//! taken off, whether the owner is to be asked. The writing of the mailbox, the run itself and
//! the question (the dialog, the provider's page) are the host's and the window's.
//!
//! A run's answer can be lost: another mailbox takes the place of the open one and the answer is
//! let go of, or the program stops mid-send. Without the mark the message would still be in the
//! Outbox at the next run and go a second time. With it, the next run of the account finds the
//! mark and holds the message ("it may have been sent"): the owner looks in Sent Items and
//! decides with Send Again. The run's answer, when it is taken in, clears the mark.
//!
//! Where the mailbox is kept by a background saver, the run waits for the saver to have written
//! the marks (a gate: [`Session::send_gate_wait`], then [`Session::send_gate_step`] at each
//! look); a save that fails sends nothing.

use std::collections::HashSet;

use mailcraft_model::{FolderKind, Id};

use crate::Session;
use crate::cmd::graph_send;

/// A run waiting for its marks to be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Gate {
    /// The mailbox opening it was started under ([`Session::openings`]).
    opening: u64,
    /// The state of the mailbox with the marks.
    rev: u64,
    account: Id,
}

/// What a look at the gate says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateStep {
    /// No run waits for its marks.
    Idle,
    /// The marks aren't written yet: wait.
    Waiting,
    /// The mailbox the run was for is gone, and its marks with it: nothing goes; let go of the run.
    Gone,
    /// The mailbox couldn't be saved: nothing is sent, and the run ends saying so (the marks are
    /// taken off, since nothing went).
    Failed(Id),
    /// The marks are written: the run goes to the host.
    Open(Id),
}

/// What pressing Send in a message of a shared mailbox comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pressed {
    /// Not a shared mailbox's message.
    Nothing,
    /// The run that takes it out asks the owner if it finds sending isn't allowed yet.
    Noted,
    /// Sending from it is known not to be allowed yet: the owner is to be asked at once.
    Ask(Id),
}

/// What the driver remembers of sending.
#[derive(Debug, Default)]
pub struct SendingDriver {
    /// The shared mailboxes whose Outbox waits for the permission to send from them: until it is
    /// given, or the owner asks with a Send/Receive, nothing of it is handed to a server.
    pub waits: HashSet<Id>,
    /// The shared mailboxes the owner pressed Send in, before it was known that it may not be sent
    /// from them yet.
    pressed: HashSet<Id>,
    /// A piece of the mail run under way was refused: at its end, the marks of what it sent are
    /// left for the next run to hold, not taken off.
    doubt: bool,
    gate: Option<Gate>,
}

impl SendingDriver {
    /// A piece of the mail run under way was refused: what it sent is held at the next run.
    pub fn doubted(&mut self) {
        self.doubt = true;
    }

    /// A Send/Receive that waited for the saver is let go of (called off, or its mailbox gone).
    pub fn forget_gate(&mut self) {
        self.gate = None;
    }

    /// Whether a run waits for its marks to be written.
    pub fn gate_waits(&self) -> bool {
        self.gate.is_some()
    }

    /// Sending from `account` is allowed, or was held for good: its Outbox no longer waits for
    /// the permission.
    pub fn allowed(&mut self, account: Id) -> bool {
        self.waits.remove(&account)
    }

    /// Account removal: nothing of these accounts' permission or run is kept.
    pub fn forget_accounts(&mut self, gone: &HashSet<Id>) {
        self.waits.retain(|account| !gone.contains(account));
        self.pressed.retain(|account| !gone.contains(account));
        if self.gate.is_some_and(|gate| gone.contains(&gate.account)) {
            self.gate = None;
        }
    }
}

impl Session {
    /// A mail run of `account` is to be handed to the host with `going` in it: a new run, so what an
    /// earlier one's refused piece said is not this one's; the messages are marked as being sent.
    /// `Some(rev)`: a mark is new, and the mailbox (at state `rev`) is to be saved before the
    /// send leaves; `None`: nothing to wait for.
    pub fn send_marking(&mut self, account: Id, going: &[Id]) -> Option<u64> {
        self.driver.sending.doubt = false;
        if !graph_send::mark_sending(&mut self.mailbox, account, going) {
            return None;
        }
        self.mailbox_changed();
        Some(self.rev())
    }

    /// The run of `account` waits for the saver to write the marks (state `rev`): one run at a
    /// time, so a gate left from another is the old mailbox's, and goes.
    pub fn send_gate_wait(&mut self, account: Id, rev: u64) {
        self.driver.sending.gate = Some(Gate { opening: self.openings(), rev, account });
    }

    /// A look at the gate: `saved` is the newest state of the mailbox that is on disk, `failed`
    /// whether the save failed.
    pub fn send_gate_step(&mut self, saved: u64, failed: bool) -> GateStep {
        let Some(gate) = self.driver.sending.gate else { return GateStep::Idle };
        if gate.opening != self.openings() {
            // (Nothing went: the mailbox it was for is gone, and its marks with it.)
            self.driver.sending.gate = None;
            return GateStep::Gone;
        }
        if !failed && saved < gate.rev {
            return GateStep::Waiting;
        }
        self.driver.sending.gate = None;
        if failed {
            self.sending_answered(gate.account);
            return GateStep::Failed(gate.account);
        }
        GateStep::Open(gate.account)
    }

    /// The run that waited for the saver `went` to the host or was called off while it waited:
    /// called off, nothing went, so nothing is in doubt.
    pub fn send_gate_went(&mut self, account: Id, went: bool) {
        if !went {
            self.sending_answered(account);
        }
    }

    /// The answer of `account`'s run was taken in: no mark is left on its messages.
    pub fn sending_answered(&mut self, account: Id) {
        if graph_send::sending_answered(&mut self.mailbox, account) {
            self.mailbox_changed();
        }
    }

    /// The run of `account` is over: when every piece of its answer was taken in, what it sent is
    /// filed and what it didn't send is known not to have gone, so no mark is left. A piece that
    /// was refused leaves them, for the next run to hold (AU3-26).
    pub fn send_run_answered(&mut self, account: Id) {
        if std::mem::take(&mut self.driver.sending.doubt) {
            log::warn!("send/receive: part of the answer was refused; what it sent is held at the next run");
        } else {
            self.sending_answered(account);
        }
    }

    /// A run of `account` begins: what an earlier run handed over without an answer coming back
    /// is held. The line for the owner (the first), when there was something.
    pub fn send_hold_unanswered(&mut self, account: Id) -> Option<String> {
        let mut said = Vec::new();
        if graph_send::hold_unanswered(&mut self.mailbox, account, self.interface_language, &mut said) {
            log::warn!("send/receive: {} message(s) handed to the server by an earlier run got no answer; held", said.len());
            self.mailbox_changed();
            return said.into_iter().next();
        }
        None
    }

    /// Whether the Outbox of the shared mailbox `account` waits for the owner's permission to send
    /// from shared mailboxes.
    pub fn shared_send_waits(&self, account: Id) -> bool {
        self.driver.sending.waits.contains(&account)
    }

    /// The owner pressed Send in `message` (#70). From a shared mailbox that may not be sent from
    /// yet, the owner is asked at once; otherwise the run that takes it out asks if it finds so,
    /// though that run starts by itself.
    pub fn shared_send_pressed_in(&mut self, message: Id) -> Pressed {
        let Some(account) = self.mailbox.message(message).map(|m| m.account) else { return Pressed::Nothing };
        if !self.mailbox.account(account).is_some_and(mailcraft_model::Account::is_shared) {
            return Pressed::Nothing;
        }
        if self.driver.sending.waits.contains(&account) {
            Pressed::Ask(account)
        } else {
            self.driver.sending.pressed.insert(account);
            Pressed::Noted
        }
    }

    /// The run of the shared mailbox `account` found the permission to send from shared mailboxes
    /// missing (#70): its Outbox waits. Whether the owner pressed Send in it since the last run
    /// (then he is asked even though the run started by itself). `None` for an account that is
    /// gone.
    pub fn shared_send_missing(&mut self, account: Id) -> Option<bool> {
        self.mailbox.account(account)?;
        self.driver.sending.waits.insert(account);
        Some(self.driver.sending.pressed.remove(&account))
    }

    /// The run of the shared mailbox `account` found nothing missing: what was pressed Send on is
    /// gone once its Outbox is empty.
    pub fn shared_send_none_waits(&mut self, account: Id) {
        let mb = &self.mailbox;
        let outbox = mb.folder_of_kind(account, FolderKind::Outbox).map_or(0, |f| mb.messages_in(f).filter(|m| m.held.is_empty()).count());
        if outbox == 0 {
            self.driver.sending.pressed.remove(&account);
        }
    }
}
