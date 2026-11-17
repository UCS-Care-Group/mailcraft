// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Explicit discovery, independent setup, and user-selected GOA-managed mail accounts.

use std::sync::mpsc::{Receiver, TryRecvError};

use crate::theme::{Tokens, paint_clipped, regular, semibold};
use egui::{Align, Align2, Frame, Layout, Response, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

pub const WIDTH: f32 = 640.0;

use crate::{MailApp, account_setup::AccountSetup};

/// The GOA provider types whose setup MailCraft understands without guessing servers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provider {
    Google,
    Microsoft365,
    OutlookCom,
    Unsupported,
}

/// Public account metadata and optional mail configuration for the import dialog.
#[derive(Clone, Debug)]
pub struct Account {
    pub email: String,
    pub name: String,
    pub provider_name: String,
    pub provider: Provider,
    /// A mail configuration that GOA can manage through its public mail interfaces.
    pub linked: Option<mailcraft_model::Account>,
}

/// Discovery never exposes D-Bus error text, which may contain account details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscoveryError {
    Unavailable,
    Failed,
    TooManyAccounts,
}

pub type Discovery = Result<Vec<Account>, DiscoveryError>;
pub type Discover = Box<dyn Fn() -> Receiver<Discovery>>;

/// The separate import dialog: explicitly link a selected account or prefill independent setup.
pub struct Import {
    rx: Option<Receiver<Discovery>>,
    accounts: Vec<Account>,
    error: Option<DiscoveryError>,
    selected: Option<usize>,
    link_error: Option<String>,
}

