// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The browser shell: web `Services` (file picker, downloads, storage), drag-and-drop and the
//! eframe web runner.

use mailcraft_engine::Session;
use mailcraft_model::Mailbox;
use mailcraft_ui_egui::{Inbox, MailApp, Services, UiState};
use wasm_bindgen::JsCast as _;

const MAILBOX_EXTS: &[&str] = &["mailcraft", "json"];
const IMPORT_EXTS: &[&str] = &["eml", "mbox", "mbx", "ics", "ical", "vcf", "vcard"];
const CANVAS_ID: &str = "mailcraft_canvas";
const LOADING_ID: &str = "mailcraft_loading";
/// Where the mailbox and the interface settings live in the browser's storage.
const MAILBOX_KEY: &str = "mailcraft.mailbox.v1";
const UI_KEY: &str = "mailcraft.ui.v1";

pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            log::error!("no document");
            return;
        };
        let Some(canvas) = document.get_element_by_id(CANVAS_ID).and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok()) else {
            log::error!("missing <canvas id=\"{CANVAS_ID}\">");
            return;
        };
        let mut options = eframe::WebOptions::default();
        if query().contains("webgl")
            && let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup
        {
            create.instance_descriptor.backends = eframe::wgpu::Backends::GL;
        }
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                options,
                Box::new(move |cc| {
                    if let Some(rs) = &cc.wgpu_render_state {
                        log::info!("mailcraft-web: wgpu backend {:?}", rs.adapter.get_info().backend);
                    }
                    let inbox: Inbox = Inbox::default();
                    let mut app = MailApp::new(Session::new(first_mailbox()), services(inbox.clone(), cc.egui_ctx.clone()));
                    if let Some(ui) = stored(UI_KEY).and_then(|text| serde_json::from_str::<UiState>(&text).ok()) {
                        app.apply_prefs(ui);
                    }
                    let saved_ui = serde_json::to_string(&app.prefs()).unwrap_or_default();
                    Ok(Box::new(WebShell { app, inbox, saved_ui, checked: 0.0 }))
                }),
            )
            .await;
        if let Some(el) = document.get_element_by_id(LOADING_ID) {
            match result {
                Ok(()) => el.remove(),
                Err(e) => el.set_inner_html(&format!("<p>MailCraft failed to start: {e:?}</p><p>A browser with WebGPU or WebGL2 is required.</p>")),
            }
        }
    });
}

fn query() -> String {
    web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default()
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn stored(key: &str) -> Option<String> {
    storage()?.get_item(key).ok()?
}

fn sample() -> Mailbox {
    let now = mailcraft_time::now();
    mailcraft_engine::sample::mailbox(now, mailcraft_time::local_offset_minutes(now))
}

/// The mailbox to start with: what the URL asks for, else the one kept in this browser, else the
/// sample (an empty mail program in a browser tab has nothing to show).
fn first_mailbox() -> Mailbox {
    let q = query();
    if q.contains("empty") {
        return Mailbox::with_local_account("", "");
    }
    if q.contains("sample") {
        return sample();
    }
    match stored(MAILBOX_KEY).map(|text| mailcraft_engine::io::read_mailbox(text.as_bytes())) {
        Some(Ok(mailbox)) => mailbox,
        Some(Err(e)) => {
            log::error!("the stored mailbox couldn't be read: {e}");
            sample()
        }
        None => sample(),
    }
}

/// Wraps the app to read dropped files asynchronously (browsers can't read them synchronously),
/// feed them through the inbox, and keep the interface settings in the browser's storage.
struct WebShell {
    app: MailApp,
    inbox: Inbox,
    /// The interface settings as last stored.
    saved_ui: String,
    checked: f64,
}

impl eframe::App for WebShell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
        for f in dropped {
            let inbox = self.inbox.clone();
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let name = f.path().file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "dropped".into());
                match f.bytes_async().await {
                    Ok(bytes) => {
                        inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes));
                        ctx.request_repaint();
                    }
                    Err(e) => log::error!("couldn't read dropped file {name}: {e}"),
                }
            });
        }
        self.app.logic(ctx);
        // Interface settings: stored when they change, checked every couple of seconds.
        let now = mailcraft_ui_egui::now_ms();
        if now - self.checked > 2000.0 {
            self.checked = now;
            if let Ok(text) = serde_json::to_string(&self.app.prefs())
                && text != self.saved_ui
            {
                if let Some(s) = storage() {
                    let _ = s.set_item(UI_KEY, &text);
                }
                self.saved_ui = text;
            }
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.app.raw_input_hook(raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
    }
}

fn services(inbox: Inbox, ctx: egui::Context) -> Services {
    let open_inbox = inbox.clone();
    Services {
        open_async: Some(Box::new(move |purpose: &str| {
            let inbox = open_inbox.clone();
            let ctx = ctx.clone();
            let dialog = match purpose {
                "mailbox" => rfd::AsyncFileDialog::new().add_filter("MailCraft mailbox", MAILBOX_EXTS),
                "import" => rfd::AsyncFileDialog::new().add_filter("Mail, calendar and contact files", IMPORT_EXTS),
                // Anything can be attached.
                _ => rfd::AsyncFileDialog::new(),
            };
            wasm_bindgen_futures::spawn_local(async move {
                let Some(file) = dialog.pick_file().await else {
                    return;
                };
                let bytes = file.read().await;
                inbox.lock().unwrap_or_else(|e| e.into_inner()).push((file.file_name(), bytes));
                ctx.request_repaint();
            });
        })),
        download: Some(Box::new(download)),
        inbox: Some(inbox),
        persist: Some(Box::new(persist)),
        // The browser's language, for an interface language left on Automatic; and the page says
        // which language it is in (`<html lang>`), for screen readers and the browser's own tools.
        system_locale: web_sys::window().and_then(|w| w.navigator().language()),
        on_language: Some(Box::new(|code: &str| {
            let root = web_sys::window().and_then(|w| w.document()).and_then(|d| d.document_element());
            if let Some(root) = root {
                let _ = root.set_attribute("lang", code);
            }
        })),
        ..Default::default()
    }
}

/// Keep the mailbox in the browser's storage. A full store is the usual failure: say so plainly.
fn persist(bytes: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let store = storage().ok_or("this browser has no storage for the page (private window?)")?;
    store
        .set_item(MAILBOX_KEY, text)
        .map_err(|_| "this browser's storage for the page is full; save the mailbox as a file (File › Open & Export)".to_string())
}

/// Trigger a browser download of `bytes` named after the last component of `path`.
fn download(path: &str, bytes: &[u8]) -> Result<(), String> {
    let js = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "download".into());
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime_for(&name));
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(js)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js)?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(js)?.dyn_into().map_err(|_| "not an anchor")?;
    a.set_href(&url);
    a.set_download(&name);
    a.style().set_property("display", "none").map_err(js)?;
    let body = document.body().ok_or("no body")?;
    body.append_child(&a).map_err(js)?;
    a.click();
    a.remove();
    // Revoke after the click has been dispatched; the download keeps its own reference.
    let revoke = wasm_bindgen::closure::Closure::once_into_js(move || {
        web_sys::Url::revoke_object_url(&url).ok();
    });
    window.set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 10_000).map_err(js)?;
    Ok(())
}

fn mime_for(name: &str) -> &'static str {
    match name.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("eml") => "message/rfc822",
        Some("mbox") => "application/mbox",
        Some("ics") => "text/calendar",
        Some("vcf") => "text/vcard",
        Some("mailcraft" | "json") => "application/json",
        Some("txt") => "text/plain",
        Some("csv") => "text/csv",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}
