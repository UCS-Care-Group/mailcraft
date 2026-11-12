// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's side of opening Encrypted messages (#30 part 6; [`crate::Services::decrypting`]).
//!
//! The window asks the engine what is to be opened ([`mailcraft_engine::Session::begin_decrypt`]),
//! hands the job to the host, which opens it on a thread of its own with the keys it can reach
//! (the system's certificate store, a smartcard, gpg-agent), and takes what comes back into the
//! engine ([`mailcraft_engine::Session::finish_decrypt`]), which holds it in memory only. No key
//! or PIN passes through here; a smartcard's PIN is asked as signing's is
//! ([`crate::digital_signatures::SigningPin`]).
//!
//! **What starts an opening, and what doesn't.** A message is opened only when the owner is
//! looking at it: the owner chose it with input of their own (a click or key on its row, the Open
//! button, a window they opened: `owner_chose`, which nothing the control channel does adds to,
//! nested params and windows included), it has stayed on screen for
//! [`DWELL_MS`] (so that arrowing through a folder of Encrypted messages opens none of them), and
//! no other is being opened (one at a time: one decryption in memory, one question for a PIN or
//! passphrase on the screen at once). Any other message waits, and its bar offers Open.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use mailcraft_engine::decrypt::{DecryptJob, Decryption, Failure, Plain};
use mailcraft_model::{Id, Message};

use crate::MailApp;

/// How often the window looks again while a message is being opened.
const POLL: Duration = Duration::from_millis(100);
/// How long a message stays on screen before it is opened (milliseconds).
pub(crate) const DWELL_MS: f64 = 300.0;
/// A message not drawn for this long (milliseconds) is on screen afresh when it is drawn again.
const GAP_MS: f64 = 1_000.0;
/// The most messages the owner's choice is remembered for, and whose dwell is tracked.
const MAX_TRACKED: usize = 64;
/// How many frames after the control channel did something what shows is not the owner's choice.
const CONTROL_FRAMES: u64 = 3;

/// Opens Encrypted messages ([`crate::Services::decrypting`]).
pub trait DecryptHost: Send + Sync {
    /// Open the message `job` names, on a thread of the host's own.
    fn open(&self, job: DecryptJob) -> Receiver<Result<Plain, Failure>>;
}

impl MailApp {
    /// What the host has finished opening is taken into the engine; the window looks again soon
    /// while any is still being opened.
    pub(crate) fn tick_decrypting(&mut self, ctx: &egui::Context) {
        if self.decrypting.is_empty() {
            return;
        }
        let mut done = Vec::new();
        for (id, rx) in &self.decrypting {
            match rx.try_recv() {
                Ok(result) => done.push((*id, Some(result))),
                // The thread went without an answer: the same as any other failure.
                Err(TryRecvError::Disconnected) => done.push((*id, None)),
                Err(TryRecvError::Empty) => {}
            }
        }
        for (id, result) in done {
            self.decrypting.remove(&id);
            self.session.finish_decrypt(id, result.unwrap_or(Err(Failure::Failed)));
            ctx.request_repaint();
        }
        if !self.decrypting.is_empty() {
            ctx.request_repaint_after(POLL);
        }
    }

    /// The message `id` as it opens, when it is an Encrypted one and has been opened: begins
    /// opening it when that is due.
    pub(crate) fn opened_view(&mut self, ctx: &egui::Context, id: Id) -> Option<Arc<Message>> {
        self.opened_view_at(ctx, id, crate::now_ms())
    }

    /// [`MailApp::opened_view`] at the time `now` (milliseconds).
    pub(crate) fn opened_view_at(&mut self, ctx: &egui::Context, id: Id, now: f64) -> Option<Arc<Message>> {
        self.session.encryption_of(id)?;
        self.tick_decrypting(ctx);
        if matches!(self.session.decryption(id), Decryption::Waiting(_))
            && self.may_begin(ctx, id, now)
            && let Some(job) = self.session.begin_decrypt(id)
        {
            match self.services.decrypting.as_ref() {
                Some(host) => {
                    self.decrypting.insert(id, host.open(job));
                }
                None => self.session.finish_decrypt(id, Err(Failure::Unavailable)),
            }
        }
        let view = self.session.decrypted_message(id);
        if view.is_some() {
            // (What an agent's screenshot is not to show: see `issue_screenshots`.)
            self.decrypted_drawn = Some(self.epoch);
        }
        view
    }

