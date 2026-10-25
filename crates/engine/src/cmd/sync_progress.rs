// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send/Receive Progress (#120): what the run under way is doing, or what the last one did,
//! account by account. Kept in the session, in memory only (never saved); `sync.apply` and
//! `sync.applyGraph` keep it up to date as the pieces of a run come in, and `sync.progress`
//! answers it.
//!
//! A run starts when a host says which accounts it covers ([`SyncProgress::begin`]), or else with
//! the first piece for an account when no run is under way. Problems may name a message by its
//! subject: that is message content.
//!
//! With the rows, how far each Microsoft account's messages have been fetched whole in the
//! background (Full messages), counted again as they come in, and whether Stop has paused that
//! until the next run starts.

use std::collections::HashMap;

use mailcraft_model::sync::{SyncPhase, SyncStage, SyncStep, Timings};
use mailcraft_model::time::Timestamp;
use mailcraft_model::{AccountKind, Id, Mailbox};
use serde_json::{Value, json};

use super::graph_down;
use crate::{CmdResult, CommandSpec, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("sync.progress", "Show Progress", "Send / Receive", progress).pure().params(
        r#"{} — answers {"running": bool, "accounts": [{"id", "name", "state": "waiting"|"running"|"done"|"stopped"|"skipped", "phase": "inbox"|"sending"|"folders"|"newMail"|"calendarAndContacts"|null, "folder", "done", "total", "new", "sent", "problems": [string], "kept": bool, "timings": {"millis", "requests", "skipped", "phases": [{"phase", "millis", "requests"}]}, "fullMessages": {"done", "total", "paused"}|null}]}; opens nothing"#,
    )]
}

/// Problems kept for an account, at most, and how long each may be.
const MAX_PROBLEMS: usize = 60;
const MAX_PROBLEM_CHARS: usize = 600;

/// Where an account is in a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunState {
    /// Its turn hasn't come.
    #[default]
    Waiting,
    Running,
    Done,
    /// Its part of the run was called off before it was through.
    Stopped,
    /// The owner left it out of the run while it waited its turn (Skip).
    Skipped,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Waiting => "waiting",
            RunState::Running => "running",
            RunState::Done => "done",
            RunState::Stopped => "stopped",
            RunState::Skipped => "skipped",
        }
    }

    fn under_way(self) -> bool {
        matches!(self, RunState::Waiting | RunState::Running)
    }
}

/// One account's row: where it is in the run, how far it has got, and what it came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountRun {
    pub account: Id,
    pub state: RunState,
    /// What it is doing (running), or did last; None before its first piece.
    pub phase: Option<SyncPhase>,
    /// The folder it is at, by its shown name; empty when it is at none.
    pub folder: String,
    /// How many of how many (messages sent, or a folder's new messages); 0 when not known.
    pub done: u32,
    pub total: u32,
    /// Which folder of how many it is at while it brings new mail down (the first is 1); 0 of 0
    /// when not known.
    pub folder_number: u32,
    pub folders: u32,
    /// New messages and messages sent so far in the run.
    pub new: usize,
    pub sent: usize,
    /// What went wrong, in plain words, in the order it was said.
    pub problems: Vec<String>,
    /// The run changed nothing for it (no new mail, nothing sent, no problems): what it shows is
    /// what the run before came to.
    pub kept: bool,
    /// How long each phase of its part of the run took and how many requests it made (#276),
    /// as the pieces said; reset when it starts a run.
    pub timings: Timings,
}

impl AccountRun {
    /// How far its part of the run has got, 0 to 1: nothing while it waits, all of it once it is
    /// over, and in between by its phase (sending, the folders, new mail folder by folder, the
    /// calendar and contacts), and within new mail by the folder it is at and the messages of it
    /// that have come.
    pub fn fraction(&self) -> f32 {
        let part = |done: u32, total: u32| if total == 0 { 0.0 } else { (done.min(total) as f32 / total as f32).clamp(0.0, 1.0) };
        match self.state {
            RunState::Waiting => 0.0,
            RunState::Done | RunState::Stopped | RunState::Skipped => 1.0,
            RunState::Running => match self.phase {
                None => 0.0,
                Some(SyncPhase::Inbox) => 0.02,
                Some(SyncPhase::Sending) => 0.1 * part(self.done, self.total),
                Some(SyncPhase::Folders) => 0.1,
                Some(SyncPhase::NewMail) => {
                    let within = part(self.done, self.total);
                    let folders = if self.folders == 0 {
                        within
                    } else {
                        ((self.folder_number.saturating_sub(1) as f32 + within) / self.folders as f32).clamp(0.0, 1.0)
                    };
                    0.15 + 0.75 * folders
                }
                Some(SyncPhase::CalendarAndContacts) => 0.92,
            },
        }
    }
}

/// What an account's part of a run came to, kept for the next run that changes nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Outcome {
    new: usize,
    sent: usize,
    problems: Vec<String>,
    stopped: bool,
}

/// How many of a Microsoft account's messages are here whole, of how many are to be: those the
/// background fetches (its Download messages in full), not the large ones that wait to be opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FullMessages {
    pub done: u32,
    pub total: u32,
}

impl FullMessages {
    /// Whether some are still to be fetched.
    pub fn waiting(self) -> bool {
        self.done < self.total
    }
}

