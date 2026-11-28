// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Sieve rule editor (#2057), under Advanced… in Rules and Alerts, for an account whose server
//! keeps MailCraft's Sieve script: a Sieve rule's name and its text (one block of Sieve, RFC 5228,
//! in MailCraft's script), with line numbers, the lines with a mistake marked, and the problems
//! listed below (a click on one puts the cursor there). Outlook has no such editor: this one is
//! MailCraft's own, and it never shows for other accounts.
//!
//! **Two checks.** While the owner types, the text is checked here once typing pauses
//! ([`mailcraft_engine::cmd::rules_sieve::editor_problems`], with the extensions the server last
//! said it has). Check asks the server itself (ManageSieve's CHECKSCRIPT, through the host, never
//! offline): the text goes inside the script MailCraft would write
//! ([`mailcraft_engine::cmd::rules_sieve::for_server_check`]), so the server sees it where it
//! would run, and the lines it names are said as the rule's own. Save runs `rules.addSieve` or
//! `rules.editSieve`, which check the text again; it waits while the text has a mistake. Warnings
//! are shown and don't keep it from being saved.
//!
//! **Hostile text.** What the server says is capped and without control characters
//! (`mailcraft_proto::managesieve::kept_text`); the text typed is at most
//! [`MAX_SIEVE_RULE`] bytes, and is the owner's, shown as it is and never run here.
//!
//! **The owner's own script, whole.** For an account whose server runs a script of the owner's
//! own (made in webmail or another program), Advanced… › Edit your Sieve script… and the button by
//! its rules open the same editor on the whole script, exactly as it was read
//! ([`OwnScript`]): a script whose lines all end in CRLF is shown with plain line breaks and
//! written with CRLF again, so text left alone goes back byte for byte. It is checked as a whole
//! script ([`mailcraft_engine::cmd::sieve_own::problems`]), Check sends it to the server as it is
//! (its line numbers are the script's own), and Save runs `sieve.putOwnScript`, which the host
//! writes only while the server's script is the one read: else the editor says it changed there
//! and offers Reload, and nothing is written. A note stays up while a webmail's marks are in it.
//!
//! **One rule as Sieve.** A rule of the owner's script opens on its own bytes alone ([`OwnRule`]):
//! the comment lines right above it and its `if`/`elsif`/`else` chain, or in a webmail's filter its
//! Sieve alone. It is checked as the script with it in place, its problems said at its own lines
//! (those elsewhere in the script apart), Check sends that whole script to the server with its line
//! numbers said as the rule's, and Save runs `sieve.putOwnBlock`, which the host writes as it
//! writes the whole script. One of MailCraft's own rules that the rule builder can't show whole
//! opens on the Sieve MailCraft writes for it, and Save makes it a Sieve rule (`rules.toSieve`).

use egui::text::{CCursor, CCursorRange, LayoutJob};
use egui::{Align, Align2, FontId, Rect, Sense, Shape, Ui, pos2, vec2};
use mailcraft_engine::cmd::rules_sieve::{ForCheck, ServerLine, editor_problems, for_server_check, server_lines};
use mailcraft_engine::cmd::sieve_blocks;
use mailcraft_engine::cmd::sieve_own::{self, Webmail};
use mailcraft_engine::sieve::{MAX_SCRIPT, digest};
use mailcraft_engine::sieve_check::Problem;
use mailcraft_model::{Id, MAX_SIEVE_RULE, Mailbox};
use serde_json::{Value, json};

use crate::forms::kit::shown;
use crate::sieve_replies::{OwnSaved, ServerCheck, SieveTrouble};
use crate::theme::{Tokens, regular};
use crate::{MailApp, now_ms};

/// How long typing pauses before the text is checked here (milliseconds).
const QUIET_MS: f64 = 300.0;
/// Most problems listed under the editor.
const MAX_LISTED: usize = 50;
/// What a new Sieve rule starts with.
const STARTER: &str =
    "# Runs on the server, in its place among your rules.\nif address :is \"from\" \"news@example.org\" {\n  fileinto \"INBOX\";\n}\n";

/// What the server said of the text, and the text it said it of.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ServerSaid {
    /// The text checked: what is said goes grey once the text has changed since.
    text: String,
    kind: Said,
    /// The server's words, a line each, with the rule's line each names.
    lines: Vec<ServerLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Said {
    Fine,
    Warnings,
    Errors,
    NotOffered,
    /// The server couldn't be asked: why.
    Failed(String),
}

/// What the editor asks of the dialog it is in, which does it once the frame is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditorAsks {
    Check,
    /// Run this command (`rules.addSieve`, `rules.editSieve` or `sieve.putOwnScript`) with these
    /// params.
    Save(&'static str, Value),
    /// Read the owner's own script from the server again, in place of the text here.
    Reload,
    Cancel,
}

