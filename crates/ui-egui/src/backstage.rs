//! Backstage (File): accounts and the mailbox, opening and exporting, options, About.

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_model::{AccountKind, Security};
use mailcraft_time::Weekday;
use serde_json::json;

use crate::theme::{Tokens, regular, semibold};
use crate::{MailApp, icons};

/// The pages: code, label, icon.
pub const PAGES: &[(&str, &str, &str)] = &[
    ("info", tn!("Info"), "info"),
    ("open", tn!("Open & Export"), "import"),
    ("print", tn!("Print"), "print"),
    ("options", tn!("Options"), "options"),
    ("about", tn!("About"), "help"),
];

pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let top = if app.integrated_titlebar { crate::chrome::MAC_TITLE_BAR } else { 0.0 };
    // The column of pages: the accent with its ink on a light window; in the dark a neutral
    // column, where the accent only marks the page in view.
    let (column, ink) = if t.dark { (t.panel, t.text) } else { (t.accent, t.on_accent) };
    crate::i18n::start_panel("backstage_nav")
        .exact_size(210.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(column))
        .show(ui, |ui| {
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
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(t.surface).inner_margin(egui::Margin { left: 36, right: 36, top: 24, bottom: 24 }))
        .show(ui, |ui| {
            ui.add_space(top);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_max_width(760.0);
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                match app.ui.backstage_page.as_str() {
                    "open" => open(app, ui, &t),
                    "print" => crate::print::page(app, ui, &t),
                    "options" => options(app, ui, &t),
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

fn info(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    heading(ui, tl!("Info"), t);
    section(ui, tl!("Accounts"), t);
    // Each account, then the shared mailboxes it opens.
    let accounts: Vec<mailcraft_model::Account> = app.session.mailbox.accounts_in_pane_order().into_iter().cloned().collect();
    for a in &accounts {
        if a.is_shared() {
            shared_card(app, ui, a, t);
            continue;
        }
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().min(560.0), 58.0), Sense::hover());
        ui.painter().rect(r, 6.0, t.panel, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        crate::widgets::avatar(ui, pos2(r.min.x + 30.0, r.center().y), 17.0, &a.address());
        let title = if a.email.is_empty() { tl!("Local mailbox").to_string() } else { a.address().full() };
        crate::theme::paint_clipped(ui.painter(), pos2(r.min.x + 58.0, r.min.y + 20.0), r.width() - 240.0, &title, semibold(13.0), t.text);
        let server = |s: &Option<mailcraft_model::Server>| {
            s.as_ref().map(|s| format!("{}{}", s.host, if s.security == Security::None { tl!(" (not encrypted)") } else { "" }))
        };
        let detail = match a.kind {
            AccountKind::Local => tl!("On this device only: no mail server").to_string(),
            AccountKind::Imap => {
                let servers = format!(
                    "{} · {}",
                    server(&a.incoming).map_or(tl!("no incoming server").to_string(), |h| format!("IMAP {h}")),
                    server(&a.outgoing).map_or(tl!("no outgoing server").to_string(), |h| format!("SMTP {h}"))
                );
                // How the owner signs in, when it isn't with a password.
                match &a.oauth {
                    Some(o) => format!("{} {} · {servers}", tl!("Signs in with"), o.provider.label()),
                    None => servers,
                }
            }
        };
        crate::theme::paint_clipped(ui.painter(), pos2(r.min.x + 58.0, r.min.y + 39.0), r.width() - 240.0, &detail, regular(11.5), t.text_dim);
        if !a.email.is_empty() {
            let br = Rect::from_min_size(pos2(r.max.x - 168.0, r.center().y - 13.0), vec2(74.0, 26.0));
            if ui.put(br, egui::Button::new(tl!("Settings"))).on_hover_text(tl!("Change this account's name, servers and sign-in")).clicked() {
                let _ = app.run("ui.dialog", json!({"name": "account", "id": a.id}));
            }
        }
        if accounts.len() > 1 {
            let br = Rect::from_min_size(pos2(r.max.x - 86.0, r.center().y - 13.0), vec2(74.0, 26.0));
            if ui.put(br, egui::Button::new(tl!("Remove"))).on_hover_text(tl!("Remove this account and the mail kept for it here")).clicked() {
                let _ = app.run("account.remove", json!({"id": a.id}));
            }
        }
        // A widget put somewhere leaves egui's cursor under it, which is inside the card: the
        // links go under the card, and what follows under them.
        ui.advance_cursor_after_rect(r);
        // Sending, calendar and contacts through Microsoft wait for the owner's say-so.
        if app.graph_consent_needed(a.id) {
            let link = ui.link(tl!("Connect calendar and contacts")).on_hover_text(tl!(
                "Sign in once more to let MailCraft send through Microsoft and show your calendar and contacts. Mail works without it."
            ));
            if link.clicked() {
                let _ = app.run("ui.connectMicrosoft", json!({"account": a.id}));
            }
        }
        // A work or school account that signs in with Microsoft opens the shared mailboxes its
        // owner has Full Access to, as Outlook does.
        if mailcraft_engine::cmd::shared::opens_shared(&app.session, a.id) {
            let link = ui.link(tl!("Add shared mailbox…")).on_hover_text(tl!(
                "Open a mailbox you have Full Access to (support@, a team's mailbox) under this account, with its own folders. It uses this account's sign-in; sending from it takes the Send As right."
            ));
            if link.clicked() {
                let _ = app.run("ui.dialog", json!({"name": "sharedMailbox", "account": a.id}));
            }
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
            crate::i18n::fmt(tl!("{n} messages"), &[("n", &n("messages").to_string())]),
            crate::i18n::fmt(tl!("{n} unread"), &[("n", &n("unread").to_string())]),
            crate::i18n::fmt(tl!("{n} contacts"), &[("n", &n("contacts").to_string())]),
            crate::i18n::fmt(tl!("{n} events"), &[("n", &n("events").to_string())]),
            crate::i18n::fmt(tl!("{n} tasks"), &[("n", &n("tasks").to_string())]),
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
}

/// The card of a shared mailbox under the account it is opened through: whose sign-in it uses,
/// and a way to close it. It has no settings of its own to change.
fn shared_card(app: &mut MailApp, ui: &mut Ui, a: &mailcraft_model::Account, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().min(560.0), 58.0), Sense::hover());
    // Set in a little, under its account.
    let r = Rect::from_min_max(pos2(r.min.x + 24.0, r.min.y), r.max);
    ui.painter().rect(r, 6.0, t.panel, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    crate::widgets::avatar(ui, pos2(r.min.x + 30.0, r.center().y), 17.0, &a.address());
    crate::theme::paint_clipped(ui.painter(), pos2(r.min.x + 58.0, r.min.y + 20.0), r.width() - 160.0, &a.address().full(), semibold(13.0), t.text);
    let owner = a.shared_of.and_then(|id| app.session.mailbox.account(id)).map(|o| o.email.clone()).unwrap_or_default();
    let detail = if owner.is_empty() {
        tl!("Shared mailbox: the account it was opened through is gone").to_string()
    } else {
        crate::i18n::fmt(tl!("Shared mailbox, opened with the sign-in of {account}"), &[("account", &owner)])
    };
    crate::theme::paint_clipped(ui.painter(), pos2(r.min.x + 58.0, r.min.y + 39.0), r.width() - 160.0, &detail, regular(11.5), t.text_dim);
    let br = Rect::from_min_size(pos2(r.max.x - 86.0, r.center().y - 13.0), vec2(74.0, 26.0));
    if ui.put(br, egui::Button::new(tl!("Remove"))).on_hover_text(tl!("Close this shared mailbox and remove the mail kept for it here")).clicked() {
        let _ = app.run("account.removeShared", json!({"id": a.id}));
    }
    // What follows goes under the card, not under the button put inside it.
    ui.advance_cursor_after_rect(r);
}

fn open(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    heading(ui, tl!("Open & Export"), t);
    section(ui, tl!("Bring in"), t);
    if tile(ui, "import", tl!("Import"), tl!("Messages (.eml, .msg, .mbox), calendars (.ics) and contacts (.vcf)"), t) {
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
    // (The labels' column is as wide as a long label needs, in a language with longer words;
    // in a right-to-left language the row runs from the right.)
    let t = Tokens::get(ui.ctx());
    let width = crate::theme::text_width(ui.painter(), label, &regular(12.5), 0.0, t.text);
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 24.0), crate::i18n::row_layout(egui::Align::Center), |ui| {
        let (r, _) = ui.allocate_exact_size(vec2((width + 12.0).max(210.0), 24.0), Sense::hover());
        let (at, align) =
            if crate::i18n::rtl() { (pos2(r.max.x, r.center().y), Align2::RIGHT_CENTER) } else { (pos2(r.min.x, r.center().y), Align2::LEFT_CENTER) };
        ui.painter().text(at, align, label, regular(12.5), t.text);
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
    row(ui, tl!("Language"), |ui| {
        use crate::i18n::{AUTO, Lang};
        let cur = app.ui.language.clone();
        let auto = cur.trim().eq_ignore_ascii_case(AUTO) || cur.trim().is_empty();
        let named = if auto { None } else { Lang::from_code(&cur) };
        let shown = match named {
            Some(lang) => lang.native_name().to_string(),
            None => crate::i18n::fmt(tl!("Use system setting ({language})"), &[("language", Lang::from_pref(AUTO).native_name())]),
        };
        egui::ComboBox::from_id_salt("opt_language").selected_text(shown).width(220.0).show_ui(ui, |ui| {
            if ui.selectable_label(auto, tl!("Use system setting")).clicked() {
                let _ = app.run("ui.language", json!({"value": AUTO}));
            }
            for lang in Lang::ALL {
                if ui.selectable_label(named == Some(lang), lang.native_name()).clicked() {
                    let _ = app.run("ui.language", json!({"value": lang.code()}));
                }
            }
        });
    });
    ui.weak(tl!("Dates, times and numbers are written as the language's region writes them. Translations other than English were machine-written and want a native speaker's review."));
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
    let mut plain = prefs.compose_plain;
    if ui.checkbox(&mut plain, tl!("Write messages in plain text (no formatting or pictures)")).changed() {
        set(app, json!({"composePlain": plain}));
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
    row(ui, tl!("When a message asks for a read receipt"), |ui| {
        use mailcraft_engine::ReceiptPolicy;
        let label = |p: ReceiptPolicy| match p {
            ReceiptPolicy::Ask => tl!("Ask each time"),
            ReceiptPolicy::Always => tl!("Always send one"),
            ReceiptPolicy::Never => tl!("Never send one"),
        };
        egui::ComboBox::from_id_salt("opt_receipts").selected_text(label(prefs.read_receipts)).show_ui(ui, |ui| {
            for (p, code) in [(ReceiptPolicy::Ask, "ask"), (ReceiptPolicy::Always, "always"), (ReceiptPolicy::Never, "never")] {
                if ui.selectable_label(p == prefs.read_receipts, label(p)).clicked() {
                    set(app, json!({"readReceipts": code}));
                }
            }
        });
    });
    // (Whose Favorites the folder pane shows, for an account that signs in with Microsoft.)
    crate::favorites_sync::options(app, ui, t);
    section(ui, tl!("Pictures in mail"), t);
    crate::reading::remote::options(app, ui, t);
    section(ui, tl!("Accounts"), t);
    let mut remember = app.ui.remember_sign_in;
    let can_remember = app.services.secrets.is_some();
    if ui
        .add_enabled(can_remember, egui::Checkbox::new(&mut remember, tl!("Remember passwords and sign-ins on this computer")))
        .on_hover_text(tl!(
            "Kept in the system's credential store, never in the mailbox or its settings. Without it you sign in again each time MailCraft starts."
        ))
        .changed()
    {
        let _ = app.run("ui.rememberSignIn", json!({"value": remember}));
    }
    if !can_remember {
        // (On Linux and the BSDs: no Secret Service answered when MailCraft started.)
        ui.weak(tl!(
            "Remembering isn't available here: no credential store answered (on Linux and BSD, GNOME Keyring, KWallet or another Secret Service). You'll be asked to sign in each time MailCraft starts."
        ));
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
    section(ui, tl!("Trust Center: signed and encrypted mail"), t);
    crate::trust_center::options(app, ui, t);
    section(ui, tl!("Notifications"), t);
    crate::notify::options(app, ui);
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
        let hour = |h: u8| mailcraft_time::time_short(h % 24, 0, prefs.twelve_hour);
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
            Some(m) => crate::i18n::duration(i64::from(m)),
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
        let label = |minutes: u8| crate::i18n::fmt(tl!("{n} minutes"), &[("n", &minutes.to_string())]);
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
    section(ui, tl!("Signatures"), t);
    signatures(app, ui, t);
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
    // A third, as Outlook has it, once there is a second.
    let third = mailcraft_engine::cmd::zones::third_zone(&app.session);
    if second.is_some() || third.is_some() {
        row(ui, tl!("Third time zone"), |ui| {
            let mut shown = third.is_some();
            if ui.checkbox(&mut shown, tl!("Show in the time bar")).changed() {
                let zone = if shown { json!("UTC") } else { serde_json::Value::Null };
                let _ = app.run("view.timeZone", json!({"third": zone}));
            }
            let Some(third) = third else { return };
            if let Some(text) = label(ui, &prefs.third_zone_label) {
                let _ = app.run("view.timeZone", json!({"thirdLabel": text}));
            }
            let mut query = typed(ui, "opt_third_zone_query");
            if let Some(zone) = crate::zones::picker(ui, egui::Id::new("opt_third_zone"), &app.session, third, &mut query, 250.0) {
                let _ = app.run("view.timeZone", json!({"third": zone.id()}));
            }
            keep(ui, "opt_third_zone_query", query);
        });
    }
    // Which zone this computer is in, in full.
    let system = app.session.system_zone();
    let words = crate::i18n::fmt(tl!("This computer is in {zone}."), &[("zone", &system.long_name(app.session.now()))]);
    ui.label(egui::RichText::new(words).size(12.0).color(Tokens::get(ui.ctx()).text_dim));
}

fn signatures(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let list = app.session.mailbox.signatures.clone();
    let default = app.session.mailbox.default_account().map(|a| a.signature.clone()).unwrap_or_default();
    for s in &list {
        let is_default = s.name.eq_ignore_ascii_case(&default);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(s.name.as_str()).font(semibold(12.5)));
            if is_default {
                ui.label(egui::RichText::new(tl!("(used for new messages)")).color(t.text_dim));
            } else if ui.small_button(tl!("Use for new messages")).clicked() {
                let _ = app.run("signature.set", json!({"name": s.name, "text": s.text, "default": true}));
            }
            if ui.small_button(tl!("Delete")).clicked() {
                let _ = app.run("signature.remove", json!({"name": s.name}));
            }
        });
        let mut text = s.text.clone();
        if ui.add(egui::TextEdit::multiline(&mut text).desired_rows(3).desired_width(420.0)).changed() {
            let _ = app.execute("signature.set", json!({"name": s.name, "text": text}));
        }
    }
    if ui.button(tl!("New Signature")).clicked() {
        // "Signature", "Signature 2"… whichever is free.
        let name = (1..100)
            .map(|n| if n == 1 { "Signature".to_string() } else { format!("Signature {n}") })
            .find(|n| app.session.mailbox.signature(n).is_none())
            .unwrap_or_else(|| "Signature".to_string());
        let me = app.session.mailbox.default_account().map(|a| a.display_name.clone()).unwrap_or_default();
        let _ = app.run("signature.set", json!({"name": name, "text": me, "default": list.is_empty()}));
    }
}

/// Who began MailCraft: named in About, here and in the Help tab's.
pub(crate) const INITIAL_CONTRIBUTOR: &str = "William Daugherty";
/// Where to find them.
pub(crate) const INITIAL_CONTRIBUTOR_LINK: &str = "https://www.linkedin.com/in/wdaugherty";

/// The line of About that names who began the program, with their link after the name. The link
/// is shown as where it leads, and stays in one piece.
pub(crate) fn initial_contributor(ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(crate::i18n::fmt(tl!("Initial contributor: {name}"), &[("name", INITIAL_CONTRIBUTOR)]));
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        let shown = INITIAL_CONTRIBUTOR_LINK.trim_start_matches("https://www.");
        if ui.link(shown).on_hover_text(INITIAL_CONTRIBUTOR_LINK).clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(INITIAL_CONTRIBUTOR_LINK));
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
    section(ui, tl!("Contributors"), t);
    initial_contributor(ui);
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
