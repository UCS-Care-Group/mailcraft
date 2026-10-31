// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Info › Automatic Replies in the window, for the accounts that sign in with Microsoft:
//! the dialog in classic Outlook's shape, the line in File › Info and the status bar that say
//! when replies are going out, and the jobs that read and change them on the server.
//!
//! **Jobs.** The engine says what to ask (`account.automaticReplies`, `account.setAutomaticReplies`)
//! and takes what came back in (`sync.applyAutomaticReplies`); the host does the asking on a
//! background thread ([`crate::Services::graph_auto_replies`]): a token of its own
//! (`mailcraft_net::oauth::refresh_graph_settings`), then Graph's `GET` and `PATCH` of the
//! mailbox settings' automatic replies. They are read when the dialog opens, when File › Info
//! is shown, and every so often while the window is open, so that the status bar can say they
//! are on; never while working offline, and never while something else of the account's is
//! being asked (a Send/Receive, a sign-in, the calendar's job).
//!
//! **The permission.** Changing them takes `MailboxSettings.ReadWrite`, which every page of a
//! sign-in asks for now; a sign-in from before that agreed to reading them only. Such a sign-in
//! reads them as ever, and the dialog and File › Info say plainly that changing them needs the
//! owner to sign in again, with a button that does it ([`MailApp::auto_replies_sign_in`]).
//! Nothing else waits on it: mail, the calendar, the categories and the working hours go on as
//! they were.
//!
//! **What a server says** is the engine's to make text of: the replies are shown as plain text in
//! plain text fields, never as a page, and what the owner types goes up as simple HTML.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;

use egui::{RichText, Ui, vec2};
use mailcraft_engine::cmd::auto_replies::{Known, reply_accounts};
use mailcraft_model::auto_replies::{AutoReplies, AutoReplyStatus, ExternalAudience};
use mailcraft_model::{Account, Id};
use serde_json::{Value, json};

use crate::forms::kit;
use crate::theme::{Tokens, regular};
use crate::{MailApp, Tokens as SignIn, now_ms};

/// How often an account's automatic replies are read while the window is open, and how long a
/// read that failed is left before the next (milliseconds).
const REFRESH_MS: f64 = 15.0 * 60_000.0;
const RETRY_MS: f64 = 2.0 * 60_000.0;
/// How wide the dialog is, and how many lines its reply fields show.
pub const WIDTH: f32 = 560.0;
const REPLY_ROWS: usize = 7;

/// What the host is asked to do with one account's automatic replies
/// ([`crate::Services::graph_auto_replies`]).
pub struct AutoRepliesJob {
    pub account: Account,
    /// The account's sign-in settings, with the application id filled in.
    pub oauth: mailcraft_model::OAuth,
    /// The account's refresh token: the job gets its own token with it.
    pub refresh: String,
    /// What to change first, in Graph's names (`account.setAutomaticReplies`' answer); `None`
    /// only reads them.
    pub change: Option<AutoReplies>,
    pub cancel: Arc<AtomicBool>,
}

/// What an [`AutoRepliesJob`] came to. Never an error as a whole: every trouble is said.
#[derive(Default)]
pub struct AutoRepliesResult {
    pub account: Id,
    /// The refresh token is the account's newest.
    pub tokens: Option<SignIn>,
    /// The automatic replies as the server has them after the job, when they could be read.
    pub setting: Option<AutoReplies>,
    /// The sign-in may change them (it covers `MailboxSettings.ReadWrite`).
    pub can_write: bool,
    /// A change was asked for and the server took it.
    pub changed: bool,
    /// A change was asked for and the sign-in only reads them: nothing was changed, and the owner
    /// is to sign in again.
    pub write_refused: bool,
    /// The sign-in covers none of the mailbox's settings: the owner is to sign in again.
    pub consent_needed: bool,
    /// The sign-in itself is no longer taken.
    pub sign_in_refused: bool,
    /// What went wrong otherwise, in a few words; empty when nothing did.
    pub error: String,
}

