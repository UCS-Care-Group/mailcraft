// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Backstage (File): accounts and the mailbox, opening and exporting, options, About.

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_model::{AccountKind, Security};
use mailcraft_time::Weekday;
use serde_json::json;

use crate::theme::{Tokens, regular, semibold};
use crate::{MailApp, icons};

/// The pages: code, label, icon.
pub const PAGES: &[(&str, &str, &str)] = &[
    ("info", "Info", "info"),
    ("open", "Open & Export", "import"),
    ("options", "Options", "options"),
    ("signatures", "Signatures", "signature"),
    ("about", "About", "help"),
];

pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let top = if app.integrated_titlebar { crate::chrome::MAC_TITLE_BAR } else { 0.0 };
    // The column of pages: the accent with its ink on a light window; in the dark a neutral
    // column, where the accent only marks the page in view.
    let (column, ink) = if t.dark { (t.panel, t.text) } else { (t.accent, t.on_accent) };
    egui::Panel::left("backstage_nav").exact_size(210.0).resizable(false).show_separator_line(false).frame(egui::Frame::NONE.fill(column)).show(
        ui,
        |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
            ui.add_space(top + 10.0);
            // Back to the mailbox.
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r.shrink2(vec2(8.0, 2.0)), 5.0, ink.linear_multiply(0.14));
            }
            icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 28.0, r.center().y), vec2(18.0, 18.0)), "previous", ink, ink);
            ui.painter().text(pos2(r.min.x + 48.0, r.center().y), Align2::LEFT_CENTER, tl!("Back"), semibold(13.5), ink);
            if resp.clicked() {
                let _ = app.run("ui.backstage", json!({"value": false}));
            }
            ui.add_space(10.0);
            for (code, label, icon) in PAGES {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
                let active = app.ui.backstage_page == *code;
                if active || resp.hovered() {
                    ui.painter().rect_filled(r.shrink2(vec2(8.0, 2.0)), 5.0, ink.linear_multiply(if active { 0.22 } else { 0.12 }));
                }
                if active && t.dark {
                    ui.painter().rect_filled(Rect::from_min_size(pos2(r.min.x + 8.0, r.center().y - 9.0), vec2(3.0, 18.0)), 1.5, t.accent);
                }
                icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 28.0, r.center().y), vec2(16.0, 16.0)), icon, ink, ink);
                ui.painter().text(
                    pos2(r.min.x + 48.0, r.center().y),
                    Align2::LEFT_CENTER,
                    tl!(label),
                    if active { semibold(13.0) } else { regular(13.0) },
                    ink,
                );
                if resp.clicked() {
                    let _ = app.run("ui.backstage", json!({"value": true, "page": code}));
                }
            }
        },
    );
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(t.surface).inner_margin(egui::Margin { left: 36, right: 36, top: 24, bottom: 24 }))
        .show(ui, |ui| {
            ui.add_space(top);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_max_width(760.0);
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                match app.ui.backstage_page.as_str() {
                    "open" => open(app, ui, &t),
                    "options" => options(app, ui, &t),
                    "signatures" => crate::signatures::page(app, ui, &t),
                    "about" => about(app, ui, &t),
                    _ => info(app, ui, &t),
                }
            });
        });
}

fn heading(ui: &mut Ui, text: &str, t: &Tokens) {
    ui.label(egui::RichText::new(text).font(semibold(22.0)).color(t.text));
    ui.add_space(4.0);
}

fn section(ui: &mut Ui, text: &str, t: &Tokens) {
    ui.add_space(10.0);
    ui.label(egui::RichText::new(text).font(semibold(14.0)).color(t.accent_text));
}

/// A wide button with an icon, a title and a line of explanation; `true` when clicked.
fn tile(ui: &mut Ui, icon: &str, title: &str, detail: &str, t: &Tokens) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().min(560.0), 52.0), Sense::click());
    ui.painter().rect(r, 6.0, if resp.hovered() { t.hover } else { t.panel }, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 26.0, r.center().y), vec2(24.0, 24.0)), icon, t.icon, t.accent);
    ui.painter().text(pos2(r.min.x + 52.0, r.min.y + 18.0), Align2::LEFT_CENTER, title, semibold(13.0), t.text);
    crate::theme::paint_clipped(ui.painter(), pos2(r.min.x + 52.0, r.min.y + 36.0), r.width() - 64.0, detail, regular(11.5), t.text_dim);
    resp.clicked()
}

