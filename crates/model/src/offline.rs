// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! **Mail to keep offline** (#143): how far back each account's mail is kept in full on this
//! computer. Older mail keeps its headers and preview here, stays in the message list, its
//! conversations and search, and is fetched from the server again when it is opened.

use std::collections::{HashMap, HashSet};

use mailcraft_time::{DateTime, Timestamp};
use serde::{Deserialize, Serialize};

use crate::{AccountKind, FolderKind, Id, Mailbox, Message};

/// Outlook's steps, shortest first. Mail received within it is kept in full on this computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum KeepOffline {
    #[serde(rename = "3d")]
    Days3,
    #[serde(rename = "1w")]
    Week1,
    #[serde(rename = "2w")]
    Weeks2,
    #[serde(rename = "1m")]
    Month1,
    #[serde(rename = "3m")]
    Months3,
    #[serde(rename = "6m")]
    Months6,
    #[serde(rename = "12m")]
    Months12,
    #[serde(rename = "2y")]
    Years2,
    #[serde(rename = "5y")]
    Years5,
    #[serde(rename = "10y")]
    Years10,
    /// Everything: what a mailbox file from before there was a window says.
    #[default]
    #[serde(rename = "all")]
    All,
}

impl KeepOffline {
    /// Every step, shortest first (the slider's).
    pub const STEPS: [KeepOffline; 11] = [
        KeepOffline::Days3,
        KeepOffline::Week1,
        KeepOffline::Weeks2,
        KeepOffline::Month1,
        KeepOffline::Months3,
        KeepOffline::Months6,
        KeepOffline::Months12,
        KeepOffline::Years2,
        KeepOffline::Years5,
        KeepOffline::Years10,
        KeepOffline::All,
    ];

    /// What a new account starts with.
    pub const NEW_ACCOUNT: KeepOffline = KeepOffline::Months12;

    pub fn is_default(&self) -> bool {
        *self == KeepOffline::default()
    }

    /// Its code, as `account.edit` takes it and the mailbox file writes it.
    pub fn code(self) -> &'static str {
        match self {
            KeepOffline::Days3 => "3d",
            KeepOffline::Week1 => "1w",
            KeepOffline::Weeks2 => "2w",
            KeepOffline::Month1 => "1m",
            KeepOffline::Months3 => "3m",
            KeepOffline::Months6 => "6m",
            KeepOffline::Months12 => "12m",
            KeepOffline::Years2 => "2y",
            KeepOffline::Years5 => "5y",
            KeepOffline::Years10 => "10y",
            KeepOffline::All => "all",
        }
    }

    /// A code ([`KeepOffline::code`]), whatever its case, or `None`.
    pub fn parse(text: &str) -> Option<KeepOffline> {
        let text = text.trim();
        KeepOffline::STEPS.into_iter().find(|k| k.code().eq_ignore_ascii_case(text))
    }

    /// In words: `3 days`, `12 months`, `All`.
    pub fn label(self) -> &'static str {
        match self {
            KeepOffline::Days3 => "3 days",
            KeepOffline::Week1 => "1 week",
            KeepOffline::Weeks2 => "2 weeks",
            KeepOffline::Month1 => "1 month",
            KeepOffline::Months3 => "3 months",
            KeepOffline::Months6 => "6 months",
            KeepOffline::Months12 => "12 months",
            KeepOffline::Years2 => "2 years",
            KeepOffline::Years5 => "5 years",
            KeepOffline::Years10 => "10 years",
            KeepOffline::All => "All",
        }
    }

    /// The earliest received time kept in full at `now`; `None` for All. Months and years are
    /// calendar ones, in UTC (a month back from 31 March is the last day of February).
    pub fn since(self, now: Timestamp) -> Option<Timestamp> {
        let at = DateTime::from_timestamp(now, 0);
        let days = |n: i64| Some(now.saturating_sub(n.saturating_mul(86_400)));
        let months = |n: i32| Some(DateTime { date: at.date.add_months(-n), ..at }.to_timestamp(0));
        match self {
            KeepOffline::Days3 => days(3),
            KeepOffline::Week1 => days(7),
            KeepOffline::Weeks2 => days(14),
            KeepOffline::Month1 => months(1),
            KeepOffline::Months3 => months(3),
            KeepOffline::Months6 => months(6),
            KeepOffline::Months12 => months(12),
            KeepOffline::Years2 => months(24),
            KeepOffline::Years5 => months(60),
            KeepOffline::Years10 => months(120),
            KeepOffline::All => None,
        }
    }
}