/// The state of the jobs and of what is said about them, in the window.
#[derive(Default)]
pub struct AutoRepliesSync {
    /// The job under way: for which account, whether it changes them, and the answer to come.
    rx: Option<(Id, bool, Receiver<AutoRepliesResult>)>,
    /// When each account's replies were last asked for, and whether that read failed.
    checked: HashMap<Id, (f64, bool)>,
    /// A change waiting for the job under way to end.
    waiting: Option<(Id, AutoReplies)>,
    /// What the owner is to be told of an account's replies when they can't be had or changed:
    /// the sign-in doesn't let MailCraft change them (or read them), or why the last job failed.
    pub(crate) trouble: HashMap<Id, Trouble>,
}

/// Why an account's automatic replies can't be had or changed just now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Trouble {
    /// The sign-in reads them and may not change them: a new sign-in asks for that.
    SignInToChange,
    /// The sign-in covers none of the mailbox's settings: a new sign-in asks for them.
    SignInToRead,
    /// Microsoft no longer takes the sign-in.
    SignInOver,
    /// The job failed: why, in a few words.
    Failed(String),
}

impl AutoRepliesSync {
    /// Whether a job is in flight.
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Whether a job for `account` is in flight, and whether it changes them.
    pub(crate) fn busy_for(&self, account: Id) -> Option<bool> {
        self.rx.as_ref().filter(|(a, _, _)| *a == account).map(|(_, changing, _)| *changing)
    }
}

impl MailApp {
    /// What is known of `account`'s automatic replies.
    pub(crate) fn auto_replies_known(&self, account: Id) -> Option<&Known> {
        self.session.auto_replies.get(&account)
    }

    /// Read every account's automatic replies when they are due. Called every frame. (Public for
    /// a host's tests.)
    pub fn auto_replies_tick_at(&mut self, now: f64) -> bool {
        if self.auto_replies.rx.is_some() || self.services.graph_auto_replies.is_none() || self.offline() {
            return false;
        }
        // One thing at a time: a Send/Receive, a sign-in, the calendar's and the tasks' jobs go first.
        if self.signing_in.is_some() || self.sync_rx.is_some() || self.graph_rx.is_some() || self.calendar.rx.is_some() || self.tasks.busy() {
            return false;
        }
        if let Some((account, change)) = self.auto_replies.waiting.take() {
            return self.start_auto_replies_job(account, Some(change), now);
        }
        if !self.auto_sync {
            return false;
        }
        for account in reply_accounts(&self.session.mailbox) {
            if matches!(self.auto_replies.trouble.get(&account), Some(Trouble::SignInOver | Trouble::SignInToRead)) {
                continue;
            }
            let due = match self.auto_replies.checked.get(&account) {
                None => true,
                Some((at, failed)) => now - at >= if *failed { RETRY_MS } else { REFRESH_MS },
            };
            if due && self.start_auto_replies_job(account, None, now) {
                return true;
            }
        }
        false
    }