/// The line under an account's name on its card in File › Info: what kind of account it is, and
/// for one on a mail server, its servers.
pub(crate) fn account_detail(app: &MailApp, a: &mailcraft_model::Account) -> String {
    let server = |s: &Option<mailcraft_model::Server>| {
        s.as_ref().map(|s| {
            if s.security == Security::None { crate::i18n::fmt(tl!("{host} (not encrypted)"), &[("host", &s.host)]) } else { s.host.clone() }
        })
    };
    match a.kind {
        AccountKind::Microsoft if a.is_shared() && app.shared_refused.contains_key(&a.id) => {
            tl!("Shared mailbox · No access: Microsoft won't open it for you. Send/Receive tries again").to_string()
        }
        AccountKind::Microsoft if a.is_shared() => match app.session.mailbox.owner_of(a.id) {
            Some(owner) => crate::i18n::fmt(tl!("Shared mailbox · opened with {owner}'s sign-in"), &[("owner", &owner.email)]),
            None => tl!("Shared mailbox · the account it was opened through is gone: remove it").to_string(),
        },
        AccountKind::Local => tl!("On this device only: no mail server").to_string(),
        AccountKind::Microsoft => tl!("Signs in with Microsoft · folders kept in step through Microsoft Graph (testing)").to_string(),
        // A JMAP account names its server.
        AccountKind::Jmap => jmap_line(a),
        // Kept as its file has it: this version neither sends nor receives with it.
        AccountKind::Unknown => tl!("Added by a newer version of MailCraft: send and receive with it there").to_string(),
        AccountKind::Imap => {
            let servers = format!(
                "{} · {}",
                server(&a.incoming).map_or(tl!("no incoming server").to_string(), |h| format!("IMAP {h}")),
                server(&a.outgoing).map_or(tl!("no outgoing server").to_string(), |h| format!("SMTP {h}"))
            );
            // How the owner signs in, when it isn't with a password.
            match &a.oauth {
                Some(o) => format!("{} · {servers}", crate::i18n::fmt(tl!("Signs in with {provider}"), &[("provider", o.provider.label())])),
                None => servers,
            }
        }
    }
}

/// A JMAP account's line about its server (also on its settings page): the name of the host alone,
/// as the whole address may carry the account's own.
pub(crate) fn jmap_line(a: &mailcraft_model::Account) -> String {
    match a.jmap.as_ref().and_then(mailcraft_model::JmapServer::host) {
        Some(host) => crate::i18n::fmt(tl!("JMAP {host}"), &[("host", host)]),
        None => tl!("JMAP · no usable server address").to_string(),
    }
}