/// Which mail of a mailbox is within its **mail to keep offline** at one time, worked out once
/// for the whole mailbox.
///
/// Always within, whatever the window: mail of an account that isn't a Microsoft account (a
/// local account's; IMAP comes later), mail the server hasn't got as it is here (no server id,
/// gone there, moved here and not there yet, being moved out of its account), a local folder's,
/// the Drafts', the Outbox's, a draft, a held message, and one that is only partly here for good.
/// Nothing the server lacks is ever outside. Age is by received date, or the date it was sent
/// when it has none; a message with neither, or from the future, is within.
#[derive(Clone, Debug, Default)]
pub struct OfflineWindow {
    /// By account: the earliest received time kept in full. An account not here keeps all.
    since: HashMap<Id, Timestamp>,
    /// Folders whose mail is always kept: Drafts, Outbox, and folders the server hasn't got.
    always: HashSet<Id>,
    /// Each folder the server has, by account and then server id (nested, so that a message's
    /// folder is looked up by `&str`, without a copy of it for every message).
    by_server: HashMap<Id, HashMap<String, Id>>,
}

impl OfflineWindow {
    pub fn of(mb: &Mailbox, now: Timestamp) -> OfflineWindow {
        let since = mb
            .accounts
            .iter()
            .filter(|a| a.kind == AccountKind::Microsoft)
            .filter_map(|a| mb.keep_offline_of(a.id).since(now).map(|t| (a.id, t)))
            .collect();
        OfflineWindow::with(mb, since)
    }

    /// As [`OfflineWindow::of`], with `account` (and the shared mailboxes it opens) kept for
    /// `keep` instead of its own: what a window it may be given would keep.
    pub fn proposed(mb: &Mailbox, now: Timestamp, account: Id, keep: KeepOffline) -> OfflineWindow {
        let since = mb
            .accounts
            .iter()
            .filter(|a| a.kind == AccountKind::Microsoft)
            .filter_map(|a| {
                let mine = a.id == account || mb.owner_of(a.id).is_some_and(|o| o.id == account);
                let k = if mine { keep } else { mb.keep_offline_of(a.id) };
                k.since(now).map(|t| (a.id, t))
            })
            .collect();
        OfflineWindow::with(mb, since)
    }

    fn with(mb: &Mailbox, since: HashMap<Id, Timestamp>) -> OfflineWindow {
        let always =
            mb.folders.iter().filter(|f| matches!(f.kind, FolderKind::Drafts | FolderKind::Outbox) || f.server.is_none()).map(|f| f.id).collect();
        let mut by_server: HashMap<Id, HashMap<String, Id>> = HashMap::new();
        for f in &mb.folders {
            if let Some(s) = &f.server {
                by_server.entry(f.account).or_default().insert(s.id.clone(), f.id);
            }
        }
        OfflineWindow { since, always, by_server }
    }

    /// Whether every window is All: nothing of the mailbox is outside.
    pub fn keeps_all(&self) -> bool {
        self.since.is_empty()
    }

    /// Whether `m` is within: kept in full on this computer.
    pub fn within(&self, m: &Message) -> bool {
        let Some(since) = self.since.get(&m.account) else { return true };
        if self.always.contains(&m.folder) || m.draft || !m.held.is_empty() || m.incomplete || m.moving_out.is_some() || m.server_draft.is_some() {
            return true;
        }
        let Some(server) = m.server.as_ref().filter(|s| !s.gone && !s.id.is_empty()) else { return true };
        // Moved here and not there yet: the server's copy is elsewhere, under another id.
        if self.by_server.get(&m.account).and_then(|f| f.get(server.folder.as_str())) != Some(&m.folder) {
            return true;
        }
        let received = if m.received != 0 { m.received } else { m.date };
        received == 0 || received >= *since
    }
}

impl Mailbox {
    /// Whether message `m` is within its account's **mail to keep offline** at `now` (for many
    /// messages, [`OfflineWindow`] works it out once).
    pub fn within_window(&self, m: &Message, now: Timestamp) -> bool {
        OfflineWindow::of(self, now).within(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::ServerMessage;
    use crate::{Account, Folder, ServerFolder};

    const NOW: Timestamp = 1_792_000_000; // 2026-10-15

    fn mailbox(keep: KeepOffline) -> (Mailbox, Id, Id) {
        let mut mb = Mailbox::with_local_account("avery@mail.example", "Avery");
        let account =
            mb.add_account(Account { email: "noor@example.org".into(), kind: AccountKind::Microsoft, keep_offline: keep, ..Default::default() });
        let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
        mb.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "inbox".into(), ..Default::default() });
        (mb, account, inbox)
    }

    fn message(account: Id, folder: Id, received: Timestamp) -> Message {
        let server = ServerMessage { id: format!("M{received}"), folder: "inbox".into(), ..Default::default() };
        Message { account, folder, received, server: Some(server), ..Default::default() }
    }

    #[test]
    fn codes_round_trip_and_order_runs_shortest_first() {
        for k in KeepOffline::STEPS {
            assert_eq!(KeepOffline::parse(k.code()), Some(k));
            assert_eq!(serde_json::to_value(k).unwrap(), serde_json::json!(k.code()));
        }
        assert_eq!(KeepOffline::parse(" 12M "), Some(KeepOffline::Months12));
        assert_eq!(KeepOffline::parse("forever"), None);
        assert!(KeepOffline::Days3 < KeepOffline::Month1 && KeepOffline::Years10 < KeepOffline::All);
        assert_eq!(KeepOffline::default(), KeepOffline::All);
    }