    pub(crate) fn auto_replies_tick(&mut self, ctx: &egui::Context) {
        self.auto_replies_tick_at(now_ms());
        if self.auto_replies.rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Hand the host a job for `account`: read its replies, after changing them as `change` says.
    fn start_auto_replies_job(&mut self, account: Id, change: Option<AutoReplies>, now: f64) -> bool {
        let Some(owner) = self.session.mailbox.account(account).cloned() else { return false };
        let Some(oauth) = owner.oauth.clone() else { return false };
        let kept = self.tokens.get(&account).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", &owner.email)).unwrap_or_default();
        if refresh.is_empty() {
            // (Nothing to renew with yet: Send/Receive signs in first.)
            if change.is_some() {
                self.status(tl!("Sign in with Send/Receive first, then change your automatic replies."));
            }
            return false;
        }
        let changing = change.is_some();
        let job = AutoRepliesJob { account: owner, oauth: self.effective_oauth(oauth), refresh, change, cancel: Default::default() };
        let Some(host) = self.services.graph_auto_replies.as_ref() else { return false };
        self.auto_replies.rx = Some((account, changing, host(job)));
        self.auto_replies.checked.insert(account, (now, false));
        if changing {
            self.session.sync_status = tl!("Changing your automatic replies…").to_string();
        }
        true
    }

    /// After the engine ran `id`: what the window does with `account.automaticReplies` (reads
    /// them on the server) and `account.setAutomaticReplies` (changes them there).
    pub(crate) fn auto_replies_after(&mut self, id: &str, r: Result<Value, String>) -> Result<Value, String> {
        let Ok(v) = &r else { return r };
        match id {
            "account.automaticReplies" => {
                if let Some(account) = v.pointer("/ask/account").and_then(Value::as_u64) {
                    self.auto_replies_read_now(account);
                }
            }
            "account.setAutomaticReplies" => {
                let account = v.get("account").and_then(Value::as_u64).unwrap_or(0);
                let change = v.get("change").cloned().map(|c| AutoReplies::from_graph(&c)).unwrap_or_default();
                self.auto_replies_change(account, change)?;
            }
            _ => {}
        }
        r
    }

    /// The window was asked to read `account`'s replies (`account.automaticReplies`): at once,
    /// when nothing else is under way, else at the next chance.
    pub(crate) fn auto_replies_read_now(&mut self, account: Id) {
        if self.auto_replies.busy_for(account).is_some() {
            return;
        }
        self.auto_replies.checked.remove(&account);
        if matches!(self.auto_replies.trouble.get(&account), Some(Trouble::Failed(_))) {
            self.auto_replies.trouble.remove(&account);
        }
        self.auto_replies_tick_at(now_ms());
    }

    /// The window was asked to change `account`'s replies (`account.setAutomaticReplies`).
    pub(crate) fn auto_replies_change(&mut self, account: Id, change: AutoReplies) -> Result<(), String> {
        if self.services.graph_auto_replies.is_none() {
            return Err(tl!("This build can't reach the server to change automatic replies. Use the desktop app.").to_string());
        }
        if self.offline() {
            return Err(crate::send_receive::said());
        }
        if matches!(self.auto_replies.trouble.get(&account), Some(Trouble::Failed(_))) {
            self.auto_replies.trouble.remove(&account);
        }
        self.auto_replies.waiting = Some((account, change));
        self.auto_replies_tick_at(now_ms());
        Ok(())
    }

    /// The job answered: keep the refresh token, take in what the server has, and say how a
    /// change went. (Public for a host's tests.)
    pub fn poll_auto_replies(&mut self) {
        let Some((account, changing, rx)) = &self.auto_replies.rx else { return };
        let (account, changing) = (*account, *changing);
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                AutoRepliesResult { account, error: tl!("Reading the automatic replies stopped unexpectedly").to_string(), ..Default::default() }
            }
        };
        self.auto_replies.rx = None;
        if changing {
            self.session.sync_status.clear();
        }
        if let Some(tokens) = &result.tokens
            && !tokens.refresh.is_empty()
            && !self.signed_in_as_other(account, tokens)
        {
            let address = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
            self.remember("refresh", &address, &tokens.refresh);
            if let Some(mail) = self.tokens.get_mut(&account) {
                mail.refresh = tokens.refresh.clone();
            }
        }
        let mut trouble = None;
        if let Some(setting) = &result.setting {
            let params = json!({"account": account, "setting": setting, "canWrite": result.can_write});
            if let Err(e) = self.session.run("sync.applyAutomaticReplies", &params) {
                trouble = Some(Trouble::Failed(e.to_string()));
            }
        }
        if result.sign_in_refused {
            log::warn!("automatic replies, account {account}: the sign-in was refused; waiting for a new one");
            trouble = Some(Trouble::SignInOver);
        } else if result.consent_needed {
            log::info!("automatic replies, account {account}: the sign-in doesn't cover the mailbox's settings");
            trouble = Some(Trouble::SignInToRead);
        } else if result.write_refused || (result.setting.is_some() && !result.can_write) {
            trouble = trouble.or(Some(Trouble::SignInToChange));
        } else if !result.error.is_empty() {
            let why: String = result.error.chars().filter(|c| !c.is_control()).take(300).collect();
            trouble = Some(Trouble::Failed(why));
        }
        let failed = matches!(trouble, Some(Trouble::Failed(_)));
        self.auto_replies.checked.insert(account, (now_ms(), failed));
        match &trouble {
            Some(t) => {
                self.auto_replies.trouble.insert(account, t.clone());
            }
            None => {
                self.auto_replies.trouble.remove(&account);
            }
        }
        if !changing {
            return;
        }
        // A change the owner asked for: said whichever way it went.
        let said = if result.changed {
            match self.auto_replies_known(account).map(|k| k.on_at(self.session.now())) {
                Some(true) => tl!("Automatic replies are on").to_string(),
                Some(false) if self.auto_replies_known(account).is_some_and(|k| k.status == AutoReplyStatus::Scheduled) => {
                    tl!("Automatic replies are set for the time range you chose").to_string()
                }
                _ => tl!("Automatic replies are off").to_string(),
            }
        } else {
            match &trouble {
                Some(Trouble::SignInToChange | Trouble::SignInToRead) => {
                    tl!("Your automatic replies weren't changed: sign in again to let MailCraft change them (File › Info).").to_string()
                }
                Some(Trouble::SignInOver) => tl!("Your automatic replies weren't changed: Microsoft no longer takes your sign-in.").to_string(),
                Some(Trouble::Failed(why)) => crate::i18n::fmt(tl!("Your automatic replies weren't changed: {error}"), &[("error", why)]),
                None => tl!("Your automatic replies weren't changed").to_string(),
            }
        };
        self.status(said);
    }

    /// Sign in again for `account`, so that MailCraft may change its automatic replies: the
    /// provider's page, which now asks for that among the rest. Their replies are read again
    /// once it is done.
    pub(crate) fn auto_replies_sign_in(&mut self, account: Id) -> Result<Value, String> {
        let answer = self.run("ui.connectMicrosoft", json!({"account": account}))?;
        self.auto_replies.checked.remove(&account);
        if matches!(self.auto_replies.trouble.get(&account), Some(Trouble::SignInOver | Trouble::SignInToRead)) {
            self.auto_replies.trouble.remove(&account);
        }
        Ok(answer)
    }

    /// What File › Info says of `account`'s automatic replies, and whether it offers to sign in
    /// again. `None` for an account that has none here.
    pub(crate) fn auto_replies_line(&self, account: Id) -> Option<(String, bool)> {
        self.services.graph_auto_replies.as_ref()?;
        if !reply_accounts(&self.session.mailbox).contains(&account) {
            return None;
        }
        let trouble = self.auto_replies.trouble.get(&account);
        Some(match trouble {
            Some(Trouble::SignInToChange) => (tl!("Automatic replies: MailCraft needs you to sign in again to change them").to_string(), true),
            Some(Trouble::SignInToRead) => (tl!("Automatic replies: MailCraft needs you to sign in again to read them").to_string(), true),
            Some(Trouble::SignInOver) => (tl!("Automatic replies: Microsoft no longer takes the sign-in").to_string(), true),
            Some(Trouble::Failed(why)) => {
                (crate::i18n::fmt(tl!("Automatic replies: couldn't be read ({why})"), &[("why", kit::shown(why, 90).as_str())]), false)
            }
            None => match self.auto_replies_known(account) {
                Some(known) => (summary(self, known), false),
                None if self.auto_replies.busy_for(account).is_some() => (tl!("Automatic replies: reading…").to_string(), false),
                None => (tl!("Automatic replies: not read yet").to_string(), false),
            },
        })
    }

    /// The accounts whose automatic replies go out now, for the status bar.
    pub(crate) fn auto_replies_on(&self) -> Vec<Id> {
        let now = self.session.now();
        reply_accounts(&self.session.mailbox).into_iter().filter(|a| self.auto_replies_known(*a).is_some_and(|k| k.on_at(now))).collect()
    }

    /// For a stand-in host (the screenshot example): `account`'s replies as a server has them.
    #[doc(hidden)]
    pub fn stand_in_auto_replies(&mut self, account: Id, setting: &AutoReplies, can_write: bool) -> Result<Value, String> {
        let params = json!({"account": account, "setting": setting, "canWrite": can_write});
        let answer = self.session.run("sync.applyAutomaticReplies", &params).map_err(|e| e.to_string())?;
        // (A host that never answers, so that File › Info and the status bar have them.)
        if self.services.graph_auto_replies.is_none() {
            self.services.graph_auto_replies = Some(Box::new(|_| std::sync::mpsc::channel().1));
        }
        if can_write {
            self.auto_replies.trouble.remove(&account);
        } else {
            self.auto_replies.trouble.insert(account, Trouble::SignInToChange);
        }
        self.auto_replies.checked.insert(account, (now_ms(), false));
        Ok(answer)
    }
}