fn info(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    heading(ui, tl!("Info"), t);
    section(ui, tl!("Accounts"), t);
    // (Each shared mailbox right under the account it is opened through, a little in: #70.)
    let accounts: Vec<mailcraft_model::Account> = app.session.mailbox.accounts_in_order().into_iter().cloned().collect();
    for a in &accounts {
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width().min(560.0), 58.0), Sense::hover());
        let r = if a.is_shared() { Rect::from_min_max(pos2(row.min.x + 24.0, row.min.y), row.max) } else { row };
        ui.painter().rect(r, 6.0, t.panel, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        crate::widgets::avatar(ui, pos2(r.min.x + 30.0, r.center().y), 17.0, &a.address());
        let title = if a.email.is_empty() { tl!("Local mailbox").to_string() } else { a.address().full() };
        // The text runs up to the buttons at the right: Settings, and Remove when there are others
        // (a shared mailbox has Remove alone).
        let buttons = if a.is_shared() { 1 } else { usize::from(!a.email.is_empty()) + usize::from(accounts.len() > 1) };
        let text_w = r.width() - 70.0 - 82.0 * buttons as f32;
        crate::theme::paint_clipped(ui.painter(), pos2(r.min.x + 58.0, r.min.y + 20.0), text_w, &title, semibold(13.0), t.text);
        let detail = account_detail(app, a);
        crate::theme::paint_clipped(ui.painter(), pos2(r.min.x + 58.0, r.min.y + 39.0), text_w, &detail, regular(11.5), t.text_dim);
        // Sending, calendar and contacts through Microsoft wait for the owner's say-so.
        if app.graph_consent_needed(a.id) {
            // An account kept in step through Graph gets its mail that way too: without it, none.
            let link = if a.kind == AccountKind::Microsoft {
                ui.link(tl!("Connect to Microsoft (mail, calendar and contacts)")).on_hover_text(tl!(
                    "Sign in once more to let MailCraft read and change your mail, send, and show your calendar and contacts through Microsoft."
                ))
            } else {
                ui.link(tl!("Connect calendar and contacts")).on_hover_text(tl!(
                    "Sign in once more to let MailCraft send through Microsoft and show your calendar and contacts. Mail works without it."
                ))
            };
            if link.clicked() {
                let _ = app.run("ui.connectMicrosoft", json!({"account": a.id}));
            }
        }
        // The buttons sit on the card: placed without moving the page's layout on (`ui.put` would
        // take it back up into the card, and what follows was drawn over it).
        let on_card = |ui: &mut Ui, right: f32, button: egui::Button<'_>| {
            let br = Rect::from_min_size(pos2(r.max.x - right, r.center().y - 13.0), vec2(74.0, 26.0));
            ui.new_child(egui::UiBuilder::new().max_rect(br)).add_sized(br.size(), button)
        };
        let settings_at = if accounts.len() > 1 { 168.0 } else { 86.0 };
        // (A shared mailbox has no settings of its own: its owner's sign-in reads it, #70.)
        if !a.email.is_empty()
            && !a.is_shared()
            && on_card(ui, settings_at, egui::Button::new(tl!("Settings")))
                .on_hover_text(tl!("Change this account's name, servers and sign-in"))
                .clicked()
        {
            let _ = app.run("ui.dialog", json!({"name": "account", "id": a.id}));
        }
        if a.is_shared() {
            if on_card(ui, 86.0, egui::Button::new(tl!("Remove")))
                .on_hover_text(tl!("Close this shared mailbox and remove the mail kept for it here. Nothing on the server changes."))
                .clicked()
            {
                let _ = app.run("account.removeSharedMailbox", json!({"id": a.id}));
            }
        } else if accounts.len() > 1
            && on_card(ui, 86.0, egui::Button::new(tl!("Remove"))).on_hover_text(tl!("Remove this account and the mail kept for it here")).clicked()
        {
            let _ = app.run("account.remove", json!({"id": a.id}));
        }
        // Which messages are kept whole on this computer (R3.2); a shared mailbox keeps as much
        // as its owner's account (#70).
        if a.kind == AccountKind::Microsoft && !a.is_shared() {
            download_in_full(app, ui, a);
            // How far back mail is kept in full (#143), beside it.
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(tl!("Mail to keep offline:"));
                crate::keep_offline::slider(app, ui, a.id, a.keep_offline);
            });
            ui.add_space(8.0);
        }
        // A mailbox of the organisation that the owner was given Full Access to (#70).
        // (Not for a personal Microsoft account, which has none.)
        if crate::shared_mailboxes::can_open_shared(a) {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                let link = ui.link(tl!("Add Shared Mailbox…")).on_hover_text(tl!(
                    "Open a mailbox of your organisation that you were given Full Access to, such as a team's, under this account"
                ));
                if link.clicked() {
                    let _ = app.run("ui.dialog", json!({"name": "sharedMailbox", "account": a.id}));
                }
            });
            ui.add_space(4.0);
        }
    }
    if tile(
        ui,
        "account",
        tl!("Add Account"),
        tl!(
            "Type your e-mail address and MailCraft finds the rest: Microsoft 365, Outlook.com, Google and other providers, or a mailbox on this device only"
        ),
        t,
    ) {
        let _ = app.run("ui.dialog", json!({"name": "account"}));
    }
    // The calendar of an account that signs in with Microsoft is kept on its server: the owner
    // can see for himself that what is changed here arrives there.
    let microsoft = |a: &&mailcraft_model::Account| a.oauth.as_ref().is_some_and(|o| o.provider == mailcraft_model::OAuthProvider::Microsoft);
    if app.services.check_calendar.is_some() && accounts.iter().any(|a| microsoft(&a)) {
        section(ui, tl!("Calendar"), t);
        let detail = if app.checking_calendar() {
            tl!("Checking: the report comes in a moment…")
        } else {
            tl!(
                "Makes one test appointment a year from today in your account's calendar, changes it and deletes it again, and shows what the server answered"
            )
        };
        if tile(ui, "calendar", tl!("Check Calendar Connection"), detail, t)
            && let Err(e) = app.run("ui.checkCalendar", json!({}))
        {
            app.status(e);
        }
    }
    section(ui, tl!("Mailbox"), t);
    if let Ok(v) = app.session.run("file.info", &json!({})) {
        let n = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
        let line = format!(
            "{} · {} · {} · {} · {}",
            tln!(n("messages"), "{n} message", "{n} messages"),
            tln!(n("unread"), "{n} unread", "{n} unread"),
            tln!(n("contacts"), "{n} contact", "{n} contacts"),
            tln!(n("events"), "{n} event", "{n} events"),
            tln!(n("tasks"), "{n} task", "{n} tasks"),
        );
        ui.label(egui::RichText::new(line).color(t.text));
        let place = match (&app.session.path, app.services.persist.is_some()) {
            (Some(p), _) => crate::i18n::fmt(tl!("Kept in {path}, saved as you go."), &[("path", &p.to_string_lossy())]),
            (None, true) => tl!("Kept in this browser's storage, saved as you go. Use Open & Export to keep a copy as a file.").to_string(),
            (None, false) => tl!("Not kept anywhere yet: use Open & Export › Save Mailbox As.").to_string(),
        };
        ui.label(egui::RichText::new(place).color(t.text_dim));
    }
    section(ui, tl!("Rules and Categories"), t);
    if tile(ui, "rules", tl!("Manage Rules"), tl!("What happens to mail as it arrives"), t) {
        let _ = app.run("ui.dialog", json!({"name": "rules"}));
    }
    if tile(ui, "categorize", tl!("Categories"), tl!("Names and colours for sorting mail, events, people and tasks"), t) {
        let _ = app.run("ui.dialog", json!({"name": "categories"}));
    }
    // Everything MailCraft keeps here, gone (#177).
    crate::data_removal::section(app, ui, t);
}

