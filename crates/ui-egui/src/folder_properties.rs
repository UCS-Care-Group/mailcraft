// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Folder Properties (#314): how many messages a folder and each folder under it hold, how many
//! unread, and how much space they take; for an account, all of its folders. Opened from a
//! folder's or an account's right-click menu, it shows what `folder.properties` answered when it
//! opened: a table that sorts by each column, where a folder's name goes to that folder.
//!
//! Folder names come from the mailbox, and from a server: they are shown as text, cut to the
//! column.

use std::collections::HashMap;

use egui::{RichText, Ui, vec2};
use mailcraft_model::{Folder, Id};
use serde_json::Value;

use crate::MailApp;
use crate::dialogs::button_row;
use crate::theme::Tokens;
use crate::widgets::primary_button;

/// The columns' widths: the folder's name, its messages, unread, size, and size with its
/// subfolders; and the server's count, when there is one, and the space between columns.
const WIDTHS: [f32; 6] = [240.0, 80.0, 70.0, 90.0, 110.0, 100.0];
const SERVER_W: f32 = 110.0;
const GAP: f32 = 16.0;
/// A row's height, and the space between rows.
const ROW_H: f32 = 20.0;
const ROW_GAP: f32 = 4.0;

/// What some messages come to (`folder.properties`' tallies).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub messages: u64,
    pub unread: u64,
    pub size: u64,
    /// Messages whose size isn't known here.
    pub unknown: u64,
}

impl Tally {
    fn read(v: &Value) -> Tally {
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        Tally { messages: n("messages"), unread: n("unread"), size: n("size"), unknown: n("unknownSize") }
    }
}

/// One folder of the table.
pub struct Row {
    pub id: Id,
    /// Its name as the window calls it ([`crate::nav::folder_name`]).
    pub name: String,
    pub path: String,
    pub depth: usize,
    /// What it holds itself, and with the folders under it.
    pub own: Tally,
    pub total: Tally,
    /// What its server last counted in it: messages and unread.
    pub server: Option<(u64, u64)>,
}

/// What the table is sorted by. [`Sort::Folder`] is the folder pane's order, with each folder
/// under its parent; the others put the largest first. A second click turns either round
/// ([`FolderProperties::descending`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Folder,
    Messages,
    Unread,
    Size,
    /// With the folders under it.
    TotalMessages,
    TotalSize,
    /// What the server last counted.
    Server,
}

pub struct FolderProperties {
    /// What it is about: a folder's name, or an account's.
    pub title: String,
    pub rows: Vec<Row>,
    pub total: Tally,
    /// Folders left out of the table (past the most it lists).
    pub more: u64,
    pub sort: Sort,
    /// The other way round from the sort's first order.
    pub descending: bool,
    /// The rows (by index) in the order shown, worked out when the sort changes, not each frame.
    order: Vec<usize>,
    /// Some row has its server's counts: they have a column.
    server: bool,
}

impl FolderProperties {
    /// For `{"id"}` (a folder; default the folder in view) or `{"account"}`: `None` when the
    /// engine refuses (no such folder or account).
    pub fn open(app: &mut MailApp, params: &Value) -> Option<FolderProperties> {
        let mut asked = serde_json::Map::new();
        for key in ["id", "account"] {
            if let Some(v) = params.get(key).filter(|v| !v.is_null()) {
                asked.insert(key.into(), v.clone());
            }
        }
        let answer = app.session.run("folder.properties", &Value::Object(asked)).ok()?;
        let mb = &app.session.mailbox;
        // (Each folder found once, and each path from its parent's, which comes first: a
        // mailbox may hold tens of thousands of folders.)
        let folders: HashMap<Id, &Folder> = mb.folders.iter().map(|f| (f.id, f)).collect();
        let mut rows: Vec<Row> = Vec::new();
        let mut at: HashMap<Id, usize> = HashMap::new();
        for r in answer["rows"].as_array().into_iter().flatten() {
            let Some(f) = r["id"].as_u64().and_then(|id| folders.get(&id)) else { continue };
            let name = crate::nav::folder_name(f).to_string();
            let path = match f.parent.and_then(|p| at.get(&p)).and_then(|i| rows.get(*i)) {
                Some(parent) => format!("{} › {name}", parent.path),
                None => shown_path(&folders, f),
            };
            let server = r.get("server").map(|s| (s["messages"].as_u64().unwrap_or(0), s["unread"].as_u64().unwrap_or(0)));
            at.insert(f.id, rows.len());
            rows.push(Row {
                id: f.id,
                name,
                path,
                depth: r["depth"].as_u64().map_or(0, |d| d as usize),
                own: Tally::read(r),
                total: Tally::read(&r["total"]),
                server,
            });
        }
        let title = match answer["folder"].as_u64().and_then(|f| mb.folder(f)) {
            Some(f) => crate::i18n::fmt(tl!("{folder} Properties"), &[("folder", crate::nav::folder_name(f))]),
            None => {
                let name = answer["account"].as_u64().and_then(|a| mb.account(a)).map(|a| crate::nav::heading_name(app, a)).unwrap_or_default();
                let name = if name.is_empty() { tl!("Mailbox").to_string() } else { name };
                crate::i18n::fmt(tl!("{folder} Properties"), &[("folder", &name)])
            }
        };
        let server = rows.iter().any(|r| r.server.is_some());
        let order = (0..rows.len()).collect();
        Some(FolderProperties {
            title,
            rows,
            total: Tally::read(&answer["total"]),
            more: answer["more"].as_u64().unwrap_or(0),
            sort: Sort::Folder,
            descending: false,
            order,
            server,
        })
    }