/// One line of what is known: off, on, or on between two times.
fn summary(app: &MailApp, known: &Known) -> String {
    let when = |at: mailcraft_time::Timestamp| {
        let wall = app.session.zone().wall(at);
        format!("{} {}", crate::dates::medium(wall.date), wall.time_short(app.session.prefs.twelve_hour))
    };
    match known.status {
        AutoReplyStatus::Disabled => tl!("Automatic replies: off").to_string(),
        AutoReplyStatus::AlwaysEnabled => tl!("Automatic replies: on, until you turn them off").to_string(),
        AutoReplyStatus::Scheduled => match (known.start, known.end) {
            (Some(start), Some(end)) => {
                crate::i18n::fmt(tl!("Automatic replies: from {start} to {end}"), &[("start", &when(start)), ("end", &when(end))])
            }
            _ => tl!("Automatic replies: during a time range").to_string(),
        },
    }
}

/// Which tab of the dialog is in front.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Inside,
    Outside,
}

/// File › Info › Automatic Replies: it only collects what the owner wants, then runs
/// `account.setAutomaticReplies`.
pub struct Draft {
    pub account: Id,
    /// Who it is for, as the title says.
    pub address: String,
    /// Send automatic replies (else don't).
    pub on: bool,
    /// Only during the time range below.
    pub ranged: bool,
    pub start_date: String,
    pub start_time: String,
    pub end_date: String,
    pub end_time: String,
    pub tab: Tab,
    pub internal: String,
    pub external: String,
    /// Reply to people outside the organisation at all, and to which of them.
    pub outside: bool,
    pub contacts_only: bool,
    /// Whether the fields were filled from what the server has (else from nothing, while it is
    /// read): they are filled once, when it comes.
    pub filled: bool,
    pub error: String,
    /// The first field takes the keyboard once.
    focus: bool,
}

