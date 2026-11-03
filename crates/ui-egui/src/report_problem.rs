// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Help › Report a Problem, and its button in the title bar beside Copy Diagnostics: the owner
//! says what went wrong and MailCraft starts the report for them, two ways.
//!
//! - **MailCraft posts nothing itself.** On GitHub (public, needs a GitHub account) the
//!   repository's new-issue page is opened in the system's browser, filled in through its
//!   address with the problem form's fields (`.github/ISSUE_TEMPLATE/problem.yml`), and the
//!   owner reads it and submits it there. By email (private, from the owner's own account) a new
//!   message to the maintainers is started in MailCraft's own composer, and nothing goes until
//!   the owner presses Send. No token, no request, no other program.
//! - **The address carries the title, what was typed, the version and the platform, and nothing
//!   else.** Never the diagnostics: they are saved to a file the owner chooses (downloaded, in
//!   the browser) and attached on the page by drag and drop; by email they are attached to the
//!   message from memory. The address never exceeds [`URL_MAX`]: the window cuts what is typed
//!   to fit and says so ([`clamp`]); a script's text that doesn't fit is refused, never cut.
//! - The diagnostics are [`diagnostics`], the very text Copy Diagnostics copies, and the window
//!   shows all of it before anything is opened.

use egui::{Ui, vec2};
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
/// The maintainers' address, read from `funding.json` when this was built (`build.rs`); empty
/// when it couldn't be.
pub(crate) const MAINTAINERS: &str = env!("MAILCRAFT_MAINTAINER_EMAIL");
/// The most of each box that is kept (a paste of a whole log is not a description).
const TITLE_CHARS: usize = 200;
const TEXT_CHARS: usize = 6_000;
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
    diagnostics.lines().find_map(|l| l.strip_prefix(prefix.as_str())).map(|v| v.chars().take(120).collect())
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

/// The account a report by email is written from: the one in view when it sends mail, else
/// the first that does (never a shared mailbox, which sends only with a permission of its own).
fn sender(app: &MailApp) -> Option<mailcraft_model::Id> {
    let mb = &app.session.mailbox;
    let sends = |a: &&mailcraft_model::Account| a.kind.sends_mail() && !a.is_shared();
    let in_view = mb.folder(app.session.view.folder).and_then(|f| mb.account(f.account)).filter(sends);
    in_view.or_else(|| mb.accounts.iter().find(sends)).map(|a| a.id)
}

/// Why a report can't go by email now; `None` when it can.
pub(crate) fn email_refused(app: &MailApp) -> Option<String> {
    if MAINTAINERS.is_empty() {
        Some(tl!("This copy of MailCraft has no address to send reports to.").to_string())
    } else if sender(app).is_none() {
        Some(tl!("Add an account that sends mail first (File › Add Account).").to_string())
    } else if app.session.compose.is_some() {
        Some(tl!("Finish or close the message you're writing first.").to_string())
    } else {
        None
    }
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
        "email" => by_email(app, &typed, with_diagnostics, open),
        _ => Err("`via` is github or email".into()),
    }
}

