// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A shared mailbox's calendars in the window (#70): kept in step through Microsoft Graph with
//! the sign-in of the account the mailbox is opened through, at the mailbox's own address, and
//! shown, read and (with edit rights) changed here.
//!
//! A shared mailbox's calendar is there to be ticked from the moment it is added (the engine
//! keeps one for it, [`mailcraft_engine::cmd::remote::ensure_shared_calendars`]). Nothing of it is
//! asked of any server until the owner ticks one: then a calendar job reads its list and, of the
//! calendars that are ticked, their events (the window, polling and caps of the owner's own).
//! The first job finds out whether the owner's sign-in covers shared calendars
//! (`Calendars.ReadWrite.Shared`); when it doesn't, the window asks the owner, with a question
//! first and then Microsoft's page, by itself and never with a sign-in. Refused, or when an
//! administrator has to approve first, the window says what to do next, as for shared
//! mailboxes' mail. File › Info shows where it stands, with a way to ask again.
//!
//! Its access token is its own (it covers what the mailbox's mail token needn't), kept in memory
//! and never mixed with the owner's or the mail's; the refresh token that comes back is the
//! owner's, kept as his. Nothing of a shared mailbox's calendar is a reminder, an alert or part of
//! what the owner's own calendars show beside it. Where the owner may edit one it is changed as
//! his own is, at the mailbox's address; what makes the server send mail from the mailbox waits for
//! the permission to send from shared mailboxes, asked as for its mail ([`MailApp::ask_shared_send_for`]).

use mailcraft_model::{Account, Id};
use serde_json::{Value, json};

use crate::dialogs::Dialog;
use crate::{MailApp, Tokens};

/// Where a shared mailbox's calendars stand with the owner's permission for reading them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Permission {
    /// A run read them with it.
    Given,
    /// A run found the owner's sign-in doesn't cover it: the owner is to be asked.
    Needed,
    /// The owner is being asked: the question, or Microsoft's page.
    Asking,
    /// Asked, and no: why, in a few words.
    Refused(String),
}

impl MailApp {
    /// Whether any calendar of the shared mailbox `shared` is ticked: the owner wants it read.
    pub(crate) fn shared_calendar_wanted(&self, shared: Id) -> bool {
        self.session.mailbox.calendars.iter().any(|c| c.origin.account == shared && c.visible)
    }

    /// The shared mailboxes whose calendars are kept in step now: one of them is ticked, and the
    /// account it is opened through signs in with Microsoft.
    pub(crate) fn shared_calendar_accounts(&self) -> Vec<Id> {
        let mailbox = &self.session.mailbox;
        let signs_in = |shared: &&Account| mailbox.owner_of(shared.id).is_some_and(|owner| owner.oauth.is_some());
        mailbox.accounts.iter().filter(|a| a.is_shared()).filter(signs_in).filter(|a| self.shared_calendar_wanted(a.id)).map(|a| a.id).collect()
    }

    /// Whether `shared`'s calendars wait for the owner's permission: not asked of the server
    /// until it is had (or asked for again).
    pub(crate) fn shared_calendars_blocked(&self, shared: Id) -> bool {
        matches!(self.shared_calendar_perm.get(&shared), Some(Permission::Needed | Permission::Asking | Permission::Refused(_)))
    }

    /// Ask the owner, once, for what a calendar ticked in a shared mailbox found missing: a
    /// question first, with the calendar in front (the owner ticked it), then Microsoft's page.
    /// Called every look at the calendar.
    pub(crate) fn shared_calendars_ask_tick(&mut self) {
        if self.dialog.is_some() || self.session.view.module != mailcraft_engine::Module::Calendar || self.ui.backstage {
            return;
        }
        let Some(shared) = self
            .shared_calendar_perm
            .iter()
            .filter(|(_, perm)| **perm == Permission::Needed)
            .map(|(shared, _)| *shared)
            .find(|shared| self.session.mailbox.account(*shared).is_some_and(Account::is_shared))
        else {
            return;
        };
        self.ask_shared_calendars(shared);
    }

    /// The question before Microsoft's page: what is asked of the owner and why.
    pub(crate) fn ask_shared_calendars(&mut self, shared: Id) -> bool {
        let Some(address) = self.session.mailbox.account(shared).filter(|a| a.is_shared()).map(|a| a.email.trim().to_string()) else { return false };
        let text = crate::i18n::fmt(
            tl!(
                "To show the calendars of {address}, MailCraft needs your permission to read the calendars of shared mailboxes. Microsoft's page opens in your browser to ask you, once.\n\nWhat you may see of a calendar is for its owner and your organisation to decide: this changes nothing about that. Until you allow it, nothing of {address}'s calendars is read."
            ),
            &[("address", &address)],
        );
        let ask = crate::dialogs::Question {
            title: tl!("Show a Shared Mailbox's Calendars").to_string(),
            text,
            action: tl!("Continue").to_string(),
            with: Value::Null,
            more: Vec::new(),
            note: None,
        };
        self.shared_calendar_perm.insert(shared, Permission::Asking);
        self.dialog = Some(Dialog::Confirm { command: "ui.allowSharedCalendars".to_string(), params: json!({"account": shared}), ask });
        true
    }