impl Draft {
    /// For `{"account"}` (default: the first account that has automatic replies here); `None`
    /// when the engine refuses (no such account, or one that has none), and the refusal is said
    /// as any command's is.
    pub fn open(app: &mut MailApp, params: &Value) -> Option<Draft> {
        let mut asked = json!({});
        if let Some(account) = params.get("account").filter(|v| !v.is_null()) {
            asked["account"] = account.clone();
        }
        let answer = app.run("account.automaticReplies", asked).ok()?;
        let account = answer.get("account").and_then(Value::as_u64)?;
        let address = app.session.mailbox.account(account).map(|a| a.email.trim().to_string()).unwrap_or_default();
        let mut draft = Draft {
            account,
            address,
            on: false,
            ranged: false,
            start_date: String::new(),
            start_time: String::new(),
            end_date: String::new(),
            end_time: String::new(),
            tab: Tab::Inside,
            internal: String::new(),
            external: String::new(),
            outside: true,
            contacts_only: false,
            filled: false,
            error: String::new(),
            focus: false,
        };
        draft.default_range(app);
        draft.fill(app);
        Some(draft)
    }

    /// The time range a new one starts with: from the next whole hour, for a week.
    fn default_range(&mut self, app: &MailApp) {
        let zone = app.session.zone();
        let now = app.session.now();
        let start = now.saturating_add(3_600 - now.rem_euclid(3_600));
        let end = start.saturating_add(7 * 86_400);
        (self.start_date, self.start_time) = wall(app, zone, start);
        (self.end_date, self.end_time) = wall(app, zone, end);
    }

