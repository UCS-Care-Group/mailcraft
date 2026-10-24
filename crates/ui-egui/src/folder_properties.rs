// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Folder Properties (#314), shaped as classic Outlook's: a dialog with tabs for a folder, or for
//! an account (Outlook's Data File Properties). General says what it is, where it is and how many
//! messages it holds, and its Folder Size… button opens, over it, the table of what the folder and
//! each folder under it (for an account, all of its folders) hold and the space they take: sorted
//! by each column, a folder's name going to that folder. The other tabs come with the features
//! behind them; until then each says so. Everything shown is what `folder.properties` answered
//! when the dialog opened.
//!
//! Folder names come from the mailbox, and from a server: they are shown as text, cut to the
//! column.

use std::collections::HashMap;

use egui::{RichText, Ui, vec2};
use mailcraft_model::{Folder, FolderKind, Id};
use serde_json::Value;

use crate::MailApp;
use crate::dialogs::button_row;
use crate::theme::{Tokens, semibold};
use crate::widgets::primary_button;

/// How wide the dialog with the tabs is, how tall a tab is (each as tall, so the dialog keeps its
/// size from tab to tab), and General's columns: what a line says, and what it is.
pub const WIDTH: f32 = 460.0;
const TAB_H: f32 = 190.0;
const LABEL_W: f32 = 90.0;
const VALUE_W: f32 = 320.0;
/// Folder Size's columns: the folder's name, its messages, unread, size, and size with its
/// subfolders; and the server's count, when there is one, and the space between columns.
const WIDTHS: [f32; 6] = [240.0, 80.0, 70.0, 90.0, 110.0, 100.0];
const SERVER_W: f32 = 110.0;
const GAP: f32 = 16.0;
/// How narrow the folder's name may get in a small window before the columns scroll sideways.
const NAME_MIN_W: f32 = 100.0;
/// A row's height, and the space between rows.
const ROW_H: f32 = 20.0;
const ROW_GAP: f32 = 4.0;
/// Room for the rows: at most this, and less in a low window. What else Folder Size holds below
/// the summary (the headings, the notes under the rows, Close and the frame) takes about `BELOW`.
const ROWS_MAX_H: f32 = 360.0;
const BELOW: f32 = 170.0;

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

/// A tab of the dialog, in classic Outlook's order. Only General has anything in it yet; a tab
/// gets its content with the feature behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    General,
    AutoArchive,
    Policy,
    Permissions,
    Synchronization,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::General, Tab::AutoArchive, Tab::Policy, Tab::Permissions, Tab::Synchronization];

    pub fn label(self) -> &'static str {
        match self {
            Tab::General => tl!("General"),
            Tab::AutoArchive => tl!("AutoArchive"),
            Tab::Policy => tl!("Policy"),
            Tab::Permissions => tl!("Permissions"),
            Tab::Synchronization => tl!("Synchronization"),
        }
    }
}

/// What General says of the folder or the account.
pub struct General {
    pub name: String,
    /// What kind of folder, or of account, it is.
    pub kind: String,
    /// Where it is: its account and the folders above it. An account has none: nothing is
    /// above it, and its servers are in what kind it is.
    pub location: Option<String>,
    /// What it holds itself; an account, in all of its folders.
    pub messages: u64,
    pub unread: u64,
}

/// One folder of Folder Size's table.
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
/// ([`FolderSize::descending`]).
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

/// Folder Size: what the folder and each folder under it, or the account's folders, hold.
pub struct FolderSize {
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

pub struct FolderProperties {
    /// "Inbox Properties": the folder's name, or the account's.
    pub title: String,
    pub tab: Tab,
    pub general: General,
    pub size: FolderSize,
    /// Folder Size is open over the tabs.
    pub size_open: bool,
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
        let total = Tally::read(&answer["total"]);
        let account = answer["account"].as_u64().and_then(|a| mb.account(a));
        let account_name = account.map(|a| crate::nav::heading_name(app, a)).filter(|n| !n.is_empty()).unwrap_or_else(|| tl!("Mailbox").to_string());
        let general = match answer["folder"].as_u64().and_then(|f| mb.folder(f)) {
            Some(f) => {
                let own = rows.first().filter(|r| r.id == f.id).map_or_else(Tally::default, |r| r.own);
                let above = f.parent.and_then(|p| folders.get(&p)).map(|p| shown_path(&folders, p));
                General {
                    name: crate::nav::folder_name(f).to_string(),
                    kind: kind_name(f.kind),
                    location: Some(above.map_or_else(|| account_name.clone(), |above| format!("{account_name} › {above}"))),
                    messages: own.messages,
                    unread: own.unread,
                }
            }
            None => General {
                name: account_name,
                kind: account.map(|a| crate::backstage::account_detail(app, a)).unwrap_or_default(),
                location: None,
                messages: total.messages,
                unread: total.unread,
            },
        };
        let title = crate::i18n::fmt(tl!("{folder} Properties"), &[("folder", &general.name)]);
        let server = rows.iter().any(|r| r.server.is_some());
        let order = (0..rows.len()).collect();
        let more = answer["more"].as_u64().unwrap_or(0);
        Some(FolderProperties {
            title,
            tab: Tab::General,
            general,
            size: FolderSize { rows, total, more, sort: Sort::Folder, descending: false, order, server },
            size_open: false,
        })
    }