/// The current run, or else the last one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncProgress {
    /// The accounts of the run, in the order they go.
    pub accounts: Vec<AccountRun>,
    /// What each account's last run came to (as last shown).
    last: HashMap<Id, Outcome>,
    /// Accounts whose part of the run goes on after their last piece of mail (the calendar and
    /// contacts come after it): the host ends it with [`SyncProgress::end`]. With each, whether
    /// that last piece said it was stopped.
    held: HashMap<Id, bool>,
    /// Full messages of each Microsoft account, as last counted.
    full: HashMap<Id, FullMessages>,
    /// When each account's full messages were last counted from the mailbox, and whether pages
    /// came since that weren't counted (a run's pages are counted at most once a second).
    counted: HashMap<Id, (Timestamp, bool)>,
    /// How many times full messages were counted from the mailbox: each count goes through every
    /// message, so a run doesn't count after every page it brings.
    recounts: u64,
    /// Stop paused the fetching of full messages, until the next run starts.
    pub full_paused: bool,
}

/// What a piece of a run says, whatever kind of account it is for.
#[derive(Default)]
pub(crate) struct Piece<'a> {
    /// The step it is at, when the piece says (else the row keeps the one before).
    pub step: Option<(SyncPhase, String, u32, u32)>,
    /// Which folder of how many the run is at, when the piece says (else the row keeps it).
    pub folders: Option<(u32, u32)>,
    pub new: usize,
    pub sent: usize,
    pub problems: &'a [String],
    /// The last piece of the account's part of the run.
    pub last: bool,
    pub stopped: bool,
    /// Time and requests since the piece before (#276).
    pub timings: Option<&'a Timings>,
}

impl SyncProgress {
    /// Whether a run is under way: an account in it is running or waiting its turn.
    pub fn running(&self) -> bool {
        self.accounts.iter().any(|a| a.state.under_way())
    }

    /// How far the run under way has got, 0 to 1, its accounts counted alike; `None` when no run
    /// is under way.
    pub fn fraction(&self) -> Option<f32> {
        if !self.running() || self.accounts.is_empty() {
            return None;
        }
        let sum: f32 = self.accounts.iter().map(AccountRun::fraction).sum();
        Some((sum / self.accounts.len() as f32).clamp(0.0, 1.0))
    }

    /// The account whose part of the run is under way now, if one is.
    pub fn current(&self) -> Option<&AccountRun> {
        self.accounts.iter().find(|r| r.state == RunState::Running)
    }

    /// A new run, of these accounts in this order, every one waiting: it takes the place of the
    /// one before. (For a host that knows which accounts a run covers; without it, a run is
    /// begun by its first piece.)
    pub fn begin(&mut self, accounts: &[Id]) {
        let mut rows: Vec<AccountRun> = Vec::new();
        for &account in accounts {
            if !rows.iter().any(|r| r.account == account) {
                rows.push(AccountRun { account, ..Default::default() });
            }
        }
        self.accounts = rows;
    }

    /// A host's word on the phase an account is in, for what no piece of mail says (the calendar
    /// and contacts, the messages Microsoft sends before the mail server is asked).
    pub fn phase(&mut self, account: Id, phase: SyncPhase) {
        self.take(account, Piece { step: Some((phase, String::new(), 0, 0)), ..Default::default() });
    }

    /// The account's part of the run doesn't end with its last piece of mail: the host has more
    /// for it, and says when it is over ([`SyncProgress::end`]).
    pub fn hold(&mut self, account: Id) {
        self.held.insert(account, false);
    }

    /// Whether the account's part of the run goes on after its mail ([`SyncProgress::hold`]).
    pub fn holding(&self, account: Id) -> bool {
        self.held.contains_key(&account)
    }

    /// A host's word that the account's part of the run is over, when no piece said so (its
    /// mail was held for the calendar, or the host gave up on it). Nothing for an account that
    /// isn't running or waiting in the run.
    pub fn end(&mut self, account: Id, stopped: bool) {
        let stopped = self.held.remove(&account).unwrap_or(false) || stopped;
        if self.accounts.iter().any(|r| r.account == account && r.state.under_way()) {
            self.take(account, Piece { last: true, stopped, ..Default::default() });
        }
    }

    /// The account's Inbox pass is over (#276): it waits for the rest of its part of the run,
    /// which goes on from its numbers. Nothing for a row that isn't running its Inbox pass.
    pub fn rest(&mut self, account: Id) {
        if let Some(row) =
            self.accounts.iter_mut().find(|r| r.account == account && r.state == RunState::Running && r.phase == Some(SyncPhase::Inbox))
        {
            (row.state, row.phase, row.folder) = (RunState::Waiting, None, String::new());
            (row.done, row.total, row.folder_number, row.folders) = (0, 0, 0, 0);
        }
    }

    /// Leave an account that is still waiting its turn out of the run: it won't be started (it
    /// needs the owner, or couldn't start). Whether it was waiting.
    pub fn leave_out(&mut self, account: Id) -> bool {
        let before = self.accounts.len();
        self.accounts.retain(|r| r.account != account || r.state != RunState::Waiting);
        self.accounts.len() != before
    }

    /// Leave an account that is waiting its turn out of the run because the owner said so: its
    /// row says it was skipped. Whether it was waiting.
    pub fn skip(&mut self, account: Id) -> bool {
        let Some(row) = self.accounts.iter_mut().find(|r| r.account == account && r.state == RunState::Waiting) else { return false };
        row.state = RunState::Skipped;
        true
    }

    /// Count the account's full messages again, from the mailbox (nothing kept for an account
    /// that isn't a Microsoft one).
    pub fn count_full(&mut self, mb: &Mailbox, account: Id, now: Timestamp) {
        if mb.account(account).is_none_or(|a| a.kind != AccountKind::Microsoft) {
            self.full.remove(&account);
            self.counted.remove(&account);
            return;
        }
        self.counted_as(mb, account, graph_down::full_counts(mb, account, now), now);
    }

