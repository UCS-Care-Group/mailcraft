// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The list under the search box that offers what the word being typed may be (#308): a keyword
//! from its start (`cat` → `category:`), the mailbox's categories after `category:`, what `is:`
//! and `has:` take, and people after `from:`, `to:` and `cc:`, each by name with the address
//! beside it, from the contacts and the mail on this computer
//! ([`mailcraft_engine::cmd::view::search_suggestions`] says what). The arrow keys
//! choose; Enter takes the one chosen, Tab that one or the first, a click the one under it. Until
//! an arrow key is pressed nothing is chosen, so Enter does what it did. The list is egui's popup:
//! Escape closes it and leaves the search (and a message being written open); the next Escape
//! clears it, as before. It stays closed until the search or the cursor changes, or the box is
//! left. With Shift, Ctrl or Alt held, the keys are not the list's; Shift+Tab leaves the box, as
//! without it.

use egui::{Context, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_model::search::Suggestion;

use crate::theme::{Tokens, c32, paint_clipped, regular};

/// How tall a row of the list is, and the side of a category's mark.
const ROW_H: f32 = 26.0;
const MARK: f32 = 10.0;

/// The list, from one frame to the next.
#[derive(Clone, Default)]
pub(crate) struct Suggesting {
    pub items: Vec<Suggestion>,
    /// The one the arrow keys chose.
    pub selected: Option<usize>,
    /// Whether the list was to show in the frame before, and whether the search or the cursor
    /// changed since: it is opened only then (when it comes to be wanted, and for a search or
    /// a cursor that changed), not on every frame, so that what egui closed (Escape) stays closed.
    wanted: bool,
    moved: bool,
    /// What `items` are for: the search, the cursor in it, the mailbox's categories (a renamed
    /// one is offered by its new name), and the mailbox as it was ([`Session::rev`] and
    /// [`Session::openings`]: the people that mail arriving, or another mailbox, brings are
    /// offered without a key being pressed). A take is only of items for the search as it is now.
    ///
    /// [`Session::rev`]: mailcraft_engine::Session::rev
    /// [`Session::openings`]: mailcraft_engine::Session::openings
    pub for_text: String,
    for_cursor: usize,
    for_categories: Vec<mailcraft_model::Category>,
    for_mailbox: (u64, u64),
}

impl Suggesting {
    fn store_id() -> egui::Id {
        egui::Id::unique("search_suggesting")
    }

    /// The list's popup, and its area.
    fn list_id() -> egui::Id {
        Self::store_id().with("list")
    }

    pub fn load(ctx: &Context) -> Suggesting {
        ctx.data(|d| d.get_temp(Self::store_id())).unwrap_or_default()
    }

    pub fn store(self, ctx: &Context) {
        ctx.data_mut(|d| d.insert_temp(Self::store_id(), self));
    }

    /// Whether the list shows, as egui last drew it.
    pub fn is_open(ctx: &Context) -> bool {
        egui::Popup::is_id_open(ctx, Self::list_id())
    }

    /// Whether the list will show in the next frame, while the search may be offered for (`free`).
    pub fn will_show(&self, ctx: &Context, free: bool) -> bool {
        free && !self.items.is_empty() && (Self::is_open(ctx) || self.moved || !self.wanted)
    }

    /// Whether the pointer is over the list.
    pub fn pointer_over(&self, ui: &Ui) -> bool {
        let list = egui::LayerId::new(egui::Order::Foreground, Self::list_id());
        ui.ctx().pointer_hover_pos().and_then(|p| ui.ctx().layer_id_at(p)) == Some(list)
    }

    /// The keys meant for the list while it shows, taken before the box sees them; which item to
    /// take, if one. (Escape is egui's: it closes the list.)
    pub fn keys(&mut self, ui: &Ui) -> Option<usize> {
        let n = self.items.len();
        let mut take = None;
        let mut back = false;
        ui.input_mut(|i| {
            // The box keeps every Tab while the list shows, so Shift+Tab is done here.
            if i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab) {
                back = true;
                return;
            }
            if !i.modifiers.is_none() {
                return;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                self.selected = Some(self.selected.map_or(0, |s| s.saturating_add(1) % n.max(1)));
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                self.selected = Some(self.selected.map_or(n.saturating_sub(1), |s| s.checked_sub(1).unwrap_or(n.saturating_sub(1))));
            }
            if self.selected.is_some() && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                take = self.selected;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
                take = Some(self.selected.unwrap_or(0));
            }
        });
        if back {
            ui.memory_mut(|m| m.move_focus(egui::FocusDirection::Previous));
        }
        take
    }

    /// The list under `under` (the search box), when it is `wanted`: opened when it comes to be,
    /// or for a search or a cursor that changed; closed when it isn't. The item clicked, if one.
    /// A click outside it is no matter of the list's: one away from the box takes the keyboard
    /// from it, and that closes the list.
    pub fn show(&mut self, ui: &Ui, under: Rect, wanted: bool) -> Option<usize> {
        let t = Tokens::get(ui.ctx());
        let open = if !wanted {
            Some(egui::SetOpenCommand::Bool(false))
        } else if !self.wanted || self.moved {
            Some(egui::SetOpenCommand::Bool(true))
        } else {
            None
        };
        (self.wanted, self.moved) = (wanted, false);
        let frame = egui::Frame::NONE.fill(t.menu).stroke(Stroke::new(1.0, t.border_strong)).corner_radius(6.0).inner_margin(egui::Margin::same(4));
        let mut take = None;
        egui::Popup::new(Self::list_id(), ui.ctx().clone(), under, ui.layer_id())
            .gap(2.0)
            .frame(frame)
            .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
            .open_memory(open)
            .show(|ui| {
                ui.set_width((under.width() - 8.0).max(160.0));
                for (i, s) in self.items.iter().enumerate() {
                    let (r, row) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                    let named = shows_address(s).map_or_else(|| s.label.clone(), |address| format!("{}, {address}", s.label));
                    row.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &named));
                    if self.selected == Some(i) || row.hovered() {
                        ui.painter().rect_filled(r, 4.0, if self.selected == Some(i) { t.selection } else { t.hover });
                    }
                    let mut x = r.min.x + 8.0;
                    if let Some(color) = s.color {
                        ui.painter().rect_filled(Rect::from_center_size(pos2(x + MARK / 2.0, r.center().y), vec2(MARK, MARK)), 2.0, c32(color));
                        x += MARK + 8.0;
                    }
                    // Somebody: the name, and the address beside it, as the recipient suggestions show them.
                    let room = (r.max.x - x - 8.0).max(0.0);
                    let w = paint_clipped(
                        ui.painter(),
                        pos2(x, r.center().y),
                        if shows_address(s).is_some() { room * 0.6 } else { room },
                        &s.label,
                        regular(12.5),
                        t.text,
                    );
                    if let Some(address) = shows_address(s) {
                        paint_clipped(ui.painter(), pos2(x + w + 8.0, r.center().y), (room - w - 8.0).max(0.0), address, regular(11.5), t.text_dim);
                    }
                    if row.clicked() {
                        take = Some(i);
                    }
                }
            });
        take
    }

    /// What to offer next frame, for `text` with the cursor at character `cursor`.
    pub fn offer(&mut self, text: &str, cursor: usize, session: &mailcraft_engine::Session) {
        let categories = session.mailbox.categories.as_slice();
        self.moved |= (self.for_text.as_str(), self.for_cursor) != (text, cursor);
        let mailbox = (session.rev(), session.openings());
        if (self.for_text.as_str(), self.for_cursor, self.for_categories.as_slice(), self.for_mailbox) == (text, cursor, categories, mailbox) {
            return;
        }
        let byte = text.char_indices().nth(cursor).map_or(text.len(), |(b, _)| b);
        let items = mailcraft_engine::cmd::view::search_suggestions(session, text, byte);
        if items != self.items {
            self.selected = None;
        }
        self.items = items;
        (self.for_text, self.for_cursor, self.for_categories, self.for_mailbox) = (text.to_string(), cursor, categories.to_vec(), mailbox);
    }

    /// Closes the list and forgets what it offered: Advanced Search opened over it.
    pub fn close(ctx: &Context) {
        egui::Popup::close_id(ctx, Self::list_id());
        Suggesting::default().store(ctx);
    }

    pub fn clear(&mut self) {
        *self = Suggesting { wanted: self.wanted, ..Suggesting::default() };
    }
}

/// The address shown beside a person's name: not when the name is the address.
fn shows_address(s: &Suggestion) -> Option<&str> {
    s.address.as_deref().filter(|address| !address.eq_ignore_ascii_case(&s.label))
}

/// `text` with `s` taken, and where the cursor goes after it (in characters).
pub(crate) fn taken(text: &str, s: &Suggestion) -> (String, usize) {
    let (Some(before), Some(after)) = (text.get(..s.range.start), text.get(s.range.end..)) else { return (text.to_string(), text.chars().count()) };
    let cursor = before.chars().count().saturating_add(s.text.chars().count());
    (format!("{before}{}{after}", s.text), cursor)
}

#[cfg(test)]
#[path = "search_suggest_tests.rs"]
mod search_suggest_tests;