/// The owner's own script being edited whole (#2057): what it is on the server, and what became
/// of a save.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OwnScript {
    /// Its name on the server.
    pub(crate) name: String,
    /// The digest of its text as read: written over only while the server's is still that.
    pub(crate) digest: String,
    /// Its text exactly as read.
    pub(crate) read: String,
    /// Every line of it ended in CRLF: shown with plain line breaks, written with CRLF again.
    crlf: bool,
    /// A webmail that wrote it, which may overwrite changes made here.
    pub(crate) webmail: Option<Webmail>,
    /// A save is under way.
    pub(crate) saving: bool,
    /// The server's script changed since it was read: nothing was written.
    pub(crate) changed: bool,
    /// It is being read again (Reload).
    reloading: bool,
    /// The jobs for its account that had ended, with and without what the server has, when
    /// Reload was asked for ([`crate::sieve_replies::SieveSync::reads`]).
    reload_from: (u64, u64),
    /// One rule of it, when that is what is edited, not the whole script.
    pub(crate) rule: Option<OwnRule>,
}

/// One rule of the owner's own script, edited as its Sieve alone (#2057).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OwnRule {
    /// The rule, as the script was read.
    pub(crate) id: Id,
    pub(crate) name: String,
    /// It is a filter of a webmail of the RainLoop kind: its settings stay as they are.
    pub(crate) filter: bool,
    /// Why the rule builder can't change it, when it can't.
    pub(crate) no_form: Option<&'static str>,
}

/// A Sieve rule being written or changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SieveEditor {
    /// The account whose server keeps it.
    pub(crate) account: Id,
    /// The rule changed; `None` for a new one.
    pub(crate) id: Option<Id>,
    pub(crate) name: String,
    pub(crate) text: String,
    /// The text the problems are of (`None` before the first check here), and those problems.
    checked: Option<String>,
    pub(crate) problems: Vec<Problem>,
    /// When the text last changed (milliseconds): the check here waits for typing to pause.
    edited: Option<u64>,
    /// The server's check under way: its ticket, and what it checks.
    asking: Option<(u64, ForCheck, String)>,
    server: Option<ServerSaid>,
    /// Put the cursor at this line and column (from 1) in the next frame.
    pub(crate) goto: Option<(usize, usize)>,
    /// What a command refused, or why Check couldn't run.
    pub(crate) error: String,
    focus: bool,
    /// The owner's own script, whole, when that is what is edited (no name then).
    pub(crate) own: Option<OwnScript>,
    /// It was saved: the dialog closes the editor.
    pub(crate) closed: bool,
    /// One of MailCraft's own rules (`id`), edited as Sieve: Save makes it a Sieve rule (#2057).
    pub(crate) convert: bool,
    /// The problems of the owner's script with a rule's text in place, elsewhere in the script.
    outside: Vec<Problem>,
    /// The name and the text as it opened: a change since is a draft (#2058).
    start: (String, String),
}

/// Whether Rules and Alerts offers Advanced… for `account`: its server keeps MailCraft's Sieve
/// script, and isn't known to offer none.
pub(crate) fn offered(app: &MailApp, account: Id) -> bool {
    app.session.mailbox.account(account).is_some_and(mailcraft_model::Account::keeps_sieve)
        && app.sieve.trouble.get(&account) != Some(&SieveTrouble::Unavailable)
}

impl SieveEditor {
    /// A new Sieve rule of `account`'s, with a short starter.
    pub(crate) fn new_rule(account: Id) -> SieveEditor {
        SieveEditor { account, text: STARTER.to_string(), focus: true, ..Default::default() }.started()
    }

    /// The Sieve rule `id`, to change; `None` when it isn't one.
    pub(crate) fn open(mb: &Mailbox, id: Id) -> Option<SieveEditor> {
        let r = mb.rules.iter().find(|r| r.id == id && r.is_sieve())?;
        Some(SieveEditor { account: r.account, id: Some(id), name: r.name.clone(), text: r.sieve.clone(), focus: true, ..Default::default() }.started())
    }

    /// The owner's own script `account`'s server runs, whole, as it was last read; `None` when it
    /// runs none (or MailCraft's).
    pub(crate) fn own_script(app: &MailApp, account: Id) -> Option<SieveEditor> {
        let (name, text) = sieve_own::own_script(&app.session, account)?;
        let lf = text.matches('\n').count();
        let crlf = lf > 0 && text.matches("\r\n").count() == lf && text.matches('\r').count() == lf;
        let own = OwnScript {
            name: name.to_string(),
            digest: digest(text),
            read: text.to_string(),
            crlf,
            webmail: sieve_own::webmail(text),
            ..Default::default()
        };
        let shown = if crlf { text.replace("\r\n", "\n") } else { text.to_string() };
        Some(SieveEditor { account, text: shown, focus: true, own: Some(own), ..Default::default() }.started())
    }

