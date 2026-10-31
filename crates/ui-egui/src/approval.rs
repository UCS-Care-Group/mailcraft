// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! When an organisation has to approve MailCraft before one of its people can sign in: what the
//! window knows of it, and its commands.
//!
//! Many organisations let only an administrator approve a new program. Microsoft's sign-in page
//! then says so instead of signing the owner in, and it is not certain that anything comes back
//! from it at all. So the owner is never left waiting: Add Account says what to do while the
//! page is still open, and has a page of its own for it ([`crate::account_setup`]), which an
//! account that exists opens too (`ui.approvalHelp`) when its sign-in, or what it was asked for
//! Outlook's Favorites, is refused for the same reason. What there is to do:
//!
//! - an owner who is an administrator approves it on Microsoft's admin-consent page, which the
//!   host opens in the browser and waits for ([`crate::Services::admin_consent`],
//!   `ui.adminConsent`); once that comes back with a yes, the account signs in as any other;
//! - any other owner copies a short message for an administrator, with that page's address in
//!   it (`ui.copyAdminMessage`). Nothing is sent by the program;
//! - and afterwards signs in again (`ui.signInAgain`).
//!
//! The address names the application, the organisation's directory when it is known and the
//! permissions, and nothing of the owner.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use mailcraft_model::{Id, OAuth};
use serde_json::{Value, json};

use crate::MailApp;
use crate::account_setup::Help;
use crate::dialogs::Dialog;
use crate::favorites_sync::Note;
use crate::forms::Form;

/// How a sign-in on Microsoft's page ended without tokens, when it is one of the ways the
/// window has something of its own to say about. The host answers a [`crate::SignInRequest`]
/// with exactly one of these as its error; anything else is shown as it is.
///
/// An administrator of the organisation has to approve the program first:
pub const NEEDS_APPROVAL: &str = crate::favorites_sync::NEEDS_APPROVAL;
/// The page came back with a no that was given on it. (Microsoft sends the same when the owner
/// leaves the page that said an administrator has to approve.)
pub const SIGN_IN_DECLINED: &str = "Microsoft's page came back without signing you in";
/// Nothing came back in the time that is waited: the page was closed, or left open.
pub const SIGN_IN_TIMED_OUT: &str = "Nothing came back from Microsoft's sign-in page";
/// No browser could be started for the page.
pub const NO_BROWSER: &str = "MailCraft couldn't open your browser";

/// How a sign-in ended without tokens, as the window tells the ways apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInEnd {
    /// [`NEEDS_APPROVAL`].
    Approval,
    /// [`SIGN_IN_DECLINED`].
    Declined,
    /// [`SIGN_IN_TIMED_OUT`].
    TimedOut,
    /// [`NO_BROWSER`].
    NoBrowser,
    /// Anything else, in the host's (or Microsoft's) words.
    Other,
}

impl SignInEnd {
    /// The way `error` names, as a host answers a sign-in with it.
    pub fn of(error: &str) -> SignInEnd {
        match error {
            NEEDS_APPROVAL => SignInEnd::Approval,
            SIGN_IN_DECLINED => SignInEnd::Declined,
            SIGN_IN_TIMED_OUT => SignInEnd::TimedOut,
            NO_BROWSER => SignInEnd::NoBrowser,
            _ => SignInEnd::Other,
        }
    }

    /// What the control channel calls it.
    pub fn code(self) -> &'static str {
        match self {
            SignInEnd::Approval => "approval",
            SignInEnd::Declined => "declined",
            SignInEnd::TimedOut => "timedOut",
            SignInEnd::NoBrowser => "noBrowser",
            SignInEnd::Other => "failed",
        }
    }
}

/// What the host needs to have an administrator approve the program for an organisation
/// ([`crate::Services::admin_consent`]).
pub struct AdminConsentRequest {
    /// The account it is for, or 0 while one is being set up.
    pub account: Id,
    /// The sign-in settings, with the application id filled in.
    pub oauth: OAuth,
    /// The organisation's directory when it is known (the id a sign-in named, or a domain of
    /// its own); empty for whichever the administrator who signs in is of.
    pub tenant: String,
    /// Set when the owner gives up waiting, for the host to stop too.
    pub cancel: Arc<AtomicBool>,
}