impl Import {
    pub(crate) fn open(app: &MailApp) -> Option<Self> {
        let discover = app.services.gnome_accounts.as_ref()?;
        Some(Self { rx: Some(discover()), accounts: Vec::new(), error: None, selected: None, link_error: None })
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(DiscoveryError::Failed),
        };
        self.rx = None;
        match result {
            Ok(accounts) if accounts.len() <= 128 => self.accounts = accounts,
            Ok(_) => self.error = Some(DiscoveryError::TooManyAccounts),
            Err(error) => self.error = Some(error),
        }
    }

    fn already_added(app: &MailApp, email: &str) -> bool {
        app.session.mailbox.accounts.iter().any(|a| a.email.trim().eq_ignore_ascii_case(email.trim()))
    }

    fn can_select(app: &MailApp, account: &Account) -> bool {
        (account.provider != Provider::Unsupported || account.linked.is_some()) && !Self::already_added(app, &account.email)
    }

    fn setup(&self, app: &MailApp) -> Option<AccountSetup> {
        let account = self.accounts.get(self.selected?)?;
        (account.provider != Provider::Unsupported && Self::can_select(app, account)).then(|| AccountSetup::from_gnome(app, account))
    }

    fn link(&self, app: &mut MailApp) -> Result<mailcraft_model::Id, String> {
        let selected = self
            .selected
            .and_then(|i| self.accounts.get(i))
            .filter(|a| Self::can_select(app, a))
            .ok_or_else(|| tl!("Choose an account first.").to_string())?;
        let account = selected.linked.as_ref().ok_or_else(|| tl!("This account cannot be linked through GNOME.").to_string())?;
        app.execute(
            "account.add",
            serde_json::json!({
                "email": account.email, "kind": account.kind, "displayName": account.display_name, "username": account.username,
                "incoming": account.incoming, "outgoing": account.outgoing, "goa": account.goa,
            }),
        )
        .and_then(|v| v.get("id").and_then(serde_json::Value::as_u64).ok_or_else(|| tl!("Could not link this account.").to_string()))
        .map_err(|e| e.to_string())
    }

    /// Return an independent setup wizard, or close after linking the selected account or cancelling.
    pub(crate) fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> (Option<AccountSetup>, bool) {
        self.poll();
        let t = Tokens::get(ui.ctx());
        ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
            ui.painter().rect_filled(r, 12.0, t.accent_soft);
            crate::icons::paint(ui.painter(), r.shrink(10.0), "mail", t.accent_text, t.accent_text);
            ui.vertical(|ui| {
                ui.label(RichText::new(tl!("Choose your account")).font(semibold(22.0)).color(t.text));
                ui.label(RichText::new(tl!("Bring mail from your GNOME accounts into MailCraft.")).color(t.text_dim));
            });
        });
        ui.add_space(12.0);
        if self.rx.is_some() {
            state_panel(ui, &t, "sync", tl!("Reading accounts from GNOME…"));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        } else if let Some(error) = self.error {
            state_panel(
                ui,
                &t,
                "warning",
                match error {
                    DiscoveryError::Unavailable => tl!("GNOME Online Accounts is unavailable. You can still use Add Account."),
                    DiscoveryError::Failed => tl!("Could not read GNOME accounts. Check access to GNOME Online Accounts and try again."),
                    DiscoveryError::TooManyAccounts => tl!("GNOME returned too many accounts. Use Add Account instead."),
                },
            );
        } else if self.accounts.is_empty() {
            state_panel(ui, &t, "account", tl!("No mail accounts enabled in GNOME were found."));
        } else {
            ui.label(RichText::new(tl!("ACCOUNTS ON THIS COMPUTER")).font(semibold(11.0)).color(t.text_dim));
            egui::ScrollArea::vertical().max_height(252.0).auto_shrink([false, true]).show(ui, |ui| {
                for (index, account) in self.accounts.iter().enumerate() {
                    let enabled = Self::can_select(app, account);
                    let status = if Self::already_added(app, &account.email) {
                        tl!("Already added")
                    } else if account.linked.is_some() {
                        tl!("Ready to link")
                    } else if account.provider != Provider::Unsupported {
                        tl!("Separate setup")
                    } else {
                        tl!("Not supported yet")
                    };
                    if account_card(ui, account, self.selected == Some(index), enabled, status, &t).clicked() {
                        self.selected = Some(index);
                        self.link_error = None;
                    }
                }
            });
        }
        let selected = self.selected.and_then(|i| self.accounts.get(i)).filter(|a| Self::can_select(app, a));
        let has_selected = selected.is_some();
        let linkable = selected.is_some_and(|a| a.linked.is_some());
        let separate = selected.is_some_and(|a| a.provider != Provider::Unsupported);
        let mut setup = None;
        let mut close = false;
        if !self.accounts.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(tl!("How would you like to add it?")).font(semibold(14.0)).color(t.text));
            ui.columns(2, |columns| {
                if option_card(
                    &mut columns[0],
                    &t,
                    tl!("Managed by GNOME"),
                    tl!("Keep your existing sign-in. GNOME manages the account connection."),
                    tl!("Link with GNOME"),
                    linkable,
                    true,
                )
                .clicked()
                {
                    match self.link(app) {
                        Ok(id) => {
                            close = true;
                            let _ = app.run("ui.sync", serde_json::json!({"account": id}));
                        }
                        Err(error) => self.link_error = Some(error),
                    }
                }
                if option_card(
                    &mut columns[1],
                    &t,
                    tl!("Managed by MailCraft"),
                    tl!("Sign in again to create a separate account in MailCraft."),
                    tl!("Set up separately"),
                    separate,
                    false,
                )
                .clicked()
                {
                    setup = self.setup(app);
                }
            });
            if has_selected && !linkable {
                ui.label(RichText::new(tl!("This account needs separate setup in this version of MailCraft.")).size(12.0).color(t.text_dim));
            }
        }
        if let Some(error) = &self.link_error {
            state_panel(ui, &t, "warning", error);
        }
        ui.add_space(6.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new(tl!("Only accounts you choose are added.")).size(11.5).color(t.text_dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close |= crate::widgets::plain_button(ui, tl!("Cancel")).clicked();
            });
        });
        (setup, close)
    }
}