    /// Fill the fields from what is known, once it is.
    fn fill(&mut self, app: &MailApp) {
        if self.filled {
            return;
        }
        let Some(known) = app.auto_replies_known(self.account) else { return };
        let zone = app.session.zone();
        self.on = known.status != AutoReplyStatus::Disabled;
        self.ranged = known.status == AutoReplyStatus::Scheduled;
        if let Some(start) = known.start {
            (self.start_date, self.start_time) = wall(app, zone, start);
        }
        if let Some(end) = known.end {
            (self.end_date, self.end_time) = wall(app, zone, end);
        }
        self.internal = known.internal.clone();
        self.external = known.external.clone();
        self.outside = known.audience != ExternalAudience::None;
        self.contacts_only = known.audience == ExternalAudience::ContactsOnly;
        self.filled = true;
    }

    /// What `account.setAutomaticReplies` is given.
    pub fn params(&self) -> Value {
        let status = match (self.on, self.ranged) {
            (false, _) => AutoReplyStatus::Disabled,
            (true, false) => AutoReplyStatus::AlwaysEnabled,
            (true, true) => AutoReplyStatus::Scheduled,
        };
        let audience = match (self.outside, self.contacts_only) {
            (false, _) => ExternalAudience::None,
            (true, true) => ExternalAudience::ContactsOnly,
            (true, false) => ExternalAudience::All,
        };
        let mut params = json!({
            "account": self.account,
            "status": status.code(),
            "internal": self.internal,
            "external": self.external,
            "audience": audience.code(),
        });
        if status == AutoReplyStatus::Scheduled {
            params["start"] = json!(kit::stamp(&self.start_date, &self.start_time));
            params["end"] = json!(kit::stamp(&self.end_date, &self.end_time));
        }
        params
    }