/// What an [`AdminConsentRequest`] came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdminConsentAnswer {
    /// An administrator approved: Microsoft's page came back with a yes.
    Approved,
    /// Whoever signed in on Microsoft's page may not approve for the organisation. With it,
    /// what Microsoft said.
    NotAdministrator(String),
    /// The page came back without the approval. With it, what Microsoft said.
    Declined(String),
    /// Nothing came back in the time that is waited.
    TimedOut,
    /// No browser could be started for the page.
    NoBrowser,
    /// Called off from here.
    Cancelled,
    /// Anything else, in the host's (or Microsoft's) words.
    Failed(String),
}

impl AdminConsentAnswer {
    /// What the control channel calls it.
    pub fn code(&self) -> &'static str {
        match self {
            AdminConsentAnswer::Approved => "approved",
            AdminConsentAnswer::NotAdministrator(_) => "notAdministrator",
            AdminConsentAnswer::Declined(_) => "declined",
            AdminConsentAnswer::TimedOut => "timedOut",
            AdminConsentAnswer::NoBrowser => "noBrowser",
            AdminConsentAnswer::Cancelled => "cancelled",
            AdminConsentAnswer::Failed(_) => "failed",
        }
    }

    /// What the window says of an approval that wasn't had (in the interface language), and
    /// what Microsoft answered, when it said anything. Both empty for one that was had, or that
    /// was called off here.
    pub(crate) fn said(&self) -> (String, String) {
        let words = |text: &String| crate::forms::kit::shown(text.trim(), 300);
        match self {
            AdminConsentAnswer::Approved | AdminConsentAnswer::Cancelled => (String::new(), String::new()),
            AdminConsentAnswer::NotAdministrator(theirs) => (
                tl!("The account that signed in on Microsoft's page is not an administrator of your organisation, so it could not approve MailCraft. Copy the message for an administrator instead.").to_string(),
                words(theirs),
            ),
            AdminConsentAnswer::Declined(theirs) => {
                (tl!("Microsoft's page came back without the approval: it was declined there, or left before it was given.").to_string(), words(theirs))
            }
            AdminConsentAnswer::TimedOut => (
                tl!("Nothing came back from Microsoft's approval page: it was closed, or five minutes went by before the approval was given.").to_string(),
                String::new(),
            ),
            AdminConsentAnswer::NoBrowser => (
                tl!("MailCraft couldn't open your browser for Microsoft's approval page. Copy the message instead: the link in it opens the same page.").to_string(),
                String::new(),
            ),
            AdminConsentAnswer::Failed(theirs) => (tl!("The approval didn't go through.").to_string(), words(theirs)),
        }
    }
}

/// The message an owner passes on to an administrator, around the address of Microsoft's page
/// (`{link}`): what the program is, what is asked of them, and what they will see afterwards.
/// Nothing of the owner is in it.
const MESSAGE: &str = "Hello,\n\nI would like to use MailCraft with my work account. MailCraft is an open-source mail and calendar program that runs on my own computer. Our organisation lets only an administrator approve a new program, so it needs your approval once, for the whole organisation. Microsoft's page for that is here:\n\n{link}\n\nAfter you approve, your browser may show a page that can't be reached. That is expected: it is an address on your own computer, where nothing is listening, and the approval has been given by then.\n\nThank you";

/// [`MESSAGE`] with `link` in it, in the interface language.
pub(crate) fn message(link: &str) -> String {
    crate::i18n::fmt(tl!(MESSAGE), &[("link", link)])
}