    /// Whether message `id` may be opened now: the owner chose it, it has stayed on screen, and
    /// nothing else is being opened.
    fn may_begin(&mut self, ctx: &egui::Context, id: Id, now: f64) -> bool {
        if !self.owner_chose.contains(&id) {
            return false;
        }
        if self.decrypt_dwell.len() >= MAX_TRACKED {
            self.decrypt_dwell.retain(|_, (_, seen)| now - *seen <= GAP_MS);
            if self.decrypt_dwell.len() >= MAX_TRACKED {
                self.decrypt_dwell.clear();
            }
        }
        let entry = self.decrypt_dwell.entry(id).or_insert((now, now));
        if now - entry.1 > GAP_MS {
            entry.0 = now;
        }
        entry.1 = now;
        let stayed = now - entry.0;
        if stayed < DWELL_MS {
            ctx.request_repaint_after(Duration::from_millis((DWELL_MS - stayed).ceil().max(1.0) as u64));
            return false;
        }
        // (One at a time: this one is asked for when the one being opened is done.)
        self.decrypting.is_empty()
    }

    /// The control channel (an agent, a script, injected input) did something in this frame.
    pub(crate) fn touch_control(&mut self) {
        self.control_epoch = Some(self.epoch);
    }

    /// Whether the control channel did something lately: what it caused isn't the owner's choice.
    /// (A few frames, for a choice shows in the frame after the input that made it; an owner's
    /// click in the same moment as an agent's is taken as the agent's, to be on the safe side.)
    pub(crate) fn control_recent(&self) -> bool {
        self.control_epoch.is_some_and(|c| self.epoch.saturating_sub(c) <= CONTROL_FRAMES)
    }

    /// A frame begins. (What the owner chose is recorded where the owner's input picks a row, in
    /// [`MailApp::run`], never inferred from the focus having moved: an Undo, a selection that
    /// shrinks and the window's start all move it without anybody choosing.)
    pub(crate) fn track_choice(&mut self) {
        self.epoch += 1;
    }

    /// The owner chose message `id` themselves: an Encrypted message may be opened.
    pub(crate) fn owner_picks(&mut self, id: Id) {
        if self.owner_chose.contains(&id) {
            return;
        }
        if self.owner_chose.len() >= MAX_TRACKED {
            self.owner_chose.remove(0);
        }
        self.owner_chose.push(id);
    }

    /// Whether message `id` waits for the owner's Open: nothing the owner did chose it.
    pub(crate) fn decryption_needs_owner(&mut self, id: Id) -> bool {
        !self.owner_chose.contains(&id) && matches!(self.session.decryption(id), Decryption::Waiting(_))
    }

    /// The owner says to open message `id` (a click in the window, never one the control channel made).
    pub(crate) fn owner_opens(&mut self, id: Id) {
        self.owner_picks(id);
    }

    /// Whether something an Encrypted message opened to is on screen: in the reading pane, or in a
    /// window of its own. Nothing of the window is given to an agent then.
    pub(crate) fn decrypted_on_screen(&self) -> bool {
        let focus = self.session.sel.focus();
        self.session.opened_decryptions().into_iter().any(|id| focus == Some(id) || self.windows.of_message(id).is_some())
    }

    /// What the layout cache kept of a message that was opened and has been let go of is let go
    /// of too (it holds its text), once a frame.
    pub(crate) fn forget_unopened_layouts(&mut self, ctx: &egui::Context) {
        let gone = self.session.decryptions_let_go();
        if gone != self.decryption_gen {
            self.decryption_gen = gone;
            crate::reading::forget_layouts(ctx, |m| self.session.decrypted_view_gone(m));
            self.session.layouts_dropped();
        }
    }
}
