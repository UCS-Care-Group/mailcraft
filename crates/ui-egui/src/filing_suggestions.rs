// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Move menus share one calculation for the current mailbox revision and message.

use mailcraft_engine::filing_suggestions::{Reason, Scan, Suggestion};
use mailcraft_model::Id;

use crate::MailApp;

#[derive(Default)]
pub(crate) struct Pending {
    key: Option<(u64, u64, Id)>,
    view: Option<mailcraft_engine::ViewState>,
    whole: bool,
    results: Vec<Suggestion>,
    #[cfg(not(target_arch = "wasm32"))]
    work: Option<std::sync::mpsc::Receiver<Vec<Suggestion>>>,
    #[cfg(target_arch = "wasm32")]
    work: Option<Scan>,
    #[cfg(not(target_arch = "wasm32"))]
    control_work: std::sync::Weak<()>,
}

pub(crate) fn for_menu(app: &mut MailApp, ctx: &egui::Context) -> Vec<Suggestion> {
    if !app.session.prefs.suggest_folders || app.session.sel.messages.len() != 1 {
        return Vec::new();
    }
    let Some(&id) = app.session.sel.messages.first() else { return Vec::new() };
    let key = (app.session.openings(), app.session.rev(), id);
    let view = app.session.view.conversations.then_some(&app.session.view);
    let whole = view.is_some() && app.session.sel.whole.contains(&id);
    let state = &mut app.filing_suggestions;
    // Only one scan at a time. A result for a selection or mailbox that changed is discarded.
    state.poll(ctx);
    if state.key != Some(key) || state.view.as_ref() != view || state.whole != whole {
        if state.work.is_some() {
            return Vec::new();
        }
        state.key = Some(key);
        state.view = view.cloned();
        state.whole = whole;
        state.results.clear();
        if let Ok(Some(scan)) = mailcraft_engine::filing_suggestions::prepare(&mut app.session, &serde_json::Value::Null) {
            state.start(scan, ctx);
        }
    }
    state.results.clone()
}

impl Pending {
    #[cfg(not(target_arch = "wasm32"))]
    fn start(&mut self, scan: Scan, ctx: &egui::Context) {
        let (send, receive) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        if std::thread::Builder::new()
            .name("filing-suggestions".into())
            .spawn(move || {
                let _ = send.send(scan.finish());
                ctx.request_repaint();
            })
            .is_ok()
        {
            self.work = Some(receive);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn poll(&mut self, _: &egui::Context) {
        use std::sync::mpsc::TryRecvError;
        match self.work.as_ref().map(std::sync::mpsc::Receiver::try_recv) {
            Some(Ok(results)) => {
                self.results = results;
                self.work = None;
            }
            Some(Err(TryRecvError::Disconnected)) => self.work = None,
            Some(Err(TryRecvError::Empty)) | None => {}
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn start(&mut self, scan: Scan, ctx: &egui::Context) {
        self.work = Some(scan);
        ctx.request_repaint();
    }

    #[cfg(target_arch = "wasm32")]
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(mut scan) = self.work.take() {
            if scan.advance(256) {
                self.results = scan.suggestions();
            } else {
                self.work = Some(scan);
                ctx.request_repaint();
            }
        }
    }
}

pub(crate) fn show(app: &MailApp, ui: &mut egui::Ui, suggestions: &[Suggestion]) -> Option<Id> {
    if suggestions.is_empty() {
        return None;
    }
    ui.label(egui::RichText::new(tl!("Suggested folders")).strong());
    let mut picked = None;
    for suggestion in suggestions {
        let label = crate::nav::folder_path(&app.session.mailbox, suggestion.folder);
        let reason = match suggestion.reason {
            Reason::Sender => tl!("Similar messages from this sender are filed here."),
            Reason::Subject => tl!("Messages with a similar subject are filed here."),
            Reason::SenderAndSubject => tl!("The sender and subject match messages filed here."),
        };
        if ui.button(label).on_hover_text(reason).clicked() {
            picked = Some(suggestion.folder);
        }
    }
    ui.separator();
    picked
}

/// The control channel can answer from the worker, after its usual agent checks.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn start_control(
    app: &mut MailApp,
    reply: std::sync::mpsc::Sender<serde_json::Value>,
    params: &serde_json::Value,
) -> Result<bool, String> {
    if app.filing_suggestions.control_work.upgrade().is_some() {
        return Err("a filing suggestion request is already running".into());
    }
    let Some(scan) = mailcraft_engine::filing_suggestions::prepare(&mut app.session, params).map_err(|error| error.to_string())? else {
        return Ok(false);
    };
    let working = std::sync::Arc::new(());
    app.filing_suggestions.control_work = std::sync::Arc::downgrade(&working);
    std::thread::Builder::new()
        .name("filing-suggestions-control".into())
        .spawn(move || {
            let suggestions = scan.finish();
            drop(working);
            let _ = reply.send(serde_json::json!({"ok": true, "result": {"suggestions": suggestions}}));
        })
        .map_err(|error| error.to_string())?;
    Ok(true)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "filing_suggestions_tests.rs"]
mod tests;