    /// File › Info's "Ask again": the owner is asked on Microsoft's page at once.
    pub(crate) fn ask_shared_calendars_again(&mut self, shared: Id) {
        if self.session.mailbox.account(shared).is_some_and(Account::is_shared) && self.dialog.is_none() {
            self.ask_shared_calendars(shared);
        }
    }

    /// `ui.allowSharedCalendars`: Microsoft's page, for reading the calendars of shared mailboxes
    /// too, because the owner ticked one of `shared`'s and said go ahead.
    pub(crate) fn begin_shared_calendars_sign_in(&mut self, shared: Id) -> Result<Value, String> {
        if !self.session.mailbox.account(shared).is_some_and(Account::is_shared) {
            return Err(tl!("That isn't a shared mailbox").to_string());
        }
        let owner = self.session.mailbox.owner_of(shared).cloned().ok_or_else(|| tl!("This shared mailbox's account is gone").to_string())?;
        let oauth = owner.oauth.clone().ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        (self.shared_next, self.shared_calendars_next) = (Some(shared), true);
        let begun = self.begin_sign_in(owner.id, oauth, &owner.email, String::new(), false, true);
        (self.shared_next, self.shared_calendars_next) = (None, false);
        if let Err(e) = begun {
            // (The page never came up: nothing was refused, and Ask again is there.)
            self.shared_calendar_perm.insert(shared, Permission::Needed);
            return Err(e);
        }
        self.shared_calendar_perm.insert(shared, Permission::Asking);
        self.session.sync_status = tl!("Allow MailCraft to read shared mailboxes' calendars on the page that opened in your browser…").to_string();
        Ok(json!({"pending": "signIn", "account": owner.id}))
    }

    /// The owner answered on Microsoft's page about shared mailboxes' calendars: with the tokens,
    /// the calendar is read at once; without, why not is said (an administrator's approval has
    /// its own page).
    pub(crate) fn shared_calendars_signed_in(&mut self, shared: Id, owner: Id, answer: Result<Tokens, String>) {
        use crate::approval::NO_BROWSER;
        use crate::favorites_sync::{NEEDS_APPROVAL, REFUSED_BY_ORGANISATION, REFUSED_FOR_ACCOUNT};
        self.session.sync_status.clear();
        let address = self.session.mailbox.account(owner).map(|a| a.email.clone()).unwrap_or_default();
        match answer {
            // (Somebody else's tokens are neither used nor remembered, #103.)
            Ok(tokens) if self.signed_in_as_other(owner, &tokens) => {
                self.shared_calendar_perm.insert(shared, Permission::Needed);
            }
            Ok(tokens) => {
                self.keep_shared_calendar_tokens(shared, owner, &address, tokens);
                // Allowed, as far as is known: the next look at the calendar finds out.
                self.shared_calendar_perm.remove(&shared);
                self.calendar.failures.remove(&shared);
                self.calendar.checked.remove(&shared);
                self.graph_checked.remove(&shared);
                self.calendar.due = true;
            }
            Err(e) if e == NEEDS_APPROVAL => {
                let said = tl!("Your organisation's administrator has to approve reading shared mailboxes' calendars once: File › Info");
                self.shared_calendar_perm.insert(shared, Permission::Refused(said.to_string()));
                self.status(said);
                // What to do next (#124): an administrator approves once, for everybody.
                let _ = self.run("ui.approvalHelp", json!({"account": owner}));
            }
            Err(e) if e == REFUSED_BY_ORGANISATION || e == REFUSED_FOR_ACCOUNT => {
                let said = tl!("Your organisation doesn't allow MailCraft to read shared mailboxes' calendars");
                self.shared_calendar_perm.insert(shared, Permission::Refused(said.to_string()));
                self.status(said);
            }
            // The page never came up: nothing was refused, and File › Info asks again.
            Err(e) if e == NO_BROWSER => {
                self.shared_calendar_perm.insert(shared, Permission::Needed);
                self.status(crate::i18n::fmt(tl!("The calendars of the shared mailbox wait: {error}"), &[("error", &e)]));
            }
            Err(e) => {
                let said = crate::i18n::fmt(tl!("Shared mailboxes' calendars weren't allowed: {error}"), &[("error", &e)]);
                self.shared_calendar_perm.insert(shared, Permission::Refused(said.clone()));
                self.status(said);
            }
        }
    }

