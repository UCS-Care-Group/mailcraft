// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Open Shared Calendar in the window (#70): another person's calendar, opened by their address.
//!
//! The dialog only collects the address and runs `calendar.openShared`; the account's next
//! calendar job reads the calendar at the person's address with the owner's sign-in and says what
//! came of it ([`MailApp::take_in_opened`]): the calendar appears under Shared Calendars, or the
//! dialog says why not (no rights, no such person, a grant that can take up to an hour). Reading
//! other people's calendars takes a permission of its own (`Calendars.ReadWrite.Shared`), asked
//! for by itself and never with a sign-in: the window asks the owner first, then Microsoft's page
//! does ([`MailApp::shared_calendars_consent`]). A no takes off the calendars that were never
//! read, so that a refusal adds nothing.
//!
//! When the server comes to list the same person's calendar as well (the invitation was
//! accepted in Outlook), the owner is offered, once, to remove the copy opened by address.

use std::collections::HashSet;

use mailcraft_model::Id;
use mailcraft_model::remote::{OpenedWhy, opened_id};
use serde_json::{Value, json};

use crate::dialogs::{Dialog, Question};
use crate::{MailApp, Tokens};

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
    offered: HashSet<String>,
    /// The accounts whose owner said no to the permission since the window opened: a job that
    /// starts by itself doesn't ask again.
    declined: HashSet<Id>,
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
        if self.graph_consent_needed.contains(&account) {
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
        self.graph_checked.remove(&account);
        self.calendar.checked.remove(&account);
        self.calendar.failures.remove(&account);
        self.calendar.due = true;
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
    /// the owner just asked for one, ask whether Microsoft's page may ask them (once); otherwise
    /// say so in the status bar, unless they said no to it already.
    pub(crate) fn shared_calendars_consent(&mut self, account: Id) {
        if self.open_shared.declined.contains(&account) {
            return;
        }
        if self.open_shared.waiting.as_ref().is_some_and(|(owner, _)| *owner == account) && self.signing_in.is_none() {
            let text = tl!(
                "To open other people's calendars, MailCraft needs your permission to read the calendars people share with you. Microsoft's page opens in your browser to ask you, once.\n\nWhat you open is kept on this computer only: it doesn't appear in Outlook or on your other devices."
            );
            let ask = Question::new(tl!("Open Shared Calendar"), text.to_string(), tl!("Continue"), Value::Null);
            self.dialog = Some(Dialog::Confirm { command: "ui.allowSharedCalendars".to_string(), params: json!({"account": account}), ask });
        } else {
            self.status(tl!("Calendars you opened by address need your permission to be read: open one again to be asked").to_string());
            self.open_shared.declined.insert(account);
        }
    }

    /// `ui.allowSharedCalendars` for an account that opens calendars by address: Microsoft's page,
    /// for reading other people's calendars too (the same permission as a shared mailbox's
    /// calendars take, asked for without what shared mailboxes' mail takes).
    pub(crate) fn begin_opened_calendars_sign_in(&mut self, account: Id) -> Result<Value, String> {
        let owner = self.session.mailbox.account(account).filter(|a| a.oauth.is_some()).cloned();
        let owner = owner.ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        let oauth = owner.oauth.clone().ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        self.shared_calendars_next = true;
        let begun = self.begin_sign_in(owner.id, oauth, &owner.email, String::new(), false, true);
        self.shared_calendars_next = false;
        begun?;
        self.session.sync_status = tl!("Allow MailCraft to read other people's calendars on the page that opened in your browser…").to_string();
        Ok(json!({"pending": "signIn", "account": owner.id}))
    }

    /// The owner answered on Microsoft's page about other people's calendars: with the tokens,
    /// the calendars are read now; without, those that were never read come off the list, and
    /// why not is said.
    pub(crate) fn opened_calendars_signed_in(&mut self, account: Id, answer: Result<Tokens, String>) {
        use crate::approval::NO_BROWSER;
        use crate::favorites_sync::{NEEDS_APPROVAL, REFUSED_BY_ORGANISATION, REFUSED_FOR_ACCOUNT};
        self.session.sync_status.clear();
        let address = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
        let said = match answer {
            // (Somebody else's tokens are neither used nor remembered, #103.)
            Ok(tokens) if self.signed_in_as_other(account, &tokens) => return,
            Ok(tokens) => {
                if !tokens.refresh.is_empty() {
                    self.remember("refresh", &address, &tokens.refresh);
                    if let Some(kept) = self.tokens.get_mut(&account) {
                        kept.refresh = tokens.refresh.clone();
                    }
                }
                self.graph_tokens.insert(account, tokens);
                self.open_shared.declined.remove(&account);
                self.read_opened_now(account);
                return;
            }
            Err(e) if e == NEEDS_APPROVAL => {
                tl!("Your organisation's administrator has to approve reading other people's calendars once: File › Info").to_string()
            }
            Err(e) if e == REFUSED_BY_ORGANISATION || e == REFUSED_FOR_ACCOUNT || e == NO_BROWSER => e,
            Err(e) => crate::i18n::fmt(tl!("Other people's calendars weren't allowed: {error}"), &[("error", &e)]),
        };
        // Nothing was refused by the server about a calendar, but the owner said no: what was
        // never read comes off the list again, so that nothing is added by a no.
        self.open_shared.declined.insert(account);
        self.drop_unread_opened(account);
        self.open_shared.outcome = Some(Err(said.clone()));
        self.open_shared.waiting = None;
        self.status(said.clone());
        if said.contains("administrator") {
            let _ = self.run("ui.approvalHelp", json!({"account": account}));
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
