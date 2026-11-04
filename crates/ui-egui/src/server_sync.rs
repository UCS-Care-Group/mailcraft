// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One question for everything a Microsoft account keeps in step with Outlook beyond mail and
//! calendar (#661): its to-dos ([`crate::task_sync`]), the rules of its server
//! ([`crate::rule_sync`]), its automatic replies ([`crate::auto_replies`]) and its list of
//! categories ([`crate::category_sync`]).
//!
//! A new sign-in's page asks for what those take (`Tasks.ReadWrite`, `MailboxSettings.ReadWrite`)
//! beside the rest. An account that signed in before is asked once, by the window, when the first
//! of the four finds its sign-in doesn't cover it: one question, then one page of Microsoft's for
//! all of it (`ui.allowSync`, `mailcraft_net::oauth::sign_in_for_sync`; Microsoft shows only what
//! wasn't given yet). A question that goes away unanswered is not put again by itself in this
//! session, for any of the four; File › Info has one line per account, with Turn On.

use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::dialogs::{Dialog, Question};
use crate::{MailApp, Tokens};

/// Where an account's permission stands, for one of the four.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Perm {
    /// Allowed, or nothing is known against it.
    Fine,
    /// Waiting for the owner.
    Waits,
    /// The owner is being asked.
    Asking,
}

impl MailApp {
    /// The answers of the to-dos', rules', calendar's and categories' jobs, taken in at `now`: a
    /// job that came to nothing waits from when its answer is read, even after the window went
    /// a long time without a frame (#661).
    pub fn take_in_sync_answers(&mut self, now: f64) {
        self.calendar.clock = now;
        self.tasks.clock = now;
        self.rules.clock = now;
        self.category_sync.clock = now;
        self.poll_calendar();
        self.poll_tasks();
        self.poll_rules();
        self.poll_categories();
    }

    /// The one question is on screen, or its page is open in the browser.
    pub(crate) fn sync_question_open(&self) -> bool {
        self.signing_in.is_some() || matches!(&self.dialog, Some(Dialog::Confirm { command, .. }) if command == "ui.allowSync")
    }

    /// Put the one question to the owner of `account`, when one of the four found its sign-in
    /// doesn't cover it. Not again by itself once it went away unanswered in this session (the
    /// one that asked takes that as a no). Returns whether it asked.
    pub(crate) fn ask_sync(&mut self, account: Id) -> bool {
        if self.sync_declined.contains(&account) {
            self.sync_not_now(account);
            return false;
        }
        self.put_sync_question(account)
    }

    /// File › Info's Turn On, and the Categories window's Allow: the question, whatever was
    /// answered before.
    pub(crate) fn ask_sync_again(&mut self, account: Id) -> bool {
        if self.dialog.is_some() || self.signing_in.is_some() {
            return false;
        }
        self.sync_declined.remove(&account);
        self.put_sync_question(account)
    }

    fn put_sync_question(&mut self, account: Id) -> bool {
        let Some(address) = self.session.mailbox.account(account).map(|a| a.email.trim().to_string()) else { return false };
        let text = crate::i18n::fmt(
            tl!(
                "To keep the tasks, rules, automatic replies and categories of {address} in step with Outlook, MailCraft needs your permission to change your To Do lists and your mailbox settings. Microsoft's page opens in your browser to ask you, once.\n\nUntil you allow it, tasks and rules stay on this computer and nothing of them is sent to Microsoft, and the categories of this mailbox can't be changed here."
            ),
            &[("address", &address)],
        );
        let ask = Question {
            title: tl!("Keep in Step with Outlook").to_string(),
            text,
            action: tl!("Continue").to_string(),
            with: Value::Null,
            more: Vec::new(),
            note: None,
            cancel: None,
            tick: None,
        };
        self.sync_mark_asking(account);
        self.dialog = Some(Dialog::Confirm { command: "ui.allowSync".to_string(), params: json!({"account": account}), ask });
        true
    }

    /// The four that wait for the owner are being asked.
    fn sync_mark_asking(&mut self, account: Id) {
        self.sync_asking.insert(account);
        self.tasks_sync_asking(account);
        self.rules_sync_asking(account);
        self.auto_replies_sync_asking(account);
        self.categories_sync_asking(account);
    }

    /// The question went away unanswered, or was put before: the four that wait take it as a no.
    pub(crate) fn sync_not_now(&mut self, account: Id) {
        self.sync_declined.insert(account);
        self.tasks_sync_not_now(account);
        self.rules_sync_not_now(account);
        self.auto_replies_sync_not_now(account);
        self.categories_sync_not_now(account);
    }

    /// A question that went away unanswered is a no for this session. Called every frame.
    pub(crate) fn sync_ask_tick(&mut self) {
        if self.sync_question_open() {
            return;
        }
        let asking: Vec<Id> = self.sync_asking.drain().collect();
        for account in asking {
            self.sync_not_now(account);
        }
    }

