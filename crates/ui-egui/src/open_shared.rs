// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Open Shared Calendar in the window (#70): another person's calendar, opened by their address.
//!
//! The dialog only collects the address and runs `calendar.openShared`; the account's next
//! calendar job reads the calendar at the person's address with the owner's sign-in and says what
//! came of it ([`MailApp::take_in_opened`]): the calendar appears under Shared Calendars, or the
//! dialog says why not (no rights, no such person, a grant that can take up to an hour). Reading
//! other people's calendars takes a permission of its own (`Calendars.ReadWrite.Shared`), which a
//! sign-in's page asks for with everything else; an account that signed in before is put the one
//! question for an older sign-in when it opens one ([`MailApp::shared_calendars_consent`],
//! [`crate::server_sync`]). A no takes off the calendars that were never read, so that a refusal
//! adds nothing.
//!
//! When the server comes to list the same person's calendar as well (the invitation was
//! accepted in Outlook), the owner is offered, once, to remove the copy opened by address.

use std::collections::HashSet;

use mailcraft_model::Id;
use mailcraft_model::remote::{OpenedWhy, opened_id};
use serde_json::{Value, json};

use crate::MailApp;
use crate::dialogs::{Dialog, Question};

/// Whether other people's calendars can be opened under `account` (the engine's rule).
pub(crate) fn can_open(account: &mailcraft_model::Account) -> bool {
    mailcraft_engine::cmd::calendar_opened::eligible(account).is_ok()
}

/// What the window keeps of opening calendars by address.
#[derive(Default)]
pub(crate) struct OpenShared {
    /// The address that was just asked for and is being read: the account and the address.
    pub(crate) waiting: Option<(Id, String)>,
    /// How it came out, for the dialog to say: the calendar's name, or why not.
    pub(crate) outcome: Option<Result<String, String>>,
    /// The people whose copy opened by address was offered for removal already.
    pub(crate) offered: HashSet<String>,
    /// The accounts whose owner said no to the permission since the window opened: a job that
    /// starts by itself doesn't ask again.
    pub(crate) declined: HashSet<Id>,
}

/// Why a calendar couldn't be opened, in words for the owner.
pub(crate) fn why_words(why: OpenedWhy, address: &str) -> String {
    let words = match why {
        OpenedWhy::NoRights => {
            tl!("You don't have access to the calendar of {address}. They need to share it with you; if they just did, try again in an hour.")
        }
        OpenedWhy::NoSuchPerson => tl!("There's no mailbox at {address}, or its calendar can't be reached through Microsoft."),
        OpenedWhy::NoPermission => tl!("MailCraft isn't allowed to read other people's calendars yet."),
        OpenedWhy::Unreachable => tl!("Couldn't reach Microsoft to open the calendar of {address}. Try again in a moment."),
    };
    crate::i18n::fmt(words, &[("address", address)])
}

impl MailApp {
    /// The owner asked for the calendar of `address` under `account` (the dialog's Open): the
    /// address is put on the list and read by the next calendar job, which starts at once.
    pub(crate) fn begin_open_shared(&mut self, account: Id, address: &str) -> Result<(), String> {
        if self.session.driver.calendar.consent_needed.contains(&account) {
            return Err(tl!("The calendar needs your permission first: File › Info › Connect calendar and contacts").to_string());
        }
        let opened = self.execute("calendar.openShared", json!({"account": account, "address": address}))?;
        let address = opened.get("address").and_then(Value::as_str).unwrap_or(address).to_string();
        self.open_shared.declined.remove(&account);
        self.open_shared.outcome = None;
        self.open_shared.waiting = Some((account, address));
        Ok(())
    }

    /// Have the account's calendar job read what was opened by address without waiting for the
    /// next quarter of an hour (the wide stretch is what lists the calendars).
    pub(crate) fn read_opened_now(&mut self, account: Id) {
        self.session.driver.calendar.checked.remove(&account);
        self.session.driver.calendar.book.forget_checked(account);
        self.session.driver.calendar.book.forget_failures(account);
        self.session.driver.calendar.due = true;
    }

