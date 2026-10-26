// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People › Merge Duplicates (#506): the groups of contacts that share an e-mail address or a
//! phone number (or the contacts picked together in the list), one at a time. The owner ticks the
//! contacts that are the same person (all of them at first), picks the one to keep and, for each
//! field whose values differ, what it has; Merge runs `people.merge` (one undo step), Skip goes on
//! to the next group. Nothing changes until Merge is clicked: Enter doesn't press it, since it
//! deletes contacts.

use std::collections::{BTreeMap, BTreeSet};

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

/// What the choices come to, worked out again only when one of them or the mailbox changes.
struct Worked {
    key: (usize, usize, Id, BTreeMap<String, Pick>, BTreeSet<Id>, u64),
    differ: Vec<(&'static str, Vec<(Id, String)>)>,
    /// The kept contact as the merge would leave it, or why the merge would be refused.
    preview: Result<Contact, String>,
}

pub struct MergeContacts {
    groups: Vec<Group>,
    /// Groups past those listed ([`people_merge::MAX_GROUPS`]).
    more: usize,
    /// The group shown.
    at: usize,
    keep: Id,
    picks: BTreeMap<String, Pick>,
    /// Contacts of the group the owner unticked: not the same person, left as they are.
    left_out: BTreeSet<Id>,
    /// Groups merged so far.
    merged: usize,
    /// Opened on the contacts picked in the list, rather than on every group.
    picked: bool,
    /// Said where there is nothing to merge (the picked contacts are gone).
    note: String,
    error: String,
    worked: Option<Worked>,
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

fn too_few() -> String {
    tl!("Fewer than two of these contacts are still here, so there is nothing to merge.").to_string()
}

impl MergeContacts {
    /// The groups for `params`: `ids`, the contacts picked together, else every group.
    pub fn open(app: &MailApp, params: &Value) -> Option<MergeContacts> {
        let mailbox = &app.session.mailbox;
        let ids: Option<Vec<Id>> = params.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect());
        let mut note = String::new();
        let (groups, more, picked) = match ids {
            Some(ids) => {
                // (Picked contacts that have gone since are left out.)
                let mut seen = BTreeSet::new();
                let here: Vec<Id> = ids.into_iter().filter(|id| mailbox.contact(*id).is_some() && seen.insert(*id)).collect();
                match people_merge::selection(mailbox, &here) {
                    Ok(g) => (vec![g], 0, true),
                    Err(_) => {
                        note = too_few();
                        (Vec::new(), 0, true)
                    }
                }
            }
            None => {
                let (groups, more) = people_merge::find(mailbox);
                (groups, more, false)
            }
        };
        let mut d = MergeContacts {
            groups,
            more,
            at: 0,
            keep: 0,
            picks: BTreeMap::new(),
            left_out: BTreeSet::new(),
            merged: 0,
            picked,
            note,
            error: String::new(),
            worked: None,
        };
        d.go_to(0);
        Some(d)
    }

    fn go_to(&mut self, at: usize) {
        self.at = at;
        self.keep = self.groups.get(at).map_or(0, |g| g.keep);
        self.picks.clear();
        self.left_out.clear();
        self.error.clear();
    }

    /// Leave out of the group shown the contacts that have gone since it was found (a Send/Receive,
    /// an undo): they are no longer listed, ticked, kept or chosen from.
    fn prune(&mut self, app: &MailApp) {
        let mailbox = &app.session.mailbox;
        let Some(group) = self.groups.get_mut(self.at) else { return };
        if group.ids.iter().all(|id| mailbox.contact(*id).is_some()) {
            return;
        }
        group.ids.retain(|id| mailbox.contact(*id).is_some());
        group.locked.retain(|(id, _)| mailbox.contact(*id).is_some());
        if mailbox.contact(self.keep).is_none() {
            self.keep = group.free().first().copied().unwrap_or(0);
            self.left_out.remove(&self.keep);
        }
        self.left_out.retain(|id| mailbox.contact(*id).is_some());
        self.picks.retain(|_, p| !matches!(p, Pick::From(id) if mailbox.contact(*id).is_none()));
    }