/// "Download messages in full" for a Microsoft account: everything, or up to a size (larger ones
/// download when opened).
fn download_in_full(app: &mut MailApp, ui: &mut Ui, a: &mailcraft_model::Account) {
    use mailcraft_model::DownloadInFull;
    let choices = [
        (DownloadInFull::UpTo10Mb, "upTo10Mb", tl!("Up to 10 MB")),
        (DownloadInFull::UpTo25Mb, "upTo25Mb", tl!("Up to 25 MB")),
        (DownloadInFull::UpTo50Mb, "upTo50Mb", tl!("Up to 50 MB")),
        (DownloadInFull::Everything, "everything", tl!("Everything")),
    ];
    let now = choices.iter().find(|(c, _, _)| *c == a.download_in_full).map_or("", |(_, _, label)| *label);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(tl!("Download messages in full:"));
        egui::ComboBox::from_id_salt(("download-in-full", a.id))
            .selected_text(now)
            .show_ui(ui, |ui| {
                for (choice, code, label) in choices {
                    if ui.selectable_label(a.download_in_full == choice, label).clicked() && a.download_in_full != choice {
                        let _ = app.run("account.edit", json!({"id": a.id, "downloadInFull": code}));
                        app.resume_fetch();
                    }
                }
            })
            .response
            .on_hover_text(tl!("Larger messages download when you open them. Choosing less removes nothing already downloaded."));
    });
    ui.add_space(8.0);
}