    /// Keep `counts`, just counted from the mailbox at `now`, as the account's full messages
    /// (nothing for an account that isn't a Microsoft one).
    pub(crate) fn counted_as(&mut self, mb: &Mailbox, account: Id, counts: graph_down::FullCounts, now: Timestamp) {
        if mb.account(account).is_none_or(|a| a.kind != AccountKind::Microsoft) {
            self.full.remove(&account);
            self.counted.remove(&account);
            return;
        }
        let total = counts.all.saturating_sub(counts.large);
        let done = total.saturating_sub(counts.waiting.saturating_sub(counts.large));
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        self.full.insert(account, FullMessages { done: count(done), total: count(total) });
        self.counted.insert(account, (now, false));
        self.recounts = self.recounts.saturating_add(1);
    }

    /// Pages of a run came for the account and the run goes on: its full messages are counted
    /// again unless they already were in this second of the clock, in which case they are counted
    /// when the next page comes in a later one, or when the account's part of the run ends
    /// ([`SyncProgress::count_full_if_due`]).
    pub fn count_full_now_and_then(&mut self, mb: &Mailbox, account: Id, now: Timestamp) {
        match self.counted.get_mut(&account) {
            Some((at, due)) if *at == now => *due = true,
            _ => self.count_full(mb, account, now),
        }
    }

    /// Count the account's full messages again if pages came since they were last counted.
    pub fn count_full_if_due(&mut self, mb: &Mailbox, account: Id, now: Timestamp) {
        if self.counted.get(&account).is_some_and(|(_, due)| *due) {
            self.count_full(mb, account, now);
        }
    }

    /// How many times full messages were counted from the mailbox, every message gone through.
    pub fn recounts(&self) -> u64 {
        self.recounts
    }

    /// Count every Microsoft account's full messages again (what a host said of another account
    /// stays; what was kept for an account that is gone goes).
    pub fn count_all_full(&mut self, mb: &Mailbox, now: Timestamp) {
        self.full.retain(|account, _| mb.account(*account).is_some());
        self.counted.retain(|account, _| mb.account(*account).is_some());
        let accounts: Vec<Id> = mb.accounts.iter().filter(|a| a.kind == AccountKind::Microsoft).map(|a| a.id).collect();
        for account in accounts {
            self.count_full(mb, account, now);
        }
    }

    /// A host's own count of the account's full messages (a stand-in that has no such mail).
    pub fn set_full(&mut self, account: Id, done: u32, total: u32) {
        self.full.insert(account, FullMessages { done: done.min(total), total });
    }

    /// The account's full messages as last counted; `None` for one never counted (or not a
    /// Microsoft account).
    pub fn full(&self, account: Id) -> Option<FullMessages> {
        self.full.get(&account).copied()
    }

    /// The account's row for a piece that came for it, running from now on. A piece for an
    /// account that isn't in the run under way joins it (one already done there carries on from
    /// what it had); when no run is under way, it begins one.
    fn row(&mut self, account: Id) -> Option<&mut AccountRun> {
        if !self.running() {
            self.begin(&[account]);
        } else if !self.accounts.iter().any(|r| r.account == account) {
            self.accounts.push(AccountRun { account, ..Default::default() });
        }
        let row = self.accounts.iter_mut().find(|r| r.account == account)?;
        if row.state != RunState::Running {
            // (A row that waits has nothing but, perhaps, its Inbox pass: that stays.)
            if row.state != RunState::Waiting {
                row.timings = Timings::default();
            }
            row.state = RunState::Running;
            row.kept = false;
        }
        Some(row)
    }

    /// Take in a piece of the account's part of a run.
    pub(crate) fn take(&mut self, account: Id, piece: Piece) {
        // The Inbox pass is never left out: what it brings counts for an account the owner
        // skipped, whose row stays skipped (#276).
        if matches!(piece.step, Some((SyncPhase::Inbox, ..)))
            && let Some(row) = self.accounts.iter_mut().find(|r| r.account == account && r.state == RunState::Skipped)
        {
            row.new = row.new.saturating_add(piece.new);
            if let Some(timings) = piece.timings {
                row.timings.absorb(timings);
            }
            return;
        }
        let Some(row) = self.row(account) else { return };
        if let Some((phase, folder, done, total)) = piece.step {
            row.phase = Some(phase);
            row.folder = folder;
            (row.done, row.total) = (done, total);
            if phase != SyncPhase::NewMail {
                (row.folder_number, row.folders) = (0, 0);
            }
        }
        if let Some((number, folders)) = piece.folders {
            (row.folder_number, row.folders) = (number.min(folders), folders);
        }
        row.new = row.new.saturating_add(piece.new);
        row.sent = row.sent.saturating_add(piece.sent);
        if let Some(timings) = piece.timings {
            row.timings.absorb(timings);
        }
        for problem in piece.problems {
            if row.problems.len() >= MAX_PROBLEMS {
                break;
            }
            let line: String = problem.chars().filter(|c| *c != '\r' && *c != '\0').take(MAX_PROBLEM_CHARS).collect();
            if !line.trim().is_empty() {
                row.problems.push(line);
            }
        }
        if !piece.last {
            return;
        }
        if let Some(stopped) = self.held.get_mut(&account) {
            *stopped |= piece.stopped;
            return;
        }
        let Some(row) = self.accounts.iter_mut().find(|r| r.account == account) else { return };
        row.state = if piece.stopped { RunState::Stopped } else { RunState::Done };
        (row.folder, row.done, row.total, row.folder_number, row.folders) = (String::new(), 0, 0, 0, 0);
        let outcome = Outcome { new: row.new, sent: row.sent, problems: row.problems.clone(), stopped: piece.stopped };
        let nothing = outcome.new == 0 && outcome.sent == 0 && outcome.problems.is_empty() && !outcome.stopped;
        match self.last.get(&account) {
            // A run that changed nothing leaves the last result in place.
            Some(before) if nothing => {
                let before = before.clone();
                if let Some(row) = self.accounts.iter_mut().find(|r| r.account == account) {
                    (row.new, row.sent, row.problems) = (before.new, before.sent, before.problems);
                    row.state = if before.stopped { RunState::Stopped } else { RunState::Done };
                    row.kept = true;
                }
            }
            _ => {
                self.last.insert(account, outcome);
            }
        }
    }