    /// The rows in the order the table shows them.
    pub fn sorted(&self) -> Vec<&Row> {
        self.order.iter().filter_map(|i| self.rows.get(*i)).collect()
    }

    /// A click on a column's heading: sorted by it, or the other way round when it already is.
    pub fn sort_by(&mut self, sort: Sort) {
        if self.sort == sort {
            self.descending = !self.descending;
        } else {
            (self.sort, self.descending) = (sort, false);
        }
        let key = |r: &Row| match sort {
            Sort::Folder | Sort::Messages => r.own.messages,
            Sort::Unread => r.own.unread,
            Sort::Size => r.own.size,
            Sort::TotalMessages => r.total.messages,
            Sort::TotalSize => r.total.size,
            Sort::Server => r.server.map_or(0, |(messages, _)| messages),
        };
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        if sort != Sort::Folder {
            // Largest first; the pane's order among equals.
            order.sort_by_key(|i| std::cmp::Reverse(self.rows.get(*i).map_or(0, key)));
        }
        if self.descending {
            order.reverse();
        }
        self.order = order;
    }

    /// How wide its window is: wider when there is a column of the server's counts.
    pub fn width(&self) -> f32 {
        let server = if self.server { SERVER_W + GAP } else { 0.0 };
        WIDTHS.iter().sum::<f32>() + GAP * (WIDTHS.len() - 1) as f32 + server + 24.0
    }

