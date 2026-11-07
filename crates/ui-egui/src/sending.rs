// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a Send/Receive sends is marked as being sent, and the mark is on disk, before the run is
//! handed to the host (AU3-26).
//!
//! A run's answer can be lost: another mailbox takes the place of the open one and the answer is
//! let go of ([`crate::jobs`]), or the program stops mid-send. Without the mark the message would
//! still be in the Outbox at the next run and go a second time. With it, the next run of the
//! account finds the mark and holds the message ("it may have been sent"): the owner looks in
//! Sent Items and decides with Send Again. The run's answer, when it is taken in, clears the mark.
//!
//! Where the mailbox is kept by the background saver, the run waits for the saver to have written
//! the marks ([`MailApp::send_gate_tick`]); a save that fails sends nothing.

use std::sync::mpsc::{Receiver, Sender, channel};

use mailcraft_engine::cmd::graph_send;
use mailcraft_model::Id;
use mailcraft_model::sync::{EwsMailResult, JmapMailResult, SyncRequest, SyncResult};
use serde_json::json;

use crate::ews_mail::EwsJob;
use crate::jmap_mail::JmapJob;
use crate::{GraphRequest, GraphResult, MailApp, MailRun};

/// Whether the marks are on disk.
enum Saved {
    /// They are (or there is no file to keep them: nothing outlives the window then).
    Now,
    /// The background saver was handed this state of the mailbox, and hasn't said yet.
    Later(u64),
    /// The mailbox couldn't be saved: nothing is sent.
    Failed(String),
}

/// A run waiting for its marks to be written, with the end its stand-in answer comes through
/// until then.
pub(crate) enum Waiting {
    Imap(Box<SyncRequest>, Sender<SyncResult>),
    Graph(Box<GraphRequest>, Sender<GraphResult>),
    Ews(Box<EwsJob>, Sender<EwsMailResult>),
    Jmap(Box<JmapJob>, Sender<JmapMailResult>),
}

/// A Send/Receive that waits for the saver.
pub(crate) struct SendGate {
    /// The mailbox opening it was started under.
    opening: u64,
    /// The state of the mailbox with the marks.
    rev: u64,
    account: Id,
    waiting: Waiting,
}

fn not_saved(why: &str) -> String {
    crate::i18n::fmt(tl!("Nothing was sent: the mailbox couldn't be saved first ({error})"), &[("error", why)])
}

impl MailApp {
    /// Mark `ids` of `account` as being sent, and write the mailbox, before they leave.
    fn saved_before_sending(&mut self, account: Id, ids: &[Id]) -> Saved {
        // (A new run: what an earlier one's refused piece said is not this one's.)
        self.send_doubt = false;
        if !graph_send::mark_sending(&mut self.session.mailbox, account, ids) {
            return Saved::Now;
        }
        self.session.mailbox_changed();
        // (A mailbox that isn't kept between runs has nothing to come back to.)
        if !self.autosave {
            return Saved::Now;
        }
        let rev = self.session.rev();
        let written = if let Some(persist) = &self.services.persist {
            mailcraft_engine::io::write_mailbox(&self.session.mailbox).and_then(|bytes| persist(&bytes))
        } else if let (Some(saver), Some(path)) = (&self.services.saver, &self.session.path) {
            saver.follow(path);
            saver.submit(path, &self.session.mailbox, rev);
            return Saved::Later(rev);
        } else if self.session.path.is_some() {
            self.session.run("file.save", &json!({})).map(|_| ()).map_err(|e| e.to_string())
        } else {
            return Saved::Now;
        };
        match written {
            Ok(()) => {
                self.saved_rev = self.saved_rev.max(rev);
                Saved::Now
            }
            Err(e) => {
                log::warn!("send/receive: the mailbox couldn't be saved before sending; nothing is sent: {e}");
                self.sending_answered(account);
                Saved::Failed(not_saved(&e))
            }
        }
    }

    /// The run waits for the saver: a stand-in for the host's answer until the marks are written.
    fn wait_for_save<A>(&mut self, rev: u64, account: Id, waiting: impl FnOnce(Sender<A>) -> Waiting) -> Receiver<A> {
        let (tx, rx) = channel();
        // (One run at a time: a gate left from another is the old mailbox's, and goes.)
        self.send_gate = Some(SendGate { opening: self.session.openings(), rev, account, waiting: waiting(tx) });
        rx
    }

    /// Hand an IMAP account's run to the host, its Outbox marked first.
    pub(crate) fn hand_imap(&mut self, account: Id, request: SyncRequest) -> Result<Receiver<SyncResult>, String> {
        let going: Vec<Id> = request.outbox.iter().map(|m| m.id).collect();
        match self.saved_before_sending(account, &going) {
            Saved::Now => {
                self.services.sync.as_ref().map(|start| start(request)).ok_or_else(|| tl!("This build can't connect to mail servers").to_string())
            }
            Saved::Later(rev) => Ok(self.wait_for_save(rev, account, |tx| Waiting::Imap(Box::new(request), tx))),
            Saved::Failed(why) => Err(why),
        }
    }

    /// Hand a Graph job to the host, what it sends marked first.
    pub(crate) fn hand_graph(&mut self, account: Id, request: GraphRequest) -> Result<Receiver<GraphResult>, String> {
        let sends = request.mail.as_ref().map(|mail| mail.sends.iter().map(|up| up.message)).into_iter().flatten();
        let going: Vec<Id> = request.outbox.iter().map(|m| m.id).chain(sends).collect();
        match self.saved_before_sending(account, &going) {
            Saved::Now => {
                self.services.graph.as_ref().map(|graph| graph(request)).ok_or_else(|| tl!("This build can't connect to Microsoft").to_string())
            }
            Saved::Later(rev) => Ok(self.wait_for_save(rev, account, |tx| Waiting::Graph(Box::new(request), tx))),
            Saved::Failed(why) => Err(why),
        }
    }

