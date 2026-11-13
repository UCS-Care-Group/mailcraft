// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Public Folders in the window (#1263): the calendars and address books of an Exchange server
//! account's public folders, and which of them are shown.
//!
//! The dialog only runs commands: List asks for the listing (`publicFolders.list`) and starts the
//! account's Send/Receive, which brings it; a tick shows a folder (`publicFolders.open`), read at
//! each Send/Receive from then on, and taking the tick off removes it (`publicFolders.close`).
//! Nothing is asked of the server before the owner presses List.

use egui::Ui;
use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::MailApp;
use crate::theme::Tokens;

/// One folder as `publicFolders.listed` answers it.
struct Row {
    id: String,
    name: String,
    calendar: bool,
    open: bool,
}

fn rows(list: Option<&Value>) -> Vec<Row> {
    let mut rows: Vec<Row> = list
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| {
            Some(Row {
                id: f.get("id")?.as_str()?.to_owned(),
                name: f.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                calendar: f.get("kind").and_then(Value::as_str) == Some("calendar"),
                open: f.get("open").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect();
    rows.sort_by_key(|row| row.name.to_lowercase());
    rows
}

/// The dialog's content for `account`; `error` is what went wrong last. Answers whether it is to close.
pub(crate) fn show(app: &mut MailApp, ui: &mut Ui, account: Id, error: &mut String, t: &Tokens) -> bool {
    let owner = app.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
    let listed = app.session.run("publicFolders.listed", &json!({"account": account})).unwrap_or_default();
    let pending = listed.get("pending").and_then(Value::as_bool).unwrap_or(false);
    let has_list = listed.get("listed").and_then(Value::as_bool).unwrap_or(false);
    let failed = listed.get("failed").and_then(Value::as_bool).unwrap_or(false);
    let mut folders = rows(listed.get("folders"));
    // (What is shown and wasn't in a listing this session: the ones picked before.)
    for open in rows(listed.get("open")) {
        if !folders.iter().any(|row| row.id == open.id) {
            folders.push(open);
        }
    }
    folders.sort_by_key(|row| row.name.to_lowercase());

    ui.label(crate::i18n::fmt(
        tl!(
            "Calendars and address books in your organisation's public folders, read with {owner}'s sign-in. The ones you tick are read at each Send/Receive, read-only, and the list of them is kept on this computer only."
        ),
        &[("owner", &owner)],
    ));
    ui.add_space(6.0);
    let mut toggled: Option<(String, bool)> = None;
    if folders.is_empty() {
        if has_list && !pending {
            ui.label(egui::RichText::new(tl!("The public folders have no calendar or address book that you can see.")).color(t.text_dim));
        }
    } else {
        egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
            for row in &folders {
                ui.horizontal(|ui| {
                    let mut on = row.open;
                    if ui.checkbox(&mut on, row.name.as_str()).changed() {
                        toggled = Some((row.id.clone(), on));
                    }
                    let kind = if row.calendar { tl!("Calendar") } else { tl!("Address book") };
                    ui.label(egui::RichText::new(kind).color(t.text_dim).size(11.0));
                });
            }
        });
    }
    if pending {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(egui::RichText::new(tl!("Reading the list of public folders from the server…")).color(t.text_dim));
        });
    }
    if failed && !pending {
        ui.label(egui::RichText::new(tl!("Couldn't read the list of public folders from the server.")).color(t.red));
    }
    if !error.is_empty() {
        ui.label(egui::RichText::new(error.as_str()).color(t.red));
    }
    let mut close = false;
    let mut list = false;
    ui.add_space(4.0);
    crate::dialogs::button_row(ui, |ui| {
        close = ui.add(egui::Button::new(tl!("Close")).min_size(egui::vec2(72.0, 26.0))).clicked();
        let label = if has_list { tl!("List Again") } else { tl!("List Public Folders") };
        list = crate::widgets::primary_button(ui, label, !pending).clicked();
    });
    if let Some((id, on)) = toggled {
        let command = if on { "publicFolders.open" } else { "publicFolders.close" };
        match app.execute(command, json!({"account": account, "id": id})) {
            Ok(_) => {
                error.clear();
                if on {
                    app.status(tl!("It is read at the next Send/Receive."));
                }
            }
            Err(e) => *error = e,
        }
    }
    if list {
        error.clear();
        match app.execute("publicFolders.list", json!({"account": account})) {
            // The listing comes with the account's Send/Receive; one already under way leaves it
            // for the next.
            Ok(_) => {
                if let Err(e) = app.start_sync(&json!({"account": account})) {
                    *error = crate::i18n::fmt(tl!("{why} The list is read at the next Send/Receive."), &[("why", &e)]);
                }
            }
            Err(e) => *error = e,
        }
    }
    close
}