fn open(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    heading(ui, tl!("Open & Export"), t);
    section(ui, tl!("Bring in"), t);
    if tile(ui, "import", tl!("Import"), tl!("Messages (.eml, .mbox), calendars (.ics) and contacts (.vcf)"), t) {
        let _ = app.run("ui.import", json!({}));
    }
    if tile(ui, "open", tl!("Open Mailbox File"), tl!("Replace what is open with a .mailcraft file"), t) {
        let _ = app.run("ui.open", json!({}));
    }
    section(ui, tl!("Save out"), t);
    if tile(ui, "save", tl!("Save Mailbox As"), tl!("Everything here in one .mailcraft file"), t) {
        let _ = app.run("file.saveAs", json!({}));
    }
    if tile(ui, "export", tl!("Export Folder"), tl!("The messages of the folder in view, as .mbox"), t) {
        let _ = app.run("file.exportFolder", json!({}));
    }
    if tile(ui, "calendar", tl!("Save Calendar"), tl!("Every event and task, as .ics"), t) {
        let _ = app.run("file.exportCalendar", json!({}));
    }
    if tile(ui, "people", tl!("Export Contacts"), tl!("Every contact, as .vcf"), t) {
        let _ = app.run("file.exportContacts", json!({}));
    }
    section(ui, tl!("Start over"), t);
    if tile(ui, "mail", tl!("Sample Mailbox"), tl!("Invented mail, meetings, people and tasks to try things on"), t) {
        let _ = app.run("file.new", json!({"sample": true}));
        app.ui.backstage = false;
    }
    if tile(ui, "newMail", tl!("Empty Mailbox"), tl!("Nothing in it: replaces what is open here"), t) {
        let _ = app.run("file.new", json!({}));
        app.ui.backstage = false;
    }
    if !app.ui.recent.is_empty() {
        section(ui, tl!("Recent"), t);
        for path in app.ui.recent.clone() {
            if ui.link(path.as_str()).clicked() {
                let _ = app.run("file.open", json!({"path": path}));
                app.ui.backstage = false;
            }
        }
    }
}

/// A labelled row: the label on the left, the control after it.
fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let t = Tokens::get(ui.ctx());
        // A label longer than the column, as a translation can be, pushes its control along.
        let wide = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), regular(12.5), t.text).size().x + 12.0).max(210.0);
        let (r, _) = ui.allocate_exact_size(vec2(wide, 24.0), Sense::hover());
        ui.painter().text(pos2(r.min.x, r.center().y), Align2::LEFT_CENTER, label, regular(12.5), t.text);
        add(ui);
    });
}

