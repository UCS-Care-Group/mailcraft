// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Independent account runs share the existing UI-thread apply path, never a mutable mailbox.
use crate::{GraphResult, MailApp, MailRun};
use mailcraft_model::{Id, sync::GraphMailResult};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::Receiver,
};

/// Bound sockets, buffered results and per-frame work on very large account lists.
pub(crate) const MAX_MAIL_RUNS: usize = 4;

pub(crate) struct MailSlot {
    pub account: Id,
    sync_rx: Option<MailRun>,
    graph_rx: Option<(Id, bool, bool, u64, Receiver<GraphResult>)>,
    graph_pieces: Option<Receiver<GraphMailResult>>,
    graph_problems: Vec<String>,
    graph_pieces_taken: usize,
    graph_after_sign_in: Option<Id>,
    sync_totals: mailcraft_engine::cmd::account::SyncTotals,
    sync_run: Option<(Id, Option<usize>, bool, bool)>,
    sync_refused: bool,
    sync_password: Option<String>,
    send_gate: Option<crate::sending::SendGate>,
    send_doubt: bool,
    cancel: Arc<AtomicBool>,
}

impl MailSlot {
    fn new(account: Id) -> Self {
        Self {
            account,
            sync_rx: None,
            graph_rx: None,
            graph_pieces: None,
            graph_problems: Vec::new(),
            graph_pieces_taken: 0,
            graph_after_sign_in: None,
            sync_totals: Default::default(),
            sync_run: None,
            sync_refused: false,
            sync_password: None,
            send_gate: None,
            send_doubt: false,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn is_kind(&self, kind: crate::jobs::JobKind, mailbox: &mailcraft_model::Mailbox) -> bool {
        use crate::jobs::JobKind;
        match kind {
            JobKind::Imap => matches!(self.sync_rx, Some(MailRun::Imap { .. })),
            JobKind::Ews => matches!(self.sync_rx, Some(MailRun::Ews { .. })),
            JobKind::Jmap => matches!(self.sync_rx, Some(MailRun::Jmap { .. })),
            JobKind::Graph | JobKind::Shared => {
                self.graph_rx.is_some() && mailbox.account(self.account).is_some_and(|a| a.is_shared()) == (kind == JobKind::Shared)
            }
            _ => false,
        }
    }

    fn swap(&mut self, app: &mut MailApp) {
        use std::mem::swap;
        swap(&mut self.sync_rx, &mut app.sync_rx);
        swap(&mut self.graph_rx, &mut app.graph_rx);
        swap(&mut self.graph_pieces, &mut app.graph_pieces);
        swap(&mut self.graph_problems, &mut app.graph_problems);
        swap(&mut self.graph_pieces_taken, &mut app.graph_pieces_taken);
        swap(&mut self.graph_after_sign_in, &mut app.graph_after_sign_in);
        swap(&mut self.sync_totals, &mut app.sync_totals);
        swap(&mut self.sync_run, &mut app.sync_run);
        swap(&mut self.sync_refused, &mut app.sync_refused);
        swap(&mut self.sync_password, &mut app.sync_password);
        swap(&mut self.send_gate, &mut app.send_gate);
        swap(&mut self.send_doubt, &mut app.send_doubt);
        swap(&mut self.cancel, &mut app.services.sync_cancel);
    }
    fn running(&self) -> bool {
        self.sync_rx.is_some() || self.graph_rx.is_some() || self.send_gate.is_some()
    }
}

impl Drop for MailSlot {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl MailApp {
    /// Free the foreground receive slot for the next account. Interactive sign-in and Inbox
    /// checks retain their existing foreground state until they have finished.
    pub(crate) fn park_mail_run(&mut self) {
        let Some((account, ..)) = self.sync_run else { return };
        // Older integrations have one shared IMAP stop flag and must stay sequential.
        if matches!(self.sync_rx, Some(MailRun::Imap { .. })) && self.services.sync_with_cancel.is_none() {
            return;
        }
        if self.send_gate.is_some()
            || self.inbox_run.is_some()
            || self.inbox_check.under_way()
            || self.parallel_mail.len() >= MAX_MAIL_RUNS
            || (self.sync_rx.is_none() && self.graph_rx.is_none() && self.send_gate.is_none())
        {
            return;
        }
        let mut slot = MailSlot::new(account);
        slot.swap(self);
        self.parallel_mail.push(slot);
    }

    pub(crate) fn cancel_parallel_mail(&mut self) {
        for run in &mut self.parallel_mail {
            run.cancel.store(true, Ordering::Relaxed);
            if let Some((_, _, called_off, _)) = run.sync_run.as_mut() {
                *called_off = false;
            }
        }
    }

    /// Each account gets a bounded turn. Completed answers are applied on this thread, through
    /// the same IMAP/JMAP/Exchange/Graph handlers, including their saved-before-send gate.
    pub(crate) fn poll_parallel_mail(&mut self) {
        let runs = std::mem::take(&mut self.parallel_mail);
        for mut run in runs {
            run.swap(self);
            if self.auto_sync
                && let Some((account, Some(baseline), false, quiet)) = self.sync_run
                && self.waiting_to_go(account) > baseline
            {
                self.services.sync_cancel.store(true, Ordering::Relaxed);
                self.sync_run = Some((account, Some(baseline), true, quiet));
            }
            self.send_gate_tick(None);
            self.poll_graph();
            self.poll_sync();
            run.swap(self);
            if run.running() {
                self.parallel_mail.push(run);
            }
        }
    }
}

#[cfg(test)]
#[path = "parallel_sync_tests.rs"]
mod tests;
