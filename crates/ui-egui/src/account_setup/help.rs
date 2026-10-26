// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What Add Account says about a sign-in on Microsoft's page: while the browser has it, when it
//! ended without signing anybody in, and when the organisation has to approve MailCraft first
//! ([`crate::approval`]).
//!
//! Many organisations let only an administrator approve a new program. Microsoft's page then
//! says so instead of signing the owner in, and nothing may ever come back from it. So the
//! waiting page says what to do about that from the start, the same things are on a page of
//! their own ([`Page::Approval`]), and a sign-in that ended says how it ended, in a sentence,
//! with the way to that page. An account that exists opens that page by itself, as a dialog
//! (`ui.approvalHelp`), when its sign-in is refused for the same reason.
//!
//! The pages only collect what the owner wants: an administrator's approval is asked for by
//! the host ([`MailApp::begin_admin_consent`]), the message for one is copied by
//! `ui.copyAdminMessage`'s own code, and a sign-in is started as any other.

use super::*;
use crate::approval::{AdminConsentAnswer, Whose};

/// The approval page: what it is doing, and what it has to say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Approval {
    /// Microsoft's admin-consent page is open in the browser, for this dialog.
    pub(super) approving: bool,
    /// Microsoft's page said that an approval is needed (a sign-in came back with it), rather
    /// than the owner who went to look.
    pub(super) said: bool,
    /// What the last try at an administrator's approval came to: in our words, and Microsoft's.
    pub(super) refused: Option<(String, String)>,
}

/// What a command asks of the dialog while it is at a sign-in with Microsoft (`ui.approvalHelp`,
/// `ui.copyAdminMessage`, `ui.adminConsent`, `ui.signInAgain`): what its own links and buttons do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Help {
    /// Show the approval page.
    Open,
    /// Copy the message for an administrator.
    Copy,
    /// Open Microsoft's approval page in the browser, and wait for its answer.
    Administrator,
    /// Sign in again.
    Again,
}

const WAITING_HELP: &str = "Your organisation lets only an administrator approve a new program. Nothing is wrong with your account or your password, and there is nothing to wait for here.";
const WHY: &str = "Your organisation lets only an administrator approve a new program, and none has approved MailCraft yet. Nothing is wrong with your account or your password.";
const ENDED_HELP: &str =
    "Your organisation then lets only an administrator approve a new program. Nothing is wrong with your account or your password.";
const DECLINED_HELP: &str = "You came back from Microsoft's page without having signed in: you said no there, or the page sent you back.";
const TIMED_OUT_HELP: &str = "The page was closed, or five minutes went by before the sign-in was finished.";
/// Said while the browser has the sign-in: some systems can't tell the program that no browser
/// could be started.
const NO_BROWSER_OPENED: &str = "If no browser opened, check that this computer has a default browser (on Windows: Settings, Apps, Default apps), then press Open the page again.";
const NO_BROWSER_HELP: &str = "Signing in happens on Microsoft's page in a web browser, and none could be started on this computer. Check that it has a default browser, then try again.";

impl AccountSetup {
    /// The page that says what to do when its organisation has to approve MailCraft first, for
    /// an account that exists. `None` for one that doesn't sign in with Microsoft.
    pub(super) fn approval_for(app: &MailApp, id: Id) -> Option<AccountSetup> {
        let account = app.session.mailbox.account(id).filter(|_| app.is_microsoft(id))?;
        let (oauth, organisation) = app.approval_of(id).ok()?;
        let mut w = AccountSetup::blank(Page::Approval);
        (w.email, w.oauth, w.existing, w.organisation) = (line(&account.email, LINE), Some(oauth), id, organisation);
        w.focus = false;
        Some(w)
    }

    /// Whether the dialog is at a sign-in with Microsoft: waiting for one, saying how one ended,
    /// or saying what to do about an approval.
    pub(crate) fn at_sign_in(&self) -> bool {
        self.oauth.is_some() && matches!(self.page, Page::SignIn | Page::Approval)
    }

    /// The account the approval page is about: one that exists, or 0 for the one being set up.
    pub(crate) fn approval_account(&self) -> Id {
        self.existing
    }

    /// Whose approval is asked for from here.
    fn whose(&self) -> Whose {
        Whose { account: self.existing, oauth: self.oauth.clone().unwrap_or_default(), tenant: self.directory() }
    }