    /// A rule as Sieve (#2057): one of the owner's own script, on its bytes alone; or one of
    /// MailCraft's own rules kept in its Sieve script, on the Sieve MailCraft writes for it. `None`
    /// when it is neither, or has no Sieve.
    pub(crate) fn as_sieve(app: &MailApp, id: Id) -> Option<SieveEditor> {
        let s = &app.session;
        if let Some(f) = sieve_blocks::find(s, id) {
            if !sieve_blocks::text_offered(s, id) {
                return None;
            }
            let text = sieve_blocks::block_text(&f);
            let lf = text.matches('\n').count();
            let crlf = if lf > 0 {
                text.matches("\r\n").count() == lf && text.matches('\r').count() == lf
            } else {
                !text.contains('\r') && mailcraft_engine::sieve_rainloop::line_end(&f.script.text) == "\r\n"
            };
            let rule = OwnRule { id, name: f.rule.name.clone(), filter: f.span.filter, no_form: f.span.form.err().map(|w| w.said()) };
            let own = OwnScript {
                name: f.script.script.clone(),
                digest: digest(&f.script.text),
                read: text.to_string(),
                crlf,
                webmail: sieve_own::webmail(&f.script.text),
                rule: Some(rule),
                ..Default::default()
            };
            let shown = if crlf { text.replace("\r\n", "\n") } else { text.to_string() };
            return Some(SieveEditor { account: f.account, text: shown, focus: true, own: Some(own), ..Default::default() }.started());
        }
        let mb = &s.mailbox;
        let r = mb.rules.iter().find(|r| r.id == id && convertible(mb, r))?;
        let made = mailcraft_engine::cmd::sieve_rules::block(mb, r, r.account).ok()?;
        let text = made.text.replace("\r\n", "\n");
        Some(SieveEditor { account: r.account, id: Some(id), name: r.name.clone(), text, focus: true, convert: true, ..Default::default() }.started())
    }

    /// As it opened: what [`SieveEditor::changed`] compares with.
    fn started(mut self) -> SieveEditor {
        self.start = (self.name.clone(), self.text.clone());
        self
    }

    /// Whether its name or text was changed since it opened (#2058).
    pub(crate) fn changed(&self) -> bool {
        self.start.0 != self.name || self.start.1 != self.text
    }

    /// Whether one rule of the owner's script is edited, not the whole.
    fn one_rule(&self) -> Option<&OwnRule> {
        self.own.as_ref().and_then(|o| o.rule.as_ref())
    }

    /// The text as it would be written: with CRLF line breaks again where the script had them.
    pub(crate) fn written(&self) -> String {
        match &self.own {
            Some(own) if own.crlf => self.text.replace("\r\n", "\n").replace('\n', "\r\n"),
            _ => self.text.clone(),
        }
    }

    /// How many bytes [`SieveEditor::written`] has, without making it: line breaks of a CRLF
    /// script count two (#2058).
    pub(crate) fn written_len(&self) -> usize {
        match &self.own {
            Some(own) if own.crlf => {
                let bare = self.text.matches('\n').count().saturating_sub(self.text.matches("\r\n").count());
                self.text.len().saturating_add(bare)
            }
            _ => self.text.len(),
        }
    }

    /// The most bytes of text.
    fn cap(&self) -> usize {
        if self.own.is_some() { MAX_SCRIPT } else { MAX_SIEVE_RULE }
    }

    /// The problems that are mistakes (not warnings).
    fn errors(&self) -> usize {
        self.problems.iter().chain(&self.outside).filter(|p| !p.warning).count()
    }

    /// Whether the text is checked here as it is now.
    fn checked_now(&self) -> bool {
        self.checked.as_deref() == Some(self.text.as_str())
    }

    /// Before the frame is drawn: the server's answer, if it came, and the check here once typing
    /// has paused.
    pub(crate) fn tick(&mut self, app: &mut MailApp, ctx: &egui::Context) {
        self.own_tick(app, ctx);
        if let Some((ticket, for_check, text)) = self.asking.clone() {
            match app.sieve_check_answer(ticket) {
                Some(result) => {
                    self.asking = None;
                    let failed = |why: String| ServerSaid { text: text.clone(), kind: Said::Failed(why), lines: Vec::new() };
                    self.server = Some(match result.checked {
                        Some(ServerCheck::Fine) => ServerSaid { text, kind: Said::Fine, lines: Vec::new() },
                        Some(ServerCheck::Warnings(said)) => ServerSaid { kind: Said::Warnings, lines: server_lines(&said, &for_check), text },
                        Some(ServerCheck::Errors(said)) => ServerSaid { kind: Said::Errors, lines: server_lines(&said, &for_check), text },
                        Some(ServerCheck::NotOffered) => ServerSaid { text, kind: Said::NotOffered, lines: Vec::new() },
                        None if result.unavailable && !result.why.is_empty() => failed(result.why),
                        None if result.unavailable => failed(tl!("This account's server doesn't offer Sieve scripts (ManageSieve).").to_string()),
                        None if result.sign_in_refused => failed(tl!("The server didn't take your sign-in.").to_string()),
                        None if !result.error.is_empty() => failed(result.error),
                        None => failed(tl!("The check stopped unexpectedly").to_string()),
                    });
                }
                None if app.sieve_check_busy(ticket) => ctx.request_repaint_after(std::time::Duration::from_millis(250)),
                None => self.asking = None,
            }
        }
        if self.checked_now() {
            return;
        }
        let now = now_ms();
        let quiet = self.edited.map_or(QUIET_MS, |at| now - at as f64);
        if quiet < QUIET_MS {
            ctx.request_repaint_after(std::time::Duration::from_millis((QUIET_MS - quiet).clamp(1.0, QUIET_MS) as u64));
            return;
        }
        self.check_here(&app.session);
    }

