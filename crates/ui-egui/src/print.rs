//! File › Print: what is printed and how, Print Preview, and Print.
//!
//! The preview paints the pages from the same layout the PDF is written from
//! (`mailcraft_engine::cmd::print::document`), item by item, in the faces the PDF is set in; it
//! never renders the PDF. Print writes the PDF and hands it to the host
//! ([`Services::print`](crate::Services::print)), which gives it to the system; in the browser
//! the PDF is downloaded instead. Save as PDF runs `file.print`, which asks where (or downloads).

use std::sync::Arc;

use egui::{Align2, Color32, FontFamily, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::cmd::print::{self as engine_print, What};
use mailcraft_engine::print::calendar::Style as CalStyle;
use mailcraft_engine::print::{Document, FontKind, Item, Page};
use serde_json::{Value, json};

use crate::MailApp;
use crate::theme::{PRINT_BOLD, PRINT_MONO, PRINT_REGULAR, Tokens, regular, semibold};

/// The choices on the Print page. What isn't chosen follows the window: the module in front,
/// the calendar's view, the days in view.
#[derive(Default)]
pub struct PrintState {
    pub what: Option<What>,
    pub style: Option<CalStyle>,
    /// The calendar's days as typed (`YYYY-MM-DD`); empty: the days in view.
    pub from: String,
    pub to: String,
    /// `letter` or `a4`; empty: letter.
    pub paper: String,
    /// The last layout, and what it was made from.
    cache: Option<(String, Result<Arc<Document>, String>)>,
}

impl PrintState {
    /// The `file.print` params for these choices (the message is the selected one).
    pub fn params(&self) -> Value {
        let mut p = json!({});
        if let Some(o) = p.as_object_mut() {
            if let Some(w) = self.what {
                o.insert("what".into(), json!(w.code()));
            }
            if let Some(s) = self.style {
                o.insert("style".into(), json!(s.code()));
            }
            let mut range = serde_json::Map::new();
            for (k, v) in [("from", &self.from), ("to", &self.to)] {
                if !v.trim().is_empty() {
                    range.insert(k.into(), json!(v.trim()));
                }
            }
            if !range.is_empty() {
                o.insert("range".into(), Value::Object(range));
            }
            if !self.paper.is_empty() {
                o.insert("paper".into(), json!(self.paper));
            }
        }
        p
    }
}

/// The params as they apply now: the style only for a calendar.
fn effective(app: &MailApp) -> Value {
    let mut p = app.print.params();
    let what = engine_print::what(&app.session, &p).unwrap_or(What::Message);
    if let Some(o) = p.as_object_mut()
        && what != What::Calendar
    {
        o.remove("style");
        o.remove("range");
    }
    p
}

/// The layout for the current choices, made again only when they or the mailbox change.
pub fn document(app: &mut MailApp) -> Result<Arc<Document>, String> {
    let params = effective(app);
    let s = &app.session;
    let key =
        format!("{params}|{}|{:?}|{:?}|{:?}|{:?}|{:?}", s.rev(), s.view.module, s.sel.focus(), s.view.date, s.view.calendar, s.prefs.week_start);
    if let Some((k, doc)) = &app.print.cache
        && *k == key
    {
        return doc.clone();
    }
    let doc = engine_print::document(&app.session, &params).map(Arc::new).map_err(|e| e.to_string());
    app.print.cache = Some((key, doc.clone()));
    doc
}

/// `ui.print`: set the choices and show File › Print; with `"go": true`, print as well.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if id != "ui.print" {
        return None;
    }
    Some(command(app, p))
}