    /// The state as JSON, for `sync.progress`: accounts the mailbox no longer has are left out.
    pub fn json(&self, mb: &Mailbox) -> Value {
        let accounts: Vec<Value> = self
            .accounts
            .iter()
            .filter_map(|r| {
                let name = mb.account(r.account)?.name.clone();
                Some(json!({
                    "id": r.account,
                    "name": name,
                    "state": r.state.as_str(),
                    "phase": r.phase.map(phase_id),
                    "folder": r.folder,
                    "done": r.done,
                    "total": r.total,
                    "new": r.new,
                    "sent": r.sent,
                    "problems": r.problems,
                    "kept": r.kept,
                    "timings": timings_json(&r.timings),
                    "fullMessages": self.full(r.account).map(|f| json!({"done": f.done, "total": f.total, "paused": self.full_paused})),
                }))
            })
            .collect();
        json!({"running": self.running(), "accounts": accounts})
    }
}

/// An account's timings for `sync.progress` (#276): numbers only, no address and no folder name.
fn timings_json(t: &Timings) -> Value {
    let phases: Vec<Value> = t.phases.iter().map(|p| json!({"phase": phase_id(p.phase), "millis": p.millis, "requests": p.requests})).collect();
    json!({"millis": t.millis(), "requests": t.requests(), "skipped": t.skipped, "phases": phases})
}

fn phase_id(phase: SyncPhase) -> &'static str {
    match phase {
        SyncPhase::Inbox => "inbox",
        SyncPhase::Sending => "sending",
        SyncPhase::Folders => "folders",
        SyncPhase::NewMail => "newMail",
        SyncPhase::CalendarAndContacts => "calendarAndContacts",
    }
}

/// A mail server's step as a row shows it: a folder's new mail counted in messages, the folder
/// being looked at with no count yet, what was sent of how many.
pub(crate) fn imap_step(step: &SyncStep) -> (SyncPhase, String, u32, u32) {
    let folder = shown(&step.folder);
    match step.stage {
        SyncStage::Sending | SyncStage::Reading => (step.stage.phase(), folder, step.done, step.total),
        SyncStage::Checking | SyncStage::Updating => (step.stage.phase(), folder, 0, 0),
        SyncStage::Connecting | SyncStage::Listed | SyncStage::Filing => (step.stage.phase(), String::new(), 0, 0),
    }
}

/// A folder's name for a row: one line, not too long.
pub(crate) fn shown(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(80).collect()
}

fn progress(s: &mut Session, _: &Value) -> CmdResult {
    Ok(s.sync_progress.json(&s.mailbox))
}

#[cfg(test)]
mod tests {
    use mailcraft_model::sync::{FolderDelta, FolderListing, GraphFolder, GraphMailResult, GraphProgress, MessagePage, SyncResult};
    use mailcraft_model::{FolderKind, Message};
    use serde_json::json;

    use super::*;