    /// Draw it; `true` when it is done with (Close, or a folder picked, which is then in view).
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        let total = self.total;
        let mut summary = vec![tln!(total.messages, "{n} message", "{n} messages"), tln!(total.unread, "{n} unread", "{n} unread"), size(total.size)];
        if total.unknown > 0 {
            summary.push(tln!(total.unknown, "{n} message of unknown size", "{n} messages of unknown size"));
        }
        ui.label(RichText::new(summary.join(" · ")).color(t.text));
        let server = self.server;
        let mut picked: Option<Id> = None;
        let mut clicked: Option<Sort> = None;
        let columns = WIDTHS.len() + usize::from(server);
        // The headings stay put above the rows; the columns line up by their widths.
        egui::Grid::new("folder_properties_headings").num_columns(columns).spacing(vec2(GAP, 4.0)).show(ui, |ui| {
            let headings = [
                (Sort::Folder, tl!("Folder"), tl!("In the folder pane's order")),
                (Sort::Messages, tl!("Messages"), tl!("Messages in the folder itself")),
                (Sort::Unread, tl!("Unread"), tl!("Unread messages in the folder itself")),
                (Sort::Size, tl!("Size"), tl!("The space the folder's own messages take on this computer")),
                (Sort::TotalMessages, tl!("Total messages"), tl!("Messages in the folder and the folders under it")),
                (Sort::TotalSize, tl!("Total size"), tl!("The space the folder and the folders under it take on this computer")),
                (
                    Sort::Server,
                    tl!("On the server"),
                    tl!("Messages in the folder as its server counted them at the last Send/Receive that read it to the end"),
                ),
            ];
            for ((sort, label, hint), width) in headings.into_iter().zip(WIDTHS.into_iter().chain([SERVER_W])).take(columns) {
                // The column it is sorted by: ▼ largest first, ▲ the other way round; the
                // pane's order has no arrow until it is turned round.
                let arrow = match (self.sort == sort, self.descending) {
                    (true, false) if sort != Sort::Folder => " ▼",
                    (true, true) => " ▲",
                    _ => "",
                };
                let head = RichText::new(format!("{label}{arrow}")).strong().color(t.text);
                let resp = cell(ui, width, sort != Sort::Folder, |ui| ui.add(egui::Button::new(head).frame(false).truncate()));
                if resp.on_hover_text(hint).clicked() {
                    clicked = Some(sort);
                }
            }
            ui.end_row();
        });
        // Only the rows in sight are laid out: an account may have thousands of folders.
        let rows = &self.rows;
        let order = &self.order;
        let tree = self.sort == Sort::Folder && !self.descending;
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = ROW_GAP;
            egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show_rows(ui, ROW_H, order.len(), |ui, shown| {
                let start = shown.start;
                egui::Grid::new("folder_properties").striped(true).start_row(start).num_columns(columns).spacing(vec2(GAP, ROW_GAP)).show(ui, |ui| {
                    let [name_w, messages_w, unread_w, size_w, total_messages_w, total_size_w] = WIDTHS;
                    for r in order.get(shown).into_iter().flatten().filter_map(|i| rows.get(*i)) {
                        // In the pane's order a folder is under its parent; in any other, its path says where it is.
                        let name = if tree { format!("{}{}", "    ".repeat(r.depth.min(12)), r.name) } else { r.path.clone() };
                        let link =
                            cell(ui, name_w, false, |ui| ui.add(egui::Button::new(RichText::new(name).color(t.accent_text)).frame(false).truncate()));
                        if link.on_hover_text(format!("{}\n{}", r.path, tl!("Go to the folder"))).clicked() {
                            picked = Some(r.id);
                        }
                        cell(ui, messages_w, true, |ui| ui.label(RichText::new(r.own.messages.to_string()).color(t.text)));
                        cell(ui, unread_w, true, |ui| ui.label(RichText::new(r.own.unread.to_string()).color(t.text)));
                        cell(ui, size_w, true, |ui| sized(ui, &t, r.own));
                        cell(ui, total_messages_w, true, |ui| ui.label(RichText::new(r.total.messages.to_string()).color(t.text)));
                        cell(ui, total_size_w, true, |ui| sized(ui, &t, r.total));
                        if server {
                            cell(ui, SERVER_W, true, |ui| {
                                if let Some((messages, unread)) = r.server {
                                    ui.label(RichText::new(messages.to_string()).color(t.text)).on_hover_text(tln!(
                                        unread,
                                        "{n} unread on the server",
                                        "{n} unread on the server"
                                    ));
                                }
                            });
                        }
                        ui.end_row();
                    }
                });
            });
        });
        if self.more > 0 {
            ui.label(RichText::new(tln!(self.more, "{n} more folder isn't listed.", "{n} more folders aren't listed.")).small().color(t.text_dim));
        }
        if total.unknown > 0 {
            ui.label(
                RichText::new(tl!(
                    "* The sizes leave out messages whose size isn't known here: only their headers are on this computer and their server gave no size, or they were too large to download."
                ))
                .small()
                .color(t.text_dim),
            );
        }
        if let Some(sort) = clicked {
            self.sort_by(sort);
        }
        let mut done = false;
        ui.add_space(4.0);
        button_row(ui, |ui| {
            if primary_button(ui, tl!("Close"), true).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                done = true;
            }
        });
        if let Some(id) = picked {
            let _ = app.run("folder.select", serde_json::json!({"id": id}));
            done = true;
        }
        done
    }
}

/// A folder's path as the window names it: each folder by [`crate::nav::folder_name`].
fn shown_path(folders: &HashMap<Id, &Folder>, f: &Folder) -> String {
    let mut parts = vec![crate::nav::folder_name(f).to_string()];
    let mut cur = f.parent;
    while let Some(p) = cur.and_then(|p| folders.get(&p)) {
        if parts.len() > 32 {
            break;
        }
        parts.push(crate::nav::folder_name(p).to_string());
        cur = p.parent;
    }
    parts.reverse();
    parts.join(" › ")
}

/// A cell `width` wide, what is in it at the left, or at the right (a number).
fn cell<R>(ui: &mut Ui, width: f32, right: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    let layout = if right { egui::Layout::right_to_left(egui::Align::Center) } else { egui::Layout::left_to_right(egui::Align::Center) };
    ui.allocate_ui_with_layout(vec2(width, ROW_H), layout, |ui| {
        ui.set_width(width);
        add(ui)
    })
    .inner
}

fn size(bytes: u64) -> String {
    mailcraft_model::size_label_in(crate::i18n::current(), bytes)
}

/// A size, marked when some of the messages it is of have a size not known here.
fn sized(ui: &mut Ui, t: &Tokens, tally: Tally) {
    if tally.unknown == 0 {
        ui.label(RichText::new(size(tally.size)).color(t.text));
    } else {
        ui.label(RichText::new(format!("{}*", size(tally.size))).color(t.text)).on_hover_text(tln!(
            tally.unknown,
            "{n} message of unknown size",
            "{n} messages of unknown size"
        ));
    }
}

#[cfg(test)]
#[path = "folder_properties_tests.rs"]
mod folder_properties_tests;