    /// What a calendar job of the shared mailbox `shared` said of the owner's permission: it covers
    /// shared calendars (the job got a token for them), or it doesn't and the owner is to be asked,
    /// or the owner's own sign-in is no longer taken.
    pub(crate) fn shared_calendars_answered(&mut self, shared: Id, result: &crate::GraphResult, errors: &mut Vec<String>) {
        if result.consent_needed {
            // (Asked about when the calendar is in front: [`MailApp::shared_calendars_ask_tick`].)
            if !matches!(self.shared_calendar_perm.get(&shared), Some(Permission::Asking | Permission::Refused(_))) {
                self.shared_calendar_perm.insert(shared, Permission::Needed);
            }
        } else if result.sign_in_refused {
            errors.push(
                tl!("The sign-in of the account this shared mailbox is opened through was refused: sign in again with Send/Receive").to_string(),
            );
        } else if !result.token_refused {
            self.shared_calendar_perm.insert(shared, Permission::Given);
        }
        // What makes the server send mail from the shared mailbox (an answer to a meeting, one
        // called off or forwarded, one with people invited) found that sending from shared
        // mailboxes isn't allowed yet (#70): it waits, nothing goes out as the owner, and the
        // owner is asked as he is for mail (the same permission, the same page).
        if result.send_consent_needed && self.session.mailbox.account(shared).is_some() {
            self.calendar.send_waits.insert(shared);
            if self.dialog.is_some() || !self.ask_shared_send_for(shared, true) {
                errors.push(
                    tl!("A meeting answer or invitation from this shared mailbox waits: sending from shared mailboxes needs your permission. Use Send/Receive.")
                        .to_string(),
                );
            }
        }
    }

    /// Keep the tokens a calendar job or the page brought back: the access token is the shared
    /// mailbox's calendars' own, the refresh token the owner's newest, for mail and Graph alike.
    pub(crate) fn keep_shared_calendar_tokens(&mut self, shared: Id, owner: Id, address: &str, tokens: Tokens) {
        if !tokens.refresh.is_empty() {
            self.remember("refresh", address, &tokens.refresh);
            if let Some(kept) = self.tokens.get_mut(&owner) {
                kept.refresh = tokens.refresh.clone();
            }
        }
        let now = mailcraft_time::now();
        self.shared_calendar_tokens.insert(shared, Tokens { expires_at: tokens.expires_at.clamp(now, now.saturating_add(86_400)), ..tokens });
    }

    /// The access token to hand a job for `shared`'s calendars: the one in memory when it is
    /// still good; else empty, and the host asks for one with the owner's refresh token.
    pub(crate) fn shared_calendar_access(&self, shared: Id) -> String {
        self.shared_calendar_tokens.get(&shared).filter(|t| t.fresh(mailcraft_time::now())).map(|t| t.access.clone()).unwrap_or_default()
    }

    /// A shared mailbox's calendar token as the page leaves it, for a host's tests (which sign
    /// nobody in).
    #[doc(hidden)]
    pub fn seed_shared_calendar_token(&mut self, shared: Id, tokens: Tokens) {
        self.shared_calendar_tokens.insert(shared, tokens);
    }

    /// Make `account`'s list of calendars and their rights due at the next look, as a quarter of an
    /// hour going by does, for a host's tests (which have a stopped clock).
    #[doc(hidden)]
    pub fn calendar_list_due(&mut self, account: Id) {
        self.graph_checked.remove(&account);
        self.calendar.checked.remove(&account);
    }

    /// What File › Info says under a shared mailbox's card about its calendars' permission, and
    /// whether there is a way to ask again.
    pub(crate) fn shared_calendar_line(&self, shared: Id) -> Option<(String, bool)> {
        self.session.mailbox.account(shared).filter(|a| a.is_shared())?;
        match self.shared_calendar_perm.get(&shared) {
            Some(Permission::Given) => Some((tl!("Calendars: allowed").to_string(), false)),
            Some(Permission::Needed) => Some((tl!("Calendars: waiting for your permission").to_string(), true)),
            Some(Permission::Asking) => Some((tl!("Calendars: asking for your permission…").to_string(), false)),
            Some(Permission::Refused(why)) => {
                Some((crate::i18n::fmt(tl!("Calendars: not allowed ({why})"), &[("why", crate::forms::kit::shown(why, 90).as_str())]), true))
            }
            None => Some((tl!("Calendars: not asked yet (ticking one of them asks)").to_string(), false)),
        }
    }
}
