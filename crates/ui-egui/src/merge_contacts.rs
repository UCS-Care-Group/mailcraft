// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People › Merge Duplicates (#506): the groups of contacts that share an e-mail address or a
//! phone number (or the contacts picked together in the list), one at a time. The owner picks
//! the contact to keep and, for each field whose values differ, what it has; Merge runs
//! `people.merge` (one undo step), Skip goes on to the next group. Nothing changes until Merge.

use std::collections::BTreeMap;

use egui::{RichText, Ui, vec2};
use mailcraft_engine::cmd::people_merge::{self, Group, LISTS, Pick};
use mailcraft_model::{Contact, Id};
use serde_json::{Value, json};

use crate::MailApp;
use crate::dialogs::button_row;
use crate::people_view::one_line;
use crate::theme::Tokens;
use crate::widgets::primary_button;

pub const WIDTH: f32 = 600.0;
/// Characters of a value a choice shows.
const SHOWN: usize = 70;

pub struct MergeContacts {
    groups: Vec<Group>,
    /// Groups past those listed ([`people_merge::MAX_GROUPS`]).
    more: usize,
    /// The group shown.
    at: usize,
    keep: Id,
    picks: BTreeMap<String, Pick>,
    /// Groups merged so far.
    merged: usize,
    /// Opened on the contacts picked in the list, rather than on every group.
    picked: bool,
    error: String,
}

/// What a field is called.
fn field_name(field: &str) -> &'static str {
    match field {
        "displayName" => tl!("Display name"),
        "first" => tl!("First name"),
        "middle" => tl!("Middle name"),
        "last" => tl!("Last name"),
        "nickname" => tl!("Nickname"),
        "company" => tl!("Company"),
        "title" => tl!("Job title"),
        "department" => tl!("Department"),
        "birthday" => tl!("Birthday"),
        "emails" => tl!("E-mail"),
        "phones" => tl!("Phone"),
        "addresses" => tl!("Address"),
        "websites" => tl!("Website"),
        "categories" => tl!("Categories"),
        "notes" => tl!("Notes"),
        _ => "",
    }
}

