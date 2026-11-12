// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Help › Report a Problem, and its button in the title bar beside Copy Diagnostics: the owner
//! says what went wrong, and sends it to the MailCraft team or starts it on GitHub (#956).
//!
//! - **Send report** (a build that names the project's report relay): the title, what was
//!   typed, the version, the platform, the GitHub username when one was given, and the
//!   diagnostics when they are ticked, as the window shows them, go to the relay in one request
//!   through the host ([`crate::Services::report_send`]), which files them privately for the
//!   maintainers. **Only the owner's own click sends**: not `ui.reportProblem` (a script or an
//!   agent can fill the window, `via: "send"`, and no more), not a click the control channel
//!   made, never by itself and never again by itself. Taken, the window says the reference
//!   (`R-<n>`, not a link) and clears what was typed; not taken, it says why in words, keeps the
//!   draft and offers Try again and Open on GitHub.
//! - **Open on GitHub**: MailCraft posts nothing itself. The repository's new-issue page
//!   (public, needs a GitHub account) is opened in the system's browser, filled in through its
//!   address with the problem form's fields (`.github/ISSUE_TEMPLATE/problem.yml`), and the owner
//!   reads it and submits it there. (Reports by email went in #956: they would arrive where
//!   nobody can triage them.)
//! - **The address carries the title, what was typed, the version and the platform, and nothing
//!   else.** Never the diagnostics: they are saved to a file the owner chooses (downloaded, in
//!   the browser) and attached on the page by drag and drop. The address never exceeds
//!   [`URL_MAX`]: what goes into it is cut to fit and that is said ([`clamp`]; the boxes
//!   themselves are left whole where Send report is offered); a script's text that doesn't fit is
//!   refused, never cut.
//! - The diagnostics are [`diagnostics`], the very text Copy Diagnostics copies (its oldest log
//!   lines left out when it is too long for a report: [`fit_diagnostics`]), and the window shows
//!   all of it before anything goes.

use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Ui, vec2};
use mailcraft_model::report::{Field, NotSent, ProblemReport, fit_diagnostics};
use serde_json::{Value, json};

use crate::MailApp;
use crate::files::base64_lite;
use crate::theme::Tokens;
use crate::widgets::primary_button;

/// The longest new-issue address opened: browsers and GitHub refuse or cut longer ones.
pub(crate) const URL_MAX: usize = 8_000;
/// Where problems are reported (the repository's own new-issue page, with its problem form).
const NEW_ISSUE: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/issues/new");
const TEMPLATE: &str = "problem.yml";
/// The most of each box that is kept (a paste of a whole log is not a description).
const TITLE_CHARS: usize = 200;
const TEXT_CHARS: usize = 6_000;
/// The longest GitHub username (39), with room for an `@` and a space.
const USERNAME_CHARS: usize = 41;
pub(crate) const WIDTH: f32 = 600.0;

