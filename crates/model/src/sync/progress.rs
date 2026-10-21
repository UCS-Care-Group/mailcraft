// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! How far a Send/Receive has got, as numbers: what a host's line for the status bar is made of,
//! and what the engine keeps per account while a run goes on (#120).

use mailcraft_i18n::{Lang, fmt};
use serde::{Deserialize, Serialize};

/// What part of a run an account is in, as the owner is told it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncPhase {
    /// The Inbox's new mail, brought first of all, before anything else of the run (#276).
    Inbox,
    /// Sending what waits in the Outbox.
    Sending,
    /// Telling the server what changed here, listing its folders, keeping a copy of what was sent.
    Folders,
    /// Bringing new mail down.
    NewMail,
    /// The calendar and the contacts.
    CalendarAndContacts,
}

impl SyncPhase {
    /// The phase's name for the owner.
    pub fn label(self) -> &'static str {
        match self {
            SyncPhase::Inbox => "Inbox",
            SyncPhase::Sending => "Sending",
            SyncPhase::Folders => "Folders",
            SyncPhase::NewMail => "New mail",
            SyncPhase::CalendarAndContacts => "Calendar and contacts",
        }
    }
}

/// How long a phase of a run took, and how many requests it made (#276): measured where the
/// requests happen and carried on the pieces of a run, so that a slow mailbox can be told from a
/// slow connection. Nothing in it names an address or a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseTime {
    pub phase: SyncPhase,
    /// Wall-clock milliseconds.
    pub millis: u64,
    /// Requests to the server (HTTP requests for Graph, commands for a mail server).
    pub requests: u32,
}

/// What a piece of a run says of the time it took: the time and requests since the piece before,
/// by phase, and the folders it left out because they hadn't changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Timings {
    pub phases: Vec<PhaseTime>,
    pub skipped: u32,
}

impl Timings {
    pub fn is_empty(&self) -> bool {
        self.phases.is_empty() && self.skipped == 0
    }

    /// Add time and requests to a phase (nothing for none of either).
    pub fn add(&mut self, phase: SyncPhase, millis: u64, requests: u32) {
        if millis == 0 && requests == 0 {
            return;
        }
        match self.phases.iter_mut().find(|p| p.phase == phase) {
            Some(p) => (p.millis, p.requests) = (p.millis.saturating_add(millis), p.requests.saturating_add(requests)),
            None => self.phases.push(PhaseTime { phase, millis, requests }),
        }
    }

    /// [`Timings::add`] for a span of time.
    pub fn add_time(&mut self, phase: SyncPhase, took: std::time::Duration, requests: u32) {
        self.add(phase, u64::try_from(took.as_millis()).unwrap_or(u64::MAX), requests);
    }

    /// Take in another piece's numbers.
    pub fn absorb(&mut self, later: &Timings) {
        for p in later.phases.iter().take(16) {
            self.add(p.phase, p.millis, p.requests);
        }
        self.skipped = self.skipped.saturating_add(later.skipped);
    }

    pub fn millis(&self) -> u64 {
        self.phases.iter().fold(0u64, |sum, p| sum.saturating_add(p.millis))
    }

    pub fn requests(&self) -> u32 {
        self.phases.iter().fold(0u32, |sum, p| sum.saturating_add(p.requests))
    }
}

/// What a mail-server run is doing at a step, finer than its [`SyncPhase`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncStage {
    /// Sending the Outbox: `done` of `total` messages sent so far (none yet: about to start).
    #[default]
    Sending,
    /// Opening the connection to the mail server.
    Connecting,
    /// The server took on what was changed here in `folder`.
    Updating,
    /// The server's folders were listed: `total` of them.
    Listed,
    /// Keeping a copy of what was sent on the server.
    Filing,
    /// Looking at `folder`, the `done`th of `total` folders.
    Checking,
    /// Bringing `folder`'s new mail down: `done` of `total` messages so far.
    Reading,
}

impl SyncStage {
    pub fn phase(self) -> SyncPhase {
        match self {
            SyncStage::Sending => SyncPhase::Sending,
            SyncStage::Connecting | SyncStage::Updating | SyncStage::Listed | SyncStage::Filing => SyncPhase::Folders,
            SyncStage::Checking | SyncStage::Reading => SyncPhase::NewMail,
        }
    }
}

/// How far a run has got, as numbers. `folder` is the mailbox's name as it is shown (not its
/// path on the server); a count that isn't known is 0.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncStep {
    pub stage: SyncStage,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub folder: String,
    pub done: u32,
    pub total: u32,
}