    /// The owner's own script: what became of a save, and a reload once it is read.
    fn own_tick(&mut self, app: &mut MailApp, ctx: &egui::Context) {
        let account = self.account;
        let Some(own) = self.own.as_mut() else { return };
        if own.saving {
            match app.sieve.own_saved.remove(&account) {
                Some(OwnSaved::Done) => {
                    own.saving = false;
                    self.closed = true;
                }
                Some(OwnSaved::Changed) => {
                    own.saving = false;
                    own.changed = true;
                }
                Some(OwnSaved::Failed(why)) => {
                    own.saving = false;
                    self.error = why;
                }
                None => ctx.request_repaint_after(std::time::Duration::from_millis(250)),
            }
        }
        if own.reloading {
            // Only a read that ended after Reload was asked for, with what the server has, starts
            // the editor over; one that failed leaves the draft as it is (#2058).
            let (done, failed) = app.sieve.reads.get(&account).copied().unwrap_or_default();
            if done <= own.reload_from.0 {
                if failed > own.reload_from.1 {
                    own.reloading = false;
                    self.error = tl!("Your Sieve script couldn't be read again: the server doesn't run it now, or can't be reached.").to_string();
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(250));
                }
                return;
            }
            let fresh = match own.rule.as_ref() {
                Some(rule) => SieveEditor::as_sieve(app, rule.id).filter(|e| e.own.is_some()),
                None => SieveEditor::own_script(app, account),
            };
            match fresh {
                Some(fresh) => *self = fresh,
                None => {
                    own.reloading = false;
                    self.error = tl!("Your Sieve script couldn't be read again: the server doesn't run it now, or can't be reached.").to_string();
                }
            }
        }
    }

    /// Reload asked for: the server is read again, and the editor starts over from what it has.
    pub(crate) fn reload(&mut self, app: &mut MailApp) {
        let Some(own) = self.own.as_mut() else { return };
        own.reloading = true;
        own.reload_from = app.sieve.reads.get(&self.account).copied().unwrap_or_default();
        self.error.clear();
        app.sieve_try_again(self.account);
    }

    /// Check the text here now ([`editor_problems`], or for the owner's own script
    /// [`sieve_own::problems`]).
    pub(crate) fn check_here(&mut self, s: &mailcraft_engine::Session) {
        let mb = &s.mailbox;
        self.outside.clear();
        if let Some(rule) = self.one_rule().map(|r| r.id) {
            let found = sieve_blocks::block_problems(s, rule, &self.written());
            self.problems = found.inside;
            self.outside = found.outside;
            self.checked = Some(self.text.clone());
            return;
        }
        self.problems = match mb.account(self.account) {
            Some(a) if self.own.is_some() && self.text.len() <= MAX_SCRIPT => sieve_own::problems(a, &self.text),
            Some(a) if self.text.len() <= MAX_SIEVE_RULE => editor_problems(a, &self.text),
            _ => Vec::new(),
        };
        self.checked = Some(self.text.clone());
    }

    /// Check asked for: the text in MailCraft's script, to the server.
    pub(crate) fn check_on_server(&mut self, app: &mut MailApp) {
        self.error.clear();
        // Larger than the server is sent, as it would be written (#2058).
        if self.own.is_some() && self.written_len() > MAX_SCRIPT {
            let why = tl!("A Sieve script is at most 1 MB.").to_string();
            self.server = Some(ServerSaid { text: self.text.clone(), kind: Said::Failed(why), lines: Vec::new() });
            return;
        }
        // The owner's own script goes as it is: the server's line numbers are its own.
        let made = if let Some(rule) = self.one_rule().map(|r| r.id) {
            sieve_blocks::for_server_check(&app.session, rule, &self.written()).map_err(|e| app.refusal_said(&e.to_string()))
        } else if self.own.is_some() {
            let script = self.written();
            let lines = script.matches('\n').count().saturating_add(1);
            Ok(ForCheck { script, first_line: 1, lines })
        } else {
            for_server_check(&app.session, self.account, self.id, &self.name, &self.text).map_err(|e| app.refusal_said(&e.to_string()))
        };
        let asked = made.and_then(|for_check| app.sieve_check_start(self.account, for_check.script.clone()).map(|ticket| (ticket, for_check)));
        match asked {
            Ok((ticket, for_check)) => {
                self.asking = Some((ticket, for_check, self.text.clone()));
                self.server = None;
            }
            Err(why) => self.server = Some(ServerSaid { text: self.text.clone(), kind: Said::Failed(why), lines: Vec::new() }),
        }
    }

    /// The command Save runs, with its params.
    pub(crate) fn command(&self) -> (&'static str, Value) {
        if let (Some(own), Some(rule)) = (&self.own, self.one_rule()) {
            return ("sieve.putOwnBlock", json!({"id": rule.id, "text": self.written(), "digest": own.digest}));
        }
        if let (true, Some(id)) = (self.convert, self.id) {
            return ("rules.toSieve", json!({"id": id, "name": self.name, "sieve": self.text}));
        }
        if let Some(own) = &self.own {
            return ("sieve.putOwnScript", json!({"account": self.account, "text": self.written(), "digest": own.digest}));
        }
        match self.id {
            Some(id) => ("rules.editSieve", json!({"id": id, "name": self.name, "sieve": self.text})),
            None => ("rules.addSieve", json!({"account": self.account, "name": self.name, "sieve": self.text})),
        }
    }

    /// Why Save can't run yet, in words; `None` when it can.
    fn lacks(&self) -> Option<&'static str> {
        if let Some(own) = &self.own {
            if own.saving {
                return Some(tl!("Saving…"));
            }
            if own.changed {
                return Some(tl!("It changed on the server since you opened it: reload it first."));
            }
            if self.text.trim().is_empty() {
                return Some(if own.rule.is_some() { tl!("A rule needs some Sieve.") } else { tl!("A Sieve script needs some Sieve.") });
            }
            if self.written_len() > MAX_SCRIPT {
                return Some(tl!("A Sieve script is at most 1 MB."));
            }
            if self.written() == own.read {
                return Some(tl!("Nothing has changed yet."));
            }
        } else if self.name.trim().is_empty() {
            return Some(tl!("A Sieve rule needs a name."));
        }
        if self.text.trim().is_empty() {
            return Some(tl!("A Sieve rule needs some Sieve."));
        }
        if self.own.is_none() && self.text.len() > MAX_SIEVE_RULE {
            return Some(tl!("A Sieve rule is at most 64 KB."));
        }
        if !self.checked_now() {
            return Some(tl!("Checking…"));
        }
        (self.errors() > 0).then_some(tl!("Correct the mistakes first."))
    }

    /// The editor, in the dialog's body. What it asks of the dialog, if anything.
    pub(crate) fn ui(&mut self, ui: &mut Ui, app: &MailApp) -> Option<EditorAsks> {
        let t = Tokens::get(ui.ctx());
        let mut asks = None;
        let dim = |ui: &mut Ui, text: &str| {
            ui.label(egui::RichText::new(text).font(regular(11.5)).color(t.text_dim));
        };
        match &self.own {
            Some(own) if own.rule.is_some() => self.rule_head(ui, &t),
            Some(own) => {
                let title = crate::i18n::fmt(tl!("Your Sieve script \"{name}\""), &[("name", &shown(&own.name, 80))]);
                ui.label(egui::RichText::new(title).font(crate::theme::semibold(12.5)).color(t.text));
                dim(
                    ui,
                    tl!(
                        "The whole script your server runs, made in another program, as it was read. Save writes back exactly this text, only while the script on the server is still the one you opened."
                    ),
                );
                // A webmail's marks: it may write over this (#2057); the note stays up.
                if let Some(webmail) = own.webmail {
                    let which = match webmail {
                        Webmail::RainLoop => "RainLoop or SnappyMail",
                        Webmail::Roundcube => "Roundcube",
                    };
                    let note = crate::i18n::fmt(
                        tl!(
                            "This script was written by a webmail ({webmail}). It may overwrite or drop the changes you make here the next time its filters are saved there."
                        ),
                        &[("webmail", which)],
                    );
                    ui.label(egui::RichText::new(note).font(regular(11.5)).color(t.orange));
                }
            }
            None => {
                let title = if self.convert {
                    tl!("Edit rule as Sieve")
                } else if self.id.is_some() {
                    tl!("Change Sieve rule")
                } else {
                    tl!("New Sieve rule")
                };
                ui.label(egui::RichText::new(title).font(crate::theme::semibold(12.5)).color(t.text));
                dim(
                    ui,
                    tl!(
                        "One block of Sieve (RFC 5228), run by the server in its place among your rules. MailCraft writes the require for it at the top of its script."
                    ),
                );
                if self.convert {
                    let note = tl!(
                        "This rule will be kept as Sieve text from now on: the rule builder no longer changes it. This is the Sieve MailCraft writes for it now."
                    );
                    ui.label(egui::RichText::new(note).font(regular(11.5)).color(t.orange));
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(tl!("Name"));
                    let name = egui::TextEdit::singleline(&mut self.name).desired_width(320.0).char_limit(mailcraft_model::MAX_NAME);
                    let _ = crate::text_menu::field(ui.add(name));
                });
            }
        }
        ui.add_space(4.0);
        if self.code(ui, &t) {
            self.edited = Some(now_ms() as u64);
            self.error.clear();
        }
        let used = self.written_len();
        let cap = self.cap();
        if used >= cap / 4 * 3 {
            let said =
                crate::i18n::fmt(tl!("{used} of {most} KB"), &[("used", &(used.div_ceil(1024)).to_string()), ("most", &(cap >> 10).to_string())]);
            ui.label(egui::RichText::new(said).font(regular(11.0)).color(if used > cap { t.red } else { t.orange }));
        }
        self.problems_list(ui, &t);
        self.outside_list(ui, &t);
        self.server_said(ui, &t);
        if self.own.as_ref().is_some_and(|o| o.changed) {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(tl!(
                    "Your Sieve script changed on the server since you opened it, so nothing was written. Reload it to see what it has now: the changes made here are lost then."
                ))
                .font(regular(11.5))
                .color(t.red),
            );
        }
        ui.add_space(6.0);
        let unavailable = app.sieve_check_unavailable(self.account);
        ui.horizontal(|ui| {
            // (The owner's own script is saved through the server alone: Save waits as Check does.)
            let lacks = self.lacks().map(str::to_string).or_else(|| unavailable.clone().filter(|_| self.own.is_some()));
            let save = ui.add_enabled(lacks.is_none(), egui::Button::new(tl!("Save")));
            let tip = if self.one_rule().is_some() {
                tl!("Write this text to the server in place of this rule; the rest of your script stays as it is")
            } else if self.own.is_some() {
                tl!("Write this text to the server in place of your script; it stays the one that runs")
            } else {
                tl!("Keep this rule; it goes to the server with your other rules")
            };
            let save = match &lacks {
                Some(why) => save.on_disabled_hover_text(why.as_str()),
                None => save.on_hover_text(tip),
            };
            if save.clicked() && lacks.is_none() {
                let (command, params) = self.command();
                asks = Some(EditorAsks::Save(command, params));
            }
            let busy = self.asking.is_some();
            let check = ui.add_enabled(unavailable.is_none() && !busy && !self.text.trim().is_empty(), egui::Button::new(tl!("Check")));
            let tip = if self.own.is_some() && self.one_rule().is_none() {
                tl!("Ask the server whether it takes this script; nothing is changed there")
            } else {
                tl!("Ask the server whether it takes this rule; nothing is changed there")
            };
            let check = match &unavailable {
                Some(why) => check.on_disabled_hover_text(why.as_str()),
                None => check.on_hover_text(tip),
            };
            if check.clicked() && unavailable.is_none() && !busy {
                asks = Some(EditorAsks::Check);
            }
            if let Some(own) = &self.own
                && (own.changed || own.reloading)
            {
                let reload = ui.add_enabled(!own.reloading, egui::Button::new(tl!("Reload")));
                if reload.on_hover_text(tl!("Read your script from the server again, in place of the text here")).clicked() {
                    asks = Some(EditorAsks::Reload);
                }
            }
            let saving = self.own.as_ref().is_some_and(|o| o.saving || o.reloading);
            if busy || saving {
                ui.spinner();
                let doing = if saving { tl!("Working with the server…") } else { tl!("Asking the server…") };
                ui.label(egui::RichText::new(doing).color(t.text_dim));
            } else if let Some(why) = &unavailable {
                ui.label(egui::RichText::new(shown(why, 160)).font(regular(11.0)).color(t.text_dim));
            }
            if ui.button(tl!("Cancel")).clicked() {
                asks = Some(EditorAsks::Cancel);
            }
        });
        crate::forms::kit::error(ui, &self.error);
        asks
    }

    /// The text with its line numbers, the lines with a mistake (or a warning) marked. Whether
    /// it changed.
    fn code(&mut self, ui: &mut Ui, t: &Tokens) -> bool {
        let font: FontId = egui::TextStyle::Monospace.resolve(ui.style());
        let id = ui.make_persistent_id("sieve_editor_text");
        let mut moved = None;
        if let Some((line, column)) = self.goto.take() {
            let at = char_at(&self.text, line, column);
            let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(at))));
            egui::TextEdit::store_state(ui.ctx(), id, state);
            ui.memory_mut(|m| m.request_focus(id));
            moved = Some(line);
        }
        if std::mem::take(&mut self.focus) {
            ui.memory_mut(|m| m.request_focus(id));
        }
        let lines = self.text.split('\n').count().max(1);
        let digits = lines.to_string().len().max(2) as f32;
        let gutter_width = digits * font.size * 0.62 + 14.0;
        // The lines a mistake or a warning is on (from 1).
        let marked = |warning: bool| -> Vec<usize> { self.problems.iter().filter(|p| p.warning == warning).map(|p| p.line).collect() };
        let (errors, warnings) = (marked(false), marked(true));
        let server_errors: Vec<usize> = match &self.server {
            Some(said) if said.kind == Said::Errors && said.text == self.text => said.lines.iter().filter_map(|l| l.line).collect(),
            _ => Vec::new(),
        };
        let (colour, layout_font) = (t.text, font.clone());
        let mut layouter = move |ui: &Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
            // (No wrapping: a row is a line, and the numbers stand beside them.)
            let mut job = LayoutJob::simple(buf.as_str().to_owned(), layout_font.clone(), colour, f32::INFINITY);
            job.keep_trailing_whitespace = true;
            ui.fonts_mut(|f| f.layout_job(job))
        };
        let mut changed = false;
        let saving = self.own.as_ref().is_some_and(|o| o.saving);
        let cap = self.cap();
        // As high as the window leaves room for, in the dialog over Rules and Alerts.
        let room = ui.ctx().content_rect().height() - 420.0;
        let most = if room.is_finite() { room.clamp(120.0, 320.0) } else { 320.0 };
        egui::ScrollArea::both().id_salt("sieve_editor_scroll").max_height(most).auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let behind = ui.painter().add(Shape::Noop);
                let (gutter, _) = ui.allocate_exact_size(vec2(gutter_width, 1.0), Sense::hover());
                let edit = egui::TextEdit::multiline(&mut self.text)
                    .id(id)
                    // Nothing typed while a save is under way: the save that closes the editor
                    // would leave it behind (#2058).
                    .interactive(!saving)
                    .code_editor()
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::symmetric(6, 4))
                    .desired_width(f32::INFINITY)
                    .desired_rows(14)
                    .char_limit(cap)
                    .layouter(&mut layouter);
                let out = edit.show(ui);
                let response = crate::text_menu::field(out.response.response.clone());
                changed = response.changed();
                let field = response.rect;
                let whole = Rect::from_min_max(pos2(gutter.min.x, field.min.y), pos2(field.max.x.max(ui.max_rect().max.x), field.max.y));
                let mut shapes = vec![
                    Shape::rect_filled(whole, 3.0, t.input),
                    Shape::rect_filled(Rect::from_min_max(whole.min, pos2(gutter.max.x, whole.max.y)), 0.0, t.panel),
                ];
                let border = if response.has_focus() { t.accent } else { t.input_border };
                for (i, row) in out.galley.rows.iter().enumerate() {
                    let line = i.saturating_add(1);
                    let r = row.rect().translate(out.galley_pos.to_vec2());
                    let band = Rect::from_min_max(pos2(gutter.max.x, r.min.y), pos2(whole.max.x, r.max.y));
                    let error = errors.contains(&line) || server_errors.contains(&line);
                    if error {
                        shapes.push(Shape::rect_filled(band, 0.0, t.red.gamma_multiply(0.18)));
                    } else if warnings.contains(&line) {
                        shapes.push(Shape::rect_filled(band, 0.0, t.orange.gamma_multiply(0.15)));
                    }
                    let number = if error { t.red } else { t.text_dim };
                    ui.painter().text(pos2(gutter.max.x - 6.0, r.center().y), Align2::RIGHT_CENTER, line.to_string(), font.clone(), number);
                    if moved == Some(line) {
                        ui.scroll_to_rect(band, Some(Align::Center));
                    }
                }
                shapes.push(Shape::rect_stroke(whole, 3.0, egui::Stroke::new(1.0, border), egui::StrokeKind::Inside));
                ui.painter().set(behind, Shape::Vec(shapes));
            });
        });
        changed
    }

    /// The problems found here, a row each; a click puts the cursor where one is.
    fn problems_list(&mut self, ui: &mut Ui, t: &Tokens) {
        if !self.checked_now() {
            return;
        }
        if self.problems.is_empty() {
            ui.label(egui::RichText::new(tl!("MailCraft finds no mistakes in it.")).font(regular(11.5)).color(t.text_dim));
            return;
        }
        let lang = crate::i18n::current();
        let mut goto = None;
        for p in self.problems.iter().take(MAX_LISTED) {
            let said = crate::i18n::fmt(
                tl!("Line {line}, column {column}: {what}"),
                &[("line", &p.line.to_string()), ("column", &p.column.to_string()), ("what", &p.what().in_lang(lang))],
            );
            let said = if p.warning { crate::i18n::fmt(tl!("Warning: {what}"), &[("what", &said)]) } else { said };
            let colour = if p.warning { t.orange } else { t.red };
            let row = ui.add(egui::Button::new(egui::RichText::new(shown(&said, 300)).font(regular(11.5)).color(colour)).frame(false));
            if row.on_hover_text(tl!("Go to this place in the text")).clicked() {
                goto = Some((p.line, p.column));
            }
        }
        if self.problems.len() > MAX_LISTED {
            let more = self.problems.len() - MAX_LISTED;
            ui.label(egui::RichText::new(tln!(more, "and {n} more", "and {n} more")).font(regular(11.5)).color(t.text_dim));
        }
        if goto.is_some() {
            self.goto = goto;
        }
    }

    /// What the server said, when it was asked; its lines that name one of the rule's take the
    /// cursor there.
    fn server_said(&mut self, ui: &mut Ui, t: &Tokens) {
        let Some(said) = &self.server else { return };
        let stale = said.text != self.text;
        let dim = |c: egui::Color32| if stale { t.text_dim } else { c };
        let whole = self.own.is_some() && self.one_rule().is_none();
        let head = match &said.kind {
            Said::Fine if whole => tl!("The server takes this script.").to_string(),
            Said::Fine => tl!("The server takes this rule.").to_string(),
            Said::Warnings if whole => tl!("The server takes this script, with warnings:").to_string(),
            Said::Warnings => tl!("The server takes this rule, with warnings:").to_string(),
            Said::Errors if whole => tl!("The server doesn't take this script:").to_string(),
            Said::Errors => tl!("The server doesn't take this rule:").to_string(),
            Said::NotOffered if whole => tl!("This server can't check a script without keeping it: it checks it when it is saved.").to_string(),
            Said::NotOffered => tl!("This server can't check a rule without keeping it: it checks it when the rule is saved.").to_string(),
            Said::Failed(why) if whole => crate::i18n::with_reason(tl!("The server couldn't check this script"), why),
            Said::Failed(why) => crate::i18n::with_reason(tl!("The server couldn't check this rule"), why),
        };
        let colour = match said.kind {
            Said::Fine => t.green,
            Said::Warnings => t.orange,
            Said::Errors | Said::Failed(_) => t.red,
            Said::NotOffered => t.text_dim,
        };
        ui.add_space(4.0);
        ui.label(egui::RichText::new(shown(&head, 400)).font(regular(11.5)).color(dim(colour)));
        if stale {
            ui.label(egui::RichText::new(tl!("(Of the text as it was: it has changed since.)")).font(regular(11.0)).color(t.text_dim));
        }
        let mut goto = None;
        for l in &said.lines {
            let text = shown(&l.text, 300);
            match l.line {
                Some(line) => {
                    let label = if whole {
                        crate::i18n::fmt(tl!("Line {line}: {said}"), &[("line", &line.to_string()), ("said", &text)])
                    } else {
                        crate::i18n::fmt(tl!("Line {line} of the rule: {said}"), &[("line", &line.to_string()), ("said", &text)])
                    };
                    let row = ui.add(egui::Button::new(egui::RichText::new(label).font(regular(11.5)).color(dim(t.text))).frame(false));
                    if row.on_hover_text(tl!("Go to this place in the text")).clicked() && !stale {
                        goto = Some((line, 1));
                    }
                }
                None => {
                    ui.label(egui::RichText::new(text).font(regular(11.5)).color(dim(t.text)));
                }
            }
        }
        if !whole && matches!(said.kind, Said::Warnings | Said::Errors) && said.lines.iter().any(|l| l.line.is_none()) {
            let note = if self.one_rule().is_some() {
                tl!("The server checked the rule inside your whole script: its line numbers count from the top of that script.")
            } else {
                tl!("The server checked the rule inside MailCraft's whole script: its line numbers count from the top of that script.")
            };
            ui.label(egui::RichText::new(note).font(regular(11.0)).color(t.text_dim));
        }
        if goto.is_some() {
            self.goto = goto;
        }
    }
}