fn options(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    heading(ui, tl!("Options"), t);
    section(ui, tl!("General"), t);
    row(ui, tl!("Interface theme"), |ui| {
        let cur = app.ui.theme;
        egui::ComboBox::from_id_salt("opt_theme").selected_text(tl!(cur.label())).show_ui(ui, |ui| {
            for a in crate::theme::Appearance::ALL {
                if ui.selectable_label(a == cur, tl!(a.label())).clicked() {
                    let _ = app.run("ui.theme", json!({"value": a.code()}));
                }
            }
        });
    });
    row(ui, tl!("Language"), |ui| {
        // Each language by its own name, so the owner finds theirs whatever the window speaks.
        let name = |l: crate::i18n::Lang| l.picker_name();
        let auto = app.ui.language == crate::i18n::AUTO;
        let cur = app.language();
        let shown = if auto { tl!("Automatic").to_string() } else { name(cur) };
        egui::ComboBox::from_id_salt("opt_language").selected_text(shown).show_ui(ui, |ui| {
            if ui.selectable_label(auto, tl!("Automatic")).clicked() {
                let _ = app.run("ui.language", json!({"language": crate::i18n::AUTO}));
            }
            for l in crate::i18n::Lang::available() {
                if ui.selectable_label(!auto && l == cur, name(l)).clicked() {
                    let _ = app.run("ui.language", json!({"language": l.code()}));
                }
            }
        });
    });
    row(ui, tl!("Interface size"), |ui| {
        let cur = app.ui.interface_size.percent();
        egui::ComboBox::from_id_salt("opt_size").selected_text(format!("{cur}%")).show_ui(ui, |ui| {
            for p in crate::theme::InterfaceSize::PRESETS {
                if ui.selectable_label(p == cur, format!("{p}%")).clicked() {
                    let _ = app.run("ui.interfaceSize", json!({"value": p}));
                }
            }
        });
    });
    if app.services.update_check.is_some() {
        section(ui, tl!("Updates"), t);
        crate::updates::options(app, ui, t);
    }
    let prefs = app.session.prefs.clone();
    let set = |app: &mut MailApp, params: serde_json::Value| {
        let _ = app.run("file.options", params);
    };
    section(ui, tl!("Mail"), t);
    let mut mark = prefs.mark_read_on_open;
    if ui.checkbox(&mut mark, tl!("Mark a message as read when it is opened in the reading pane")).changed() {
        set(app, json!({"markReadOnOpen": mark}));
    }
    let mut quote = prefs.quote_on_reply;
    if ui.checkbox(&mut quote, tl!("Include the original message when replying")).changed() {
        set(app, json!({"quoteOnReply": quote}));
    }
    row(ui, tl!("Preview lines in the message list"), |ui| {
        egui::ComboBox::from_id_salt("opt_preview").selected_text(prefs.preview_lines.to_string()).show_ui(ui, |ui| {
            for n in 0u8..=3 {
                if ui.selectable_label(n == prefs.preview_lines, n.to_string()).clicked() {
                    set(app, json!({"previewLines": n}));
                }
            }
        });
    });
    row(ui, tl!("Messages to download per folder"), |ui| {
        egui::ComboBox::from_id_salt("opt_sync")
            .selected_text(if prefs.sync_limit == 0 { tl!("All").to_string() } else { prefs.sync_limit.to_string() })
            .show_ui(ui, |ui| {
                for n in [100u32, 500, 2000, 10_000, 0] {
                    if ui.selectable_label(n == prefs.sync_limit, if n == 0 { tl!("All").to_string() } else { n.to_string() }).clicked() {
                        set(app, json!({"syncLimit": n}));
                    }
                }
            });
    });
    // (Whose Favorites the folder pane shows, for an account that signs in with Microsoft.)
    crate::favorites_sync::options(app, ui, t);
    // Where the system can show them (not in the browser).
    if app.services.alert.is_some() {
        section(ui, tl!("Desktop alerts"), t);
        crate::alerts::options(app, ui, t);
    }
    section(ui, tl!("Pictures in mail"), t);
    crate::reading::remote::options(app, ui, t);
    section(ui, tl!("Accounts"), t);
    let mut remember = app.ui.remember_sign_in;
    if ui
        .checkbox(&mut remember, tl!("Remember passwords and sign-ins on this computer"))
        .on_hover_text(tl!(
            "Kept in the system's credential store, never in the mailbox or its settings. Without it you sign in again each time MailCraft starts."
        ))
        .changed()
    {
        let _ = app.run("ui.rememberSignIn", json!({"value": remember}));
    }
    // Said in the status bar once; kept in sight here for as long as the session lasts.
    if remember && let Some(trouble) = app.secrets_trouble {
        ui.label(egui::RichText::new(trouble.notice()).color(t.text_dim));
    }
    row(ui, tl!("Microsoft application ID"), |ui| {
        // What Microsoft knows this installation by when an account signs in on its page; an
        // identifier from an app registration, not a secret.
        let built_in = !app.services.microsoft_client_id.is_empty();
        let mut id = app.ui.microsoft_client_id.clone();
        let hint = if built_in { tl!("The one this build came with") } else { tl!("Needed for Outlook.com and Microsoft 365 accounts") };
        let edit = egui::TextEdit::singleline(&mut id).hint_text(hint).desired_width(300.0);
        if ui.add(edit).changed() {
            let _ = app.execute("ui.microsoftClientId", json!({"value": id.trim()}));
        }
    });
    row(ui, tl!("Send and receive automatically"), |ui| {
        let label = |n: u32| match n {
            0 => tl!("Only when I ask").to_string(),
            1 => tl!("Every minute").to_string(),
            n => crate::i18n::fmt(tl!("Every {n} minutes"), &[("n", &n.to_string())]),
        };
        egui::ComboBox::from_id_salt("opt_sync_minutes").selected_text(label(prefs.sync_minutes)).show_ui(ui, |ui| {
            for n in [1u32, 5, 15, 30, 60, 0] {
                if ui.selectable_label(n == prefs.sync_minutes, label(n)).clicked() {
                    set(app, json!({"syncMinutes": n}));
                }
            }
        });
    });
    section(ui, tl!("Calendar"), t);
    let mut twelve = prefs.twelve_hour;
    if ui.checkbox(&mut twelve, tl!("Use a 12-hour clock (3:05 PM)")).changed() {
        set(app, json!({"twelveHour": twelve}));
    }
    row(ui, tl!("Week starts on"), |ui| {
        egui::ComboBox::from_id_salt("opt_week").selected_text(tl!(prefs.week_start.name())).show_ui(ui, |ui| {
            for d in [Weekday::Sun, Weekday::Mon, Weekday::Sat] {
                if ui.selectable_label(d == prefs.week_start, tl!(d.name())).clicked() {
                    set(app, json!({"weekStart": d.name()}));
                }
            }
        });
    });
    row(ui, tl!("Working hours"), |ui| {
        let hour = |h: u8| crate::dates::time(h % 24, 0, prefs.twelve_hour);
        egui::ComboBox::from_id_salt("opt_work_start").selected_text(hour(prefs.work_start)).show_ui(ui, |ui| {
            for h in 0u8..prefs.work_end.min(23) {
                if ui.selectable_label(h == prefs.work_start, hour(h)).clicked() {
                    set(app, json!({"workStart": h}));
                }
            }
        });
        ui.label("–");
        egui::ComboBox::from_id_salt("opt_work_end").selected_text(hour(prefs.work_end)).show_ui(ui, |ui| {
            for h in (prefs.work_start + 1)..=24 {
                if ui.selectable_label(h == prefs.work_end, hour(h)).clicked() {
                    set(app, json!({"workEnd": h}));
                }
            }
        });
    });
    // (The mailbox's own working hours are the ones in effect unless this says otherwise.)
    crate::calendar_view::work_hours_option(app, ui);
    row(ui, tl!("Reminder for new appointments"), |ui| {
        let label = |m: Option<u32>| match m {
            None => tl!("None").to_string(),
            Some(0) => tl!("At start time").to_string(),
            Some(m) => crate::dates::duration(i64::from(m)),
        };
        egui::ComboBox::from_id_salt("opt_reminder").selected_text(label(prefs.default_reminder)).show_ui(ui, |ui| {
            for m in [None, Some(0u32), Some(5), Some(10), Some(15), Some(30), Some(60)] {
                if ui.selectable_label(m == prefs.default_reminder, label(m)).clicked() {
                    set(app, json!({"defaultReminder": m}));
                }
            }
        });
    });
    row(ui, tl!("Time scale"), |ui| {
        // What one row of the calendar's hours stands for.
        let current = mailcraft_engine::cmd::calendar_extra::time_scale(&app.session);
        let label = |minutes: u8| tln!(minutes, "{n} minute", "{n} minutes");
        egui::ComboBox::from_id_salt("opt_time_scale").selected_text(label(current)).show_ui(ui, |ui| {
            for minutes in mailcraft_engine::cmd::calendar_extra::TIME_SCALES {
                if ui.selectable_label(minutes == current, label(minutes)).clicked() {
                    let _ = app.run("view.timeScale", json!({"minutes": minutes}));
                }
            }
        });
    });
    let mut numbers = prefs.week_numbers;
    if ui.checkbox(&mut numbers, tl!("Show week numbers in the Month view and the date navigator")).changed() {
        let _ = app.run("view.weekNumbers", json!({"value": numbers}));
    }
    crate::reading::invitation_options(app, ui);
    time_zones(app, ui, &prefs);
}

