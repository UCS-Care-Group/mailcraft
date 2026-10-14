//! The owner's choice of how the Favorites of Outlook are read for an account of an
//! organisation: Microsoft Graph, Exchange Web Services, or not now. The same three (two, once
//! Exchange Web Services is gone) are shown as the last step of adding such an account
//! ([`crate::account_setup`]), in a dialog of their own that the line under the Favorites
//! heading opens (`ui.dialog` with `favoritesSource`), and as a row in File › Options.
//!
//! All of them only collect the choice: `ui.favoritesSource` does what follows from it.

use egui::{Align2, Response, RichText, Sense, Stroke, StrokeKind, Ui, WidgetInfo, WidgetType, pos2, vec2};
use mailcraft_model::{FavoritesSource, Id};
use serde_json::{Value, json};

use crate::MailApp;
use crate::theme::{Tokens, regular, semibold};

/// The dialog's width in points.
pub const WIDTH: f32 = 480.0;

/// What a way of reading them is called, and what the owner is told of it: how long it lasts
/// and whose yes it takes. Neither is promised to work, and neither is said to need nobody's
/// approval: a program that is not Microsoft's is approved in an organisation whichever way it
/// reads (by the owner where the organisation lets them, else by an administrator), and the
/// approval an administrator gives with MailCraft's link covers Graph's way, not this other one.
pub(crate) fn words(source: FavoritesSource) -> (&'static str, &'static str) {
    match source {
        FavoritesSource::Graph => (
            tn!("Microsoft Graph"),
            tn!(
                "The lasting way. In an organisation, an administrator has to approve it once; if they have approved MailCraft already, nothing more is needed."
            ),
        ),
        FavoritesSource::Ews => (
            tn!("Exchange Web Services"),
            tn!(
                "An older way that Microsoft switches off on 1 April 2027. It works where your organisation lets you approve it yourself; elsewhere it needs an administrator too."
            ),
        ),
        FavoritesSource::None | FavoritesSource::Unset => (tn!("Not now"), tn!("MailCraft keeps its own Favorites.")),
    }
}

/// What is said above the choice, wherever it is put.
pub(crate) const ABOUT: &str = tn!(
    "MailCraft can show the folders you keep under Favorites in Outlook, in the same order. They are only read, never changed. Choose how they are read:"
);
/// And under it.
pub(crate) const AFTER: &str = tn!("Your mail works the same whichever you choose, and you can change this later in File › Options.");

/// The ways the owner of `account` may choose from today, in the order they are shown.
pub(crate) fn offered(app: &MailApp, account: Id) -> Vec<FavoritesSource> {
    let codes = mailcraft_engine::cmd::account::favorites_sources_offered(&app.session, app.directory(account));
    codes.into_iter().filter_map(FavoritesSource::parse).collect()
}

/// One way to choose: a mark, what it is called, and a sentence or two about it.
fn option(ui: &mut Ui, t: &Tokens, source: FavoritesSource, on: bool) -> Response {
    let (title, detail) = words(source);
    let (title, detail) = (tl!(title), tl!(detail));
    let width = ui.available_width().max(120.0);
    let said = ui.painter().layout(detail.to_string(), regular(12.0), t.text_dim, (width - 46.0).max(60.0));
    let height = 34.0 + said.size().y + 11.0;
    let (r, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    // (What is said of it is part of what it is called, for whoever can't see it painted.)
    resp.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, on, format!("{title}. {detail}")));
    let lit = on || resp.hovered() || resp.has_focus();
    let fill = if on {
        t.checked
    } else if resp.hovered() {
        t.hover
    } else {
        t.panel
    };
    ui.painter().rect(r, 6.0, fill, Stroke::new(1.0, if lit { t.accent } else { t.border }), StrokeKind::Inside);
    let mark = pos2(r.min.x + 18.0, r.min.y + 20.0);
    ui.painter().circle_stroke(mark, 6.5, Stroke::new(1.5, if on { t.accent } else { t.input_border }));
    if on {
        ui.painter().circle_filled(mark, 3.5, t.accent);
    }
    ui.painter().text(pos2(r.min.x + 36.0, r.min.y + 20.0), Align2::LEFT_CENTER, title, semibold(13.0), t.text);
    ui.painter().galley(pos2(r.min.x + 36.0, r.min.y + 33.0), said, t.text_dim);
    resp
}

/// The ways to choose from, one under the other; `picked` is the one that is marked. Whether
/// the owner picked one in this pass.
pub(crate) fn rows(app: &mut MailApp, ui: &mut Ui, account: Id, picked: &mut Option<FavoritesSource>) -> bool {
    let t = Tokens::get(ui.ctx());
    let mut changed = false;
    for source in offered(app, account) {
        let resp = option(ui, &t, source, *picked == Some(source));
        let kind = match source {
            FavoritesSource::Graph => "favoritesChooseGraph",
            FavoritesSource::Ews => "favoritesChooseEws",
            _ => "favoritesChooseNone",
        };
        app.hit.add(kind, account, resp.rect);
        if resp.clicked() {
            *picked = Some(source);
            changed = true;
        }
    }
    changed
}

