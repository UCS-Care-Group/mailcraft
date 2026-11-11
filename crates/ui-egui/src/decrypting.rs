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

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use mailcraft_engine::decrypt::{DecryptJob, Failure, Plain};
use mailcraft_model::{Id, Message};

use crate::MailApp;

/// How often the window looks again while a message is being opened.
const POLL: Duration = Duration::from_millis(100);

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
    /// opening it when that hasn't begun.
    pub(crate) fn opened_view(&mut self, ctx: &egui::Context, id: Id) -> Option<Arc<Message>> {
        self.session.encryption_of(id)?;
        if let Some(job) = self.session.begin_decrypt(id) {
            match self.services.decrypting.as_ref() {
                Some(host) => {
                    self.decrypting.insert(id, host.open(job));
                }
                None => self.session.finish_decrypt(id, Err(Failure::Unavailable)),
            }
        }
        self.tick_decrypting(ctx);
        self.session.decrypted_message(id)
    }
}