fn command(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let text = |k: &str| p.get(k).and_then(Value::as_str);
    if let Some(w) = text("what") {
        app.print.what = Some(What::parse(w).ok_or_else(|| format!("can't print \"{w}\": a message, calendar, contacts or tasks"))?);
    }
    if let Some(s) = text("style") {
        app.print.style = Some(CalStyle::parse(s).ok_or_else(|| format!("no calendar style \"{s}\": day, workWeek, week or month"))?);
    }
    if let Some(range) = p.get("range") {
        let date = |k: &str| range.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        for k in ["from", "to"] {
            let d = date(k);
            if !d.is_empty() && mailcraft_engine::time::Date::parse_iso(&d).is_none() {
                return Err(format!("`{k}` must be a date like 2026-10-11"));
            }
        }
        app.print.from = date("from");
        app.print.to = date("to");
    }
    if let Some(paper) = text("paper") {
        if mailcraft_engine::print::Paper::parse(paper).is_none() {
            return Err(format!("no paper \"{paper}\": letter or a4"));
        }
        app.print.paper = paper.to_ascii_lowercase();
    }
    app.ui.backstage = true;
    app.ui.backstage_page = "print".into();
    let doc = document(app);
    let pages = doc.as_ref().map(|d| d.pages.len()).unwrap_or(0);
    let params = effective(app);
    let what = engine_print::what(&app.session, &params).map(What::code).unwrap_or("message");
    let mut answer = json!({"what": what, "pages": pages, "params": params, "printer": app.services.print.is_some()});
    if let Err(e) = &doc
        && let Some(o) = answer.as_object_mut()
    {
        o.insert("problem".into(), json!(e));
    }
    if p.get("go").and_then(Value::as_bool) == Some(true) {
        let how = print_now(app)?;
        if let Some(o) = answer.as_object_mut() {
            o.insert("printed".into(), json!(how));
        }
    }
    Ok(answer)
}

/// Print what the page shows: to the system's printing (desktop), or as a download (browser).
/// Answers how it went: `"printer"` or `"download"`.
pub fn print_now(app: &mut MailApp) -> Result<&'static str, String> {
    let doc = document(app)?;
    let bytes = mailcraft_engine::print::pdf::write(&doc);
    let name = engine_print::suggested_name(&app.session, &effective(app));
    if let Some(print) = &app.services.print {
        print(&name, &bytes)?;
        app.status(crate::i18n::fmt(tl!("Sent {name} to the printer"), &[("name", &name)]));
        return Ok("printer");
    }
    if let Some(download) = &app.services.download {
        download(&name, &bytes)?;
        app.status(crate::i18n::fmt(tl!("Downloaded {name}: print it from there"), &[("name", &name)]));
        return Ok("download");
    }
    Err(tl!("There's no printer here; Save as PDF writes the print to a file").to_string())
}

const KINDS: [(What, &str); 4] = [
    (What::Message, tn!("Message (Memo Style)")),
    (What::Calendar, tn!("Calendar")),
    (What::Contacts, tn!("Contacts (Card Style)")),
    (What::Tasks, tn!("Tasks (Table Style)")),
];

const STYLES: [(CalStyle, &str); 4] = [
    (CalStyle::Day, tn!("Daily Style")),
    (CalStyle::WorkWeek, tn!("Work Week Style")),
    (CalStyle::Week, tn!("Weekly Style")),
    (CalStyle::Month, tn!("Monthly Style")),
];