/// The choice in a dialog of its own, for an account that exists.
pub struct ChoiceDialog {
    account: Id,
    picked: Option<FavoritesSource>,
    error: String,
}

impl ChoiceDialog {
    /// The dialog for the account `p.account` names, else the first that has a choice. `None`
    /// when no account has one (a personal Microsoft account reads its Favorites through
    /// Microsoft Graph and nothing else; any other account has none in Outlook).
    pub fn open(app: &MailApp, p: &Value) -> Option<ChoiceDialog> {
        let named = p.get("account").and_then(Value::as_u64);
        let first = || app.session.mailbox.accounts.iter().map(|a| a.id).find(|id| app.favorites_choice(*id));
        let account = named.or_else(first).filter(|id| app.favorites_choice(*id))?;
        // What it is set to is marked, when that can still be chosen.
        let picked = Some(app.favorites_source(account)).filter(|source| offered(app, account).contains(source));
        Some(ChoiceDialog { account, picked, error: String::new() })
    }

    pub fn name(&self) -> &'static str {
        "favoritesSource"
    }

    pub fn title(&self) -> &'static str {
        tl!("Outlook's Favorites")
    }

    pub fn width(&self) -> f32 {
        WIDTH
    }

    /// Lay the dialog out. Returns `true` when it should close.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        let who = app.session.mailbox.account(self.account).map(|a| a.email.clone()).unwrap_or_default();
        ui.label(RichText::new(who).font(semibold(13.0)).color(t.text));
        ui.label(RichText::new(tl!(ABOUT)).color(t.text));
        if rows(app, ui, self.account, &mut self.picked) {
            self.error.clear();
        }
        ui.label(RichText::new(tl!(AFTER)).color(t.text_dim).size(11.5));
        if !self.error.is_empty() {
            ui.label(RichText::new(self.error.as_str()).color(t.red));
        }
        let ready = self.picked.is_some();
        let (mut ok, mut cancel) = (false, false);
        ui.add_space(4.0);
        crate::dialogs::button_row(ui, |ui| {
            cancel = ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0))).clicked();
            let go = crate::widgets::primary_button(ui, tl!("Continue"), ready);
            // (Known by what it says to whoever can't see it painted.)
            go.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ready, tl!("Continue")));
            app.hit.add("favoritesChooseContinue", self.account, go.rect);
            ok = go.clicked();
        });
        if cancel {
            return true;
        }
        if let (true, Some(source)) = (ok, self.picked) {
            match app.execute("ui.favoritesSource", json!({"account": self.account, "source": source.code()})) {
                Ok(_) => return true,
                Err(e) => self.error = e,
            }
        }
        false
    }
}

/// File › Options: how the Favorites of Outlook are read for `account`, as three choices side
/// by side (the one in effect is marked), and under them what there is to say of it.
pub(crate) fn options_row(app: &mut MailApp, ui: &mut Ui, t: &Tokens, account: Id) {
    let who = app.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
    // An account that never chose reads them through Microsoft Graph when it has a list.
    let from_server = mailcraft_engine::cmd::folder::favorites_from_server(&app.session, account);
    let now = match app.favorites_source(account) {
        FavoritesSource::Unset if from_server => Some(FavoritesSource::Graph),
        FavoritesSource::Unset => None,
        chosen => Some(chosen),
    };
    let mut choose: Option<FavoritesSource> = None;
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.label(RichText::new(crate::forms::kit::shown(who.trim(), 60)).color(t.text));
        for source in offered(app, account) {
            let (title, detail) = words(source);
            let resp = ui.selectable_label(now == Some(source), tl!(title)).on_hover_text(tl!(detail));
            let kind = match source {
                FavoritesSource::Graph => "favoritesGraph",
                FavoritesSource::Ews => "favoritesEws",
                _ => "favoritesNone",
            };
            app.hit.add(kind, account, resp.rect);
            if resp.clicked() && now != Some(source) {
                choose = Some(source);
            }
        }
    });
    if let Some(note) = app.favorites_note_of(account).filter(|note| !matches!(note, super::Note::Offer)) {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label(RichText::new(tl!(note.text())).color(t.text_dim));
            for step in app.favorites_steps(account, &note) {
                if ui.link(tl!(step.label())).on_hover_text(tl!(step.about())).clicked() {
                    step_taken(app, account, step);
                }
            }
        });
    }
    if let Some(source) = choose
        && let Err(e) = app.run("ui.favoritesSource", json!({"account": account, "source": source.code()}))
    {
        app.status(e);
    }
}

/// Do what a link under a quiet line says ([`super::Step`]).
pub(crate) fn step_taken(app: &mut MailApp, account: Id, step: super::Step) {
    let done = match step {
        super::Step::CopyMessage => app.run("ui.copyAdminMessage", json!({"account": account})),
        super::Step::Help => app.run("ui.approvalHelp", json!({"account": account})),
        super::Step::Use(source) => app.run("ui.favoritesSource", json!({"account": account, "source": source.code()})),
    };
    if let Err(e) = done {
        app.status(e);
    }
}