impl SyncStep {
    pub fn new(stage: SyncStage, folder: &str, done: usize, total: usize) -> Self {
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        SyncStep { stage, folder: folder.to_string(), done: count(done), total: count(total) }
    }

    /// The step in one short line, for a status bar (`Inbox: 150 of 500`).
    pub fn line(&self) -> String {
        self.line_in(Lang::English)
    }

    /// [`line`](Self::line) in `lang`.
    pub fn line_in(&self, lang: Lang) -> String {
        let folder = shown(&self.folder);
        let (done, total) = (self.done.to_string(), self.total.to_string());
        let values = [("folder", folder.as_str()), ("done", done.as_str()), ("total", total.as_str()), ("n", total.as_str())];
        let template = match self.stage {
            SyncStage::Sending if self.done == 0 => lang.tn(u64::from(self.total), "Sending {n} message", "Sending {n} messages"),
            SyncStage::Sending => lang.t("Sent {done} of {total}"),
            SyncStage::Connecting => lang.t("Connecting to the mail server"),
            SyncStage::Updating => lang.t("Updated {folder} on the server"),
            SyncStage::Listed => lang.tn(u64::from(self.total), "Found {n} folder", "Found {n} folders"),
            SyncStage::Filing => lang.t("Keeping a copy of what was sent"),
            SyncStage::Checking => lang.t("Checking {folder} ({done} of {total})"),
            SyncStage::Reading => lang.t("{folder}: {done} of {total}"),
        };
        fmt(template, &values)
    }
}

/// A folder's name for a line: one line, not too long, whatever a server or a host said.
fn shown(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(80).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_reads_as_the_line_it_was() {
        for (step, line) in [
            (SyncStep::new(SyncStage::Sending, "", 0, 1), "Sending 1 message"),
            (SyncStep::new(SyncStage::Sending, "", 0, 3), "Sending 3 messages"),
            (SyncStep::new(SyncStage::Sending, "", 2, 3), "Sent 2 of 3"),
            (SyncStep::new(SyncStage::Connecting, "", 0, 0), "Connecting to the mail server"),
            (SyncStep::new(SyncStage::Updating, "Archive", 0, 0), "Updated Archive on the server"),
            (SyncStep::new(SyncStage::Listed, "", 0, 1), "Found 1 folder"),
            (SyncStep::new(SyncStage::Listed, "", 0, 6), "Found 6 folders"),
            (SyncStep::new(SyncStage::Filing, "", 0, 0), "Keeping a copy of what was sent"),
            (SyncStep::new(SyncStage::Checking, "Inbox", 1, 5), "Checking Inbox (1 of 5)"),
            (SyncStep::new(SyncStage::Reading, "Inbox", 150, 500), "Inbox: 150 of 500"),
        ] {
            assert_eq!(step.line(), line);
        }
        let odd = SyncStep { stage: SyncStage::Reading, folder: "In\r\nbox".repeat(50), done: u32::MAX, total: 0 };
        assert!(!odd.line().contains('\n') && odd.line().chars().count() < 120, "{}", odd.line());
        assert_eq!(SyncStep::new(SyncStage::Reading, "", usize::MAX, 0).done, u32::MAX);
        let json = serde_json::to_string(&SyncStep::new(SyncStage::Reading, "Inbox", 2, 4)).unwrap();
        assert_eq!(json, r#"{"stage":"reading","folder":"Inbox","done":2,"total":4}"#);
        assert_eq!(SyncStage::Updating.phase(), SyncPhase::Folders);
        assert_eq!(SyncStage::Checking.phase().label(), "New mail");
        let fr = mailcraft_i18n::test_support::po(&[
            ("Found {n} folder", "{n} dossier trouvé"),
            ("{folder}: {done} of {total}", "{folder} : {done} sur {total}"),
        ]);
        mailcraft_i18n::test_support::with_catalogue(Lang::French, &fr, || {
            assert_eq!(SyncStep::new(SyncStage::Listed, "", 0, 1).line_in(Lang::French), "1 dossier trouvé");
            assert_eq!(SyncStep::new(SyncStage::Reading, "Inbox", 2, 4).line_in(Lang::French), "Inbox : 2 sur 4");
            assert_eq!(SyncStep::new(SyncStage::Listed, "", 0, 2).line(), "Found 2 folders");
        });
    }
}