/// The File › Print page.
pub(crate) fn page(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    ui.label(egui::RichText::new(tl!("Print")).font(semibold(22.0)).color(t.text));
    ui.add_space(4.0);
    let params = effective(app);
    let what = engine_print::what(&app.session, &params).unwrap_or(What::Message);
    let row = |ui: &mut Ui, label: &str, add: &mut dyn FnMut(&mut Ui)| {
        ui.horizontal(|ui| {
            ui.add_sized(vec2(110.0, 22.0), egui::Label::new(egui::RichText::new(label).color(t.text_dim)));
            add(ui);
        });
    };
    let mut chosen: Option<Value> = None;
    row(ui, tl!("Print"), &mut |ui| {
        let label = KINDS.iter().find(|k| k.0 == what).map_or("", |k| k.1);
        egui::ComboBox::from_id_salt("print_what").width(220.0).selected_text(tl!(label)).show_ui(ui, |ui| {
            for (w, label) in KINDS {
                if ui.selectable_label(w == what, tl!(label)).clicked() {
                    chosen = Some(json!({"what": w.code()}));
                }
            }
        });
    });
    if what == What::Calendar {
        let style = engine_print::calendar_style(&app.session, &params).unwrap_or_default();
        row(ui, tl!("Style"), &mut |ui| {
            let label = STYLES.iter().find(|s| s.0 == style).map_or("", |s| s.1);
            egui::ComboBox::from_id_salt("print_style").width(220.0).selected_text(tl!(label)).show_ui(ui, |ui| {
                for (s, label) in STYLES {
                    if ui.selectable_label(s == style, tl!(label)).clicked() {
                        chosen = Some(json!({"style": s.code()}));
                    }
                }
            });
        });
        let (mut from, mut to) = (app.print.from.clone(), app.print.to.clone());
        let mut typed = false;
        row(ui, tl!("Days"), &mut |ui| {
            typed |= ui.add(egui::TextEdit::singleline(&mut from).desired_width(100.0).hint_text(tl!("from YYYY-MM-DD"))).changed();
            ui.label(egui::RichText::new("–").color(t.text_dim));
            typed |= ui.add(egui::TextEdit::singleline(&mut to).desired_width(100.0).hint_text(tl!("to YYYY-MM-DD"))).changed();
        });
        if typed {
            // Taken as typed; a date that isn't one yet shows as the page's problem.
            app.print.from = from;
            app.print.to = to;
        }
    }
    row(ui, tl!("Paper"), &mut |ui| {
        let a4 = app.print.paper == "a4";
        egui::ComboBox::from_id_salt("print_paper").width(220.0).selected_text(if a4 { "A4" } else { "Letter" }).show_ui(ui, |ui| {
            if ui.selectable_label(!a4, "Letter").clicked() {
                chosen = Some(json!({"paper": "letter"}));
            }
            if ui.selectable_label(a4, "A4").clicked() {
                chosen = Some(json!({"paper": "a4"}));
            }
        });
    });
    if let Some(p) = chosen {
        let _ = app.run("ui.print", p);
    }
    let doc = document(app);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let ok = doc.is_ok();
        let label = if app.services.print.is_some() || app.services.download.is_none() { tl!("Print") } else { tl!("Download PDF to Print") };
        if crate::widgets::primary_button(ui, label, ok).clicked() {
            let _ = app.run("ui.print", json!({"go": true}));
        }
        if ui.add_enabled(ok, egui::Button::new(tl!("Save as PDF…"))).clicked() {
            let p = effective(app);
            let _ = app.run("file.print", p);
        }
    });
    ui.add_space(8.0);
    match doc {
        Err(e) => {
            ui.label(egui::RichText::new(e).color(t.red));
        }
        Ok(doc) => {
            let n = doc.pages.len();
            let count = if n == 1 {
                tl!("Print Preview: 1 page").to_string()
            } else {
                crate::i18n::fmt(tl!("Print Preview: {n} pages"), &[("n", &n.to_string())])
            };
            ui.label(egui::RichText::new(count).font(semibold(13.0)).color(t.accent_text));
            ui.add_space(4.0);
            preview(ui, &doc, t);
        }
    }
}