    /// The directory of the organisation an approval is asked of, where it can be known. For an
    /// account that exists: what a sign-in or its settings said. For one that is being set up:
    /// the directory its settings name, else the address's own domain (an organisation's mail
    /// domain is a name of its directory; an address at a provider of personal mail has no
    /// organisation). Else empty: the administrator who signs in says which it is.
    fn directory(&self) -> String {
        if self.existing != 0 {
            return self.organisation.clone();
        }
        let named = self.oauth.as_ref().map_or("", |oauth| oauth.tenant.trim());
        if Directory::of_tenant(named) == Directory::Organisation && !named.eq_ignore_ascii_case("organizations") {
            return named.to_string();
        }
        let edge = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
        match address(&self.email) {
            Some((_, domain))
                if self.service != "Outlook.com"
                    && provider::by_domain(domain).is_none()
                    && edge(domain.chars().next())
                    && edge(domain.chars().next_back()) =>
            {
                domain.to_ascii_lowercase()
            }
            _ => String::new(),
        }
    }

    /// Show the approval page. `said`: Microsoft's page said that an approval is needed.
    pub(super) fn open_approval(&mut self, said: bool) {
        self.go(Page::Approval);
        self.focus = false;
        self.approval = Approval { said, ..Approval::default() };
    }

    /// The sign-in the wizard was waiting for brought no tokens: `error` says how it ended.
    pub(super) fn sign_in_ended(&mut self, error: String) {
        let end = SignInEnd::of(&error);
        (self.ended, self.approved) = (Some(end), false);
        // The owner is on the approval page already (and said so before Microsoft's page did):
        // it stays, and what it says stands.
        if self.page == Page::Approval {
            self.approval.said |= end == SignInEnd::Approval;
            return;
        }
        match end {
            SignInEnd::Approval => self.open_approval(true),
            // (Said by the page, in its own words.)
            SignInEnd::Declined | SignInEnd::TimedOut | SignInEnd::NoBrowser => self.error.clear(),
            SignInEnd::Other => self.error = error,
        }
    }

    /// "I'm an administrator": have the host open Microsoft's approval page, and wait for it.
    pub(super) fn ask_administrator(&mut self, app: &mut MailApp) -> Result<(), String> {
        // (Said before anything is given up for it.)
        if let Some(why) = app.admin_consent_unavailable() {
            return Err(why);
        }
        // Microsoft's approval page is open for this dialog already: one at a time.
        if self.approval.approving {
            return Err(tl!("Finish signing in first (your browser is waiting)").to_string());
        }
        // One page of Microsoft's at a time: a sign-in that is still waiting gives way.
        self.stop_sign_in(app);
        let begun = app.begin_admin_consent(&self.whose());
        if self.page != Page::Approval {
            self.open_approval(false);
        }
        (self.approval.refused, self.approval.approving) = (None, begun.is_ok());
        self.error.clear();
        begun
    }

    /// Copy the message for an administrator (as `ui.copyAdminMessage` answers).
    pub(super) fn copy_message(&mut self, app: &mut MailApp) -> Result<Value, String> {
        app.copy_admin_message_for(&self.whose())
    }

    /// "Try again", once an administrator has approved: the sign-in afresh. For an account that
    /// exists, what was refused is asked for once more, and the dialog is finished.
    pub(super) fn again(&mut self, app: &mut MailApp) -> Result<Value, String> {
        if self.existing != 0 {
            let asked = app.sign_in_again_for(self.existing)?;
            self.done = true;
            return Ok(asked);
        }
        self.stop_sign_in(app);
        if std::mem::take(&mut self.approval.approving) {
            app.cancel_admin_consent();
        }
        self.approval = Approval::default();
        self.start_sign_in(app);
        if self.waiting { Ok(json!({"pending": "signIn", "account": 0})) } else { Err(self.error.clone()) }
    }

    /// Do what a command asks, as the dialog's own links and buttons do it.
    pub(crate) fn run(&mut self, app: &mut MailApp, what: Help) -> Result<Value, String> {
        if !self.at_sign_in() {
            return Err(tl!("No sign-in with Microsoft is under way").to_string());
        }
        match what {
            Help::Open => {
                if self.page != Page::Approval {
                    self.open_approval(false);
                }
                Ok(json!({"dialog": "account", "page": "approval", "account": self.existing}))
            }
            Help::Copy => self.copy_message(app),
            Help::Administrator => match self.ask_administrator(app) {
                Ok(()) => Ok(json!({"pending": "adminConsent", "account": self.existing})),
                Err(e) => {
                    self.error.clone_from(&e);
                    Err(e)
                }
            },
            Help::Again => self.again(app),
        }
    }

