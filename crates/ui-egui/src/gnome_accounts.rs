// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Optional, explicit discovery of system accounts. No GOA credentials are requested or kept.

use std::sync::mpsc::{Receiver, TryRecvError};

use egui::Ui;

use crate::{MailApp, account_setup::AccountSetup};

/// The GOA provider types whose setup MailCraft understands without guessing servers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provider {
    Google,
    Microsoft365,
    OutlookCom,
    Unsupported,
}

/// Public account metadata, kept only for the lifetime of the import dialog.
#[derive(Clone, Debug)]
pub struct Account {
    pub email: String,
    pub name: String,
    pub provider_name: String,
    pub provider: Provider,
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

/// The separate import dialog. Selecting an account only prefills the existing setup wizard.
pub struct Import {
    rx: Option<Receiver<Discovery>>,
    accounts: Vec<Account>,
    error: Option<DiscoveryError>,
    selected: Option<usize>,
}

impl Import {
    pub(crate) fn open(app: &MailApp) -> Option<Self> {
        let discover = app.services.gnome_accounts.as_ref()?;
        Some(Self { rx: Some(discover()), accounts: Vec::new(), error: None, selected: None })
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
        account.provider != Provider::Unsupported && !Self::already_added(app, &account.email)
    }

    fn setup(&self, app: &MailApp) -> Option<AccountSetup> {
        let account = self.accounts.get(self.selected?)?;
        Self::can_select(app, account).then(|| AccountSetup::from_gnome(app, account))
    }

    /// A selected setup wizard, or whether to close. No account is added before normal setup succeeds.
    pub(crate) fn show(&mut self, app: &MailApp, ui: &mut Ui) -> (Option<AccountSetup>, bool) {
        self.poll();
        ui.label(tl!("Choose an account to set up in MailCraft. You will sign in separately; your GNOME accounts stay unchanged."));
        ui.add_space(8.0);
        if self.rx.is_some() {
            ui.spinner();
            ui.label(tl!("Reading accounts from GNOME…"));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        } else if let Some(error) = self.error {
            ui.label(match error {
                DiscoveryError::Unavailable => tl!("GNOME Online Accounts is unavailable. You can still use Add Account."),
                DiscoveryError::Failed => tl!("Could not read GNOME accounts. Check access to GNOME Online Accounts and try again."),
                DiscoveryError::TooManyAccounts => tl!("GNOME returned too many accounts. Use Add Account instead."),
            });
        } else if self.accounts.is_empty() {
            ui.label(tl!("No mail accounts enabled in GNOME were found."));
        }
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            for (index, account) in self.accounts.iter().enumerate() {
                let enabled = Self::can_select(app, account);
                ui.add_enabled_ui(enabled, |ui| {
                    let label = format!("{} — {}", account.email, account.provider_name);
                    if ui.selectable_label(self.selected == Some(index), label).clicked() {
                        self.selected = Some(index);
                    }
                });
                if Self::already_added(app, &account.email) {
                    ui.label(tl!("Already added to MailCraft"));
                } else if account.provider == Provider::Unsupported {
                    ui.label(tl!("Import is not supported for this provider yet. Use Add Account."));
                }
            }
        });
        ui.separator();
        let mut setup = None;
        let mut close = false;
        ui.horizontal(|ui| {
            let enabled = self.selected.and_then(|i| self.accounts.get(i)).is_some_and(|a| Self::can_select(app, a));
            if crate::widgets::primary_button(ui, tl!("Continue"), enabled).clicked() {
                setup = self.setup(app);
            }
            close = ui.button(tl!("Cancel")).clicked();
        });
        (setup, close)
    }
}

#[cfg(test)]
mod tests;