    /// `ui.allowSync` (and the older ids that ask for one of the four): Microsoft's page, for all
    /// of them, because the owner said go ahead.
    pub(crate) fn begin_sync_sign_in(&mut self, account: Id) -> Result<Value, String> {
        let owner = self.session.mailbox.account(account).filter(|a| a.oauth.is_some() && !a.is_shared()).cloned();
        let owner = owner.ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        let oauth = owner.oauth.clone().filter(|o| o.provider == mailcraft_model::OAuthProvider::Microsoft);
        let oauth = oauth.ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        self.sync_next = true;
        let begun = self.begin_sign_in(owner.id, oauth, &owner.email, String::new(), false, true);
        self.sync_next = false;
        begun?;
        self.sync_declined.remove(&account);
        self.sync_mark_asking(account);
        self.session.sync_status =
            tl!("Allow MailCraft to keep this mailbox in step with Outlook on the page that opened in your browser…").to_string();
        Ok(json!({"pending": "signIn", "account": owner.id}))
    }

    /// The owner answered on Microsoft's page: with the tokens, the four go on at once; without,
    /// why not is said, once.
    pub(crate) fn sync_signed_in(&mut self, account: Id, answer: Result<Tokens, String>) {
        use crate::approval::NO_BROWSER;
        use crate::favorites_sync::{NEEDS_APPROVAL, REFUSED_BY_ORGANISATION, REFUSED_FOR_ACCOUNT};
        self.session.sync_status.clear();
        self.sync_asking.remove(&account);
        let address = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
        let refusal = match answer {
            // (Somebody else's tokens are neither used nor remembered, #103.)
            Ok(tokens) if self.signed_in_as_other(account, &tokens) => {
                self.sync_waits_again(account);
                return;
            }
            Ok(tokens) => {
                if !tokens.refresh.is_empty() {
                    self.remember("refresh", &address, &tokens.refresh);
                    if let Some(kept) = self.tokens.get_mut(&account) {
                        kept.refresh = tokens.refresh.clone();
                    }
                }
                log::info!("sync, account {account}: the owner allowed keeping it in step with Outlook; the next jobs find out");
                self.tasks_allowed(account);
                self.rules_allowed(account);
                self.auto_replies_allowed(account);
                self.categories_allowed(account);
                return;
            }
            Err(e) if e == NEEDS_APPROVAL => {
                let said =
                    tl!("Your organisation's administrator has to approve MailCraft keeping this mailbox in step with Outlook once: File › Info");
                let _ = self.run("ui.approvalHelp", json!({"account": account}));
                Some(said.to_string())
            }
            Err(e) if e == REFUSED_BY_ORGANISATION || e == REFUSED_FOR_ACCOUNT => {
                Some(tl!("Your organisation doesn't let MailCraft keep this mailbox in step with Outlook").to_string())
            }
            Err(e) if e == NO_BROWSER => {
                self.sync_not_now(account);
                self.status(crate::i18n::fmt(tl!("Tasks and rules stay on this computer for now: {error}"), &[("error", &e)]));
                None
            }
            Err(e) => Some(crate::i18n::fmt(tl!("MailCraft wasn't allowed to keep this mailbox in step with Outlook: {error}"), &[("error", &e)])),
        };
        if let Some(said) = refusal {
            // (Not put again by itself, for any of the four: File › Info has Turn On.)
            self.sync_declined.insert(account);
            self.tasks_sync_refused(account, &said);
            self.rules_sync_refused(account, &said);
            self.auto_replies_sync_refused(account, &said);
            self.categories_sync_refused(account, &said);
            self.status(said);
        }
    }

    /// The page signed in somebody else: the four wait as they did.
    fn sync_waits_again(&mut self, account: Id) {
        self.tasks_sync_waits(account);
        self.rules_sync_waits(account);
        self.auto_replies_sync_waits(account);
        self.categories_sync_waits(account);
    }

    /// What File › Info says under a Microsoft account's card: one line for the four, what each
    /// of them stands at (for the tip), and whether Turn On is offered. `None` for an account
    /// none of them is kept for.
    pub(crate) fn sync_line(&self, account: Id) -> Option<(String, String, bool)> {
        let parts: Vec<(String, bool, Perm)> =
            [self.tasks_sync_part(account), self.rules_sync_part(account), self.auto_replies_sync_part(account), self.categories_sync_part(account)]
                .into_iter()
                .flatten()
                .collect();
        if parts.is_empty() {
            return None;
        }
        let details = parts.iter().map(|(line, ..)| line.as_str()).collect::<Vec<_>>().join("\n");
        let again = parts.iter().any(|(_, again, _)| *again);
        let line = if parts.iter().any(|(.., p)| *p == Perm::Asking) {
            tl!("Sync with Outlook: asking for your permission…")
        } else if again || parts.iter().any(|(.., p)| *p == Perm::Waits) {
            tl!("Sync with Outlook: off")
        } else {
            tl!("Sync with Outlook: on")
        };
        Some((line.to_string(), details, again))
    }
}