    /// Take in what an administrator's approval came to, once the host knows.
    pub(super) fn poll_approval(&mut self, app: &mut MailApp) {
        match app.take_admin_consent(self.existing) {
            Some(AdminConsentAnswer::Approved) => {
                self.approval = Approval::default();
                if self.existing == 0 {
                    // Said on the page that follows, which signs the account in by itself.
                    self.start_sign_in(app);
                    self.approved = true;
                } else {
                    // (It signs in again by itself: started where the answer was taken in.)
                    self.done = true;
                }
            }
            Some(AdminConsentAnswer::Cancelled) => self.approval.approving = false,
            Some(refused) => (self.approval.approving, self.approval.refused) = (false, Some(refused.said())),
            // Given up from elsewhere (Cancel Sign-In): back to what there is to do.
            None if self.approval.approving && !app.approving() => self.approval.approving = false,
            None => {}
        }
    }

    // ---- the pages ----

    /// The two things an owner can do about an approval that is needed, as buttons across the
    /// page (`details`: each with a sentence about it). Which one was pressed.
    fn approval_steps(&self, app: &mut MailApp, ui: &mut Ui, t: &Tokens, details: bool) -> Option<Action> {
        let about = |text: &'static str| if details { tl!(text) } else { "" };
        let mut action = None;
        let administrator = step(
            ui,
            t,
            tl!("I'm an administrator: approve it for my organisation"),
            about(
                "Opens Microsoft's approval page in your browser. Sign in there with an administrator account and accept; MailCraft carries on by itself.",
            ),
        );
        app.hit.add("approvalAdministrator", self.existing, administrator.rect);
        if administrator.clicked() {
            action = Some(Action::Administrator);
        }
        let copy = step(
            ui,
            t,
            tl!("Copy a message for my administrator"),
            about("A short note with the link to that page, to paste into mail or chat. MailCraft sends nothing itself."),
        );
        app.hit.add("approvalCopyMessage", self.existing, copy.rect);
        if copy.clicked() {
            action = Some(Action::CopyMessage);
        }
        action
    }

