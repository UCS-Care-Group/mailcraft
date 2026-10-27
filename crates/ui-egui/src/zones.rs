// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Time zones in the window: the list a zone is picked from, and how a zone is called in a line.
//!
//! What a zone is, and how times are kept, is the engine's (`mailcraft_engine::cmd::zones`,
//! `mailcraft_model::zoned`); here is only how it looks. The names of zones are our own wording:
//! the label a zone has (its Windows name without "Standard", which is an identifier any
//! calendar file may carry) and the place the database names it after.

use egui::Ui;
use mailcraft_engine::Session;
use mailcraft_time::{Timestamp, Zone};

use crate::theme::Tokens;

/// Most zones one opening of the list shows; typing finds the rest.
const LISTED: usize = 160;

/// `(UTC-05:00) Central Time`: a zone in the field that shows which one is chosen.
pub(crate) fn short(zone: Zone, now: Timestamp) -> String {
    if zone.is_local() { tl!("This computer's time").to_string() } else { format!("({}) {}", zone.offset_label(now), tl!(&zone.label())) }
}

/// `(UTC-05:00) Central Time · Chicago`: a zone as a row of the list.
pub(crate) fn row(zone: Zone, now: Timestamp) -> String {
    let (label, place) = (zone.label(), zone.place());
    if place.is_empty() || label.contains(&place) { short(zone, now) } else { format!("{} · {place}", short(zone, now)) }
}

/// A drop-down that shows the zone `current` and lists the zones to choose from, the computer's
/// own first, then the ones chosen lately, then west to east; what is typed into its field
/// (`query`) narrows the list, to any zone there is. The zone that was picked, when one was.
pub(crate) fn picker(ui: &mut Ui, id: egui::Id, s: &Session, current: Zone, query: &mut String, width: f32) -> Option<Zone> {
    let now = s.now();
    let t = Tokens::get(ui.ctx());
    let mut picked = None;
    let was_open = ui.data(|data| data.get_temp::<bool>(id).unwrap_or(false));
    let shown = egui::ComboBox::from_id_salt(id)
        .width(width)
        .height(300.0)
        .selected_text(short(current, now))
        .truncate()
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            ui.set_min_width(width.max(300.0));
            let field = ui.add(egui::TextEdit::singleline(query).hint_text(tl!("Search time zones")).desired_width(f32::INFINITY));
            let field = crate::text_menu::field(field);
            // The list has just opened: what is typed goes into its field.
            if !was_open {
                field.request_focus();
            }
            let zones = mailcraft_engine::cmd::zones::choices(s, query, LISTED);
            if zones.is_empty() {
                ui.label(egui::RichText::new(tl!("No time zone is called that")).color(t.text_dim));
            }
            for zone in zones {
                if ui.selectable_label(zone == current, row(zone, now)).clicked() {
                    picked = Some(zone);
                    ui.close();
                }
            }
        });
    let open = shown.inner.is_some();
    ui.data_mut(|data| data.insert_temp(id, open));
    if picked.is_some() || (!open && was_open) {
        query.clear();
    }
    picked
}

/// What the calendar's time bar says of a zone: the label the owner gave it, else a short name
/// for it, its offset from UTC when it has no name of a few letters.
pub(crate) fn bar_label(zone: Zone, given: &str, now: Timestamp) -> String {
    let given = given.trim();
    if !given.is_empty() {
        return given.chars().take(12).collect();
    }
    // `UTC-5`, `UTC+5:30`: short enough for the bar, and what a traveller counts in.
    let minutes = zone.offset_at(now);
    let sign = if minutes < 0 { '-' } else { '+' };
    match (minutes.abs() / 60, minutes.abs() % 60) {
        (0, 0) => "UTC".into(),
        (hours, 0) => format!("UTC{sign}{hours}"),
        (hours, rest) => format!("UTC{sign}{hours}:{rest:02}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_are_called_by_their_offset_and_their_name() {
        // October 12, 2026.
        let now = 1_791_817_200;
        let zone = |name: &str| Zone::named(name).unwrap();
        assert_eq!(short(zone("America/Chicago"), now), "(UTC-05:00) Central Time");
        assert_eq!(row(zone("America/Chicago"), now), "(UTC-05:00) Central Time · Chicago");
        assert_eq!(row(zone("Asia/Tokyo"), now), "(UTC+09:00) Tokyo Time");
        assert_eq!(row(zone("America/Detroit"), now), "(UTC-04:00) Detroit");
        assert_eq!(row(Zone::UTC, now), "(UTC) UTC");
        assert_eq!(short(Zone::LOCAL, now), "This computer's time");
        assert_eq!(bar_label(zone("America/Chicago"), "", now), "UTC-5");
        assert_eq!(bar_label(zone("Asia/Kolkata"), "", now), "UTC+5:30");
        assert_eq!(bar_label(Zone::UTC, " ", now), "UTC");
        assert_eq!(bar_label(zone("Asia/Kolkata"), " Pune office, second floor ", now), "Pune office,");
    }
}