/// What Copy Diagnostics copies (`ui.copyDiagnostics`): the host's report (the version, the
/// platform and the redacted end of the log), else the version and the platform; then how long
/// the last Send/Receive took, account by account (#276).
pub(crate) fn diagnostics(app: &MailApp) -> String {
    let mut text = match &app.services.diagnostics {
        Some(report) => report(),
        None => format!(
            "### MailCraft diagnostics\n\n- Version: {}\n- Platform: {} {}\n\nNo log is kept here.\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    };
    text.push_str(&crate::sync_panel::timings_report(app));
    text
}

/// The value of a `- Name: value` line of the diagnostics (`Version`, `Platform`).
fn line_of(diagnostics: &str, name: &str) -> Option<String> {
    let prefix = format!("- {name}: ");
    // (At most 120 UTF-16 code units, as the report relay counts.)
    let mut units = 0usize;
    diagnostics.lines().find_map(|l| l.strip_prefix(prefix.as_str())).map(|v| {
        v.chars()
            .take_while(|c| {
                units = units.saturating_add(c.len_utf16());
                units <= mailcraft_model::report::MAX_LINE
            })
            .collect()
    })
}

/// The version and the platform, as the diagnostics give them (else this build's).
pub(crate) fn version_platform(diagnostics: &str) -> (String, String) {
    let version = line_of(diagnostics, "Version").unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    let platform = line_of(diagnostics, "Platform").unwrap_or_else(|| format!("{} {}", std::env::consts::OS, std::env::consts::ARCH));
    (version, platform)
}

/// What is reported, as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Typed {
    pub title: String,
    pub what: String,
    pub expected: String,
}

impl Typed {
    fn from_params(p: &Value) -> Typed {
        let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
        Typed { title: s("title"), what: s("what"), expected: s("expected") }
    }

    /// The issue's "What happened" box: what happened, then what was expected.
    fn body(&self) -> String {
        if self.expected.is_empty() { self.what.clone() } else { format!("{}\n\n**What I expected**\n\n{}", self.what, self.expected) }
    }

    /// The text Send report sends: what happened, then what was expected, in plain words (the
    /// relay shows it as it is, not as Markdown).
    fn plain_body(&self) -> String {
        if self.expected.is_empty() { self.what.clone() } else { format!("{}\n\nWhat I expected:\n{}", self.what, self.expected) }
    }
}

/// The line put in the issue's Diagnostics box when a file was saved to attach there.
const ATTACH_HERE: &str = "Drag the diagnostics file MailCraft saved for this report (mailcraft-diagnostics-….txt) into this box to attach it. Read it first: it is what the Report a Problem window showed.";

/// `text` for a query string: everything but letters, digits and `-._*` as `%XX`, a space as `%20`.
fn encode(text: &str) -> String {
    mailcraft_link::encode_component(text)
}

/// The new-issue page for `typed`, with the problem form chosen and its fields filled: the
/// title, what happened, the version, the platform and, when a file of diagnostics was saved,
/// the line that says to attach it. Nothing else.
pub(crate) fn issue_url(typed: &Typed, attach: bool, version: &str, platform: &str) -> String {
    let mut url = format!("{NEW_ISSUE}?template={TEMPLATE}");
    let mut field = |key: &str, value: &str| {
        if !value.is_empty() {
            url.push('&');
            url.push_str(key);
            url.push('=');
            url.push_str(&encode(value));
        }
    };
    field("title", &typed.title);
    field("version", version);
    field("platform", platform);
    field("what", &typed.body());
    if attach {
        field("diagnostics", ATTACH_HERE);
    }
    url
}

/// How long `c` is in the address.
fn width(c: char) -> usize {
    let mut b = [0u8; 4];
    encode(c.encode_utf8(&mut b)).len()
}

/// `text` with characters taken off its end until the address is at least `over` shorter.
fn cut(text: &mut String, over: usize) {
    let mut gone = 0;
    while gone < over {
        let Some(c) = text.pop() else { return };
        gone = gone.saturating_add(width(c));
    }
}

/// `typed`, cut until its address is [`URL_MAX`] at most: what was expected first, then what
/// happened, then the title, each from its end. Whether anything was cut.
pub(crate) fn clamp(typed: &mut Typed, attach: bool, version: &str, platform: &str) -> bool {
    let mut cut_any = false;
    // (Each turn takes something off a box that has some, so it ends.)
    for _ in 0..64 {
        let len = issue_url(typed, attach, version, platform).len();
        let Some(over) = len.checked_sub(URL_MAX).filter(|o| *o > 0) else { return cut_any };
        let field = [&mut typed.expected, &mut typed.what, &mut typed.title].into_iter().find(|f| !f.is_empty());
        let Some(field) = field else { return cut_any };
        cut(field, over);
        cut_any = true;
    }
    cut_any
}

/// Whether `ui.reportProblem` with `params` saves a file of the diagnostics (GitHub, with them).
pub(crate) fn saves_file(params: &Value) -> bool {
    params.get("diagnostics").and_then(Value::as_bool) == Some(true) && params.get("via").and_then(Value::as_str).unwrap_or("github") == "github"
}

/// The name the diagnostics file is offered under.
pub(crate) fn file_name(app: &MailApp) -> String {
    format!("mailcraft-diagnostics-{}.txt", app.session.today().iso())
}

/// `ui.reportProblem` (for scripts and the window's buttons; by hand without `what`, the window
/// opens instead: [`crate::dialogs::input_dialog`]).
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    (id == "ui.reportProblem").then(|| report(app, p))
}

fn report(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let typed = Typed::from_params(p);
    if typed.what.is_empty() {
        return Err(tl!("Say what happened (`what`)").to_string());
    }
    if typed.title.chars().count() > TITLE_CHARS || typed.what.chars().count() > TEXT_CHARS || typed.expected.chars().count() > TEXT_CHARS {
        return Err(tl!("That is too long for a report: shorten it").to_string());
    }
    let with_diagnostics = p.get("diagnostics").and_then(Value::as_bool).unwrap_or(false);
    let open = p.get("open").and_then(Value::as_bool).unwrap_or(true);
    match p.get("via").and_then(Value::as_str).unwrap_or("github") {
        "github" => on_github(app, &typed, with_diagnostics, p.get("path").and_then(Value::as_str), open),
        // A script or an agent may fill the window; only the owner's click on Send report sends.
        "send" => draft(app, p),
        _ => Err("`via` is github or send".into()),
    }
}