    /// Untick `id` (it is not the same person): it is merged no more, nor any field taken from it.
    fn leave_out(&mut self, id: Id) {
        self.left_out.insert(id);
        self.picks.retain(|_, p| *p != Pick::From(id));
    }

    /// The contacts of the group that are merged: those that can change and are ticked, the kept
    /// one always.
    fn ticked(&self) -> Vec<Id> {
        self.groups
            .get(self.at)
            .map(|g| g.free().into_iter().filter(|id| *id == self.keep || !self.left_out.contains(id)).collect())
            .unwrap_or_default()
    }

    /// The contacts merged into the kept one.
    fn others(&self) -> Vec<Id> {
        self.ticked().into_iter().filter(|id| *id != self.keep).collect()
    }

    /// What differs and what the merge would come to: the last frame's, unless a choice or the
    /// mailbox changed since.
    fn work(&mut self, app: &MailApp) -> Worked {
        let listed = self.groups.get(self.at).map_or(0, |g| g.ids.len());
        let key = (self.at, listed, self.keep, self.picks.clone(), self.left_out.clone(), app.session.rev());
        match self.worked.take() {
            Some(w) if w.key == key => w,
            _ => {
                let mailbox = &app.session.mailbox;
                let lang = crate::i18n::current();
                let differ = people_merge::differing(mailbox, &self.ticked());
                let preview = people_merge::merged(mailbox, self.keep, &self.others(), &self.picks).map_err(|e| e.in_lang(lang));
                Worked { key, differ, preview }
            }
        }
    }

    /// Merge the ticked contacts of the group, of those still here; whether it was done. When it
    /// can't be, `error` says why.
    fn press_merge(&mut self, app: &mut MailApp) -> bool {
        let here = |id: &Id| app.session.mailbox.contact(*id).is_some();
        let others: Vec<Id> = self.others().into_iter().filter(here).collect();
        if !here(&self.keep) || others.is_empty() {
            self.error = too_few();
            return false;
        }
        let fields: serde_json::Map<String, Value> = self
            .picks
            .iter()
            .filter(|(_, p)| !matches!(p, Pick::From(id) if !here(id)))
            .map(|(k, p)| {
                let v = match p {
                    Pick::All => json!("all"),
                    Pick::From(id) => json!(id),
                };
                (k.clone(), v)
            })
            .collect();
        match app.run("people.merge", json!({"keep": self.keep, "others": others, "fields": fields})) {
            Ok(_) => {
                self.merged = self.merged.saturating_add(1);
                true
            }
            Err(e) => {
                self.error = e;
                false
            }
        }
    }

    /// The dialog's contents; `true` when it is done. `enter` only closes the last page: it never
    /// merges.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| self.contents(app, ui, enter)).inner
    }

