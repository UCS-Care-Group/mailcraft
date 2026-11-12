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
//! session, for any of the four; File › Info has one line per account, with Ask again.

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

/// How much of a refusal's reason File › Info and the Categories window show: the reasons the one
/// question gives are whole in every language (the longest is about 160 characters), and a
/// server's own words are cut after this many.
pub(crate) const REASON_SHOWN: usize = 200;

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

    /// File › Info's Ask again, and the Categories window's Allow: the question, whatever was
    /// answered before.
    pub(crate) fn ask_sync_again(&mut self, account: Id) -> bool {
        if self.dialog.is_some() || self.signing_in.is_some() {
            return false;
        }
        self.sync_declined.remove(&account);
        self.put_sync_question(account)
    }

    /// Whether keeping `account` in step with Outlook covers its rules and its categories too: an
    /// account kept in step through Microsoft Graph. A Microsoft account still on IMAP keeps only its
    /// to-dos and its automatic replies there, and the question and the texts say only those.
    pub(crate) fn sync_covers_lists(&self, account: Id) -> bool {
        self.session.mailbox.keeps_rules(account)
    }

    /// Whether `account`'s to-dos are kept in step with Microsoft To Do already (their permission
    /// was given by itself, as an older version asked it): the question and its texts then say
    /// nothing of them.
    fn sync_tasks_kept(&self, account: Id) -> bool {
        self.tasks.perm.get(&account) == Some(&crate::task_sync::Permission::Given)
    }

    /// Whether `account`'s sign-in lets MailCraft change its mailbox settings already (its rules,
    /// automatic replies or categories were changed with it, as an older version asked it by
    /// itself) while its to-dos wait: the question and its texts then say only the to-dos.
    fn sync_only_tasks(&self, account: Id) -> bool {
        let settings = matches!(self.rules.perm.get(&account), Some(crate::rule_sync::Permission::Given))
            || matches!(self.auto_replies.perm.get(&account), Some(crate::auto_replies::Permission::Given))
            || matches!(self.category_sync.perm.get(&account), Some(crate::task_sync::Permission::Given));
        settings && !self.sync_tasks_kept(account)
    }

    /// File › Info's tip for Ask again, saying only what `account` keeps in step.
    pub(crate) fn sync_tip(&self, account: Id) -> &'static str {
        if self.sync_only_tasks(account) {
            return tl!(
                "Ask for permission to read and change your To Do lists and tasks: Microsoft's page opens in your browser. Without it, tasks stay on this computer."
            );
        }
        match (self.sync_covers_lists(account), self.sync_tasks_kept(account)) {
            (true, false) => tl!(
                "Ask for permission to keep your tasks, rules, automatic replies and categories in step with Outlook: Microsoft's page opens in your browser. Without it, tasks and rules stay on this computer and the categories can't be changed here."
            ),
            (true, true) => tl!(
                "Ask for permission to keep your rules, automatic replies and categories in step with Outlook: Microsoft's page opens in your browser. Without it, rules stay on this computer and the categories can't be changed here."
            ),
            (false, false) => tl!(
                "Ask for permission to keep your tasks and automatic replies in step with Outlook: Microsoft's page opens in your browser. Without it, tasks stay on this computer."
            ),
            (false, true) => tl!(
                "Ask for permission to keep your automatic replies in step with Outlook: Microsoft's page opens in your browser. Without it, your automatic replies aren't changed."
            ),
        }
    }

    fn put_sync_question(&mut self, account: Id) -> bool {
        let Some(address) = self.session.mailbox.account(account).map(|a| a.email.trim().to_string()) else { return false };
        let text = match (self.sync_covers_lists(account), self.sync_tasks_kept(account)) {
            _ if self.sync_only_tasks(account) => tl!(
                "To keep the tasks of {address} in step with Microsoft To Do, MailCraft needs your permission to read and change your To Do lists and tasks. Microsoft's page opens in your browser to ask you, once.\n\nUntil you allow it, tasks stay on this computer and nothing of them is sent to Microsoft."
            ),
            (true, false) => tl!(
                "To keep the tasks, rules, automatic replies and categories of {address} in step with Outlook, MailCraft needs your permission to change your To Do lists and your mailbox settings. Microsoft's page opens in your browser to ask you, once.\n\nUntil you allow it, tasks and rules stay on this computer and nothing of them is sent to Microsoft, your automatic replies aren't changed, and the categories of this mailbox can't be changed here."
            ),
            (true, true) => tl!(
                "To keep the rules, automatic replies and categories of {address} in step with Outlook, MailCraft needs your permission to change your mailbox settings. Microsoft's page opens in your browser to ask you, once.\n\nUntil you allow it, rules stay on this computer and nothing of them is sent to Microsoft, your automatic replies aren't changed, and the categories of this mailbox can't be changed here."
            ),
            (false, false) => tl!(
                "To keep the tasks and automatic replies of {address} in step with Outlook, MailCraft needs your permission to change your To Do lists and your mailbox settings. Microsoft's page opens in your browser to ask you, once.\n\nUntil you allow it, tasks stay on this computer and nothing of them is sent to Microsoft, and your automatic replies aren't changed."
            ),
            (false, true) => tl!(
                "To keep the automatic replies of {address} in step with Outlook, MailCraft needs your permission to change your mailbox settings. Microsoft's page opens in your browser to ask you, once.\n\nUntil you allow it, your automatic replies aren't changed."
            ),
        };
        let text = crate::i18n::fmt(text, &[("address", &address)]);
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
        let refusal = match answer {
            // (Somebody else's tokens are neither used nor remembered, #103.)
            Ok(tokens) if self.signed_in_as_other(account, &tokens) => {
                self.sync_waits_again(account);
                return;
            }
            Ok(tokens) => {
                if !tokens.refresh.is_empty() {
                    self.remember("refresh", account, &tokens.refresh);
                    if let Some(kept) = self.tokens.get_mut(&account) {
                        kept.refresh = tokens.refresh.clone();
                    }
                }
                log::info!("sync: the owner allowed keeping it in step with Outlook; the next jobs find out");
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
                let said = match (self.sync_covers_lists(account), self.sync_tasks_kept(account)) {
                    _ if self.sync_only_tasks(account) => tl!("Tasks stay on this computer for now: {error}"),
                    (true, false) => tl!(
                        "For now, tasks and rules stay on this computer, your automatic replies aren't changed and the categories can't be changed here: {error}"
                    ),
                    (true, true) => {
                        tl!(
                            "For now, rules stay on this computer, your automatic replies aren't changed and the categories can't be changed here: {error}"
                        )
                    }
                    (false, false) => tl!("For now, tasks stay on this computer and your automatic replies aren't changed: {error}"),
                    (false, true) => tl!("For now, your automatic replies aren't changed: {error}"),
                };
                self.sync_not_now(account);
                self.status(crate::i18n::fmt(said, &[("error", &e)]));
                None
            }
            Err(e) => Some(crate::i18n::fmt(tl!("MailCraft wasn't allowed to keep this mailbox in step with Outlook: {error}"), &[("error", &e)])),
        };
        if let Some(said) = refusal {
            // (Not put again by itself, for any of the four: File › Info has Ask again.)
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
    /// of them stands at (for the tip), and whether Ask again is offered. `None` for an account
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
        // A part is off while it waits, or has a way to ask again; the line says on or off only
        // when every part is, and partly on when they differ (their tip says which).
        let off = |(_, again, p): &(String, bool, Perm)| *again || *p == Perm::Waits;
        let line = if parts.iter().any(|(.., p)| *p == Perm::Asking) {
            tl!("Sync with Outlook: asking for your permission…")
        } else if parts.iter().all(off) {
            tl!("Sync with Outlook: off")
        } else if parts.iter().any(off) {
            tl!("Sync with Outlook: partly on")
        } else {
            tl!("Sync with Outlook: on")
        };
        Some((line.to_string(), details, again))
    }
}