/// `via: "send"`: the window, filled in with what `p` says, for the owner to read and send
/// themselves. Nothing is sent.
/// Refused while any window is open over the main one, so that nothing the owner is writing (or
/// a report on its way) is replaced.
fn draft(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    if app.dialog.is_some() {
        return Err(tl!("Another window is open: the report wasn't filled in.").to_string());
    }
    let draft = Draft::new(app, p);
    app.dialog = Some(crate::dialogs::Dialog::ReportProblem(Box::new(draft)));
    app.sync_panel = None;
    Ok(json!({"via": "send", "draft": true, "sent": false}))
}

fn on_github(app: &mut MailApp, typed: &Typed, with_diagnostics: bool, path: Option<&str>, open: bool) -> Result<Value, String> {
    // (Fitted as the window shows them, so the file holds what the window showed.)
    let text = fit_diagnostics(&diagnostics(app));
    let (version, platform) = version_platform(&text);
    let url = issue_url(typed, with_diagnostics, &version, &platform);
    if url.len() > URL_MAX {
        // Never cut behind the owner's back: the window cuts, and says so, before it gets here.
        return Err(tl!("What you wrote is too long for a link to GitHub: shorten it, and add the rest on GitHub").to_string());
    }
    let mut answer = json!({"via": "github", "url": url, "length": url.len()});
    let ctx = app.ctx.clone();
    if open && ctx.is_none() {
        return Err(tl!("The window isn't up yet.").to_string());
    }
    if with_diagnostics {
        let name = file_name(app);
        match path {
            Some(path) if open => {
                save(path, &text)?;
                answer["saved"] = json!(path);
            }
            Some(_) => {}
            // No file named: the bytes are the answer (the browser downloads them).
            None => {
                answer["name"] = json!(name);
                answer["data"] = json!(base64_lite::encode(text.as_bytes()));
            }
        }
    }
    if let (true, Some(ctx)) = (open, ctx) {
        ctx.open_url(egui::OpenUrl::new_tab(url));
        app.status(if with_diagnostics {
            tl!("GitHub's page is open: attach the diagnostics file there, then submit. MailCraft sent nothing.")
        } else {
            tl!("GitHub's page is open: check it, then submit. MailCraft sent nothing.")
        });
    }
    Ok(answer)
}

/// Write the diagnostics to `path`, whole or not at all.
#[cfg(not(target_arch = "wasm32"))]
fn save(path: &str, text: &str) -> Result<(), String> {
    mailcraft_engine::store::write_atomic(std::path::Path::new(path), text.as_bytes())
        .map_err(|e| crate::i18n::fmt(tl!("The diagnostics couldn't be saved: {error}"), &[("error", &e.to_string())]))
}

/// The browser names no file on disk: it downloads the bytes instead (no `path`).
#[cfg(target_arch = "wasm32")]
fn save(_path: &str, _text: &str) -> Result<(), String> {
    Err(tl!("File dialogs aren't available here").to_string())
}

/// What the Report a Problem window holds while it is open.
pub struct Draft {
    pub(crate) typed: Typed,
    /// A GitHub username to mention if the report is made public (Send report only).
    pub(crate) username: String,
    pub(crate) include: bool,
    /// The diagnostics, read once when the window opened, as Copy Diagnostics has them (fitted to
    /// the relay's room when they are very long: [`fit_diagnostics`]). What is shown is what goes.
    pub(crate) text: String,
    /// What was typed was cut to fit an address.
    pub(crate) cut: bool,
    pub(crate) error: String,
    /// The save dialog for the diagnostics is open: the window stays, and closes once the file
    /// is saved (`files::poll`), so that nothing typed is lost to a cancelled dialog.
    pub(crate) waiting: bool,
    /// What goes to GitHub was cut to fit its address while the boxes keep it all: the window
    /// stays open once the page is (`files::poll` too), so that the rest can be copied.
    pub(crate) keep_open: bool,
    /// Send report was pressed: the relay's answer comes through this.
    pub(crate) sending: Option<Receiver<Result<u64, NotSent>>>,
    /// The relay took the report under this reference; the draft is cleared.
    pub(crate) sent: Option<u64>,
    /// Why the last Send report didn't go, or wasn't taken. The draft is kept.
    pub(crate) not_sent: Option<NotSent>,
}