    #[test]
    fn since_counts_calendar_months_back() {
        assert_eq!(KeepOffline::Days3.since(NOW), Some(NOW - 3 * 86_400));
        assert_eq!(KeepOffline::All.since(NOW), None);
        // 31 March 2026 12:00 UTC, a month back: 28 February.
        let march = DateTime { date: mailcraft_time::Date::new(2026, 3, 31).unwrap(), hour: 12, minute: 0, second: 0 }.to_timestamp(0);
        let feb = DateTime { date: mailcraft_time::Date::new(2026, 2, 28).unwrap(), hour: 12, minute: 0, second: 0 }.to_timestamp(0);
        assert_eq!(KeepOffline::Month1.since(march), Some(feb));
        assert!(KeepOffline::Years10.since(NOW).unwrap() < KeepOffline::Years5.since(NOW).unwrap());
    }

    #[test]
    fn mail_within_by_received_date_and_edges() {
        let (mb, account, inbox) = mailbox(KeepOffline::Month1);
        let w = OfflineWindow::of(&mb, NOW);
        assert!(w.within(&message(account, inbox, NOW - 86_400)));
        assert!(!w.within(&message(account, inbox, NOW - 40 * 86_400)));
        // No date at all: within (its age isn't known). From the future: within.
        assert!(w.within(&message(account, inbox, 0)));
        assert!(w.within(&message(account, inbox, NOW + 365 * 86_400)));
        // No received date: the date it was sent.
        let sent_long_ago = Message { date: NOW - 400 * 86_400, ..message(account, inbox, 0) };
        assert!(!w.within(&sent_long_ago));
        assert!(mb.within_window(&message(account, inbox, NOW), NOW));
    }

    #[test]
    fn what_the_server_lacks_is_always_within() {
        let (mut mb, account, inbox) = mailbox(KeepOffline::Days3);
        let local = mb.accounts[0].id;
        let local_inbox = mb.folder_of_kind(local, FolderKind::Inbox).unwrap();
        let drafts = mb.folder_of_kind(account, FolderKind::Drafts).unwrap();
        let outbox = mb.folder_of_kind(account, FolderKind::Outbox).unwrap();
        let own = mb.alloc_id();
        mb.folders.push(Folder { id: own, account, name: "Here only".into(), ..Default::default() });
        let archive = mb.folder_of_kind(account, FolderKind::Archive).unwrap();
        mb.folder_mut(archive).unwrap().server = Some(ServerFolder { id: "archive".into(), ..Default::default() });
        let old = NOW - 3_000 * 86_400;
        let w = OfflineWindow::of(&mb, NOW);
        assert!(!w.within(&message(account, inbox, old)), "the plain case is outside");
        for folder in [drafts, outbox, own] {
            assert!(w.within(&message(account, folder, old)));
        }
        assert!(w.within(&Message { account: local, folder: local_inbox, received: old, ..Default::default() }), "a local account's");
        let base = message(account, inbox, old);
        assert!(w.within(&Message { server: None, ..base.clone() }), "not on the server");
        assert!(w.within(&Message { held: "may have gone".into(), ..base.clone() }));
        assert!(w.within(&Message { draft: true, ..base.clone() }));
        assert!(w.within(&Message { incomplete: true, ..base.clone() }));
        let gone = ServerMessage { gone: true, ..base.server.clone().unwrap() };
        assert!(w.within(&Message { server: Some(gone), ..base.clone() }));
        // Moved into another folder here, not there yet.
        assert!(w.within(&Message { folder: archive, ..base.clone() }));
        let there = ServerMessage { folder: "archive".into(), ..base.server.clone().unwrap() };
        assert!(!w.within(&Message { folder: archive, server: Some(there), ..base }), "once the server has it there");
    }

    #[test]
    fn a_shared_mailbox_follows_its_owner() {
        let (mut mb, owner, _) = mailbox(KeepOffline::Week1);
        mb.accounts.iter_mut().find(|a| a.id == owner).unwrap().oauth = Some(Default::default());
        let shared = mb.add_shared_mailbox(owner, "support@example.org").unwrap();
        assert_eq!(mb.keep_offline_of(shared), KeepOffline::Week1);
        mb.accounts.iter_mut().find(|a| a.id == shared).unwrap().keep_offline = KeepOffline::All;
        assert_eq!(mb.keep_offline_of(shared), KeepOffline::Week1, "its own says nothing");
        let w = OfflineWindow::proposed(&mb, NOW, owner, KeepOffline::Days3);
        let inbox = mb.folder_of_kind(shared, FolderKind::Inbox).unwrap();
        mb.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "inbox".into(), ..Default::default() });
        let w2 = OfflineWindow::proposed(&mb, NOW, owner, KeepOffline::Days3);
        let m = message(shared, inbox, NOW - 5 * 86_400);
        assert!(w.within(&m), "its folder had no server id yet");
        assert!(!w2.within(&m));
        assert!(OfflineWindow::of(&mb, NOW).within(&m), "a week, as its owner has it now");
    }
}