    /// The browser has the sign-in. The page says so, and what to do if Microsoft's page says
    /// no instead of signing the owner in: nothing has to come back for that to be said.
    pub(super) fn signing_in_page(&mut self, app: &mut MailApp, ui: &mut Ui, pass: &Pass) -> Option<Action> {
        let t = pass.t;
        let provider = self.oauth.as_ref().map(|o| o.provider.label()).unwrap_or_default();
        let who = shown(self.email.trim(), 120);
        let mut action = None;
        page(ui, &t, |ui| {
            if self.approved {
                note(ui, tl!("Your organisation has approved MailCraft. Now sign in with your own account."), t.green);
            }
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                ui.add(egui::Spinner::new().size(30.0).color(t.accent));
                ui.add_space(8.0);
                let open = fmt(tl!("{provider}'s sign-in page is open in your browser"), &[("provider", provider)]);
                ui.label(RichText::new(open).font(medium(13.5)).color(t.text));
                let then = fmt(tl!("Sign in there as {address}. This window carries on by itself once you have."), &[("address", &who)]);
                ui.label(RichText::new(then).color(t.text_dim));
                // Windows' opener can't say that no browser was started, so this is said always.
                ui.label(RichText::new(tl!(NO_BROWSER_OPENED)).color(t.text_dim));
            });
            ui.add_space(8.0);
            rule(ui, &t);
            ui.label(RichText::new(tl!("If that page says you need admin approval")).font(semibold(13.0)).color(t.text));
            note(ui, tl!(WAITING_HELP), t.text);
            action = self.approval_steps(app, ui, &t, false);
            ui.horizontal_wrapped(|ui| {
                let more = ui.link(tl!("More about this"));
                app.hit.add("signInApprovalHelp", self.existing, more.rect);
                if more.clicked() {
                    action = Some(Action::Approval);
                }
                // The way out when the provider's page shows an error of another kind.
                if !self.mail_only {
                    let hint = tl!("Asks for mail alone, without the calendar and contacts, which can be connected later");
                    if ui.link(tl!("Trouble on that page? Sign in for mail only")).on_hover_text(hint).clicked() {
                        action = Some(Action::MailOnly);
                    }
                }
            });
            error(ui, &self.error);
        });
        let (mut cancel, mut again) = (false, false);
        strip(ui, self.page, |ui| {
            cancel = button(ui, tl!("Cancel"), false, true).clicked();
            let reopen = button(ui, tl!("Open the page again"), false, true)
                .on_hover_text(tl!("Closed the page? This opens Microsoft's sign-in page afresh."));
            app.hit.add("signInTryAgain", self.existing, reopen.rect);
            again = reopen.clicked();
        });
        if cancel {
            Some(Action::Back)
        } else if again {
            Some(Action::Go)
        } else {
            action
        }
    }

    /// A sign-in that ended without signing anybody in: how, in a sentence of the window's own
    /// (or in Microsoft's words, when it is nothing the window knows), the way to try again,
    /// and the way to the page about an approval.
    ///
    /// A page that came back with a no, or didn't come back, may have been the one that asks
    /// for an administrator: Microsoft sends the same no for the way back from that page as for
    /// the owner's own, and nothing at all for a tab that was closed. So what to do about an
    /// approval is on this page itself then, under what happened.
    pub(super) fn sign_in_ended_page(&mut self, app: &mut MailApp, ui: &mut Ui, pass: &Pass) -> Option<Action> {
        let t = pass.t;
        let (title, text) = match self.ended {
            Some(SignInEnd::Declined) => (crate::approval::SIGN_IN_DECLINED, DECLINED_HELP),
            Some(SignInEnd::TimedOut) => (crate::approval::SIGN_IN_TIMED_OUT, TIMED_OUT_HELP),
            Some(SignInEnd::NoBrowser) => (crate::approval::NO_BROWSER, NO_BROWSER_HELP),
            _ => ("Signing in didn't work", ""),
        };
        let may_need_approval = matches!(self.ended, Some(SignInEnd::Declined | SignInEnd::TimedOut));
        let (mut approval, mut step) = (false, None);
        page(ui, &t, |ui| {
            heading(ui, tl!(title), &t);
            note(ui, &shown(self.email.trim(), 120), t.text_dim);
            if !text.is_empty() {
                note(ui, tl!(text), t.text);
            }
            error(ui, &self.error);
            if may_need_approval {
                rule(ui, &t);
                ui.label(RichText::new(tl!("If that page said you need admin approval")).font(semibold(13.0)).color(t.text));
                note(ui, tl!(ENDED_HELP), t.text);
                step = self.approval_steps(app, ui, &t, false);
                let more = ui.link(tl!("More about this"));
                app.hit.add("signInApprovalHelp", self.existing, more.rect);
                approval = more.clicked();
            } else if self.ended.is_some() {
                // Whatever it was: it may have ended for want of an approval all the same.
                let link = ui.link(tl!("My organisation has to approve MailCraft first: what to do"));
                app.hit.add("signInApprovalHelp", self.existing, link.rect);
                approval = link.clicked();
            }
        });
        let (mut again, mut manual, mut mail_only) = (false, false, false);
        strip(ui, self.page, |ui| {
            let retry = button(ui, tl!("Try again"), true, true);
            app.hit.add("signInTryAgain", self.existing, retry.rect);
            again = retry.clicked();
            manual = button(ui, tl!("Set up manually"), false, true).clicked();
            if !self.mail_only {
                mail_only = button(ui, tl!("Mail only"), false, true).on_hover_text(tl!("Sign in again, asking for mail alone")).clicked();
            }
        });
        if approval {
            Some(Action::Approval)
        } else if step.is_some() {
            step
        } else if manual {
            Some(Action::Manual)
        } else if mail_only {
            Some(Action::MailOnly)
        } else {
            (again || pass.submit).then_some(Action::Go)
        }
    }

    /// "Your organisation has to approve MailCraft first": why, and what to do next, as actions.
    pub(super) fn approval_page(&mut self, app: &mut MailApp, ui: &mut Ui, pass: &Pass) -> Option<Action> {
        if self.approval.approving {
            return self.approving_page(ui, pass);
        }
        let t = pass.t;
        let mut action = None;
        page(ui, &t, |ui| {
            heading(ui, tl!("Your organisation has to approve MailCraft first"), &t);
            note(ui, &shown(self.email.trim(), 120), t.text_dim);
            // What a try at an administrator's approval came to, and what Microsoft answered.
            if let Some((ours, theirs)) = &self.approval.refused {
                note(ui, ours, t.red);
                if !theirs.is_empty() {
                    note(ui, &fmt(tl!("Microsoft answered: {words}"), &[("words", theirs)]), t.text_dim);
                }
            }
            if self.approval.said {
                note(ui, &format!("{} {}", tl!("Microsoft's page said so."), tl!(WHY)), t.text);
            } else {
                note(ui, tl!(WHY), t.text);
            }
            ui.add_space(2.0);
            ui.label(RichText::new(tl!("What to do next")).font(semibold(13.0)).color(t.text));
            action = self.approval_steps(app, ui, &t, true);
            note(
                ui,
                tl!("If you have an administrator account of your own, the first choice lets you sign in with it on Microsoft's page."),
                t.text_dim,
            );
            note(ui, tl!("Once an administrator has approved, try again: the sign-in then goes through."), t.text_dim);
            error(ui, &self.error);
        });
        let (mut again, mut not_now) = (false, false);
        strip(ui, self.page, |ui| {
            let retry = button(ui, tl!("Try again"), true, true).on_hover_text(tl!("Sign in again, once an administrator has approved"));
            app.hit.add("approvalTryAgain", self.existing, retry.rect);
            again = retry.clicked();
            let later = button(ui, tl!("Not now"), false, true).on_hover_text(tl!("Closes this window. Nothing is kept."));
            app.hit.add("approvalNotNow", self.existing, later.rect);
            not_now = later.clicked();
        });
        // (Enter is nobody's here: each of these opens a page of Microsoft's, or closes this one.)
        if not_now {
            Some(Action::NotNow)
        } else if again {
            Some(Action::Go)
        } else {
            action
        }
    }

    /// Microsoft's approval page is open in the browser: the page waits for its answer.
    fn approving_page(&mut self, ui: &mut Ui, pass: &Pass) -> Option<Action> {
        let t = pass.t;
        page(ui, &t, |ui| {
            ui.add_space(14.0);
            ui.vertical_centered(|ui| {
                ui.add(egui::Spinner::new().size(30.0).color(t.accent));
                ui.add_space(8.0);
                ui.label(RichText::new(tl!("Microsoft's approval page is open in your browser")).font(medium(13.5)).color(t.text));
                let then = tl!("Sign in there with an administrator account and accept. This window carries on by itself.");
                ui.label(RichText::new(then).color(t.text_dim));
            });
            ui.add_space(10.0);
            note(
                ui,
                tl!("If that page says the account is not an administrator, press Cancel here and copy the message for an administrator instead."),
                t.text_dim,
            );
            ui.add_space(6.0);
        });
        let mut cancel = false;
        strip(ui, self.page, |ui| cancel = button(ui, tl!("Cancel"), false, true).clicked());
        cancel.then_some(Action::Back)
    }
}