fn not_saved() -> String {
    tl!("The diagnostics weren't saved, so GitHub's page wasn't opened. Try again, or untick Include diagnostics.").to_string()
}

/// Why the report can't be sent as it is, found here before anything went.
pub(crate) fn why_not_sendable(field: Field) -> &'static str {
    match field {
        Field::Version | Field::Platform | Field::Request => {
            tl!("This report is too long to send: shorten what you wrote, or untick Include diagnostics.")
        }
        field => why_not_sent(NotSent::Refused(field)),
    }
}

/// Why a report wasn't sent, in words, with what the owner can do.
pub(crate) fn why_not_sent(why: NotSent) -> &'static str {
    match why {
        NotSent::Refused(Field::Title) => tl!("Give the report a title, on one line and in 200 characters at most."),
        NotSent::Refused(Field::Text) => tl!("Say what happened in 6,000 characters at most, what you expected included."),
        NotSent::Refused(Field::Username) => {
            tl!("That isn't a GitHub username: letters, digits and single hyphens, 39 at most. Leave the box empty if you have none.")
        }
        NotSent::Refused(Field::Diagnostics) => tl!("The diagnostics are too long for a report. Untick Include diagnostics, or open it on GitHub."),
        NotSent::Refused(Field::Version | Field::Platform | Field::Request) => {
            tl!("The report service didn't take the report as it is. Shorten it and try again, or open it on GitHub.")
        }
        NotSent::RateLimited => tl!("The report service has taken as many reports as it can for now. Try again later, or open it on GitHub."),
        NotSent::Disabled => tl!("The report service isn't taking reports just now. Try again later, or open it on GitHub."),
        NotSent::Unavailable => tl!("The report service isn't working just now. Try again later, or open it on GitHub."),
        NotSent::Failed => tl!("The report service couldn't file the report. Try again, or open it on GitHub."),
        NotSent::Unreachable => tl!("The report service couldn't be reached. Check that you're online and try again, or open it on GitHub."),
        NotSent::TooSlow => tl!("The report service didn't answer in time, so the report may have arrived. Try again later, or open it on GitHub."),
        NotSent::Garbled => {
            tl!(
                "The report service's answer wasn't one MailCraft understands, so the report may have arrived. Try again later, or open it on GitHub."
            )
        }
    }
}

impl Draft {
    pub(crate) fn new(app: &MailApp, params: &Value) -> Draft {
        let mut typed = Typed::from_params(params);
        // (A script's or an agent's text is held to what the boxes hold.)
        typed.title = typed.title.chars().take(TITLE_CHARS).collect();
        typed.what = typed.what.chars().take(TEXT_CHARS).collect();
        typed.expected = typed.expected.chars().take(TEXT_CHARS).collect();
        let username: String = params.get("username").and_then(Value::as_str).unwrap_or_default().trim().chars().take(USERNAME_CHARS).collect();
        Draft {
            typed,
            username,
            include: params.get("diagnostics").and_then(Value::as_bool).unwrap_or(true),
            text: fit_diagnostics(&diagnostics(app)),
            cut: false,
            error: String::new(),
            waiting: false,
            keep_open: false,
            sending: None,
            sent: None,
            not_sent: None,
        }
    }

    /// Something is written in it, or a report is on its way: nothing else may take its place.
    pub(crate) fn busy(&self) -> bool {
        self.sending.is_some() || !self.typed.title.trim().is_empty() || !self.typed.what.trim().is_empty() || !self.typed.expected.trim().is_empty()
    }

    /// The params of `ui.reportProblem` for `via`, with `typed` as what was typed.
    fn params_of(&self, typed: &Typed, via: &str) -> Value {
        json!({"title": typed.title, "what": typed.what, "expected": typed.expected, "diagnostics": self.include, "via": via})
    }

    /// The report Send report sends: what the window shows, and nothing else.
    pub(crate) fn report(&self) -> ProblemReport {
        let (version, platform) = version_platform(&self.text);
        let diagnostics = self.include.then_some(self.text.as_str());
        ProblemReport::new(&self.typed.title, &self.typed.plain_body(), &version, &platform, &self.username, diagnostics)
    }