    /// A session with an IMAP account and a Microsoft account kept in step through Graph.
    fn session() -> (Session, Id, Id) {
        let mut s = Session::empty();
        let servers = json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}});
        let imap = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
        let graph = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        (s, imap, graph)
    }

    fn progress(s: &mut Session) -> Value {
        s.run("sync.progress", &json!({})).unwrap()
    }

    fn row(s: &mut Session, account: Id) -> Value {
        let all = progress(s);
        all["accounts"].as_array().and_then(|rows| rows.iter().find(|r| r["id"] == json!(account)).cloned()).unwrap_or(Value::Null)
    }

    fn apply(s: &mut Session, piece: SyncResult) {
        s.run("sync.apply", &json!({"result": piece})).unwrap();
    }

    fn graph(s: &mut Session, piece: GraphMailResult) {
        s.run("sync.applyGraph", &json!({"result": piece})).unwrap();
    }

    fn step(stage: SyncStage, folder: &str, done: usize, total: usize) -> Option<SyncStep> {
        Some(SyncStep::new(stage, folder, done, total))
    }

    fn mail(uids: &[u32], uid_seen: u32) -> Vec<FolderDelta> {
        let new = uids
            .iter()
            .map(|uid| Message { uid: *uid, subject: format!("Mail {uid}"), message_id: format!("m{uid}@mail.example"), ..Default::default() })
            .collect();
        vec![FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen, new, ..Default::default() }]
    }

    #[test]
    fn a_run_is_followed_account_by_account() {
        let (mut s, imap, ms) = session();
        assert_eq!(progress(&mut s), json!({"running": false, "accounts": []}));
        // A host that knows the run's accounts says so: both wait their turn, in that order.
        s.sync_progress.begin(&[imap, ms, imap]);
        let all = progress(&mut s);
        assert_eq!(all["running"], json!(true));
        let states: Vec<(u64, &str)> =
            all["accounts"].as_array().unwrap().iter().map(|r| (r["id"].as_u64().unwrap(), r["state"].as_str().unwrap())).collect();
        assert_eq!(states, [(imap, "waiting"), (ms, "waiting")]);
        assert_eq!(all["accounts"][0]["name"], json!(s.mailbox.account(imap).unwrap().name));

        // The IMAP account's pieces: its phase, folder and counts as they go.
        let piece = |step: Option<SyncStep>| SyncResult { account: imap, partial: true, progress: String::new(), step, ..Default::default() };
        apply(&mut s, piece(step(SyncStage::Sending, "", 0, 2)));
        let r = row(&mut s, imap);
        assert_eq!(
            (r["state"].as_str(), r["phase"].as_str(), r["done"].as_u64(), r["total"].as_u64()),
            (Some("running"), Some("sending"), Some(0), Some(2))
        );
        // (A host that says only the numbers has its line made of them.)
        assert_eq!(s.sync_status, "Sending 2 messages");
        apply(
            &mut s,
            SyncResult { errors: vec!["Couldn't send \"Quarterly figures\": refused".into()], ..piece(step(SyncStage::Sending, "", 1, 2)) },
        );
        apply(&mut s, piece(step(SyncStage::Checking, "Inbox", 1, 3)));
        let r = row(&mut s, imap);
        assert_eq!((r["phase"].as_str(), r["folder"].as_str(), r["total"].as_u64()), (Some("newMail"), Some("Inbox"), Some(0)));
        apply(&mut s, SyncResult { folders: mail(&[5, 6], 0), ..piece(step(SyncStage::Reading, "Inbox", 2, 3)) });
        let r = row(&mut s, imap);
        assert_eq!((r["folder"].as_str(), r["done"].as_u64(), r["total"].as_u64(), r["new"].as_u64()), (Some("Inbox"), Some(2), Some(3), Some(2)));
        assert_eq!(row(&mut s, ms)["state"], json!("waiting"));
        // Its last piece: done, with what it came to and its problem.
        apply(&mut s, SyncResult { account: imap, folders: mail(&[4], 6), ..Default::default() });
        let r = row(&mut s, imap);
        assert_eq!(
            (r["state"].as_str(), r["new"].as_u64(), r["folder"].as_str(), r["kept"].as_bool()),
            (Some("done"), Some(3), Some(""), Some(false))
        );
        assert_eq!(r["problems"], json!(["Couldn't send \"Quarterly figures\": refused"]));
        assert_eq!(progress(&mut s)["running"], json!(true), "the Microsoft account is still to go");

        // The Microsoft account's pieces: the folders, then a folder's mail, then stopped.
        let inbox = GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, total: 500, ..Default::default() };
        let listing = Some(FolderListing { folders: vec![inbox], complete: true });
        graph(&mut s, GraphMailResult { account: ms, listing, partial: true, ..Default::default() });
        assert_eq!((row(&mut s, ms)["state"].as_str(), row(&mut s, ms)["phase"].as_str()), (Some("running"), Some("folders")));
        let at = Some(GraphProgress { folder: "in".into(), read: 150, total: 500, number: 1, folders: 1 });
        graph(&mut s, GraphMailResult { account: ms, progress: at, partial: true, ..Default::default() });
        let r = row(&mut s, ms);
        assert_eq!(
            (r["phase"].as_str(), r["folder"].as_str(), r["done"].as_u64(), r["total"].as_u64()),
            (Some("newMail"), Some("Inbox"), Some(150), Some(500))
        );
        graph(&mut s, GraphMailResult { account: ms, stopped: true, ..Default::default() });
        let all = progress(&mut s);
        assert_eq!((all["running"].as_bool(), all["accounts"][1]["state"].as_str()), (Some(false), Some("stopped")));
        assert_eq!(all["accounts"][0]["state"], json!("done"));
    }

    #[test]
    fn a_run_that_changed_nothing_keeps_the_last_result() {
        let (mut s, imap, ms) = session();
        // No run was begun: the first piece begins one, of its account alone.
        apply(&mut s, SyncResult { account: imap, folders: mail(&[1, 2], 2), ..Default::default() });
        let r = row(&mut s, imap);
        assert_eq!((r["state"].as_str(), r["new"].as_u64(), r["kept"].as_bool()), (Some("done"), Some(2), Some(false)));
        // A run every few minutes that brings nothing doesn't wipe it with "0 new".
        apply(&mut s, SyncResult { account: imap, partial: true, step: step(SyncStage::Connecting, "", 0, 0), ..Default::default() });
        assert_eq!((row(&mut s, imap)["state"].as_str(), row(&mut s, imap)["new"].as_u64()), (Some("running"), Some(0)));
        apply(&mut s, SyncResult { account: imap, ..Default::default() });
        let r = row(&mut s, imap);
        assert_eq!((r["state"].as_str(), r["new"].as_u64(), r["kept"].as_bool()), (Some("done"), Some(2), Some(true)));
        // A run with a problem is something to tell: it takes the place of the last result...
        apply(&mut s, SyncResult { account: imap, errors: vec!["Couldn't open \"Clutter\": no".into()], ..Default::default() });
        let r = row(&mut s, imap);
        assert_eq!((r["new"].as_u64(), r["problems"].as_array().map(Vec::len), r["kept"].as_bool()), (Some(0), Some(1), Some(false)));
        // ...and so is one that was stopped, even with nothing in it.
        apply(&mut s, SyncResult { account: imap, stopped: true, ..Default::default() });
        let r = row(&mut s, imap);
        assert_eq!((r["state"].as_str(), r["problems"].as_array().map(Vec::len), r["kept"].as_bool()), (Some("stopped"), Some(0), Some(false)));
        // A new run takes the place of the last: only its accounts are in it.
        graph(&mut s, GraphMailResult { account: ms, ..Default::default() });
        let all = progress(&mut s);
        assert_eq!((all["accounts"].as_array().map(Vec::len), all["accounts"][0]["id"].as_u64()), (Some(1), Some(ms)));
        assert_eq!(all["accounts"][0]["state"], json!("done"));
        // An answer for an account that isn't there changes nothing.
        let before = s.sync_progress.clone();
        assert!(s.run("sync.apply", &json!({"result": {"account": 424242}})).is_err());
        assert!(s.run("sync.applyGraph", &json!({"result": {"account": imap}})).is_err());
        assert_eq!(s.sync_progress, before);
    }

    #[test]
    fn graph_mail_counts_as_new_and_problems_are_kept_within_bounds() {
        let (mut s, _, ms) = session();
        let inbox = GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() };
        graph(
            &mut s,
            GraphMailResult {
                account: ms,
                listing: Some(FolderListing { folders: vec![inbox], complete: true }),
                partial: true,
                ..Default::default()
            },
        );
        let page = |id: &str| {
            serde_json::from_value::<MessagePage>(json!({"folder": "in", "messages": [{"server": {"id": id, "folder": "in"}, "subject": "Hello"}]}))
                .unwrap()
        };
        let errors: Vec<String> = (0..100).map(|n| format!("Problem {n}: {}", "x".repeat(2_000))).collect();
        graph(&mut s, GraphMailResult { account: ms, pages: vec![page("AAA1"), page("AAA2")], errors, partial: true, ..Default::default() });
        graph(&mut s, GraphMailResult { account: ms, whole: true, ..Default::default() });
        let r = row(&mut s, ms);
        let problems = r["problems"].as_array().unwrap();
        assert_eq!((r["state"].as_str(), problems.len()), (Some("done"), MAX_PROBLEMS));
        assert!(problems.iter().all(|p| p.as_str().is_some_and(|p| p.chars().count() <= MAX_PROBLEM_CHARS)));
        assert_eq!((r["new"].as_u64(), s.mailbox.messages.values().filter(|m| m.account == ms).count()), (Some(2), 2));
        // Junk params are nothing to it: it reads and opens nothing.
        for junk in [json!(null), json!([1, 2]), json!({"account": "x", "accounts": [[]]}), json!("sync")] {
            assert_eq!(s.run("sync.progress", &junk).unwrap()["accounts"].as_array().map(Vec::len), Some(1));
        }
    }

    #[test]
    fn skipped_accounts_and_full_messages() {
        let (mut s, imap, ms) = session();
        s.sync_progress.begin(&[imap, ms]);
        // Skip is for an account waiting its turn: its row says so, and the run goes on without it.
        assert!(s.sync_progress.skip(ms));
        assert!(!s.sync_progress.skip(ms) && !s.sync_progress.skip(424_242));
        assert_eq!(row(&mut s, ms)["state"], json!("skipped"));
        apply(&mut s, SyncResult { account: imap, partial: true, ..Default::default() });
        assert!(!s.sync_progress.skip(imap), "running");
        apply(&mut s, SyncResult { account: imap, ..Default::default() });
        assert_eq!(progress(&mut s)["running"], json!(false));

        // Full messages: none counted before the mail comes; an account on a mail server has none.
        assert_eq!((row(&mut s, ms)["fullMessages"].clone(), row(&mut s, imap)["fullMessages"].clone()), (Value::Null, Value::Null));
        let inbox = GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() };
        graph(
            &mut s,
            GraphMailResult {
                account: ms,
                listing: Some(FolderListing { folders: vec![inbox], complete: true }),
                partial: true,
                ..Default::default()
            },
        );
        let page = serde_json::from_value::<MessagePage>(json!({"folder": "in", "messages": [
            {"server": {"id": "AAA1", "folder": "in"}, "subject": "One"},
            {"server": {"id": "AAA2", "folder": "in"}, "subject": "Two"},
            {"server": {"id": "AAA3", "folder": "in"}, "subject": "Three"}]}))
        .unwrap();
        graph(&mut s, GraphMailResult { account: ms, pages: vec![page], whole: true, ..Default::default() });
        let unfetched = s.mailbox.messages.values().filter(|m| m.account == ms && m.unfetched).count() as u32;
        assert_eq!(row(&mut s, ms)["fullMessages"], json!({"done": 3 - unfetched, "total": 3, "paused": false}));
        // As they are fetched whole, counted again; Stop's pause is said with them.
        let mime = b"Subject: One\r\n\r\nWords.\r\n".to_vec();
        let fetched = json!({"account": ms, "fetched": [{"id": "AAA1", "mime": mime}]});
        s.run("sync.applyGraphFetch", &json!({"result": fetched})).unwrap();
        s.sync_progress.full_paused = true;
        let full = row(&mut s, ms)["fullMessages"].clone();
        assert_eq!((full["total"].as_u64(), full["paused"].as_bool()), (Some(3), Some(true)));
        assert!(full["done"].as_u64() >= Some(1));
        // The IMAP account is never counted; a host may say its own numbers, never past the total.
        s.sync_progress.count_full(&s.mailbox.clone(), imap, 0);
        s.sync_progress.set_full(ms, 900, 40);
        assert_eq!(s.sync_progress.full(ms), Some(FullMessages { done: 40, total: 40 }));
        assert_eq!(s.sync_progress.full(imap), None);
        s.run("account.remove", &json!({"id": ms})).unwrap();
        s.sync_progress.count_all_full(&s.mailbox.clone(), 0);
        assert_eq!(s.sync_progress.full(ms), None);
    }

    #[test]
    fn a_host_says_what_no_piece_says() {
        let (mut s, imap, ms) = session();
        s.sync_progress.begin(&[ms, imap]);
        // The calendar and contacts after a Graph account's mail: its last piece of mail doesn't
        // end its row, the host does.
        s.sync_progress.hold(ms);
        assert!(s.sync_progress.holding(ms));
        graph(&mut s, GraphMailResult { account: ms, partial: true, ..Default::default() });
        graph(&mut s, GraphMailResult { account: ms, stopped: true, ..Default::default() });
        assert_eq!(row(&mut s, ms)["state"], json!("running"));
        s.sync_progress.phase(ms, SyncPhase::CalendarAndContacts);
        assert_eq!(row(&mut s, ms)["phase"], json!("calendarAndContacts"));
        s.sync_progress.end(ms, false);
        assert_eq!((row(&mut s, ms)["state"].as_str(), s.sync_progress.holding(ms)), (Some("stopped"), false), "as its last piece said");
        // Ending a row that is over already changes nothing.
        let before = s.sync_progress.clone();
        s.sync_progress.end(ms, false);
        s.sync_progress.end(424_242, true);
        assert_eq!(s.sync_progress, before);
        // A waiting account can be left out; one that has run can't.
        assert!(!s.sync_progress.leave_out(ms));
        assert!(s.sync_progress.leave_out(imap));
        let all = progress(&mut s);
        assert_eq!((all["running"].as_bool(), all["accounts"].as_array().map(Vec::len)), (Some(false), Some(1)));
        // A waiting account that is ended (the run was stopped) says so.
        s.sync_progress.begin(&[imap]);
        s.sync_progress.end(imap, true);
        assert_eq!(row(&mut s, imap)["state"], json!("stopped"));
    }

    /// How far a run has got, for the status bar's bar: by account, by phase, by folder of how
    /// many and by message of how many, never going back as the pieces come in; none once over.
    #[test]
    fn a_run_says_how_far_it_has_got() {
        let (mut s, imap, ms) = session();
        assert_eq!(s.sync_progress.fraction(), None);
        s.sync_progress.begin(&[imap, ms]);
        assert_eq!(s.sync_progress.fraction(), Some(0.0));
        assert!(s.sync_progress.current().is_none(), "both wait");
        let mut seen = vec![0.0];
        let mut at = |s: &mut Session| {
            let f = s.sync_progress.fraction().unwrap();
            assert!(seen.last().is_some_and(|last| f >= *last), "{f} after {seen:?}");
            seen.push(f);
        };
        let piece = |step: Option<SyncStep>| SyncResult { account: imap, partial: true, step, ..Default::default() };
        for p in [
            step(SyncStage::Sending, "", 0, 2),
            step(SyncStage::Sending, "", 2, 2),
            step(SyncStage::Connecting, "", 0, 0),
            step(SyncStage::Listed, "", 0, 4),
            step(SyncStage::Checking, "Inbox", 1, 4),
            step(SyncStage::Reading, "Inbox", 50, 100),
            step(SyncStage::Reading, "Inbox", 100, 100),
            step(SyncStage::Checking, "Archive", 2, 4),
            step(SyncStage::Checking, "Sent", 4, 4),
        ] {
            apply(&mut s, piece(p));
            at(&mut s);
        }
        assert_eq!(s.sync_progress.current().map(|r| (r.account, r.folder_number, r.folders)), Some((imap, 4, 4)));
        // Half way through the Inbox, the first of four folders: an eighth of new mail.
        let inbox = AccountRun {
            state: RunState::Running,
            phase: Some(SyncPhase::NewMail),
            done: 50,
            total: 100,
            folder_number: 1,
            folders: 4,
            ..Default::default()
        };
        assert!((inbox.fraction() - (0.15 + 0.75 / 8.0)).abs() < 1e-6);
        apply(&mut s, SyncResult { account: imap, ..Default::default() });
        at(&mut s);
        assert_eq!(s.sync_progress.fraction(), Some(0.5), "one account of two is done");
        let row = s.sync_progress.accounts.iter().find(|r| r.account == imap).unwrap();
        assert_eq!((row.folder_number, row.folders), (0, 0), "nothing kept of where it was");

        // A Microsoft account says which folder of how many in its progress.
        let progress = Some(GraphProgress { folder: "in".into(), read: 10, total: 20, number: 2, folders: 3 });
        graph(&mut s, GraphMailResult { account: ms, progress, partial: true, ..Default::default() });
        at(&mut s);
        let row = s.sync_progress.current().unwrap();
        assert_eq!((row.account, row.folder_number, row.folders), (ms, 2, 3));
        // Odd numbers never go past the end.
        let odd = AccountRun {
            state: RunState::Running,
            phase: Some(SyncPhase::NewMail),
            done: u32::MAX,
            total: 1,
            folder_number: 9,
            folders: 3,
            ..Default::default()
        };
        assert!(odd.fraction() <= 1.0);
        graph(&mut s, GraphMailResult { account: ms, ..Default::default() });
        assert_eq!(s.sync_progress.fraction(), None, "over");
    }

    /// #276: the time and requests the pieces carry add up on the account's row, by phase, and a
    /// new run starts them again; `sync.progress` says them as numbers and nothing else.
    #[test]
    fn the_pieces_time_and_requests_add_up_on_the_row_and_start_again_with_a_run() {
        use mailcraft_model::sync::{SyncPhase, Timings};
        let (mut s, imap, ms) = session();
        let timed = |phase, millis, requests, skipped| {
            let mut t = Timings::default();
            t.add(phase, millis, requests);
            t.skipped = skipped;
            t
        };
        let piece = |timings| SyncResult { account: imap, partial: true, timings, ..Default::default() };
        s.sync_progress.begin(&[imap, ms]);
        apply(&mut s, piece(timed(SyncPhase::Folders, 1_200, 3, 0)));
        apply(&mut s, piece(timed(SyncPhase::NewMail, 800, 5, 2)));
        apply(&mut s, piece(timed(SyncPhase::NewMail, 400, 2, 0)));
        apply(&mut s, SyncResult { account: imap, timings: timed(SyncPhase::NewMail, 100, 1, 0), ..Default::default() });
        let r = row(&mut s, imap);
        assert_eq!(r["state"], json!("done"));
        assert_eq!(
            r["timings"],
            json!({"millis": 2_500, "requests": 11, "skipped": 2, "phases": [
                {"phase": "folders", "millis": 1_200, "requests": 3},
                {"phase": "newMail", "millis": 1_300, "requests": 8},
            ]})
        );
        // Another account has its own.
        assert_eq!(row(&mut s, ms)["timings"]["requests"], json!(0));
        graph(&mut s, GraphMailResult { account: ms, timings: timed(SyncPhase::Folders, 90, 4, 0), ..Default::default() });
        assert_eq!(row(&mut s, ms)["timings"]["requests"], json!(4));
        assert_eq!(row(&mut s, imap)["timings"]["requests"], json!(11), "not the other account's");
        // The next run starts them again.
        apply(&mut s, SyncResult { account: imap, timings: timed(SyncPhase::Folders, 10, 1, 0), ..Default::default() });
        assert_eq!(
            row(&mut s, imap)["timings"],
            json!({"millis": 10, "requests": 1, "skipped": 0, "phases": [{"phase": "folders", "millis": 10, "requests": 1}]})
        );
        // Hostile numbers add up without overflow.
        let huge = piece(timed(SyncPhase::NewMail, u64::MAX, u32::MAX, u32::MAX));
        apply(&mut s, huge.clone());
        apply(&mut s, huge);
        assert_eq!(row(&mut s, imap)["timings"]["requests"], json!(u32::MAX));
    }

    /// #276: an Inbox pass is never the end of an account's part of the run; its time and new mail
    /// stay with the row while the account waits for its run, which adds to them.
    #[test]
    fn an_inbox_pass_does_not_end_the_account_and_its_numbers_stay_for_the_run_after_it() {
        use mailcraft_model::sync::{SyncPhase, Timings};
        let (mut s, imap, ms) = session();
        let timed = |phase, millis, requests| {
            let mut t = Timings::default();
            t.add(phase, millis, requests);
            t
        };
        s.sync_progress.begin(&[imap, ms]);
        // Even a piece that says it is the last.
        graph(&mut s, GraphMailResult { account: ms, inbox_pass: true, timings: timed(SyncPhase::Inbox, 300, 1), ..Default::default() });
        let r = row(&mut s, ms);
        assert_eq!((r["state"].as_str(), r["phase"].as_str()), (Some("running"), Some("inbox")), "{r}");
        s.sync_progress.rest(ms);
        let r = row(&mut s, ms);
        assert_eq!((r["state"].as_str(), r["phase"].as_str(), r["timings"]["requests"].as_u64()), (Some("waiting"), None, Some(1)), "{r}");
        // `rest` leaves a row that isn't in its pass alone.
        s.sync_progress.rest(imap);
        assert_eq!(row(&mut s, imap)["state"], json!("waiting"));
        // The run itself goes on from there and adds to it.
        graph(&mut s, GraphMailResult { account: ms, timings: timed(SyncPhase::NewMail, 100, 2), ..Default::default() });
        let r = row(&mut s, ms);
        assert_eq!(r["state"], json!("done"));
        assert_eq!(
            r["timings"],
            json!({"millis": 400, "requests": 3, "skipped": 0, "phases": [
                {"phase": "inbox", "millis": 300, "requests": 1},
                {"phase": "newMail", "millis": 100, "requests": 2},
            ]})
        );
    }

    /// #276: an account the owner skipped keeps its row skipped, with the pass it still had.
    #[test]
    fn a_skipped_account_keeps_its_pass_on_a_skipped_row() {
        use mailcraft_model::sync::{SyncPhase, Timings};
        let (mut s, imap, ms) = session();
        s.sync_progress.begin(&[imap, ms]);
        assert!(s.sync_progress.skip(ms));
        let mut t = Timings::default();
        t.add(SyncPhase::Inbox, 200, 1);
        s.sync_progress.phase(ms, SyncPhase::Inbox);
        graph(&mut s, GraphMailResult { account: ms, inbox_pass: true, partial: true, timings: t, ..Default::default() });
        let r = row(&mut s, ms);
        assert_eq!(r["state"], json!("skipped"));
        assert_eq!(r["timings"]["requests"], json!(1));
    }

    /// #276: only a Microsoft account whose Inbox has been read to its end has an Inbox pass.
    #[test]
    fn only_an_account_in_step_through_graph_with_a_stored_inbox_link_has_an_inbox_pass() {
        use super::super::graph_mail::graph_inbox_request;
        let (s, imap, ms) = session();
        assert!(graph_inbox_request(&s, imap).is_none(), "a mail server's account has none");
        assert!(graph_inbox_request(&s, ms).is_none(), "no Inbox link yet");
        assert!(graph_inbox_request(&s, 9_999).is_none());
    }
}