    /// Draw the tabs; Enter closes it when `enter` (not while Folder Size is open over it). `true`
    /// when it is done with.
    pub fn show(&mut self, ui: &mut Ui, enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        // On a second line when the tabs' names don't fit on one.
        ui.horizontal_wrapped(|ui| {
            for tab in Tab::ALL {
                if ui.selectable_label(self.tab == tab, tab.label()).clicked() {
                    self.tab = tab;
                }
            }
        });
        ui.separator();
        ui.allocate_ui_with_layout(vec2(ui.available_width(), TAB_H), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_height(TAB_H);
            match self.tab {
                Tab::General => self.general_ui(ui, &t),
                _ => {
                    ui.label(RichText::new(tl!("Pending development")).color(t.text_dim));
                }
            }
        });
        let mut done = false;
        ui.add_space(4.0);
        button_row(ui, |ui| {
            if primary_button(ui, tl!("Close"), true).clicked() || enter {
                done = true;
            }
        });
        done
    }

    fn general_ui(&mut self, ui: &mut Ui, t: &Tokens) {
        let g = &self.general;
        let lines = [
            Some((tl!("Name"), g.name.clone())),
            Some((tl!("Type"), g.kind.clone())),
            g.location.clone().map(|l| (tl!("Location"), l)),
            Some((tl!("Messages"), g.messages.to_string())),
            Some((tl!("Unread"), g.unread.to_string())),
        ];
        egui::Grid::new("folder_properties_general").num_columns(2).spacing(vec2(GAP, 6.0)).show(ui, |ui| {
            for (label, value) in lines.into_iter().flatten() {
                cell(ui, LABEL_W, false, |ui| ui.label(RichText::new(label).color(t.text_dim)));
                cell(ui, VALUE_W, false, |ui| ui.add(egui::Label::new(RichText::new(value).color(t.text)).truncate()));
                ui.end_row();
            }
        });
        ui.add_space(8.0);
        if ui.button(tl!("Folder Size…")).on_hover_text(tl!("How many messages each folder holds and the space they take")).clicked() {
            self.size_open = true;
        }
    }

    /// Folder Size, over the tabs; `escape` and `enter` (pressed this frame) close it as its
    /// Close does. `true` when a folder was picked: it is then in view, and the dialog is done
    /// with.
    pub fn show_size(&mut self, app: &mut MailApp, ctx: &egui::Context, escape: bool, enter: bool) -> bool {
        let t = Tokens::get(ctx);
        let frame = egui::Frame::window(&ctx.global_style()).fill(t.surface).inner_margin(egui::Margin::same(16));
        // No wider or taller than the window, so that its title and Close stay in sight.
        let room = ctx.content_rect();
        let width = self.size.width().min(room.width() - 48.0).max(160.0);
        let height = room.height() - 48.0;
        let name = self.general.name.clone();
        let mut closed = escape || enter;
        let mut picked = None;
        egui::Modal::new(egui::Id::unique("folder-size"))
            .frame(frame)
            .backdrop_color(egui::Color32::from_black_alpha(if t.dark { 110 } else { 60 }))
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                ui.label(RichText::new(tl!("Folder Size")).font(semibold(14.0)).color(t.text));
                ui.add(egui::Label::new(RichText::new(name).color(t.text_dim)).truncate());
                let (close, pick) = self.size.show(ui, &t, height);
                closed |= close;
                picked = pick;
            });
        // A folder removed since the table was made can't be gone to: Folder Size stays open.
        if let Some(id) = picked
            && app.run_or_else("folder.select", serde_json::json!({"id": id})).is_ok()
        {
            return true;
        }
        if closed {
            self.size_open = false;
        }
        false
    }
}