    /// Send the report through the host, once: the owner pressed Send report (or Try again).
    /// A report the relay would refuse is refused here, said as that, and nothing goes.
    fn send(&mut self, app: &MailApp) {
        let report = self.report();
        self.error.clear();
        if let Err(field) = report.json() {
            self.not_sent = None;
            self.error = why_not_sendable(field).to_string();
            return;
        }
        let Some(send) = &app.services.report_send else { return };
        self.not_sent = None;
        self.sending = Some(send(report));
    }

    /// The relay's answer, once it has come: sent clears the draft, anything else keeps it.
    fn poll(&mut self) {
        let Some(answer) = &self.sending else { return };
        let got = match answer.try_recv() {
            Ok(got) => got,
            Err(TryRecvError::Empty) => return,
            // (The host's thread ended without a word: it may have sent the report.)
            Err(TryRecvError::Disconnected) => Err(NotSent::Garbled),
        };
        self.sending = None;
        match got {
            Ok(reference) => {
                self.sent = Some(reference);
                self.not_sent = None;
                self.typed = Typed::default();
                self.username.clear();
                self.cut = false;
                self.keep_open = false;
            }
            Err(why) => self.not_sent = Some(why),
        }
    }

    /// Lay the window out; `true` when it is to close.
    pub(crate) fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        self.poll();
        if let Some(reference) = self.sent {
            return show_sent(ui, reference);
        }
        // Send report, in a build that names the project's relay; Open on GitHub alone otherwise.
        let can_send = app.services.report_send.is_some();
        let sending = self.sending.is_some();
        let t = Tokens::get(ui.ctx());
        let dim = |s: &str| egui::RichText::new(s.to_string()).color(t.text_dim).size(11.5);
        ui.add_enabled_ui(!sending, |ui| {
            if can_send {
                ui.label(tl!("Say what went wrong. Nothing is sent until you press Send report, and then only what this window shows."));
            } else {
                ui.label(tl!("Say what went wrong. MailCraft sends nothing by itself: you check the report and send it."));
                ui.label(
                    egui::RichText::new(tl!(
                        "GitHub issues are public: don't put email addresses, names or anything from your messages in them. Describe what happened in your own words."
                    ))
                    .color(t.red),
                );
            }
            ui.label(tl!("Title"));
            crate::text_menu::field(ui.add(egui::TextEdit::singleline(&mut self.typed.title).char_limit(TITLE_CHARS).desired_width(f32::INFINITY)));
            ui.label(tl!("What happened"));
            let what = ui.add(egui::TextEdit::multiline(&mut self.typed.what).char_limit(TEXT_CHARS).desired_rows(4).desired_width(f32::INFINITY));
            crate::text_menu::field(what);
            ui.label(tl!("What you expected (optional)"));
            let expected =
                ui.add(egui::TextEdit::multiline(&mut self.typed.expected).char_limit(TEXT_CHARS).desired_rows(2).desired_width(f32::INFINITY));
            crate::text_menu::field(expected);
            if can_send {
                ui.label(tl!("GitHub username (optional)"));
                let name = egui::TextEdit::singleline(&mut self.username).char_limit(USERNAME_CHARS).desired_width(240.0);
                crate::text_menu::field(ui.add(name));
                ui.label(dim(tl!("Only to mention you if we make the report public on GitHub.")));
                // The relay counts what happened and what was expected together, as UTF-16.
                let units = self.typed.plain_body().trim().encode_utf16().count();
                if units > mailcraft_model::report::MAX_TEXT {
                    let over = crate::i18n::fmt(
                        tl!("What happened and what you expected come to {n} characters together: a report takes {max}."),
                        &[("n", &units.to_string()), ("max", &mailcraft_model::report::MAX_TEXT.to_string())],
                    );
                    ui.label(egui::RichText::new(over).color(t.orange).size(11.5));
                }
            } else {
                // What GitHub's address can carry: the boxes are cut to it, and that is said.
                let (version, platform) = version_platform(&self.text);
                self.cut |= clamp(&mut self.typed, self.include, &version, &platform);
            }
            if self.cut {
                let said = if can_send {
                    tl!("GitHub's page has as much as a link can carry: copy the rest from here into it.")
                } else {
                    tl!("That's as much as a link to GitHub can carry: add the rest on GitHub once the page is open.")
                };
                ui.label(egui::RichText::new(said).color(t.orange).size(11.5));
            }
            ui.checkbox(&mut self.include, tl!("Include diagnostics"));
            if self.include {
                egui::ScrollArea::vertical().max_height(150.0).auto_shrink([false, true]).show(ui, |ui| {
                    let shown =
                        ui.add(egui::TextEdit::multiline(&mut self.text.as_str()).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                    crate::text_menu::read_only(shown);
                });
                ui.label(dim(if can_send {
                    tl!("Send report sends them as shown. Open on GitHub saves them to a file you attach there; they are never put in the link.")
                } else {
                    tl!("They are saved to a file you choose, which you attach to the issue on GitHub. They are never put in the link.")
                }));
            }
        });
        if can_send {
            ui.add_space(2.0);
            ui.label(tl!(
                "Sent to the MailCraft team privately. If it's a real problem we may make it public on GitHub, with what you wrote and the diagnostics, so we can track it. Leave out anything private."
            ));
        } else {
            ui.label(dim(tl!("GitHub is public and needs a GitHub account.")));
        }
        if let Some(why) = self.not_sent {
            ui.label(egui::RichText::new(why_not_sent(why)).color(t.red));
        }
        if !self.error.is_empty() {
            ui.label(egui::RichText::new(self.error.as_str()).color(t.red));
        }
        if self.waiting && app.file_dialog.is_none() {
            // Still here once the dialog is gone: the file wasn't saved.
            self.waiting = false;
            self.error = not_saved();
        }
        if self.waiting {
            ui.label(dim(tl!("Choose where to save the diagnostics: GitHub's page opens once they are saved.")));
        }
        if sending {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(dim(tl!("Sending the report…")));
            });
        }
        let ready = !self.typed.what.trim().is_empty() && !self.waiting && !sending;
        let mut github = false;
        let mut send = false;
        let mut cancel = false;
        ui.add_space(4.0);
        crate::dialogs::button_row(ui, |ui| {
            // (Not while the report is on its way: its answer is waited for, and said.)
            cancel = ui.add_enabled(!sending, egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0))).clicked();
            let github_tip = tl!("Opens GitHub's page in your browser, filled in: you check it and submit it there.");
            if can_send {
                let label = if self.not_sent.is_some() { tl!("Try again") } else { tl!("Send report") };
                let pressed = primary_button(ui, label, ready).on_hover_text(tl!("Sends the report to the MailCraft team, as this window shows it."));
                // Only the owner's own click sends: never a click a script or an agent made.
                send = pressed.clicked() && ready && !app.driven;
                let open = ui.add_enabled(ready, egui::Button::new(tl!("Open on GitHub")).min_size(vec2(72.0, 26.0))).on_hover_text(github_tip);
                github = open.clicked() && ready;
            } else {
                github = primary_button(ui, tl!("Open on GitHub"), ready).on_hover_text(github_tip).clicked() && ready;
            }
        });
        if send {
            self.send(app);
            return false;
        }
        if !github {
            return cancel;
        }
        // Where Send report is offered, the boxes are left whole for it: only what goes into
        // GitHub's address is cut, and the window stays open with the rest when it is.
        let mut typed = self.typed.clone();
        let (version, platform) = version_platform(&self.text);
        let cut = can_send && clamp(&mut typed, self.include, &version, &platform);
        match app.run("ui.reportProblem", self.params_of(&typed, "github")) {
            Ok(v) if v.get("pending").is_some() => {
                self.waiting = true;
                self.keep_open = cut;
                self.error.clear();
                false
            }
            Ok(v) if v.get("cancelled").is_some() => {
                self.error = not_saved();
                false
            }
            Ok(_) if cut => {
                self.cut = true;
                false
            }
            Ok(_) => true,
            Err(e) => {
                self.error = e;
                false
            }
        }
    }
}

/// The window once the relay took the report: its reference (not a link), and Close.
fn show_sent(ui: &mut Ui, reference: u64) -> bool {
    let t = Tokens::get(ui.ctx());
    ui.add_space(6.0);
    let sent = crate::i18n::fmt(tl!("Sent, reference R-{reference}."), &[("reference", &reference.to_string())]);
    ui.label(egui::RichText::new(sent).size(15.0).strong());
    ui.add_space(4.0);
    ui.label(tl!("Thank you. The MailCraft team has it under this reference."));
    ui.label(egui::RichText::new(tl!("What you wrote has been cleared from this window.")).color(t.text_dim).size(11.5));
    ui.add_space(8.0);
    let mut close = false;
    crate::dialogs::button_row(ui, |ui| close = primary_button(ui, tl!("Close"), true).clicked());
    close
}

#[cfg(test)]
mod tests;