    /// What taking in the account's list of calendars said of those opened by address
    /// (`sync.applyCalendars`' answer): refusals, with why, and a person who is listed twice.
    pub(crate) fn take_in_opened(&mut self, account: Id, applied: &Value) {
        let waiting = self.open_shared.waiting.clone().filter(|(owner, _)| *owner == account);
        let refused = applied.get("refused").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        for one in refused.iter().take(20) {
            let address = one.get("address").and_then(Value::as_str).unwrap_or_default();
            let why = serde_json::from_value::<OpenedWhy>(one.get("why").cloned().unwrap_or(Value::Null)).unwrap_or_default();
            let said = why_words(why, address);
            if waiting.as_ref().is_some_and(|(_, wanted)| wanted.eq_ignore_ascii_case(address)) {
                self.open_shared.outcome = Some(Err(said.clone()));
                self.open_shared.waiting = None;
            }
            self.status(said);
        }
        // The one that was asked for came in: its name is said.
        if let Some((_, wanted)) = waiting.filter(|_| self.open_shared.waiting.is_some()) {
            let id = opened_id(&wanted);
            let name = self
                .session
                .mailbox
                .calendars
                .iter()
                .find(|c| c.origin.account == account && c.origin.id.eq_ignore_ascii_case(&id))
                .map(|c| c.name.clone());
            if let Some(name) = name {
                self.status(crate::i18n::fmt(
                    tl!("Opened the calendar of {name}. It is listed under Shared Calendars, on this computer only."),
                    &[("name", &name)],
                ));
                self.open_shared.outcome = Some(Ok(name));
                self.open_shared.waiting = None;
            }
        }
        let duplicates = applied.get("duplicates").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        for one in duplicates.iter().take(20) {
            let address = one.get("address").and_then(Value::as_str).unwrap_or_default().to_string();
            let name = one.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            if !address.is_empty() && self.dialog.is_none() && self.open_shared.offered.insert(address.to_ascii_lowercase()) {
                self.offer_to_remove_copy(&address, &name);
            }
        }
        // A shared mailbox open here whose calendar the server lists as well (an invitation
        // accepted in Outlook): offered once to hide that copy. The answer is kept in the
        // mailbox file; a question closed without one is asked again the next time the window
        // opens, not before.
        let twice = applied.get("listedTwice").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        for one in twice.iter().take(20) {
            let address = one.get("address").and_then(Value::as_str).unwrap_or_default().to_string();
            let name = one.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            if !address.is_empty() && self.dialog.is_none() && self.open_shared.offered.insert(format!("twice:{}", address.to_ascii_lowercase())) {
                self.offer_to_hide_copy(&address, &name);
            }
        }
    }

    /// The server lists the calendar of the shared mailbox at `address` among the account's own
    /// calendars too, as `name`: offer, once, to hide that copy, the shared mailbox's own calendar
    /// being the fuller one.
    fn offer_to_hide_copy(&mut self, address: &str, name: &str) {
        let text = crate::i18n::fmt(
            tl!(
                "The calendar of the shared mailbox {address} is shown twice: under My Calendars with the shared mailbox, and under Shared Calendars as \"{name}\", because it was also shared with you and accepted.\n\nHide the copy under Shared Calendars? The shared mailbox's own calendar stays, and lets you change events, answer meeting requests and send from the mailbox where you may. Nothing on the server changes: right-click the shared mailbox's calendar to show the copy again."
            ),
            &[("address", address), ("name", name)],
        );
        let ask = Question::new(tl!("Calendar Listed Twice"), text, tl!("Hide the Copy"), json!({"hide": true}))
            .or(tl!("Keep Both"), json!({"hide": false}));
        self.dialog = Some(Dialog::Confirm { command: "calendar.hideListedCopy".to_string(), params: json!({"address": address}), ask });
    }

