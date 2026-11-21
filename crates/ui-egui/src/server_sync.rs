// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One question for everything an older sign-in of a Microsoft account doesn't cover: keeping its
//! to-dos ([`crate::task_sync`]), the rules of its server ([`crate::rule_sync`]), its automatic
//! replies ([`crate::auto_replies`]) and its list of categories ([`crate::category_sync`]) in step
//! with Outlook (#661), and its shared mailboxes and the calendars shared with its owner
//! ([`crate::shared_permission`], #70).
//!
//! A new sign-in's page asks for what all of those take (`Tasks.ReadWrite`,
//! `MailboxSettings.ReadWrite`, `Mail.ReadWrite.Shared`, `Mail.Send.Shared`,
//! `Calendars.ReadWrite.Shared`) beside the rest, and none of them is asked for by itself. An
//! account that signed in before is asked once, by the window, when the first of them finds its
//! sign-in doesn't cover it: one question, then one page of Microsoft's for all of it
//! (`ui.allowSync`, `mailcraft_net::oauth::sign_in_for_sync`; Microsoft shows only what wasn't
//! given yet). A question that goes away unanswered is not put again by itself in this session;
//! the owner's own doing (a send from a shared mailbox, a calendar opened by address) puts it
//! again. File › Info has one line per account, with Ask again.

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
        self.session.driver.calendar.clock = now;
        self.session.driver.tasks.clock = now;
        self.session.driver.rules.clock = now;
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
        self.shared_page_given.remove(&account);
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
        self.session.driver.tasks.perm.get(&account) == Some(&crate::task_sync::Permission::Given)
    }

    /// Whether `account`'s sign-in lets MailCraft change its mailbox settings already (its rules,
    /// automatic replies or categories were changed with it, as an older version asked it by
    /// itself) while its to-dos wait: the question and its texts then say only the to-dos.
    fn sync_only_tasks(&self, account: Id) -> bool {
        let settings = matches!(self.session.driver.rules.perm.get(&account), Some(crate::rule_sync::Permission::Given))
            || matches!(self.session.driver.auto_replies.perm.get(&account), Some(crate::auto_replies::Permission::Given))
            || matches!(self.category_sync.perm.get(&account), Some(crate::task_sync::Permission::Given));
        settings && !self.sync_tasks_kept(account)
    }

    /// Whether one of the four waits for the one question (the shared mailboxes' part aside).
    fn sync_four_wait(&self, account: Id) -> bool {
        [self.tasks_sync_part(account), self.rules_sync_part(account), self.auto_replies_sync_part(account), self.categories_sync_part(account)]
            .into_iter()
            .flatten()
            .any(|(.., perm)| perm != Perm::Fine)
    }

    /// Whether only the shared mailboxes and the calendars shared with the owner wait: the
    /// question and its texts then say only those.
    fn sync_only_shared(&self, account: Id) -> bool {
        !self.sync_four_wait(account) && self.shared_sync_waits(account)
    }

    /// File › Info's tip for Ask again, saying only what `account` keeps in step.
    pub(crate) fn sync_tip(&self, account: Id) -> &'static str {
        if self.sync_only_shared(account) {
            return tl!(
                "Ask for permission to read and send from shared mailboxes and to read the calendars shared with you: Microsoft's page opens in your browser. Without it, nothing of them is read or sent."
            );
        }
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
            _ if self.sync_only_shared(account) => tl!(
                "To read and send from the shared mailboxes you open with {address}, and to show the calendars shared with you, MailCraft needs your permission. Microsoft's page opens in your browser to ask you, once, for everything MailCraft uses that you haven't allowed yet.\n\nUntil you allow it, nothing of those mailboxes and calendars is read, mail and meeting answers from a shared mailbox wait on this computer, and nothing is sent from your own address in their place."
            ),
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
        let mut text = crate::i18n::fmt(text, &[("address", &address)]);
        // (What else the page asks for: the shared mailboxes and calendars, for an account that
        // may have them, so that nothing is asked again for them later.)
        if !self.sync_only_shared(account) && self.shared_on_page(account) {
            text.push_str("\n\n");
            text.push_str(tl!(
                "The same page asks to read and send from the shared mailboxes you have access to and to read the calendars shared with you, so that MailCraft doesn't have to ask again."
            ));
        }
        let title = if self.sync_only_shared(account) { tl!("Shared Mailboxes and Calendars") } else { tl!("Keep in Step with Outlook") };
        let ask = Question {
            title: title.to_string(),
            text,
            action: tl!("Continue").to_string(),
            with: Value::Null,
            more: Vec::new(),
            note: None,
            cancel: None,
            tick: None,
            calendar: None,
            cut: None,
            listing: None,
        };
        self.sync_mark_asking(account);
        self.set_dialog(Some(Dialog::Confirm { command: "ui.allowSync".to_string(), params: json!({"account": account}), ask: Box::new(ask) }));
        true
    }

    /// What waits for the owner is being asked.
    fn sync_mark_asking(&mut self, account: Id) {
        self.sync_asking.insert(account);
        self.tasks_sync_asking(account);
        self.rules_sync_asking(account);
        self.auto_replies_sync_asking(account);
        self.categories_sync_asking(account);
        self.shared_sync_asking(account);
    }

    /// The question went away unanswered, or was put before: what waits takes it as a no.
    pub(crate) fn sync_not_now(&mut self, account: Id) {
        self.sync_declined.insert(account);
        self.tasks_sync_not_now(account);
        self.rules_sync_not_now(account);
        self.auto_replies_sync_not_now(account);
        self.categories_sync_not_now(account);
        self.shared_sync_not_now(account);
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

    /// `ui.allowSync` (and the older ids that ask for one part of it): Microsoft's page, for all
    /// of it, because the owner said go ahead (or asked for a shared mailbox to be read).
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

    /// The owner answered on Microsoft's page: with the tokens, everything that waited goes on at
    /// once (`read`, a shared mailbox the owner asked to be read, is read now); without, why not
    /// is said, once.
    pub(crate) fn sync_signed_in(&mut self, account: Id, answer: Result<Tokens, String>, read: Option<Id>) {
        use crate::approval::NO_BROWSER;
        use crate::favorites_sync::{NEEDS_APPROVAL, REFUSED_BY_ORGANISATION, REFUSED_FOR_ACCOUNT};
        self.session.sync_status.clear();
        let only_shared = self.sync_only_shared(account);
        self.sync_asking.remove(&account);
        // A shared mailbox being added that won't be read now: it isn't kept, and the dialog says
        // why (S1); otherwise why is said in the status bar.
        let not_read = |app: &mut MailApp, said: String| match read.filter(|r| app.shared_checking.contains(r)) {
            Some(shared) => app.finish_shared_check(shared, Err(said)),
            None => app.status(said),
        };
        let refusal = match answer {
            // (Somebody else's tokens are neither used nor remembered, #103.)
            Ok(tokens) if self.signed_in_as_other(account, &tokens) => {
                // (What `signed_in_as_other` said is said to whoever waited for the answer.)
                let said = self.status_msg.as_ref().map(|(said, _)| said.clone()).unwrap_or_default();
                self.sync_waits_again(account, &said);
                if let Some(shared) = read.filter(|r| self.shared_checking.contains(r)) {
                    self.finish_shared_check(shared, Err(said));
                }
                return;
            }
            Ok(tokens) => {
                if !tokens.refresh.is_empty() {
                    self.remember("refresh", account, &tokens.refresh);
                    if let Some(kept) = self.tokens.get_mut(&account) {
                        kept.refresh = tokens.refresh.clone();
                    }
                }
                log::info!("sync: the owner allowed what his older sign-in lacked; the next jobs find out");
                if tokens.sync_not_given {
                    // (Its page was asked again for the shared mailboxes' permissions alone: the
                    // organisation has no To Do lists or settings for this account to give, and
                    // that is not put again by itself.)
                    let said = tl!("Your organisation doesn't let MailCraft keep this mailbox in step with Outlook");
                    self.sync_declined.insert(account);
                    self.tasks_sync_refused(account, said);
                    self.rules_sync_refused(account, said);
                    self.auto_replies_sync_refused(account, said);
                    self.categories_sync_refused(account, said);
                } else {
                    self.tasks_allowed(account);
                    self.rules_allowed(account);
                    self.auto_replies_allowed(account);
                    self.categories_allowed(account);
                }
                // (A page that came back without the shared permissions didn't have them for this
                // account: they are not allowed, and it is not put again for them by itself.)
                let said = if tokens.shared_not_given {
                    self.shared_sync_not_given(account, read)
                } else {
                    // (Noted, so that a job that still finds them missing doesn't open it again.)
                    self.shared_page_given.insert(account);
                    self.shared_sync_allowed(account, read)
                };
                if let Some(said) = said {
                    not_read(self, said);
                }
                return;
            }
            Err(e) if e == NEEDS_APPROVAL => {
                let said = if only_shared {
                    tl!("Your organisation's administrator has to approve MailCraft reading shared mailboxes and calendars once: File › Info")
                } else {
                    tl!("Your organisation's administrator has to approve MailCraft keeping this mailbox in step with Outlook once: File › Info")
                };
                let _ = self.run("ui.approvalHelp", json!({"account": account}));
                Some((said.to_string(), "approval"))
            }
            Err(e) if e == REFUSED_BY_ORGANISATION || e == REFUSED_FOR_ACCOUNT => {
                let said = if only_shared {
                    tl!("Your organisation doesn't let MailCraft read shared mailboxes and the calendars shared with you")
                } else {
                    tl!("Your organisation doesn't let MailCraft keep this mailbox in step with Outlook")
                };
                Some((said.to_string(), "organisation"))
            }
            Err(e) if e == NO_BROWSER => {
                let said = match (self.sync_covers_lists(account), self.sync_tasks_kept(account)) {
                    _ if only_shared => tl!("For now, nothing of shared mailboxes is read or sent: {error}"),
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
                not_read(self, crate::i18n::fmt(said, &[("error", &e)]));
                None
            }
            Err(e) if only_shared => {
                Some((crate::i18n::fmt(tl!("Shared mailboxes and calendars weren't allowed: {error}"), &[("error", &e)]), "declined"))
            }
            Err(e) => Some((
                crate::i18n::fmt(tl!("MailCraft wasn't allowed to keep this mailbox in step with Outlook: {error}"), &[("error", &e)]),
                "declined",
            )),
        };
        if let Some((said, kind)) = refusal {
            // (Not put again by itself: File › Info has Ask again.)
            self.sync_declined.insert(account);
            self.tasks_sync_refused(account, &said);
            self.rules_sync_refused(account, &said);
            self.auto_replies_sync_refused(account, &said);
            self.categories_sync_refused(account, &said);
            self.shared_sync_refused(account, &said, kind);
            not_read(self, said);
        }
    }

    /// The page signed in somebody else: what waited waits as it did.
    fn sync_waits_again(&mut self, account: Id, said: &str) {
        self.tasks_sync_waits(account);
        self.rules_sync_waits(account);
        self.auto_replies_sync_waits(account);
        self.categories_sync_waits(account);
        self.shared_sync_waits_again(account, said);
    }

    /// What File › Info says under a Microsoft account's card: one line for the four, what each
    /// of them stands at (for the tip), and whether Ask again is offered. `None` for an account
    /// none of them is kept for.
    pub(crate) fn sync_line(&self, account: Id) -> Option<(String, String, bool)> {
        let parts: Vec<(String, bool, Perm)> = [
            self.tasks_sync_part(account),
            self.rules_sync_part(account),
            self.auto_replies_sync_part(account),
            self.categories_sync_part(account),
            self.shared_sync_part(account),
        ]
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