impl SieveEditor {
    /// The head of the editor of one rule of the owner's script: which, what Save does, why the
    /// rule builder can't change it, and a webmail that keeps its own copy.
    fn rule_head(&self, ui: &mut Ui, t: &Tokens) {
        let (Some(own), Some(rule)) = (&self.own, self.one_rule()) else { return };
        let title = crate::i18n::fmt(
            tl!("Rule \"{rule}\" of your Sieve script \"{name}\""),
            &[("rule", &shown(&rule.name, 80)), ("name", &shown(&own.name, 80))],
        );
        ui.label(egui::RichText::new(title).font(crate::theme::semibold(12.5)).color(t.text));
        let dim = |ui: &mut Ui, text: &str| {
            ui.label(egui::RichText::new(text).font(regular(11.5)).color(t.text_dim));
        };
        dim(
            ui,
            tl!(
                "This rule's Sieve alone, as it is in your script. Save puts your text in its place and leaves the rest of the script as it is, only while the script on the server is still the one you opened."
            ),
        );
        if let Some(why) = rule.no_form.filter(|_| !rule.filter) {
            dim(ui, &crate::i18n::fmt(tl!("The rule builder can't change it: {why}."), &[("why", tl!(why))]));
        }
        let note = if rule.filter {
            Some(tl!(
                "Your webmail (RainLoop or SnappyMail) keeps its own copy of this filter's settings, which stay as they are. It may put its version back the next time filters are saved there, and MailCraft changes the filter only as Sieve from now on."
            ))
        } else if own.webmail.is_some() {
            Some(tl!(
                "This script was written by a webmail. It may overwrite or drop the changes you make here the next time its filters are saved there."
            ))
        } else {
            None
        };
        if let Some(note) = note {
            ui.label(egui::RichText::new(note).font(regular(11.5)).color(t.orange));
        }
    }