fn account_card(ui: &mut Ui, account: &Account, selected: bool, enabled: bool, status: &str, t: &Tokens) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 76.0), if enabled { Sense::click() } else { Sense::hover() });
    let label = format!("{} — {}", account.email, account.provider_name);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, enabled, selected, &label));
    let hovered = enabled && resp.hovered();
    let edge = if selected || resp.has_focus() { t.accent } else { t.border };
    let fill = if selected {
        t.accent_soft
    } else if hovered {
        t.hover_subtle
    } else {
        t.panel
    };
    ui.painter().rect(r, 10.0, fill, Stroke::new(if selected { 1.5 } else { 1.0 }, edge), StrokeKind::Inside);
    let avatar = egui::Rect::from_center_size(pos2(r.left() + 30.0, r.center().y), vec2(36.0, 36.0));
    ui.painter().rect_filled(avatar, 10.0, if selected { t.surface } else { t.pill_quiet });
    let ink = if enabled { t.text } else { t.text_disabled };
    crate::icons::paint(ui.painter(), avatar.shrink(8.0), "account", if selected { t.accent_text } else { ink }, t.accent);
    let text_x = r.left() + 60.0;
    let badge_w = ui.painter().layout_no_wrap(status.to_string(), regular(10.5), t.text_dim).size().x.min(130.0) + 16.0;
    let badge = egui::Rect::from_min_size(pos2(r.right() - badge_w - 42.0, r.center().y - 10.0), vec2(badge_w, 20.0));
    paint_clipped(ui.painter(), pos2(text_x, r.top() + 27.0), (badge.left() - text_x - 12.0).max(0.0), &account.email, semibold(13.5), ink);
    paint_clipped(
        ui.painter(),
        pos2(text_x, r.top() + 49.0),
        (badge.left() - text_x - 12.0).max(0.0),
        &account.provider_name,
        regular(11.5),
        t.text_dim,
    );
    ui.painter().rect_filled(badge, 5.0, if account.linked.is_some() && enabled { t.accent_soft } else { t.pill_quiet });
    ui.painter().text(
        badge.center(),
        Align2::CENTER_CENTER,
        status,
        regular(10.5),
        if account.linked.is_some() && enabled { t.accent_soft_text } else { t.text_dim },
    );
    let center = pos2(r.right() - 21.0, r.center().y);
    ui.painter().circle_stroke(center, 7.0, Stroke::new(1.5, if selected { t.accent } else { t.border_strong }));
    if selected {
        ui.painter().circle_filled(center, 3.5, t.accent);
    }
    resp.on_hover_text(label)
}

fn option_card(ui: &mut Ui, t: &Tokens, title: &str, detail: &str, button: &str, enabled: bool, primary: bool) -> Response {
    Frame::new()
        .fill(t.panel)
        .corner_radius(10)
        .stroke(Stroke::new(1.0, if enabled && primary { t.accent } else { t.border }))
        .inner_margin(14)
        .show(ui, |ui| {
            let width = ui.available_width();
            ui.set_min_width(width);
            ui.label(RichText::new(title).font(semibold(13.5)).color(t.text));
            ui.allocate_ui_with_layout(vec2(width, 40.0), Layout::top_down(Align::Min), |ui| {
                ui.set_min_height(40.0);
                ui.label(RichText::new(detail).size(12.0).color(t.text_dim));
            });
            let fill = if !enabled {
                t.pressed
            } else if primary {
                t.accent
            } else {
                t.input
            };
            let ink = if !enabled {
                t.text_disabled
            } else if primary {
                t.on_accent
            } else {
                t.text
            };
            ui.add_enabled(
                enabled,
                egui::Button::new(RichText::new(button).font(semibold(12.5)).color(ink))
                    .fill(fill)
                    .stroke(Stroke::new(1.0, if primary { fill } else { t.input_border }))
                    .corner_radius(6)
                    .min_size(vec2(width, 34.0)),
            )
        })
        .inner
}

fn state_panel(ui: &mut Ui, t: &Tokens, icon: &str, detail: &str) {
    Frame::new().fill(t.panel).corner_radius(10).stroke(Stroke::new(1.0, t.border)).inner_margin(18).show(ui, |ui| {
        ui.set_min_width((ui.available_width()).max(0.0));
        ui.horizontal_wrapped(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
            crate::icons::paint(ui.painter(), r, icon, t.icon, t.accent);
            ui.label(RichText::new(detail).color(t.text_dim));
        });
    });
}

#[cfg(test)]
mod tests;
