// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A calendar sharing invitation, above the message that carries it (#70): "Open this calendar"
//! opens the sender's calendar by their address (`calendar.openShared`), as Open Shared Calendar
//! does, kept on this computer only.
//!
//! What marks a message as one: Exchange's sharing messages carry the sharing headers of the
//! public protocol description ([MS-OXSHARE]): `X-Sharing-Provider-Guid` and a folder type of
//! `IPF.Appointment` (`X-Sharing-Remote-Type`, or `X-Sharing-Local-Type`), a calendar. Nothing
//! else is believed to be one: a message that merely says in words that it shares a calendar is
//! not. Nothing in the message is run or followed; only its headers and its sender's address are
//! read, and the address is checked as for Open Shared Calendar before anything is sent.

use std::sync::Arc;

use egui::Ui;
use mailcraft_model::Message;
use mailcraft_model::remote::{opened_address, opened_id};

use crate::MailApp;
use crate::theme::{Tokens, regular};

/// Most headers of a message that are looked at.
const MAX_HEADERS_READ: usize = 2_000;
/// The folder type of a calendar, in a sharing message's headers.
const CALENDAR_FOLDER: &str = "IPF.Appointment";

/// Whether the message is a sharing invitation for a calendar, by its headers.
pub(crate) fn is_calendar_share(m: &Message) -> bool {
    let header = |name: &str| {
        m.headers.iter().take(MAX_HEADERS_READ).find(|(have, _)| have.eq_ignore_ascii_case(name)).map(|(_, value)| value.trim().to_string())
    };
    let calendar = |value: Option<String>| {
        value.is_some_and(|value| value.len() <= 64 && value.to_ascii_lowercase().starts_with(&CALENDAR_FOLDER.to_ascii_lowercase()))
    };
    header("X-Sharing-Provider-Guid").is_some_and(|guid| !guid.is_empty())
        && (calendar(header("X-Sharing-Remote-Type")) || calendar(header("X-Sharing-Local-Type")))
}

/// The address of whoever shared, when it is one a mailbox can be opened at.
pub(crate) fn sharer(m: &Message) -> Option<String> {
    mailcraft_model::mailbox_address(&m.from.email)
}

/// The account the calendar would be opened under: the message's own, or the one a shared
/// mailbox is opened through, when it is one that can; else the first that can.
fn account_for(app: &MailApp, m: &Message) -> Option<u64> {
    let mailbox = &app.session.mailbox;
    let can = |id: u64| mailbox.account(id).filter(|a| crate::open_shared::can_open(a)).map(|a| a.id);
    let owner = mailbox.owner_of(m.account).map(|a| a.id).unwrap_or(m.account);
    can(owner).or_else(|| mailbox.accounts.iter().find(|a| crate::open_shared::can_open(a)).map(|a| a.id))
}

/// The bar, when the message is one: who shared, and the button.
pub(super) fn bar(app: &mut MailApp, ui: &mut Ui, m: &Arc<Message>) {
    if !is_calendar_share(m) {
        return;
    }
    let (Some(address), Some(account)) = (sharer(m), account_for(app, m)) else { return };
    let t = Tokens::get(ui.ctx());
    // Already opened, or listed by the server (the invitation was accepted in Outlook): nothing to do.
    let mailbox = &app.session.mailbox;
    let id = opened_id(&address);
    let there = mailbox.calendars.iter().any(|c| {
        c.origin.account == account
            && (c.origin.id.eq_ignore_ascii_case(&id)
                || (opened_address(&c.origin.id).is_none() && c.owner.as_ref().is_some_and(|o| o.address.eq_ignore_ascii_case(&address))))
    });
    let name = if m.from.name.trim().is_empty() { address.clone() } else { m.from.name.trim().to_string() };
    egui::Frame::NONE.fill(t.panel).inner_margin(egui::Margin { left: 20, right: 12, top: 8, bottom: 9 }).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let line = if there {
                crate::i18n::fmt(tl!("{name} shared a calendar with you. It is in your calendar list."), &[("name", &name)])
            } else {
                crate::i18n::fmt(tl!("{name} shared a calendar with you."), &[("name", &name)])
            };
            ui.label(egui::RichText::new(line).font(regular(12.0)).color(t.text));
            if !there
                && ui
                    .button(tl!("Open this calendar"))
                    .on_hover_text(tl!("Opens it by their address, on this computer only: it won't appear in Outlook or on your other devices"))
                    .clicked()
                && let Err(e) = app.begin_open_shared(account, &address)
            {
                app.status(e);
            }
        });
    });
}