/// Whose approval is asked for: an account that exists, or the one that is being set up.
#[derive(Clone, Debug, Default)]
pub(crate) struct Whose {
    /// The account, or 0 while one is being set up.
    pub account: Id,
    /// Its sign-in settings, as it has them (an empty application id: this installation's).
    pub oauth: OAuth,
    /// Its organisation's directory when that is known; else empty.
    pub tenant: String,
}

/// An approval the host is busy with (Microsoft's page is open in the browser).
struct Asking {
    account: Id,
    rx: Receiver<AdminConsentAnswer>,
    cancel: Arc<AtomicBool>,
}

/// What the app remembers of approvals that were asked for. All of it is for the run.
#[derive(Default)]
pub(crate) struct Approvals {
    asking: Option<Asking>,
    /// What the last one came to, until the page that asked for it has taken it.
    answer: Option<(Id, AdminConsentAnswer)>,
}

/// What a command of the approval help is for.
enum Target {
    /// The account that is being set up (Add Account is open on its sign-in).
    Wizard,
    /// An account that exists.
    Account(Id),
}

impl MailApp {
    /// Whether an administrator's approval is being waited for (Microsoft's admin-consent page
    /// is open in the browser).
    pub fn approving(&self) -> bool {
        self.approvals.asking.is_some()
    }

    /// The account dialog, when it is open: Add Account, Account Settings, or the approval page
    /// of an account that exists.
    fn account_dialog(&self) -> Option<&crate::account_setup::AccountSetup> {
        match &self.dialog {
            Some(Dialog::Form(Form::Account(setup))) => Some(setup.as_ref()),
            _ => None,
        }
    }

    /// Do something with the account dialog that needs the app as well; `None` when it isn't
    /// open. It stays open unless what was done says it is finished.
    fn with_account_dialog<R>(&mut self, act: impl FnOnce(&mut crate::account_setup::AccountSetup, &mut MailApp) -> R) -> Option<R> {
        let mut dialog = self.dialog.take()?;
        let (out, finished) = match &mut dialog {
            Dialog::Form(Form::Account(setup)) => {
                let out = act(setup, self);
                (Some(out), setup.finished())
            }
            _ => (None, false),
        };
        // (Nothing that is done there opens another dialog; should it, that one stays.)
        if !finished && self.dialog.is_none() {
            self.dialog = Some(dialog);
        }
        out
    }

    /// What a command of the approval help is for: the account `p.account` names; else the one
    /// the account dialog is about (the one being set up, while Add Account is at its sign-in
    /// with Microsoft); else the first account that signs in with Microsoft.
    fn approval_target(&self, p: &Value) -> Result<Target, String> {
        if p.get("account").and_then(Value::as_u64).is_none()
            && let Some(setup) = self.account_dialog()
            && setup.at_sign_in()
        {
            return Ok(match setup.approval_account() {
                0 => Target::Wizard,
                account => Target::Account(account),
            });
        }
        self.favorites_account(p).map(Target::Account)
    }

    /// Whose approval a command asks for, for an account that exists.
    fn whose(&self, account: Id) -> Result<Whose, String> {
        let (oauth, tenant) = self.approval_of(account)?;
        Ok(Whose { account, oauth, tenant })
    }

    /// The message for an administrator and the address in it, for `whose`.
    pub(crate) fn admin_message(&self, whose: &Whose) -> Result<(String, String), String> {
        let link = self.admin_link(&self.effective_oauth(whose.oauth.clone()), &whose.tenant)?;
        Ok((message(&link), link))
    }

    /// Put the message for an administrator on the clipboard, for the owner to pass on. Nothing
    /// is opened and nothing is sent; the status bar says where it is.
    pub(crate) fn copy_admin_message_for(&mut self, whose: &Whose) -> Result<Value, String> {
        let (text, link) = self.admin_message(whose)?;
        if let Some(ctx) = &self.ctx {
            ctx.copy_text(text.clone());
        }
        self.status(tl!("The message is on the clipboard: paste it into mail or chat for your organisation's administrator"));
        Ok(json!({"account": whose.account, "url": link, "text": text}))
    }

