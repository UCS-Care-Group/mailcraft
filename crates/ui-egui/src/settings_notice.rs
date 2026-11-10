// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The line above the status bar that says the settings file couldn't be read, so MailCraft
//! started with its own settings. Without it the window just looks different, and the owner is
//! left to work out why.

use egui::Ui;
use serde_json::{Value, json};

use crate::MailApp;

/// What became of a settings file that couldn't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsUnread {
    /// Moved aside under this name (a file name, beside where it was), to look at or put back.
    SetAside(String),
    /// It couldn't be moved aside, so it stays where it was and this run's settings aren't saved
    /// over it.
    LeftInPlace,
    /// It couldn't be read at all (no permission, or it isn't a file), so nothing was moved; it
    /// stays where it was and this run's settings aren't saved over it.
    CouldntRead,
}

/// The line, for as long as the owner hasn't put it away. Worded when drawn, so it is in the
/// interface language even though the file was read before the language was known.
pub(crate) fn bar(app: &mut MailApp, ui: &mut Ui) {
    let Some(unread) = app.settings_unread.clone() else { return };
    let said = match unread {
        SettingsUnread::SetAside(name) => crate::i18n::fmt(
            tl!("The settings file (ui.json) couldn't be read, so the default settings were restored. The old file was renamed {file}."),
            &[("file", &name)],
        ),
        SettingsUnread::LeftInPlace => tl!(
            "The settings file (ui.json) couldn't be read, so the default settings were restored. It couldn't be renamed, so it was left as it is and won't be overwritten."
        )
        .to_string(),
        SettingsUnread::CouldntRead => tl!(
            "The settings file (ui.json) couldn't be read, so the default settings were restored. It was left as it is and won't be overwritten."
        )
        .to_string(),
    };
    crate::updates::bar(app, ui, "settings_unread", &said, &[("settingsNoticeOk", tl!("OK"), false)], |app, _| {
        let _ = app.run("ui.dismissSettingsNotice", json!({}));
    });
}

/// `ui.dismissSettingsNotice`: puts the line away for this run.
pub(crate) fn dismiss(app: &mut MailApp) -> Result<Value, String> {
    let was = app.settings_unread.take().is_some();
    Ok(json!({"dismissed": was}))
}

#[cfg(test)]
#[path = "settings_notice_tests.rs"]
mod tests;