    /// Hand an Exchange server account's run to the host, what it sends marked first.
    pub(crate) fn hand_ews(&mut self, account: Id, job: EwsJob) -> Result<Receiver<EwsMailResult>, String> {
        let going: Vec<Id> = job.request.sends.iter().map(|up| up.message).collect();
        match self.saved_before_sending(account, &going) {
            Saved::Now => self.services.ews.as_ref().map(|run| run(job)).ok_or_else(|| tl!("This build can't connect to mail servers").to_string()),
            Saved::Later(rev) => Ok(self.wait_for_save(rev, account, |tx| Waiting::Ews(Box::new(job), tx))),
            Saved::Failed(why) => Err(why),
        }
    }

    /// Hand a JMAP account's run to the host, what it sends marked first.
    pub(crate) fn hand_jmap(&mut self, account: Id, job: JmapJob) -> Result<Receiver<JmapMailResult>, String> {
        let going: Vec<Id> = job.request.sends.iter().map(|up| up.message).collect();
        match self.saved_before_sending(account, &going) {
            Saved::Now => self.services.jmap.as_ref().map(|run| run(job)).ok_or_else(|| tl!("This build can't connect to mail servers").to_string()),
            Saved::Later(rev) => Ok(self.wait_for_save(rev, account, |tx| Waiting::Jmap(Box::new(job), tx))),
            Saved::Failed(why) => Err(why),
        }
    }

    /// Every frame, once the saver has reported: the run that waited for its marks to be written
    /// goes to the host, or, when the save failed (`failed`), sends nothing and ends saying so.
    pub(crate) fn send_gate_tick(&mut self, failed: Option<&str>) {
        let Some(gate) = &self.send_gate else { return };
        if gate.opening != self.session.openings() {
            // (Nothing went: the mailbox it was for is gone, and its marks with it.)
            self.send_gate = None;
            return;
        }
        if failed.is_none() && self.saved_rev < gate.rev {
            return;
        }
        let Some(SendGate { account, waiting, .. }) = self.send_gate.take() else { return };
        if let Some(e) = failed {
            log::warn!("send/receive: the mailbox couldn't be saved before sending; nothing is sent: {e}");
            self.sending_answered(account);
            let errors = vec![not_saved(e)];
            // The stand-in answers: the run ends as any run that couldn't go.
            match waiting {
                Waiting::Imap(_, tx) => drop(tx.send(SyncResult { account, errors, partial: false, ..Default::default() })),
                Waiting::Graph(_, tx) => drop(tx.send(GraphResult { account, errors, ..Default::default() })),
                Waiting::Ews(_, tx) => drop(tx.send(EwsMailResult { account, errors, partial: false, ..Default::default() })),
                Waiting::Jmap(_, tx) => drop(tx.send(JmapMailResult { account, errors, partial: false, ..Default::default() })),
            }
            return;
        }
        // The host's answer takes the stand-in's place, in the run that is still this one.
        let went = match waiting {
            Waiting::Imap(request, _) => {
                if let (Some(start), Some(MailRun::Imap { account: whose, rx, .. })) = (&self.services.sync, &mut self.sync_rx)
                    && *whose == account
                {
                    *rx = start(*request);
                    true
                } else {
                    false
                }
            }
            Waiting::Graph(request, _) => {
                if let (Some(graph), Some((whose, _, _, _, rx))) = (&self.services.graph, &mut self.graph_rx)
                    && *whose == account
                {
                    *rx = graph(*request);
                    true
                } else {
                    false
                }
            }
            Waiting::Ews(job, _) => {
                if let (Some(run), Some(MailRun::Ews { account: whose, pieces, .. })) = (&self.services.ews, &mut self.sync_rx)
                    && *whose == account
                {
                    *pieces = run(*job);
                    true
                } else {
                    false
                }
            }
            Waiting::Jmap(job, _) => {
                if let (Some(run), Some(MailRun::Jmap { account: whose, pieces, .. })) = (&self.services.jmap, &mut self.sync_rx)
                    && *whose == account
                {
                    *pieces = run(*job);
                    true
                } else {
                    false
                }
            }
        };
        // The run was called off while it waited: nothing went, so nothing is in doubt.
        if !went {
            self.sending_answered(account);
        }
    }

    /// The answer of `account`'s run was taken in: no mark is left on its messages.
    pub(crate) fn sending_answered(&mut self, account: Id) {
        if graph_send::sending_answered(&mut self.session.mailbox, account) {
            self.session.mailbox_changed();
        }
    }

    /// The run of `account` is over: when every piece of its answer was taken in, what it sent is
    /// filed and what it didn't send is known not to have gone, so no mark is left. A piece that
    /// was refused leaves them, for the next run to hold (AU3-26).
    pub(crate) fn run_answered(&mut self, account: Id) {
        if std::mem::take(&mut self.send_doubt) {
            log::warn!("send/receive: part of the answer was refused; what it sent is held at the next run");
        } else {
            self.sending_answered(account);
        }
    }

    /// A run of `account` begins: what an earlier run handed over without an answer coming back
    /// is held, and the owner told.
    pub(crate) fn hold_unanswered(&mut self, account: Id) {
        let mut said = Vec::new();
        if graph_send::hold_unanswered(&mut self.session.mailbox, account, self.session.interface_language, &mut said) {
            log::warn!("send/receive: {} message(s) handed to the server by an earlier run got no answer; held", said.len());
            self.session.mailbox_changed();
            if let Some(first) = said.first() {
                self.status(first.clone());
            }
        }
    }
}