    /// The server lists the calendar of the person at `address` too: offer, once, to remove the
    /// copy that was opened by address.
    fn offer_to_remove_copy(&mut self, address: &str, name: &str) {
        let text = crate::i18n::fmt(
            tl!(
                "The calendar of {name} is in your calendar list now, because it was shared with you and accepted. You also opened it by address, so it is there twice.\n\nRemove the copy opened by address? The other one stays, and nothing on the server changes."
            ),
            &[("name", name)],
        );
        let ask = Question::new(tl!("Calendar Listed Twice"), text, tl!("Remove"), Value::Null);
        self.dialog = Some(Dialog::Confirm { command: "calendar.removeShared".to_string(), params: json!({"address": address}), ask });
    }

    /// A calendar job found that the sign-in doesn't cover reading other people's calendars: when
    /// the owner just asked for one, put the one question for an older sign-in (its page asks for
    /// this with everything else, [`crate::server_sync`]); otherwise say so in the status bar,
    /// unless they said no to it already.
    pub(crate) fn shared_calendars_consent(&mut self, account: Id) {
        if self.open_shared.declined.contains(&account) {
            return;
        }
        let asked_for = self.open_shared.waiting.as_ref().is_some_and(|(owner, _)| *owner == account);
        // (The Open Shared Calendar dialog that made the request stays up while the job runs: the
        // question takes its place, and a no still answers the request, in the status bar.)
        let own_dialog = matches!(self.dialog, Some(Dialog::OpenCalendar { account: owner, .. }) if owner == account);
        let kept = if asked_for && own_dialog { self.dialog.take() } else { None };
        if asked_for && self.signing_in.is_none() && self.dialog.is_none() && self.ask_sync_again(account) {
            return;
        }
        if kept.is_some() {
            self.dialog = kept;
        }
        let said = tl!("Calendars you opened by address need your permission to be read: open one again to be asked").to_string();
        // (A request the owner just made is answered, not left waiting: another dialog is up.)
        if asked_for {
            self.opened_not_allowed(account, said.clone());
        }
        self.status(said);
        self.open_shared.declined.insert(account);
    }

    /// Whether calendars opened by address under `account` wait for the owner's permission.
    pub(crate) fn opened_waits(&self, account: Id) -> bool {
        let has = self.session.mailbox.account(account).is_some_and(|a| !a.opened_calendars.is_empty());
        has && (self.open_shared.declined.contains(&account) || self.opened_waiting_for(account))
    }

    /// Whether a calendar was just opened by address under `account` and waits to be read.
    pub(crate) fn opened_waiting_for(&self, account: Id) -> bool {
        self.open_shared.waiting.as_ref().is_some_and(|(owner, _)| *owner == account)
    }

    /// The owner allowed reading other people's calendars: those opened by address are read now.
    pub(crate) fn opened_allowed(&mut self, account: Id) {
        self.open_shared.declined.remove(&account);
        if self.session.mailbox.account(account).is_some_and(|a| !a.opened_calendars.is_empty()) {
            self.read_opened_now(account);
        }
    }

    /// The owner didn't allow it (`said` is why): what was never read comes off the list again, so
    /// that nothing is added by a no, and the dialog says why.
    pub(crate) fn opened_not_allowed(&mut self, account: Id, said: String) {
        self.open_shared.declined.insert(account);
        self.drop_unread_opened(account);
        if self.opened_waiting_for(account) {
            self.open_shared.outcome = Some(Err(said));
            self.open_shared.waiting = None;
        }
    }

    /// Take off the list the calendars of `account` that were opened by address and never read.
    fn drop_unread_opened(&mut self, account: Id) {
        let Some(owner) = self.session.mailbox.account(account) else { return };
        let unread: Vec<String> = owner
            .opened_calendars
            .iter()
            .filter(|opened| {
                let id = opened_id(&opened.address);
                !self.session.mailbox.calendars.iter().any(|c| c.origin.account == account && c.origin.id.eq_ignore_ascii_case(&id))
            })
            .map(|opened| opened.address.clone())
            .collect();
        for address in unread {
            let _ = self.execute("calendar.removeShared", json!({"account": account, "address": address}));
        }
    }
}