    /// Draw it. `true` when it is done with (OK went through, or Cancel).
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        self.fill(app);
        let t = Tokens::get(ui.ctx());
        let mut pass = kit::Pass::new(ui, &app.session, &mut self.focus);
        let trouble = app.auto_replies.trouble.get(&self.account).cloned();
        let reading = app.auto_replies.busy_for(self.account).is_some();
        let may_change = !matches!(trouble, Some(Trouble::SignInToChange | Trouble::SignInToRead | Trouble::SignInOver));
        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
        // What stands in the way of a change, and the way past it.
        if let Some(trouble) = &trouble {
            let (text, sign_in) = match trouble {
                Trouble::SignInToChange => (
                    tl!(
                        "MailCraft can read your automatic replies but not change them yet. Sign in again and allow it to change your mailbox settings: Microsoft's page opens in your browser. Mail and your calendar work as they are meanwhile."
                    )
                    .to_string(),
                    true,
                ),
                Trouble::SignInToRead => (
                    tl!(
                        "MailCraft needs you to sign in again to read and change your automatic replies: Microsoft's page opens in your browser. Mail and your calendar work as they are meanwhile."
                    )
                    .to_string(),
                    true,
                ),
                Trouble::SignInOver => (tl!("Microsoft no longer takes your sign-in. Sign in again to read and change your automatic replies.").to_string(), true),
                Trouble::Failed(why) => (crate::i18n::fmt(tl!("Your automatic replies couldn't be read: {error}"), &[("error", why)]), false),
            };
            notice(ui, &t, &text, |ui| {
                if sign_in {
                    let button = ui.add_enabled(app.signing_in.is_none() && !app.offline(), egui::Button::new(tl!("Sign In Again")));
                    app.hit.add("autoRepliesSignIn", self.account, button.rect);
                    if button.clicked()
                        && let Err(e) = app.auto_replies_sign_in(self.account)
                    {
                        self.error = e;
                    }
                }
            });
        } else if reading && !self.filled {
            ui.label(RichText::new(tl!("Reading your automatic replies from the server…")).color(t.text_dim));
        } else if app.offline() {
            ui.label(RichText::new(crate::send_receive::said()).color(t.text_dim));
        }
        let editable = may_change && !app.offline();
        ui.add_enabled_ui(editable, |ui| {
            if ui.radio(!self.on, tl!("Don't send automatic replies")).clicked() {
                self.on = false;
            }
            if ui.radio(self.on, tl!("Send automatic replies")).clicked() {
                self.on = true;
            }
            ui.add_enabled_ui(self.on, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    ui.checkbox(&mut self.ranged, tl!("Only send during this time range:"));
                });
                ui.add_enabled_ui(self.ranged, |ui| {
                    for (label, key, date, time) in [
                        (tl!("Start time:"), "start", &mut self.start_date, &mut self.start_time),
                        (tl!("End time:"), "end", &mut self.end_date, &mut self.end_time),
                    ] {
                        ui.horizontal(|ui| {
                            ui.add_space(44.0);
                            let (r, _) = ui.allocate_exact_size(vec2(70.0, kit::ROW_H), egui::Sense::hover());
                            ui.painter().text(r.left_center(), egui::Align2::LEFT_CENTER, label, regular(13.0), t.text);
                            pass.date(ui, ("autoReplies", key, "date"), date, true);
                            pass.time(ui, ("autoReplies", key, "time"), time, true, None);
                        });
                    }
                });
                ui.add_space(4.0);
                ui.label(tl!("Each sender gets one automatic reply, with the message below:"));
                ui.horizontal(|ui| {
                    if ui.selectable_label(self.tab == Tab::Inside, tl!("Inside My Organization")).clicked() {
                        self.tab = Tab::Inside;
                    }
                    let outside = if self.outside { tl!("Outside My Organization (On)") } else { tl!("Outside My Organization (Off)") };
                    if ui.selectable_label(self.tab == Tab::Outside, outside).clicked() {
                        self.tab = Tab::Outside;
                    }
                });
                ui.separator();
                match self.tab {
                    Tab::Inside => {
                        kit::multiline(ui, ("autoReplies", "internal"), &mut self.internal, REPLY_ROWS);
                    }
                    Tab::Outside => {
                        ui.checkbox(&mut self.outside, tl!("Reply to people outside my organization"));
                        ui.add_enabled_ui(self.outside, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_space(22.0);
                                if ui.radio(self.contacts_only, tl!("Only my contacts")).clicked() {
                                    self.contacts_only = true;
                                }
                                if ui.radio(!self.contacts_only, tl!("Everyone outside my organization")).clicked() {
                                    self.contacts_only = false;
                                }
                            });
                            kit::multiline(ui, ("autoReplies", "external"), &mut self.external, REPLY_ROWS - 2);
                        });
                    }
                }
            });
        });
        kit::error(ui, &self.error);
        let dates_ok = !self.on
            || !self.ranged
            || (kit::date_ok(&self.start_date, true)
                && kit::date_ok(&self.end_date, true)
                && kit::time_ok(&self.start_time, true)
                && kit::time_ok(&self.end_time, true));
        let changing = app.auto_replies.busy_for(self.account) == Some(true);
        let footer = kit::footer(ui, tl!("OK"), editable && dates_ok && !changing, false);
        if footer.cancel {
            return true;
        }
        if (footer.ok || pass.submit) && editable && dates_ok {
            match app.run("account.setAutomaticReplies", self.params()) {
                Ok(_) => return true,
                Err(e) => self.error = e,
            }
        }
        false
    }

    /// The dialog's title: whose replies they are.
    pub fn title(&self) -> String {
        if self.address.is_empty() {
            tl!("Automatic Replies").to_string()
        } else {
            crate::i18n::fmt(tl!("Automatic Replies - {address}"), &[("address", &self.address)])
        }
    }
}