/// The pages, one under the other, as wide as there is room for (at most 560 points).
pub fn preview(ui: &mut Ui, doc: &Document, t: &Tokens) {
    let width = ui.available_width().clamp(120.0, 560.0);
    let scale = width / doc.paper.width.max(1.0);
    let height = doc.paper.height * scale;
    for (i, page) in doc.pages.iter().enumerate() {
        let (r, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        // Only what is on screen is drawn: a print can have hundreds of pages.
        if ui.is_rect_visible(r) {
            paint_page(ui, r, page, doc, scale, t);
        }
        ui.horizontal(|ui| {
            ui.add_space((width / 2.0 - 30.0).max(0.0));
            ui.label(egui::RichText::new(format!("{} / {}", i + 1, doc.pages.len())).font(regular(11.0)).color(t.text_dim));
        });
        ui.add_space(8.0);
    }
}

fn color(c: mailcraft_engine::print::Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// One page: paper with a shadow, then its items. The paper is white in either theme.
pub fn paint_page(ui: &Ui, r: Rect, page: &Page, doc: &Document, scale: f32, t: &Tokens) {
    let painter = ui.painter_at(r.expand(4.0));
    painter.rect_filled(r.translate(vec2(2.0, 3.0)), 2.0, Color32::from_black_alpha(if t.dark { 120 } else { 40 }));
    painter.rect(r, 1.0, Color32::WHITE, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
    let painter = ui.painter_at(r);
    let at = |x: f32, y: f32| pos2(r.min.x + x * scale, r.min.y + y * scale);
    let families = ui.ctx().fonts(|f| f.definitions().families.contains_key(&FontFamily::Name(PRINT_REGULAR.into())));
    let font = |kind: FontKind, size: f32| -> FontId {
        let size = (size * scale).max(1.0);
        match (families, kind) {
            (true, FontKind::Regular) => FontId::new(size, FontFamily::Name(PRINT_REGULAR.into())),
            (true, FontKind::Bold) => FontId::new(size, FontFamily::Name(PRINT_BOLD.into())),
            (true, FontKind::Mono) => FontId::new(size, FontFamily::Name(PRINT_MONO.into())),
            (false, FontKind::Bold) => semibold(size),
            (false, FontKind::Mono) => FontId::monospace(size),
            (false, FontKind::Regular) => regular(size),
        }
    };
    for item in &page.items {
        match item {
            Item::Text(text) => {
                let galley = painter.layout_no_wrap(text.text.clone(), font(text.font, text.size), color(text.color));
                // The page gives the baseline; the galley is placed by its top.
                let baseline = galley.rows.first().and_then(|row| row.glyphs.first()).map_or(text.size * scale * 0.8, |g| g.pos.y);
                let p = at(text.x, text.y);
                painter.galley(pos2(p.x, p.y - baseline), galley, color(text.color));
            }
            Item::Rect { x, y, w, h, fill, stroke, width } => {
                let rect = Rect::from_min_size(at(*x, *y), vec2(w * scale, h * scale));
                if let Some(c) = fill {
                    painter.rect_filled(rect, 0.0, color(*c));
                }
                if let Some(c) = stroke {
                    painter.rect_stroke(rect, 0.0, Stroke::new((width * scale).max(0.5), color(*c)), egui::StrokeKind::Middle);
                }
            }
            Item::Line { x1, y1, x2, y2, width, color: c } => {
                painter.line_segment([at(*x1, *y1), at(*x2, *y2)], Stroke::new((width * scale).max(0.5), color(*c)));
            }
            Item::Image { x, y, w, h, picture } => {
                // A picture is shown as where it goes and how big it is: the PDF has the pixels.
                let rect = Rect::from_min_size(at(*x, *y), vec2(w * scale, h * scale));
                painter.rect(rect, 0.0, Color32::from_gray(238), Stroke::new(0.5, Color32::from_gray(190)), egui::StrokeKind::Inside);
                if let Some(p) = doc.pictures.get(*picture)
                    && rect.height() > 14.0
                {
                    painter.text(rect.center(), Align2::CENTER_CENTER, format!("{} × {}", p.width, p.height), regular(9.0), Color32::from_gray(120));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use egui_kittest::kittest::Queryable;
    use mailcraft_engine::Session;
    use serde_json::json;

    use crate::{MailApp, Services};

    /// Wednesday 2026-10-14 16:00 UTC.
    const NOW: i64 = 1_791_993_600;

    fn app(services: Services) -> MailApp {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), services);
        app.autosave = false;
        app
    }

    fn harness(app: MailApp) -> egui_kittest::Harness<'static, MailApp> {
        egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        )
    }

    #[test]
    fn the_print_page_previews_the_pages() {
        let mut a = app(Services::default());
        let id = a.session.rows()[0].id;
        a.run("mail.select", json!({"id": id})).unwrap();
        let r = a.run("ui.print", json!({})).unwrap();
        assert_eq!((r["what"].as_str(), a.ui.backstage, a.ui.backstage_page.as_str()), (Some("message"), true, "print"));
        let pages = r["pages"].as_u64().unwrap();
        assert!(pages >= 1);
        let mut h = harness(a);
        h.run();
        h.get_by_label_contains("Print Preview:");
        h.get_by_label(&format!("1 / {pages}"));
        // The calendar in its monthly style: a page a month.
        h.state_mut().run("ui.print", json!({"what": "calendar", "style": "month", "range": {"from": "2026-10-01", "to": "2026-11-30"}})).unwrap();
        h.run();
        h.get_by_label("Print Preview: 2 pages");
        h.get_by_label("2 / 2");
        // Every kind lays out and shows, in both themes.
        for (what, theme) in [("contacts", "dark"), ("tasks", "light")] {
            h.state_mut().run("ui.theme", json!({"value": theme})).unwrap();
            h.state_mut().run("ui.print", json!({"what": what})).unwrap();
            h.run();
            h.get_by_label_contains("Print Preview:");
        }
    }

    #[test]
    fn print_hands_the_pdf_to_the_host_or_downloads_it() {
        let printed: Arc<Mutex<Vec<(String, Vec<u8>)>>> = Arc::default();
        let sink = printed.clone();
        let services = Services {
            print: Some(Box::new(move |name: &str, bytes: &[u8]| {
                sink.lock().unwrap().push((name.to_string(), bytes.to_vec()));
                Ok(())
            })),
            ..Services::default()
        };
        let mut a = app(services);
        let r = a.run("ui.print", json!({"what": "tasks", "go": true})).unwrap();
        assert_eq!(r["printed"], "printer");
        let got = printed.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "Tasks.pdf");
        assert!(got[0].1.starts_with(b"%PDF-1.7"));
        drop(got);

        // The browser: a download.
        let downloads: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = downloads.clone();
        let services = Services {
            download: Some(Box::new(move |name: &str, _: &[u8]| {
                sink.lock().unwrap().push(name.to_string());
                Ok(())
            })),
            ..Services::default()
        };
        let mut a = app(services);
        assert_eq!(a.run("ui.print", json!({"what": "contacts", "go": true})).unwrap()["printed"], "download");
        assert_eq!(downloads.lock().unwrap().as_slice(), ["Contacts.pdf"]);

        // Nowhere to print: said, and nothing else happens.
        let mut a = app(Services::default());
        assert!(a.run("ui.print", json!({"what": "tasks", "go": true})).is_err());
    }

    #[test]
    fn print_choices_are_checked_and_the_shortcut_opens_the_page() {
        let mut a = app(Services::default());
        assert!(a.execute("ui.print", json!({"what": "faxes"})).is_err());
        assert!(a.execute("ui.print", json!({"style": "fortnightly"})).is_err());
        assert!(a.execute("ui.print", json!({"range": {"from": "soon"}})).is_err());
        assert!(a.execute("ui.print", json!({"paper": "tabloid"})).is_err());
        for junk in [json!(null), json!([]), json!("x"), json!({"what": 5, "range": 7, "go": "yes"})] {
            let _ = a.execute("ui.print", junk);
        }
        // Mod+P is File › Print's own key, and in the window it shows the page first.
        assert_eq!(crate::keys::resolve(&a, "Mod+P"), Some("file.print"));
        // Save as PDF without a host to ask where: the command answers with the bytes.
        a.session.view.module = mailcraft_engine::Module::Tasks;
        let r = a.run("file.print", json!({})).unwrap();
        assert!(r["data"].is_string());
    }
}