    /// Why this build can't open Microsoft's approval page at all, when it can't.
    pub(crate) fn admin_consent_unavailable(&self) -> Option<String> {
        let unable = tl!("This build can't open Microsoft's approval page. Copy the message for an administrator instead.");
        self.services.admin_consent.is_none().then(|| unable.to_string())
    }

    /// Hand an approval to the host: Microsoft's admin-consent page opens in the browser, and
    /// the answer is picked up with [`take_admin_consent`](Self::take_admin_consent). One page
    /// of Microsoft's at a time: not while a sign-in or another approval is waited for.
    pub(crate) fn begin_admin_consent(&mut self, whose: &Whose) -> Result<(), String> {
        if let Some(why) = self.admin_consent_unavailable() {
            return Err(why);
        }
        if self.signing_in.is_some() || self.approvals.asking.is_some() {
            return Err(tl!("Finish signing in first (your browser is waiting)").to_string());
        }
        if self.offline() {
            return Err(crate::send_receive::said());
        }
        let Some(ask) = &self.services.admin_consent else { return Ok(()) };
        let cancel = Arc::new(AtomicBool::new(false));
        let request = AdminConsentRequest {
            account: whose.account,
            oauth: self.effective_oauth(whose.oauth.clone()),
            tenant: whose.tenant.clone(),
            cancel: cancel.clone(),
        };
        let rx = ask(request);
        self.approvals.answer = None;
        self.approvals.asking = Some(Asking { account: whose.account, rx, cancel });
        self.session.sync_status = tl!("Microsoft's approval page is open in your browser…").to_string();
        Ok(())
    }

    /// Give up an approval that is being waited for, if one is: the account it was for.
    pub(crate) fn cancel_admin_consent(&mut self) -> Option<Id> {
        let asking = self.approvals.asking.take()?;
        asking.cancel.store(true, Ordering::Relaxed);
        self.session.sync_status.clear();
        Some(asking.account)
    }

    /// What the approval that was asked for `account` came to, once it has: taken by the page
    /// that asked for it.
    pub(crate) fn take_admin_consent(&mut self, account: Id) -> Option<AdminConsentAnswer> {
        match &self.approvals.answer {
            Some((whose, _)) if *whose == account => self.approvals.answer.take().map(|(_, answer)| answer),
            _ => None,
        }
    }