impl FolderSize {
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
            Sort::Folder => 0,
            Sort::Messages => r.own.messages,
            Sort::Unread => r.own.unread,
            Sort::Size => r.own.size,
            Sort::TotalMessages => r.total.messages,
            Sort::TotalSize => r.total.size,
            Sort::Server => r.server.map_or(0, |(messages, _)| messages),
        };
        let rows = &self.rows;
        let key_of = |i: &usize| rows.get(*i).map_or(0, key);
        let mut order: Vec<usize> = (0..rows.len()).collect();
        // The pane's order, or turned round; any other column largest first, or smallest, with
        // the pane's order among equals either way (the sort is stable).
        match (sort, self.descending) {
            (Sort::Folder, false) => {}
            (Sort::Folder, true) => order.reverse(),
            (_, false) => order.sort_by_key(|i| std::cmp::Reverse(key_of(i))),
            (_, true) => order.sort_by_key(key_of),
        }
        self.order = order;
    }

    /// How wide it is: wider when there is a column of the server's counts.
    pub fn width(&self) -> f32 {
        let server = if self.server { SERVER_W + GAP } else { 0.0 };
        WIDTHS.iter().sum::<f32>() + GAP * (WIDTHS.len() - 1) as f32 + server + 24.0
    }

    /// Draw the table and its Close, in a window `height` high: whether Close was pressed, and
    /// the folder whose name was.
    fn show(&mut self, ui: &mut Ui, t: &Tokens, height: f32) -> (bool, Option<Id>) {
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
        // In a narrow window the folder's name gives up room first; past that, the columns
        // scroll sideways. In a low one there are fewer rows in sight.
        let [name_w, messages_w, unread_w, size_w, total_messages_w, total_size_w] = WIDTHS;
        let shrink = (self.width() - ui.available_width()).clamp(0.0, name_w - NAME_MIN_W);
        let widths = [name_w - shrink, messages_w, unread_w, size_w, total_messages_w, total_size_w, SERVER_W];
        let table_w = self.width() - shrink;
        let rows_h = (height - ui.min_rect().height() - BELOW).clamp(ROW_H * 3.0, ROWS_MAX_H);
        egui::ScrollArea::horizontal().id_salt("folder_size_columns").auto_shrink([false, true]).show(ui, |ui| {
            ui.set_width(table_w);
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
                for ((sort, label, hint), width) in headings.into_iter().zip(widths).take(columns) {
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
                egui::ScrollArea::vertical().max_height(rows_h).auto_shrink([false, true]).show_rows(ui, ROW_H, order.len(), |ui, shown| {
                    let start = shown.start;
                    egui::Grid::new("folder_properties").striped(true).start_row(start).num_columns(columns).spacing(vec2(GAP, ROW_GAP)).show(
                        ui,
                        |ui| {
                            let [name_w, messages_w, unread_w, size_w, total_messages_w, total_size_w, server_w] = widths;
                            for r in order.get(shown).into_iter().flatten().filter_map(|i| rows.get(*i)) {
                                // In the pane's order a folder is under its parent; in any other, its path says where it is.
                                let name = if tree { format!("{}{}", "    ".repeat(r.depth.min(12)), r.name) } else { r.path.clone() };
                                let link = cell(ui, name_w, false, |ui| {
                                    ui.add(egui::Button::new(RichText::new(name).color(t.accent_text)).frame(false).truncate())
                                });
                                if link.on_hover_text(format!("{}\n{}", r.path, tl!("Go to the folder"))).clicked() {
                                    picked = Some(r.id);
                                }
                                cell(ui, messages_w, true, |ui| ui.label(RichText::new(r.own.messages.to_string()).color(t.text)));
                                cell(ui, unread_w, true, |ui| ui.label(RichText::new(r.own.unread.to_string()).color(t.text)));
                                cell(ui, size_w, true, |ui| sized(ui, t, r.own));
                                cell(ui, total_messages_w, true, |ui| ui.label(RichText::new(r.total.messages.to_string()).color(t.text)));
                                cell(ui, total_size_w, true, |ui| sized(ui, t, r.total));
                                if server {
                                    cell(ui, server_w, true, |ui| {
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
                        },
                    );
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
        let mut close = false;
        ui.add_space(4.0);
        button_row(ui, |ui| {
            close = primary_button(ui, tl!("Close"), true).clicked();
        });
        (close, picked)
    }
}

/// What kind of folder one of `kind` is, as General says it: a folder every account has is
/// named for what it is.
fn kind_name(kind: FolderKind) -> String {
    match kind {
        FolderKind::Custom => tl!("Mail folder").to_string(),
        kind => crate::i18n::fmt(tl!("Mail folder: the account's {folder}"), &[("folder", crate::i18n::current().folder(kind.default_name()))]),
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
