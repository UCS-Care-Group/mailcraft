// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account that has to be signed in again (#1556): its server turned the sign-in down, or the
//! sign-in ran out, and its mail stops arriving until the owner signs in.
//!
//! - **One mark for every kind of account**, [`mailcraft_model::Account::sign_in_needed`], kept in
//!   the mailbox file so that a restart still says it. Each way of keeping an account in step sets
//!   it where it already knew the sign-in was refused, and clears it at the first run or sign-in
//!   that goes through: IMAP and SMTP (a password, or a Google or Microsoft token), Microsoft
//!   Graph, JMAP, Exchange Web Services, and a sign-in on the provider's page (`poll_sign_in`).
//! - **Shown in two places, never as a dialog**: Account Problems at the top right of the window
//!   (`indicator`), whose menu lists each account with Sign in again, and a mark beside the
//!   account in the folder pane (`marker`). A round by itself that is refused again changes
//!   nothing on screen.
//! - **Sign in again** is the account's own Send/Receive (`ui.sync {"account"}`), which asks for
//!   the password or opens the provider's page, as it always did.

use egui::{Align2, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_model::Id;
use serde_json::json;

use crate::theme::{Tokens, medium, regular, semibold};
use crate::{MailApp, icons};

/// How wide the folder pane's mark is, its icon and the room before it.
pub(crate) const MARK_ROOM: f32 = 18.0;
const MARK: f32 = 13.0;

impl MailApp {
    /// Note that `account` has to be signed in again (`needed`), or has just signed in: the
    /// mark every kind of account shares ([`mailcraft_engine::cmd::account::note_sign_in_needed`]).
    pub(crate) fn note_sign_in_needed(&mut self, account: Id, needed: bool) {
        if mailcraft_engine::cmd::account::note_sign_in_needed(&mut self.session, account, needed) {
            log::info!("account {account}: {}", if needed { "has to be signed in again" } else { "signed in again" });
        }
    }

    /// The accounts that have to be signed in again, in the folder pane's order, with the name
    /// the folder pane gives each.
    pub(crate) fn to_sign_in(&self) -> Vec<(Id, String)> {
        let named = |a: &mailcraft_model::Account| if a.name.trim().is_empty() { a.name_by_default() } else { a.name.clone() };
        self.session.mailbox.needing_sign_in().into_iter().map(|a| (a.id, named(a))).collect()
    }

    /// Sign in again: the account's own Send/Receive, which asks for its password or opens its
    /// provider's page. What stops it is said in the status bar.
    fn sign_in_again_from_problems(&mut self, account: Id) {
        if let Err(e) = self.run("ui.sync", json!({"account": account})) {
            self.status(e);
        }
    }
}

/// Account Problems, at the top right of the window while an account has to be signed in again
/// (item kind `accountProblems`): a click lists each of them with Sign in again
/// (`accountProblemsSignIn`, by account id). Nothing when every account is signed in.
pub(crate) fn indicator(app: &mut MailApp, ui: &mut Ui, h: f32) {
    let accounts = app.to_sign_in();
    if accounts.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let label = tl!("Account Problems");
    let font = medium(11.5);
    let w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(label.to_string(), font.clone(), t.text).size().x);
    let (r, resp) = ui.allocate_exact_size(vec2(w + 34.0, h), Sense::click());
    app.hit.add("accountProblems", 0, r);
    let fill = if resp.is_pointer_button_down_on() {
        t.pressed
    } else if resp.hovered() {
        t.hover
    } else {
        t.title_bar
    };
    ui.painter().rect(r, h / 2.0, fill, Stroke::new(1.0, t.orange), egui::StrokeKind::Inside);
    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 15.0, r.center().y), vec2(16.0, 16.0)), "warning", t.orange, t.orange);
    ui.painter().text(pos2(r.min.x + 26.0, r.center().y), Align2::LEFT_CENTER, label, font, t.text);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let tip = tln!(accounts.len(), "{n} account has to be signed in again", "{n} accounts have to be signed in again");
    let resp = resp.on_hover_text(tip);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(280.0);
        ui.label(egui::RichText::new(label).font(semibold(13.0)).color(t.text));
        ui.label(egui::RichText::new(tl!("Mail doesn't arrive for these accounts until you sign in again.")).font(regular(12.0)).color(t.text_dim));
        ui.add_space(4.0);
        for (account, name) in accounts {
            ui.horizontal(|ui| {
                icons::paint(
                    ui.painter(),
                    Rect::from_center_size(ui.cursor().min + vec2(8.0, 10.0), vec2(MARK, MARK)),
                    "warning",
                    t.orange,
                    t.orange,
                );
                ui.add_space(20.0);
                ui.label(egui::RichText::new(crate::forms::kit::shown(&name, 60)).font(regular(13.0)).color(t.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let again = ui.button(tl!("Sign in again"));
                    app.hit.add("accountProblemsSignIn", account, again.rect);
                    if again.clicked() {
                        ui.close();
                        app.sign_in_again_from_problems(account);
                    }
                });
            });
        }
    });
}

/// The folder pane's mark beside an account that has to be signed in again (item kind
/// `accountSignInNeeded`, by account id), its left edge at `left` and centred on `y`: a click is
/// Sign in again. Answers how wide it is (nothing for an account that is signed in).
pub(crate) fn marker(app: &mut MailApp, ui: &mut Ui, account: Id, left: f32, y: f32) -> f32 {
    if !app.session.mailbox.account(account).is_some_and(|a| a.sign_in_needed) {
        return 0.0;
    }
    let t = Tokens::get(ui.ctx());
    let r = Rect::from_center_size(Pos2::new(left + MARK_ROOM - MARK / 2.0, y), vec2(MARK, MARK));
    let resp = ui.interact(r.expand(2.0), ui.scope_id().with(("signInNeeded", account)), Sense::click());
    app.hit.add("accountSignInNeeded", account, r);
    icons::paint(ui.painter(), r, "warning", t.orange, t.orange);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Sign in again")));
    if resp.on_hover_text(tl!("This account has to be signed in again: its mail doesn't arrive until you do. Click to sign in.")).clicked() {
        app.sign_in_again_from_problems(account);
    }
    MARK_ROOM
}