fn on_github(app: &mut MailApp, typed: &Typed, with_diagnostics: bool, path: Option<&str>, open: bool) -> Result<Value, String> {
    let text = diagnostics(app);
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

/// The body of a report by email: what happened, what was expected, the version and the platform.
pub(crate) fn email_body(typed: &Typed, attached: bool, version: &str, platform: &str) -> String {
    let mut body = typed.what.clone();
    if !typed.expected.is_empty() {
        body.push_str(&format!("\n\nWhat I expected:\n{}", typed.expected));
    }
    body.push_str(&format!("\n\nMailCraft version: {version}\nPlatform: {platform}\n"));
    if attached {
        body.push_str("The diagnostics are attached.\n");
    }
    body
}

fn by_email(app: &mut MailApp, typed: &Typed, with_diagnostics: bool, open: bool) -> Result<Value, String> {
    if let Some(why) = email_refused(app) {
        return Err(why);
    }
    let text = diagnostics(app);
    let (version, platform) = version_platform(&text);
    let subject = if typed.title.is_empty() { "MailCraft problem report".to_string() } else { typed.title.clone() };
    let body = email_body(typed, with_diagnostics, &version, &platform);
    let name = file_name(app);
    let mut answer = json!({"via": "email", "to": MAINTAINERS, "subject": subject});
    if !open {
        if with_diagnostics {
            answer["attached"] = json!(name);
        }
        return Ok(answer);
    }
    let account = sender(app);
    app.execute("mail.new", json!({"to": MAINTAINERS, "subject": subject, "body": body, "account": account}))?;
    if with_diagnostics {
        let attached = app.execute("compose.attach", json!({"name": name, "data": base64_lite::encode(text.as_bytes()), "mime": "text/plain"}));
        if let Err(e) = attached {
            // A message without the file it says it has is not left behind.
            let _ = app.execute("compose.close", json!({}));
            return Err(e);
        }
        answer["attached"] = json!(name);
    }
    app.status(tl!("Your report is ready in a new message: check it, then press Send."));
    Ok(answer)
}

/// What the Report a Problem window holds while it is open.
pub struct Draft {
    pub(crate) typed: Typed,
    pub(crate) include: bool,
    /// The diagnostics, read once when the window opened, as Copy Diagnostics has them.
    pub(crate) text: String,
    /// What was typed was cut to fit an address.
    pub(crate) cut: bool,
    pub(crate) error: String,
    /// The save dialog for the diagnostics is open: the window stays, and closes once the file
    /// is saved (`files::poll`), so that nothing typed is lost to a cancelled dialog.
    pub(crate) waiting: bool,
}

fn not_saved() -> String {
    tl!("The diagnostics weren't saved, so GitHub's page wasn't opened. Try again, or untick Include diagnostics.").to_string()
}

impl Draft {
    pub(crate) fn new(app: &MailApp, params: &Value) -> Draft {
        let mut typed = Typed::from_params(params);
        typed.title = typed.title.chars().take(TITLE_CHARS).collect();
        Draft { typed, include: true, text: diagnostics(app), cut: false, error: String::new(), waiting: false }
    }

    /// The params of `ui.reportProblem` for `via`.
    pub(crate) fn params(&self, via: &str) -> Value {
        json!({"title": self.typed.title, "what": self.typed.what, "expected": self.typed.expected, "diagnostics": self.include, "via": via})
    }

    /// Lay the window out; `true` when it is to close.
    pub(crate) fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        let dim = |s: &str| egui::RichText::new(s.to_string()).color(t.text_dim).size(11.5);
        ui.label(tl!("Say what went wrong. MailCraft sends nothing by itself: you check the report and send it."));
        ui.label(
            egui::RichText::new(tl!(
                "GitHub issues are public: don't put email addresses, names or anything from your messages in them. Describe what happened in your own words."
            ))
            .color(t.red),
        );
        ui.label(tl!("Title"));
        crate::text_menu::field(ui.add(egui::TextEdit::singleline(&mut self.typed.title).char_limit(TITLE_CHARS).desired_width(f32::INFINITY)));
        ui.label(tl!("What happened"));
        let what = ui.add(egui::TextEdit::multiline(&mut self.typed.what).char_limit(TEXT_CHARS).desired_rows(4).desired_width(f32::INFINITY));
        crate::text_menu::field(what);
        ui.label(tl!("What you expected (optional)"));
        let expected =
            ui.add(egui::TextEdit::multiline(&mut self.typed.expected).char_limit(TEXT_CHARS).desired_rows(2).desired_width(f32::INFINITY));
        crate::text_menu::field(expected);
        // What GitHub's address can carry: the boxes are cut to it, and that is said.
        let (version, platform) = version_platform(&self.text);
        self.cut |= clamp(&mut self.typed, self.include, &version, &platform);
        if self.cut {
            ui.label(
                egui::RichText::new(tl!("That's as much as a link to GitHub can carry: add the rest on GitHub once the page is open."))
                    .color(t.orange)
                    .size(11.5),
            );
        }
        ui.checkbox(&mut self.include, tl!("Include diagnostics"));
        if self.include {
            egui::ScrollArea::vertical().max_height(150.0).auto_shrink([false, true]).show(ui, |ui| {
                let shown = ui.add(egui::TextEdit::multiline(&mut self.text.as_str()).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                crate::text_menu::read_only(shown);
            });
            ui.label(dim(tl!(
                "On GitHub they are saved to a file you choose, which you attach to the issue; by email they are attached to the message. They are never put in the link."
            )));
        }
        ui.label(dim(tl!(
            "GitHub is public and needs a GitHub account. Email is private and goes from your own account to MailCraft's maintainers."
        )));
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
        let ready = !self.typed.what.trim().is_empty() && !self.waiting;
        let email_no = email_refused(app);
        let mut via = None;
        let mut cancel = false;
        ui.add_space(4.0);
        crate::dialogs::button_row(ui, |ui| {
            cancel = ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0))).clicked();
            let mail = ui.add_enabled(ready && email_no.is_none(), egui::Button::new(tl!("Send by Email")).min_size(vec2(96.0, 26.0)));
            let mail = match &email_no {
                Some(why) => mail.on_disabled_hover_text(why.as_str()),
                None => mail.on_hover_text(tl!("A new message to MailCraft's maintainers, from your account: nothing is sent until you press Send.")),
            };
            if mail.clicked() {
                via = Some("email");
            }
            let github = primary_button(ui, tl!("Open on GitHub"), ready)
                .on_hover_text(tl!("Opens GitHub's page in your browser, filled in: you check it and submit it there."));
            if github.clicked() && ready {
                via = Some("github");
            }
        });
        if let Some(why) = email_no.filter(|_| ready) {
            ui.label(dim(&why));
        }
        let Some(via) = via else { return cancel };
        match app.run("ui.reportProblem", self.params(via)) {
            Ok(v) if v.get("pending").is_some() => {
                self.waiting = true;
                self.error.clear();
                false
            }
            Ok(v) if v.get("cancelled").is_some() => {
                self.error = not_saved();
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

#[cfg(test)]
mod tests;
