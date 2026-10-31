// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The picker of Insert › Item and Insert › Contact Card: a message or a contact to attach to the
//! message being written, found by typing. It only chooses: the choice runs
//! `compose.attachItem` or `insert.businessCard` with its id.

use egui::Ui;
use mailcraft_model::{Id, contains_ci};
use serde_json::json;

use crate::MailApp;
use crate::dialogs::buttons;
use crate::theme::{paint_clipped, regular};

/// Most rows the picker lists.
const MAX_ROWS: usize = 200;
/// The dialog's width.
pub const WIDTH: f32 = 520.0;

/// What is picked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickKind {
    Message,
    Contact,
}

#[derive(Debug)]
pub struct Pick {
    pub kind: PickKind,
    query: String,
    selected: Option<Id>,
    error: String,
}

impl Pick {
    pub fn new(kind: PickKind) -> Pick {
        Pick { kind, query: String::new(), selected: None, error: String::new() }
    }

    /// The command the choice runs.
    pub fn command(&self) -> &'static str {
        match self.kind {
            PickKind::Message => "compose.attachItem",
            PickKind::Contact => "insert.businessCard",
        }
    }

    pub fn title(&self) -> &'static str {
        match self.kind {
            PickKind::Message => tl!("Attach Item"),
            PickKind::Contact => tl!("Attach a Contact Card"),
        }
    }

    /// Draws it; true once the choice is attached or it is cancelled.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        let t = crate::theme::Tokens::get(ui.ctx());
        let hint = match self.kind {
            PickKind::Message => tl!("Search by subject or sender"),
            PickKind::Contact => tl!("Search contacts"),
        };
        let resp = crate::text_menu::field(ui.add(egui::TextEdit::singleline(&mut self.query).hint_text(hint).desired_width(f32::INFINITY)));
        crate::text_menu::hold(&resp);
        let found = rows(app, self.kind, &self.query);
        if self.selected.is_none_or(|id| !found.iter().any(|r| r.0 == id)) {
            self.selected = found.first().map(|r| r.0);
        }
        let mut chosen = None;
        egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 1.0);
            for (id, first, second) in &found {
                let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 38.0), egui::Sense::click());
                if self.selected == Some(*id) {
                    ui.painter().rect_filled(rect, 4.0, t.selection);
                } else if r.hovered() {
                    ui.painter().rect_filled(rect, 4.0, t.hover);
                }
                let (x, w) = (rect.min.x + 10.0, rect.width() - 20.0);
                paint_clipped(ui.painter(), egui::pos2(x, rect.min.y + 11.0), w, first, regular(13.0), t.text);
                paint_clipped(ui.painter(), egui::pos2(x, rect.min.y + 27.0), w, second, regular(11.0), t.text_dim);
                if r.clicked() {
                    self.selected = Some(*id);
                }
                if r.double_clicked() {
                    chosen = Some(*id);
                }
            }
            if found.is_empty() {
                ui.label(egui::RichText::new(tl!("Nothing matches.")).color(t.text_dim));
            }
        });
        if !self.error.is_empty() {
            ui.label(egui::RichText::new(self.error.as_str()).color(t.red));
        }
        let (ok, cancel) = buttons(ui, tl!("Attach"), self.selected.is_some());
        if (ok || enter) && chosen.is_none() {
            chosen = self.selected;
        }
        if let Some(id) = chosen {
            match app.execute(self.command(), json!({"id": id})) {
                Ok(_) => return true,
                Err(e) => self.error = e,
            }
        }
        cancel
    }
}

/// What matches `query`, newest message first or contacts in the People list's order, as
/// `(id, first line, second line)`.
pub fn rows(app: &MailApp, kind: PickKind, query: &str) -> Vec<(Id, String, String)> {
    let mb = &app.session.mailbox;
    let q = query.trim();
    match kind {
        PickKind::Message => {
            let mut v: Vec<&mailcraft_model::Message> = mb
                .messages
                .values()
                .map(|m| m.as_ref())
                .filter(|m| q.is_empty() || contains_ci(&m.subject, q) || contains_ci(&m.from.name, q) || contains_ci(&m.from.email, q))
                .collect();
            v.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
            v.into_iter()
                .take(MAX_ROWS)
                .map(|m| {
                    let subject = if m.subject.trim().is_empty() { tl!("(no subject)").to_string() } else { m.subject.clone() };
                    let folder = mb.folder(m.folder).map(|f| f.name.clone()).unwrap_or_default();
                    (m.id, subject, format!("{} · {}", m.from.display(), folder))
                })
                .collect()
        }
        PickKind::Contact => mailcraft_engine::cmd::people::sorted(&app.session, q)
            .into_iter()
            .take(MAX_ROWS)
            .filter_map(|id| mb.contact(id))
            .map(|c| (c.id, c.name(), c.address().map(|a| a.email).unwrap_or_default()))
            .collect(),
    }
}