impl MergeContacts {
    /// The groups for `params`: `ids`, the contacts picked together, else every group.
    pub fn open(app: &MailApp, params: &Value) -> Option<MergeContacts> {
        let mailbox = &app.session.mailbox;
        let ids: Option<Vec<Id>> = params.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect());
        let (groups, more, picked) = match ids {
            Some(ids) => (vec![people_merge::selection(mailbox, &ids).ok()?], 0, true),
            None => {
                let (groups, more) = people_merge::find(mailbox);
                (groups, more, false)
            }
        };
        let mut d = MergeContacts { groups, more, at: 0, keep: 0, picks: BTreeMap::new(), merged: 0, picked, error: String::new() };
        d.go_to(0);
        Some(d)
    }

    fn go_to(&mut self, at: usize) {
        self.at = at;
        self.keep = self.groups.get(at).map_or(0, |g| g.keep);
        self.picks.clear();
        self.error.clear();
    }

    /// The contacts merged into the kept one.
    fn others(&self) -> Vec<Id> {
        self.groups.get(self.at).map(|g| g.free().into_iter().filter(|id| *id != self.keep).collect()).unwrap_or_default()
    }

    /// The dialog's contents; `true` when it is done.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| self.contents(app, ui, enter)).inner
    }

    fn contents(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        let Some(group) = self.groups.get(self.at).cloned() else {
            // Nothing (left) to merge.
            let text = if self.merged > 0 {
                tln!(self.merged as u64, "{n} group of contacts was merged.", "{n} groups of contacts were merged.")
            } else {
                tl!("No two contacts share an e-mail address or a phone number.").to_string()
            };
            ui.label(text);
            ui.add_space(4.0);
            let mut ok = enter;
            button_row(ui, |ui| ok |= primary_button(ui, tl!("OK"), true).clicked());
            return ok;
        };
        // Which group, and what its contacts share.
        if !self.picked {
            let total = self.groups.len().saturating_add(self.more);
            let heading = crate::i18n::fmt(tl!("Group {n} of {total}"), &[("n", &(self.at + 1).to_string()), ("total", &total.to_string())]);
            ui.label(RichText::new(heading).strong());
            if !group.shares.is_empty() {
                let shares = group.shares.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
                ui.label(RichText::new(crate::i18n::fmt(tl!("They share {what}."), &[("what", &one_line(&shares, 120))])).color(t.text_dim));
            }
        } else {
            ui.label(RichText::new(tl!("The contacts you picked")).strong());
        }
        ui.add_space(6.0);
        ui.label(tl!("Keep this contact, and merge the others into it:"));
        let lang = crate::i18n::current();
        egui::ScrollArea::vertical().id_salt("merge_members").max_height(170.0).auto_shrink([false, true]).show(ui, |ui| {
            for id in &group.ids {
                let Some(c) = app.session.mailbox.contact(*id) else { continue };
                let locked = group.locked.iter().find(|(l, _)| l == id).map(|(_, why)| why.in_lang(lang));
                ui.horizontal(|ui| {
                    let on = self.keep == *id;
                    if ui.add_enabled(locked.is_none(), egui::RadioButton::new(on, RichText::new(one_line(&c.name(), 60)).strong())).clicked() && !on
                    {
                        self.keep = *id;
                        self.error.clear();
                    }
                    ui.label(RichText::new(one_line(&summary(c), 80)).color(t.text_dim));
                });
                if let Some(why) = locked {
                    ui.add_space(-2.0);
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        ui.label(RichText::new(why).color(t.text_dim).italics());
                    });
                }
            }
            if group.more > 0 {
                ui.label(RichText::new(tln!(group.more as u64, "and {n} more, not merged now", "and {n} more, not merged now")).color(t.text_dim));
            }
        });
        let mergeable = group.mergeable();
        let others = self.others();
        let preview = if mergeable { Some(people_merge::merged(&app.session.mailbox, self.keep, &others, &self.picks)) } else { None };
        ui.add_space(6.0);
        if mergeable {
            let differ = people_merge::differing(&app.session.mailbox, &group.free());
            if differ.is_empty() {
                ui.label(RichText::new(tl!("Their details are the same; the kept contact loses nothing.")).color(t.text_dim));
            } else {
                ui.label(tl!("Where they differ, the kept contact has:"));
                egui::ScrollArea::vertical().id_salt("merge_fields").max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                    egui::Grid::new("merge_fields_grid").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                        for (field, values) in differ {
                            ui.label(field_name(field));
                            self.choice(ui, field, &values, preview.as_ref().and_then(|p| p.as_ref().ok()));
                            ui.end_row();
                        }
                    });
                });
            }
            ui.add_space(4.0);
            let n = others.len() as u64;
            ui.label(
                RichText::new(tln!(
                    n,
                    "{n} contact is deleted once the kept one is saved. Undo brings it back.",
                    "{n} contacts are deleted once the kept one is saved. Undo brings them back."
                ))
                .color(t.text_dim),
            );
        } else {
            ui.label(RichText::new(tl!("These contacts can't be merged here: fewer than two of them can be changed.")).color(t.text_dim));
        }
        let refused = match &preview {
            Some(Err(e)) => e.in_lang(lang),
            _ => String::new(),
        };
        for line in [&self.error, &refused].into_iter().filter(|l| !l.is_empty()) {
            ui.label(RichText::new(line.as_str()).color(t.red));
        }
        let can = matches!(preview, Some(Ok(_)));
        let (mut merge, mut skip, mut close) = (false, false, false);
        ui.add_space(4.0);
        button_row(ui, |ui| {
            close = ui.add(egui::Button::new(tl!("Close")).min_size(vec2(72.0, 26.0))).clicked();
            if self.groups.len() > 1 || self.more > 0 {
                skip = ui.add(egui::Button::new(tl!("Skip")).min_size(vec2(72.0, 26.0))).clicked();
            }
            merge = primary_button(ui, tl!("Merge"), can).clicked() || (enter && can);
        });
        if close {
            return true;
        }
        if merge {
            let fields: serde_json::Map<String, Value> = self
                .picks
                .iter()
                .map(|(k, p)| {
                    (
                        k.clone(),
                        match p {
                            Pick::All => json!("all"),
                            Pick::From(id) => json!(id),
                        },
                    )
                })
                .collect();
            match app.run("people.merge", json!({"keep": self.keep, "others": others, "fields": fields})) {
                Ok(_) => {
                    self.merged = self.merged.saturating_add(1);
                    if self.picked {
                        return true;
                    }
                    self.go_to(self.at.saturating_add(1));
                }
                Err(e) => self.error = e,
            }
        } else if skip {
            self.go_to(self.at.saturating_add(1));
        }
        false
    }

    /// The choice for one field: everybody's (a field of several values), or one contact's.
    fn choice(&mut self, ui: &mut Ui, field: &'static str, values: &[(Id, String)], merged: Option<&Contact>) {
        let list = LISTS.contains(&field);
        let all = tl!("All of them");
        let pick = self.picks.get(field).copied();
        let shown = match pick {
            Some(Pick::From(id)) => values.iter().find(|(v, _)| *v == id).map(|(_, s)| s.clone()).unwrap_or_default(),
            _ if list => all.to_string(),
            _ => merged.map(|c| people_merge::field_text(c, field)).unwrap_or_default(),
        };
        egui::ComboBox::from_id_salt(("merge_field", field)).width(400.0).selected_text(one_line(&shown, SHOWN)).show_ui(ui, |ui| {
            if list && ui.selectable_label(matches!(pick, None | Some(Pick::All)), all).clicked() {
                self.picks.remove(field);
            }
            for (id, value) in values {
                let on = match pick {
                    Some(Pick::From(x)) => x == *id,
                    _ => !list && *value == shown,
                };
                if ui.selectable_label(on, one_line(value, SHOWN)).clicked() {
                    self.picks.insert(field.to_string(), Pick::From(*id));
                }
            }
        });
    }
}

/// A contact's addresses, numbers and company, on one line.
fn summary(c: &Contact) -> String {
    let emails = c.emails.iter().map(|e| e.value.trim()).filter(|e| !e.is_empty()).take(3);
    let phones = c.phones.iter().map(|p| p.value.trim()).filter(|p| !p.is_empty()).take(2);
    emails.chain(phones).chain(std::iter::once(c.company.trim()).filter(|s| !s.is_empty())).collect::<Vec<_>>().join(" · ")
}

#[cfg(test)]
#[path = "merge_contacts_tests.rs"]
mod merge_contacts_tests;