/// A thin line across the page, between what is happening and what else there is to do.
fn rule(ui: &mut Ui, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().max(0.0), 9.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, t.border));
}

/// One thing to do, as a button as wide as the page: what it is called and, under that, a
/// sentence about it (none: one row).
fn step(ui: &mut Ui, t: &Tokens, title: &str, detail: &str) -> Response {
    let width = ui.available_width().max(120.0);
    let said = (!detail.is_empty()).then(|| ui.painter().layout(detail.to_string(), regular(12.0), t.text_dim, (width - 24.0).max(60.0)));
    let height = said.as_ref().map_or(34.0, |said| 30.0 + said.size().y + 10.0);
    let (r, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    // Known by what it says to whoever can't see it painted: the sentence about it is part of that.
    let label = if detail.is_empty() { title.to_string() } else { format!("{title}. {detail}") };
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
    let lit = resp.hovered() || resp.has_focus();
    ui.painter().rect(r, 6.0, if lit { t.hover } else { t.panel }, Stroke::new(1.0, if lit { t.accent } else { t.border }), StrokeKind::Inside);
    paint_clipped(ui.painter(), pos2(r.min.x + 12.0, r.min.y + 17.0), r.width() - 24.0, title, semibold(13.0), t.accent_text);
    if let Some(said) = said {
        ui.painter().galley(pos2(r.min.x + 12.0, r.min.y + 29.0), said, t.text_dim);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