    /// The approval came back. For an account that exists, what follows is done here, whether
    /// or not its page is open: approved, it signs in again by itself; refused, the status bar
    /// says so. For one that is being set up, its dialog takes it from here. (Public for a
    /// host's tests, which have no window.)
    pub fn poll_admin_consent(&mut self) {
        let Some(asking) = &self.approvals.asking else { return };
        let answer = match asking.rx.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => AdminConsentAnswer::Failed(tl!("Waiting for the approval stopped unexpectedly").to_string()),
        };
        let account = asking.account;
        self.approvals.asking = None;
        self.session.sync_status.clear();
        // (How it went, never what was said or by whom.)
        log::info!("approval by an administrator, account {account}: {}", answer.code());
        if account != 0 {
            match &answer {
                AdminConsentAnswer::Approved => {
                    self.status(tl!("Your organisation has approved MailCraft"));
                    if let Err(e) = self.sign_in_again_for(account) {
                        self.status(e);
                    }
                }
                AdminConsentAnswer::Cancelled => {}
                refused => self.status(refused.said().0),
            }
        }
        self.approvals.answer = Some((account, answer));
    }

    /// "Try again" for an account that exists: what was refused for want of an approval is
    /// asked for once more. That is what its Favorites in Outlook were to be read with, when
    /// the folder pane says they wait on an administrator; else its sign-in, by Send/Receive.
    pub(crate) fn sign_in_again_for(&mut self, account: Id) -> Result<Value, String> {
        match self.favorites_note_of(account) {
            Some(Note::Approval) => self.execute("ui.connectFavorites", json!({"account": account})),
            _ => self.execute("ui.sync", json!({"account": account})),
        }
    }

    /// An approval that was asked for from Add Account has nobody waiting for it once that
    /// dialog is gone, however it went: the host is told to stop. Called every frame.
    pub(crate) fn sweep_admin_consent(&mut self) {
        let orphaned = self.approvals.asking.as_ref().is_some_and(|asking| asking.account == 0) && self.account_dialog().is_none();
        if orphaned {
            self.cancel_admin_consent();
        }
        // (And an answer nobody took is nobody's.)
        if self.account_dialog().is_none() {
            self.approvals.answer = None;
        }
    }

    // ---- commands ----

    /// `ui.approvalHelp`: open the page that says what to do when an organisation has to
    /// approve MailCraft first. In Add Account, while it is at its sign-in, that is its own
    /// page; for an account that exists, a dialog with the same page.
    fn approval_help(&mut self, p: &Value) -> Result<Value, String> {
        match self.approval_target(p)? {
            Target::Wizard => self.in_wizard(Help::Open),
            Target::Account(account) => {
                // (Already open for it: nothing to do.)
                if self.account_dialog().is_some_and(|setup| setup.at_sign_in() && setup.approval_account() == account) {
                    return Ok(json!({"dialog": "account", "page": "approval", "account": account}));
                }
                match Dialog::open("account", self, &json!({"id": account, "page": "approval"})) {
                    Some(dialog) => {
                        self.dialog = Some(dialog);
                        Ok(json!({"dialog": "account", "page": "approval", "account": account}))
                    }
                    None => Err(tl!("This account doesn't sign in with Microsoft").to_string()),
                }
            }
        }
    }

    /// `ui.copyAdminMessage`: put a short message for an administrator on the clipboard, with
    /// the address of Microsoft's admin-consent page in it. Nothing is opened or sent.
    fn copy_admin_message(&mut self, p: &Value) -> Result<Value, String> {
        match self.approval_target(p)? {
            Target::Wizard => self.in_wizard(Help::Copy),
            Target::Account(account) => {
                let whose = self.whose(account)?;
                self.copy_admin_message_for(&whose)
            }
        }
    }

    /// `ui.adminConsent`: open Microsoft's admin-consent page in the browser, for an owner who
    /// is an administrator, and wait for its answer.
    fn admin_consent(&mut self, p: &Value) -> Result<Value, String> {
        match self.approval_target(p)? {
            Target::Wizard => self.in_wizard(Help::Administrator),
            Target::Account(account) => {
                // The page of that account, when it is open, shows that it is waiting.
                if self.account_dialog().is_some_and(|setup| setup.at_sign_in() && setup.approval_account() == account) {
                    return self.in_wizard(Help::Administrator);
                }
                let whose = self.whose(account)?;
                self.begin_admin_consent(&whose)?;
                Ok(json!({"pending": "adminConsent", "account": account}))
            }
        }
    }

    /// `ui.signInAgain`: "Try again", once an administrator has approved.
    fn sign_in_again(&mut self, p: &Value) -> Result<Value, String> {
        match self.approval_target(p)? {
            Target::Wizard => self.in_wizard(Help::Again),
            Target::Account(account) => {
                if self.account_dialog().is_some_and(|setup| setup.at_sign_in() && setup.approval_account() == account) {
                    return self.in_wizard(Help::Again);
                }
                self.sign_in_again_for(account)
            }
        }
    }

    /// Have the account dialog do `what`, as its own links and buttons do.
    fn in_wizard(&mut self, what: Help) -> Result<Value, String> {
        let done = self.with_account_dialog(|setup, app| setup.run(app, what));
        done.unwrap_or_else(|| Err(tl!("No sign-in with Microsoft is under way").to_string()))
    }
}

/// The `ui.*` commands of the approval help; `None` for any other.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "ui.approvalHelp" => app.approval_help(p),
        "ui.copyAdminMessage" => app.copy_admin_message(p),
        "ui.adminConsent" => app.admin_consent(p),
        "ui.signInAgain" => app.sign_in_again(p),
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