    /// The mistakes the script would have elsewhere with this rule's text in place.
    fn outside_list(&self, ui: &mut Ui, t: &Tokens) {
        if !self.checked_now() {
            return;
        }
        let lang = crate::i18n::current();
        for p in self.outside.iter().filter(|p| !p.warning).take(MAX_LISTED) {
            let said = crate::i18n::fmt(
                tl!("Line {line} of your script, outside this rule: {what}"),
                &[("line", &p.line.to_string()), ("what", &p.what().in_lang(lang))],
            );
            ui.label(egui::RichText::new(shown(&said, 300)).font(regular(11.5)).color(t.red));
        }
    }
}

/// Whether one of MailCraft's own rules can be edited as Sieve (#2057): kept in the Sieve script on
/// its IMAP account's server, changeable, and said in Sieve.
pub(crate) fn convertible(mb: &Mailbox, r: &mailcraft_model::Rule) -> bool {
    !r.is_sieve()
        && r.keep == mailcraft_model::Keep::Server
        && mb.account(r.account).is_some_and(mailcraft_model::Account::keeps_sieve)
        && mailcraft_engine::cmd::rules::cannot_edit(mb, r).is_none()
        && mailcraft_engine::cmd::sieve_rules::block(mb, r, r.account).is_ok()
}

/// Whether a rule can be edited as Sieve here: one of the owner's own script with some Sieve (not
/// while a change of that script is saved), or one of MailCraft's own ([`convertible`]).
pub(crate) fn text_offered(app: &MailApp, r: &mailcraft_model::Rule) -> bool {
    if mailcraft_engine::cmd::sieve_elsewhere::is_elsewhere(r.id) {
        return !app.sieve_own_busy(r.account) && sieve_blocks::text_offered(&app.session, r.id);
    }
    convertible(&app.session.mailbox, r)
}

/// The index in characters of `line`, `column` (both from 1) in `text`: the end of the line
/// when the column is past it, the end of the text when the line is.
pub(crate) fn char_at(text: &str, line: usize, column: usize) -> usize {
    let mut at = 0usize;
    for (n, l) in text.split('\n').enumerate() {
        let length = l.chars().count();
        if n.saturating_add(1) == line {
            return at.saturating_add(column.saturating_sub(1).min(length));
        }
        at = at.saturating_add(length).saturating_add(1);
    }
    text.chars().count()
}