/// A date and a time of day as the dialog's fields show them.
fn wall(app: &MailApp, zone: mailcraft_time::Zone, at: mailcraft_time::Timestamp) -> (String, String) {
    let wall = zone.wall(at);
    (wall.date.iso(), wall.time_short(app.session.prefs.twelve_hour))
}

/// A band across the top of the dialog that says what stands in the way, with its button.
fn notice(ui: &mut Ui, t: &Tokens, text: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new().fill(t.panel).stroke(egui::Stroke::new(1.0, t.border)).corner_radius(6.0).inner_margin(egui::Margin::same(10)).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(t.text));
            add(ui);
        },
    );
    ui.add_space(4.0);
}

/// File › Info's Automatic Replies: a tile for each account that has them here, filled while
/// replies go out, as Outlook's is; a click opens the dialog. Nothing for a mailbox without one.
pub(crate) fn info_tiles(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    if app.services.graph_auto_replies.is_none() {
        return;
    }
    let accounts = reply_accounts(&app.session.mailbox);
    if accounts.is_empty() {
        return;
    }
    ui.add_space(10.0);
    ui.label(RichText::new(tl!("Automatic Replies")).font(crate::theme::semibold(14.0)).color(t.accent_text));
    let now = app.session.now();
    for account in accounts {
        let on = app.auto_replies_known(account).is_some_and(|k| k.on_at(now));
        let address = app.session.mailbox.account(account).map(|a| a.email.trim().to_string()).unwrap_or_default();
        let (line, _) = app.auto_replies_line(account).unwrap_or_default();
        let detail = if app.auto_replies_known(account).is_some() || app.auto_replies.trouble.contains_key(&account) {
            line
        } else {
            tl!("Let people know you're away or can't answer for a while: your mailbox replies by itself").to_string()
        };
        let title = if on {
            crate::i18n::fmt(tl!("Automatic replies are being sent for {address}"), &[("address", &address)])
        } else {
            crate::i18n::fmt(tl!("Automatic Replies for {address}"), &[("address", &address)])
        };
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().min(560.0), 52.0), egui::Sense::click());
        let fill = match (on, resp.hovered()) {
            (true, _) => t.flagged_row,
            (false, true) => t.hover,
            (false, false) => t.panel,
        };
        ui.painter().rect(r, 6.0, fill, egui::Stroke::new(1.0, if on { t.orange } else { t.border }), egui::StrokeKind::Inside);
        let icon = egui::Rect::from_center_size(egui::pos2(r.min.x + 26.0, r.center().y), vec2(24.0, 24.0));
        crate::icons::paint(ui.painter(), icon, "reply", t.icon, t.accent);
        crate::theme::paint_clipped(
            ui.painter(),
            egui::pos2(r.min.x + 52.0, r.min.y + 18.0),
            r.width() - 64.0,
            &title,
            crate::theme::semibold(13.0),
            t.text,
        );
        crate::theme::paint_clipped(ui.painter(), egui::pos2(r.min.x + 52.0, r.min.y + 36.0), r.width() - 64.0, &detail, regular(11.5), t.text_dim);
        app.hit.add("autoRepliesTile", account, r);
        if resp.clicked() {
            let _ = app.run("ui.dialog", json!({"name": "automaticReplies", "account": account}));
        }
    }
}

/// The status bar's word while replies go out for some account, as Outlook's: a click opens the
/// dialog for the first.
pub(crate) fn status_item(app: &mut MailApp, ui: &mut Ui, t: &Tokens, size: f32) {
    let Some(first) = app.auto_replies_on().first().copied() else { return };
    let link = ui.link(RichText::new(tl!("Automatic Replies")).size(size).color(t.orange));
    app.hit.add("autoRepliesOn", first, link.rect);
    if link.on_hover_text(tl!("Your mailbox is sending automatic replies. Click to change them.")).clicked() {
        let _ = app.run("ui.dialog", json!({"name": "automaticReplies", "account": first}));
    }
    ui.add_space(10.0);
}