    fn contents(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        self.prune(app);
        let Some(group) = self.groups.get(self.at).cloned() else {
            // Nothing (left) to merge.
            let text = if !self.note.is_empty() {
                self.note.clone()
            } else if self.merged > 0 {
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
        ui.label(tl!("Tick the contacts that are the same person, and choose the one to keep:"));
        let lang = crate::i18n::current();
        egui::ScrollArea::vertical().id_salt("merge_members").max_height(170.0).auto_shrink([false, true]).show(ui, |ui| {
            for id in &group.ids {
                let Some(c) = app.session.mailbox.contact(*id) else { continue };
                let locked = group.locked.iter().find(|(l, _)| l == id).map(|(_, why)| why.in_lang(lang));
                let kept = self.keep == *id;
                ui.horizontal(|ui| {
                    let mut on = locked.is_none() && (kept || !self.left_out.contains(id));
                    let tip = if kept { tl!("The kept contact is always merged") } else { tl!("Merge this contact") };
                    if ui.add_enabled(locked.is_none() && !kept, egui::Checkbox::without_text(&mut on)).on_hover_text(tip).changed() {
                        if on {
                            self.left_out.remove(id);
                        } else {
                            self.leave_out(*id);
                        }
                        self.error.clear();
                    }
                    let name = RichText::new(one_line(&c.name(), 60)).strong();
                    if ui.add_enabled(locked.is_none(), egui::RadioButton::new(kept, name)).on_hover_text(tl!("Keep this one")).clicked() && !kept {
                        self.keep = *id;
                        self.left_out.remove(id);
                        self.error.clear();
                    }
                    ui.label(RichText::new(one_line(&summary(c), 80)).color(t.text_dim));
                });
                if let Some(why) = locked {
                    ui.add_space(-2.0);
                    ui.horizontal(|ui| {
                        ui.add_space(48.0);
                        ui.label(RichText::new(why).color(t.text_dim).italics());
                    });
                }
            }
            if group.more > 0 {
                ui.label(RichText::new(tln!(group.more as u64, "and {n} more, not merged now", "and {n} more, not merged now")).color(t.text_dim));
            }
        });
        let others = self.others();
        let mergeable = group.mergeable() && !others.is_empty();
        let worked = self.work(app);
        let (differ, preview) = (&worked.differ, &worked.preview);
        ui.add_space(6.0);
        if mergeable {
            if differ.is_empty() {
                ui.label(RichText::new(tl!("Their details are the same; the kept contact loses nothing.")).color(t.text_dim));
            } else {
                ui.label(tl!("Where they differ, the kept contact has:"));
                egui::ScrollArea::vertical().id_salt("merge_fields").max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                    egui::Grid::new("merge_fields_grid").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                        for (field, values) in differ {
                            ui.label(field_name(field));
                            self.choice(ui, field, values, preview.as_ref().ok());
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
        } else if group.mergeable() {
            ui.label(RichText::new(tl!("Tick at least one more contact to merge into the kept one.")).color(t.text_dim));
        } else {
            ui.label(RichText::new(tl!("These contacts can't be merged here: fewer than two of them can be changed.")).color(t.text_dim));
        }
        let refused = match preview {
            Err(e) if mergeable => e.clone(),
            _ => String::new(),
        };
        for line in [&self.error, &refused].into_iter().filter(|l| !l.is_empty()) {
            ui.label(RichText::new(line.as_str()).color(t.red));
        }
        let can = mergeable && preview.is_ok();
        // Kept for the next frame.
        self.worked = Some(worked);
        let (mut merge, mut skip, mut close) = (None, None, false);
        ui.add_space(4.0);
        // (Merge is a plain button: it deletes contacts, so Enter is not a way to it. Each group's
        // buttons are widgets of their own, so that the keyboard on one group's Merge is not on
        // the next one's.)
        let at = self.at;
        button_row(ui, |ui| {
            ui.push_id(("merge_group", at), |ui| {
                close = ui.add(egui::Button::new(tl!("Close")).min_size(vec2(72.0, 26.0))).clicked();
                if self.groups.len() > 1 || self.more > 0 {
                    skip = Some(ui.add(egui::Button::new(tl!("Skip")).min_size(vec2(72.0, 26.0))));
                }
                merge = Some(ui.add_enabled(can, egui::Button::new(tl!("Merge")).min_size(vec2(72.0, 26.0))));
            });
        });
        if close {
            return true;
        }
        let pressed = |r: &Option<egui::Response>| r.as_ref().is_some_and(egui::Response::clicked);
        let moved = if pressed(&merge) && self.press_merge(app) {
            if self.picked {
                return true;
            }
            true
        } else {
            pressed(&skip)
        };
        if moved {
            // Nothing of this group keeps the keyboard: Enter or Space must not go on to the next.
            for r in [&merge, &skip].into_iter().flatten() {
                r.surrender_focus();
            }
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