/// The time zone the calendar is shown in, and a second one for its time bar, each with a label
/// for the bar: this computer's own zone unless another is chosen, as in Outlook's options.
fn time_zones(app: &mut MailApp, ui: &mut Ui, prefs: &mailcraft_engine::Prefs) {
    use mailcraft_time::Zone;
    // What is typed into a zone's list, kept from frame to frame.
    let typed = |ui: &Ui, key: &str| ui.data(|data| data.get_temp::<String>(egui::Id::new(key))).unwrap_or_default();
    let keep = |ui: &Ui, key: &str, text: String| ui.data_mut(|data| data.insert_temp(egui::Id::new(key), text));
    let label = |ui: &mut Ui, text: &str| {
        let mut text = text.to_string();
        let field = egui::TextEdit::singleline(&mut text).hint_text(tl!("Label")).desired_width(80.0).char_limit(24);
        ui.add(field).changed().then_some(text)
    };
    let follows = Zone::named(&prefs.time_zone).is_none();
    row(ui, tl!("Time zone"), |ui| {
        if let Some(text) = label(ui, &prefs.zone_label) {
            let _ = app.run("view.timeZone", json!({"label": text}));
        }
        let mut query = typed(ui, "opt_zone_query");
        if let Some(zone) = crate::zones::picker(ui, egui::Id::new("opt_zone"), &app.session, app.session.zone(), &mut query, 250.0) {
            let _ = app.run("view.timeZone", json!({"zone": zone.id()}));
        }
        keep(ui, "opt_zone_query", query);
        let mut own = follows;
        if ui.checkbox(&mut own, tl!("This computer's, wherever it goes")).changed() {
            // (Unticked, the calendar stays in the zone it is in now, wherever the computer goes.)
            let zone = if own { serde_json::Value::Null } else { json!(app.session.zone().id()) };
            let _ = app.run("view.timeZone", json!({"zone": zone}));
        }
    });
    let second = mailcraft_engine::cmd::zones::second_zone(&app.session);
    row(ui, tl!("Second time zone"), |ui| {
        let mut shown = second.is_some();
        if ui.checkbox(&mut shown, tl!("Show in the time bar")).changed() {
            let zone = if shown { json!("UTC") } else { serde_json::Value::Null };
            let _ = app.run("view.timeZone", json!({"second": zone}));
        }
        let Some(second) = second else { return };
        if let Some(text) = label(ui, &prefs.second_zone_label) {
            let _ = app.run("view.timeZone", json!({"secondLabel": text}));
        }
        let mut query = typed(ui, "opt_second_zone_query");
        if let Some(zone) = crate::zones::picker(ui, egui::Id::new("opt_second_zone"), &app.session, second, &mut query, 250.0) {
            let _ = app.run("view.timeZone", json!({"second": zone.id()}));
        }
        keep(ui, "opt_second_zone_query", query);
    });
}

/// Everyone who has worked on MailCraft, on GitHub.
pub(crate) const CONTRIBUTORS_LINK: &str = "https://github.com/UCS-Care-Group/mailcraft/graphs/contributors";

/// The line of About that credits the people who made the program: a link to the project's
/// contributors, shown as where it leads, in one piece.
pub(crate) fn contributors(ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(tl!("Made by its contributors:"));
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        let shown = CONTRIBUTORS_LINK.trim_start_matches("https://");
        if ui.link(shown).on_hover_text(CONTRIBUTORS_LINK).clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(CONTRIBUTORS_LINK));
        }
    });
}

fn about(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    heading(ui, tl!("About MailCraft"), t);
    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Version {version}"), &[("version", env!("CARGO_PKG_VERSION"))])).color(t.text_dim));
    ui.label(tl!(
        "Mail, calendar, people and tasks, written from scratch in Rust. It runs natively on macOS, Windows, Linux and BSD, and in the browser."
    ));
    ui.label(tl!("Open source under the MIT License or the Apache License 2.0, at your option."));
    if app.services.update_check.is_some() {
        section(ui, tl!("Updates"), t);
        crate::updates::about(app, ui, t);
    }
    section(ui, tl!("Contributors"), t);
    contributors(ui);
    if ui.button(tl!("Show Contributors")).clicked() {
        app.ui.backstage = false;
        let _ = app.run("ui.dialog", json!({"name": "contributors"}));
    }
    section(ui, tl!("What it is made of"), t);
    ui.label(tl!("The interface fonts are Inter, Source Sans 3 and JetBrains Mono (SIL Open Font License 1.1). Every icon is drawn in code and is original work. No artwork, fonts or text from other mail programs are included."));
    section(ui, tl!("Your mail stays yours"), t);
    ui.label(tl!("Messages are shown without running anything in them, and the pictures a message keeps on the web are downloaded only when you ask. Passwords are kept in memory only while the app is open."));
    section(ui, tl!("Scripts and agents"), t);
    let parity = mailcraft_engine::catalog::parity(&app.session.registry);
    ui.label(crate::i18n::fmt(
        tl!("Everything the interface does is a command: {commands} of them, covering {percent}% of the feature catalog. The command line, the control channel and the MCP server run the same commands."),
        &[("commands", &parity["commands"].to_string()), ("percent", &parity["percent"].to_string())],
    ));
    if ui.button(tl!("Search Commands")).clicked() {
        app.ui.backstage = false;
        let _ = app.run("ui.dialog", json!({"name": "commands"}));
    }
}
