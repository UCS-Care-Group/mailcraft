// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send/Receive Progress (#120): a panel in the main window, over its foot on the right, that
//! says what the Send/Receive under way is doing, or what the last one did, account by account:
//! the phase and counts of the one running, the ones waiting their turn, what each one that is
//! done came to, and the problems of each under its row. It opens only when the owner asks
//! (the Send/Receive text in the status bar, Send / Receive › Show Progress,
//! `ui.dialog {"name": "syncProgress"}`), and never by itself. It isn't modal: Escape or a click
//! anywhere else closes it. What it shows is the engine's ([`mailcraft_engine::cmd::sync_progress`])
//! with the problems the window keeps ([`MailApp::sync_problem_lines`]).
//!
//! A waiting account has Skip (`ui.skipSync`), which leaves it out of the run; under each
//! Microsoft account, how far its messages have been fetched whole (Full messages), or that Stop
//! has paused that until the next Send/Receive.
//!
//! Its left and top edges make it wider and taller (it stands on the window's foot at the
//! right), and the size is kept in `ui.json` (#145); a list longer than it has a solid scroll bar
//! in the window's own colours.
//!
//! Problems may name a message by its subject; messages being sent are only counted.

use egui::{Align2, Rect, RichText, Sense, Ui, pos2, vec2};
use mailcraft_engine::cmd::sync_progress::{AccountRun, FullMessages, RunState};
use mailcraft_model::Id;
use mailcraft_model::sync::SyncPhase;
use serde_json::json;

use crate::MailApp;
use crate::i18n;
use crate::theme::{Tokens, regular, semibold};

/// How wide the panel is at first.
pub(crate) const WIDTH: f32 = 440.0;
/// How narrow and how wide the owner can make it (#145); never wider than the window, though.
const MIN_WIDTH: f32 = 320.0;
const MAX_WIDTH: f32 = 900.0;
/// How low and how tall the list of accounts can be made; 0 is as tall as what it lists.
const MIN_HEIGHT: f32 = 80.0;
const MAX_HEIGHT: f32 = 1200.0;

/// The panel's width and its list's height as they are kept (`ui.json` may say anything).
pub(crate) fn sane_size(width: f32, height: f32) -> (f32, f32) {
    let width = if width.is_finite() { width.clamp(MIN_WIDTH, MAX_WIDTH) } else { WIDTH };
    let height = if height.is_finite() && height > 0.0 { height.clamp(MIN_HEIGHT, MAX_HEIGHT) } else { 0.0 };
    (width, height)
}

/// The panel, while it is open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Panel {
    /// It was opened for the problems: the first of them is scrolled to (once).
    pub to_problem: bool,
}

impl Panel {
    /// The panel a name of `ui.dialog` opens: `syncProgress`, or `syncProblems` (at the first
    /// problem).
    pub fn named(name: &str) -> Option<Panel> {
        match name {
            "syncProgress" => Some(Panel { to_problem: false }),
            "syncProblems" => Some(Panel { to_problem: true }),
            _ => None,
        }
    }
}

/// One account's row as the panel shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub account: Id,
    pub name: String,
    /// Where it is in the run; `None` for an account that has problems kept from a run that
    /// isn't the one shown (it never got started in it, say).
    pub run: Option<AccountRun>,
    pub problems: Vec<String>,
    /// Its full messages, while some are still to be fetched; and whether Stop has paused that.
    pub full: Option<FullMessages>,
    pub paused: bool,
}

impl Row {
    /// The Full messages line under the row, while some are still to be fetched.
    pub fn full_line(&self) -> Option<String> {
        let full = self.full.filter(|f| f.waiting())?;
        let counted = i18n::fmt(tl!("Full messages: {done} of {total}"), &[("done", &full.done.to_string()), ("total", &full.total.to_string())]);
        Some(if self.paused { i18n::fmt(tl!("{counted} · Paused until the next Send/Receive"), &[("counted", &counted)]) } else { counted })
    }

    /// Whether it waits its turn (and can be skipped).
    pub fn waiting(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.state == RunState::Waiting)
    }

    /// What the row says under the account's name, in one line.
    pub fn line(&self) -> String {
        let problems = || tln!(self.problems.len(), "{n} problem", "{n} problems");
        let Some(run) = &self.run else { return problems() };
        match run.state {
            RunState::Waiting => tl!("Waiting").to_string(),
            RunState::Skipped => tl!("Skipped").to_string(),
            RunState::Running => {
                let Some(phase) = run.phase else { return tl!("Starting…").to_string() };
                let counted = match (run.done, run.total) {
                    (_, 0) if run.done == 0 => String::new(),
                    (done, 0) => done.to_string(),
                    (done, total) => i18n::fmt(tl!("{done} of {total}"), &[("done", &done.to_string()), ("total", &total.to_string())]),
                };
                let detail: Vec<&str> = [run.folder.as_str(), counted.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
                if detail.is_empty() {
                    tl!(phase.label()).to_string()
                } else {
                    i18n::fmt(tl!("{phase}: {detail}"), &[("phase", tl!(phase.label())), ("detail", &detail.join(", "))])
                }
            }
            _ if !self.problems.is_empty() => problems(),
            RunState::Stopped => self.with_time(tl!("Stopped").to_string()),
            RunState::Done => {
                if run.new == 0 && run.sent == 0 {
                    return self.with_time(tl!("Done: up to date").to_string());
                }
                let mut line = i18n::fmt(tl!("Done: {n} new"), &[("n", &run.new.to_string())]);
                if run.sent > 0 {
                    line.push_str(&i18n::fmt(tl!(", {n} sent"), &[("n", &run.sent.to_string())]));
                }
                self.with_time(line)
            }
        }
    }

    /// `line` and how long the account's part of the run took (#276), when that was measured.
    fn with_time(&self, line: String) -> String {
        match self.run.as_ref().map(|r| r.timings.millis()) {
            Some(millis) if millis > 0 => i18n::fmt(tl!("{result} · {time}"), &[("result", &line), ("time", &took(millis))]),
            _ => line,
        }
    }

    /// How long each phase took, under the row (#276): the phases that are through while the
    /// account runs, all of them once it is done or stopped. `None` when nothing was measured.
    pub fn phases_line(&self) -> Option<String> {
        let run = self.run.as_ref()?;
        let through = |phase: SyncPhase| run.state != RunState::Running || run.phase != Some(phase);
        let parts: Vec<String> = run
            .timings
            .phases
            .iter()
            .filter(|p| p.millis > 0 && through(p.phase))
            .map(|p| i18n::fmt(tl!("{phase} {time}"), &[("phase", tl!(p.phase.label())), ("time", &took(p.millis))]))
            .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// How far it has got, 0 to 1, when that is known (it is running and has a total).
    pub fn fraction(&self) -> Option<f32> {
        let run = self.run.as_ref().filter(|r| r.state == RunState::Running && r.total > 0)?;
        Some((run.done as f32 / run.total as f32).clamp(0.0, 1.0))
    }

    fn running(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.state == RunState::Running)
    }
}

/// A span of time for a row, short: `0.4 s`, `12 s`, `2 min 05 s` (#276).
pub(crate) fn took(millis: u64) -> String {
    let tenths = millis.saturating_add(50) / 100;
    if tenths < 100 {
        i18n::fmt(tl!("{n} s"), &[("n", &format!("{}.{}", tenths / 10, tenths % 10))])
    } else if tenths < 600 {
        i18n::fmt(tl!("{n} s"), &[("n", &((tenths + 5) / 10).to_string())])
    } else {
        let seconds = (tenths + 5) / 10;
        i18n::fmt(tl!("{m} min {s} s"), &[("m", &(seconds / 60).to_string()), ("s", &format!("{:02}", seconds % 60))])
    }
}

/// What Copy Diagnostics adds of the last Send/Receive (#276): per account, in the run's order,
/// where its state, the time of each phase, the requests made and the folders skipped. The
/// accounts are numbered and named by kind; no address, name or folder is in it. In English,
/// like the rest of the report, whatever the interface language.
pub(crate) fn timings_report(app: &MailApp) -> String {
    let mb = &app.session.mailbox;
    let mut out = String::from("\nSend/Receive timings (last run, per account):\n");
    let mut any = false;
    for (n, run) in app.session.sync_progress.accounts.iter().enumerate() {
        let Some(account) = mb.account(run.account) else { continue };
        any = true;
        let kind = match account.kind {
            mailcraft_model::AccountKind::Microsoft => "Microsoft Graph",
            mailcraft_model::AccountKind::Imap => "mail server",
            mailcraft_model::AccountKind::Jmap => "JMAP",
            mailcraft_model::AccountKind::Exchange => "Exchange Web Services",
            _ => "other",
        };
        let t = &run.timings;
        let phases: Vec<String> = t.phases.iter().map(|p| format!("{} {} ms ({} requests)", p.phase.label(), p.millis, p.requests)).collect();
        let phases = if phases.is_empty() { "no time measured".to_string() } else { phases.join(", ") };
        out.push_str(&format!(
            "- Account {} ({kind}): {}, {} ms in all, {} requests, {} folders skipped; {phases}\n",
            n + 1,
            run.state.as_str(),
            t.millis(),
            t.requests(),
            t.skipped
        ));
    }
    if !any {
        out.push_str("- none yet\n");
    }
    out
}

/// The rows, in the run's order; then any account with problems that isn't in it.
pub(crate) fn rows(app: &MailApp) -> Vec<Row> {
    let mb = &app.session.mailbox;
    let problems = |account: Id| app.account_problem_lines(account);
    let progress = &app.session.sync_progress;
    let (full, paused) = (|account: Id| progress.full(account), progress.full_paused);
    let mut rows: Vec<Row> = app
        .session
        .sync_progress
        .accounts
        .iter()
        .filter_map(|run| {
            let name = mb.account(run.account)?.name.clone();
            Some(Row { account: run.account, name, run: Some(run.clone()), problems: problems(run.account), full: full(run.account), paused })
        })
        .collect();
    for (account, name, _, lines) in &app.sync_problems {
        if !rows.iter().any(|r| r.account == *account) && !lines.is_empty() {
            let name = mb.account(*account).map_or_else(|| name.clone(), |a| a.name.clone());
            rows.push(Row { account: *account, name, run: None, problems: problems(*account), full: full(*account), paused });
        }
    }
    rows
}

/// The panel's layer: over the windows.
pub(crate) fn layer() -> egui::LayerId {
    egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("sync_progress_panel"))
}

/// Escape closes the panel when it is shown: over the windows, the search and the selection,
/// under a dialog and an open menu (#393, #406). A held Escape is one press.
pub(crate) fn escape(app: &mut MailApp, ctx: &egui::Context) {
    if app.sync_panel.is_some() && crate::keys::take_escape(ctx, layer()) {
        app.sync_panel = None;
    }
}

/// The panel, when it is open, over the foot of the window on the right.
pub(crate) fn show(app: &mut MailApp, ctx: &egui::Context) {
    let Some(panel) = app.sync_panel.clone() else { return };
    // File's pages cover the window; a dialog has the window's attention.
    if app.ui.backstage {
        app.sync_panel = None;
        return;
    }
    if app.dialog.is_some() {
        return;
    }
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    let (kept_width, kept_height) = sane_size(app.ui.sync_panel_width, app.ui.sync_panel_height);
    let width = kept_width.min(screen.width() - 48.0).max(200.0);
    // What the edges were dragged by this frame (#145), and the list's height as shown.
    let mut wider = 0.0;
    let mut taller = 0.0;
    let mut list_height = None;
    let rows = rows(app);
    let syncing = app.syncing();
    let mut stop = false;
    let mut skip = None;
    let mut close = false;
    let mut scrolled = false;
    let shown = egui::Area::new(layer().id).order(egui::Order::Foreground).anchor(Align2::RIGHT_BOTTOM, vec2(-12.0, -32.0)).show(ctx, |ui| {
        let framed = egui::Frame::popup(ui.style()).fill(t.surface).stroke(egui::Stroke::new(1.0, t.border_strong)).inner_margin(12).show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
            // The heading, with Stop while a run goes on, and the way out.
            ui.horizontal(|ui| {
                ui.label(RichText::new(tl!("Send/Receive Progress")).font(semibold(14.0)).color(t.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let shut = crate::widgets::icon_button(ui, "close", tl!("Close"), 12.0, true);
                    app.hit.add("syncPanelClose", 0, shut.rect);
                    close |= shut.clicked();
                    if syncing {
                        let r = ui
                            .add(egui::Button::new(tl!("Stop")).min_size(vec2(60.0, 24.0)))
                            .on_hover_text(tl!("Stop this Send/Receive; what has arrived stays"));
                        app.hit.add("syncStop", 0, r.rect);
                        stop |= r.clicked();
                    }
                    if app.has_sync_problems() && ui.link(RichText::new(tl!("Copy problems")).font(regular(12.0))).clicked() {
                        ui.ctx().copy_text(app.sync_problem_lines().join("\n"));
                    }
                });
            });
            ui.add_space(2.0);
            if rows.is_empty() {
                let none = if app.server_accounts().is_empty() {
                    tl!("No account is connected to a mail server (File › Add Account)")
                } else {
                    tl!("Nothing has been sent or received yet.")
                };
                ui.label(RichText::new(none).font(regular(12.5)).color(t.text_dim));
                return;
            }
            let most = (screen.height() - 220.0).clamp(120.0, 520.0);
            // As tall as the owner made it, else as tall as what it lists up to `most`; never
            // taller than the window has room for.
            let room = (screen.height() - 140.0).max(MIN_HEIGHT);
            let scroll = egui::ScrollArea::vertical().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded);
            let limit = if kept_height > 0.0 { kept_height.min(room) } else { most };
            let scroll = if kept_height > 0.0 {
                scroll.min_scrolled_height(limit).max_height(limit).auto_shrink([false, false])
            } else {
                // As tall as what it listed the frame before: the panel grows to that,
                // rather than keeping its last height and scrolling the last few points.
                let listed = ui.data(|d| d.get_temp::<f32>(egui::Id::unique("sync_panel_listed"))).unwrap_or(0.0);
                scroll.min_scrolled_height(listed.min(limit)).max_height(limit).auto_shrink([false, true])
            };
            // The bar stands out against the panel, in light and in dark, and stays at its
            // full width while there is more to see.
            let own = ui.style().clone();
            let style = ui.style_mut();
            style.spacing.scroll = egui::style::ScrollStyle { bar_width: 8.0, ..egui::style::ScrollStyle::solid() };
            style.visuals.extreme_bg_color = t.panel;
            style.visuals.widgets.inactive.bg_fill = t.text_dim;
            style.visuals.widgets.hovered.bg_fill = t.text;
            style.visuals.widgets.active.bg_fill = t.accent;
            // (What egui leaves it: no more than the room below, either.)
            let cap = limit.min(ui.available_height());
            let out = scroll.show(ui, |ui| {
                // (What is in the list keeps the window's own look: Skip is a button as anywhere.)
                ui.set_style(own);
                let mut said = 0u64;
                for (n, row) in rows.iter().enumerate() {
                    if n > 0 {
                        ui.add_space(4.0);
                        let y = ui.cursor().min.y;
                        ui.painter().hline(ui.max_rect().x_range(), y, egui::Stroke::new(1.0, t.border));
                        ui.add_space(6.0);
                    }
                    let top = ui.cursor().min.y;
                    if let Some(r) = account_row(ui, &t, row) {
                        app.hit.add("syncSkip", row.account, r.rect);
                        if r.clicked() {
                            skip = Some(row.account);
                        }
                    }
                    for (k, problem) in row.problems.iter().enumerate() {
                        let r = problem_line(ui, &t, problem);
                        app.hit.add("syncProblem", said, r);
                        if said == 0 && k == 0 && panel.to_problem {
                            ui.scroll_to_rect(r, Some(egui::Align::TOP));
                            scrolled = true;
                        }
                        said += 1;
                    }
                    let rect = Rect::from_min_max(pos2(ui.max_rect().min.x, top), pos2(ui.max_rect().max.x, ui.cursor().min.y));
                    app.hit.add("syncRow", row.account, rect);
                }
            });
            list_height = Some(out.inner_rect.height());
            ui.data_mut(|d| d.insert_temp(egui::Id::unique("sync_panel_listed"), out.content_size.y));
            app.hit.add("syncList", 0, out.inner_rect);
            // Whether there is more than it shows: the bar is there (as egui reckons it).
            if out.content_size.y > cap.ceil() {
                let bar = Rect::from_min_max(pos2(out.inner_rect.max.x - ui.spacing().scroll.bar_width, out.inner_rect.min.y), out.inner_rect.max);
                app.hit.add("syncScrollBar", 0, bar);
            }
        });
        // The panel stands on the window's foot at the right: it is made wider by its left
        // edge and taller by its top edge, or both at its top left corner.
        let r = framed.response.rect;
        let grip = 5.0;
        let edges = [
            ("syncPanelLeft", Rect::from_min_max(pos2(r.min.x, r.min.y + grip), pos2(r.min.x + grip, r.max.y)), egui::CursorIcon::ResizeHorizontal),
            ("syncPanelTop", Rect::from_min_max(pos2(r.min.x + grip, r.min.y), pos2(r.max.x, r.min.y + grip)), egui::CursorIcon::ResizeVertical),
            ("syncPanelCorner", Rect::from_min_size(r.min, vec2(grip * 2.0, grip * 2.0)), egui::CursorIcon::ResizeNwSe),
        ];
        for (name, rect, cursor) in edges {
            let edge = ui.interact(rect, egui::Id::unique(("sync_panel_edge", name)), Sense::drag()).on_hover_cursor(cursor);
            app.hit.add(name, 0, rect);
            if edge.dragged() {
                let d = edge.drag_delta();
                if name != "syncPanelTop" {
                    wider -= d.x;
                }
                if name != "syncPanelLeft" {
                    taller -= d.y;
                }
            }
        }
    });
    if wider != 0.0 {
        app.ui.sync_panel_width = (width + wider).clamp(MIN_WIDTH, MAX_WIDTH.min((screen.width() - 48.0).max(MIN_WIDTH)));
    }
    if taller != 0.0
        && let Some(shown) = list_height
    {
        app.ui.sync_panel_height = (shown + taller).clamp(MIN_HEIGHT, MAX_HEIGHT.min((screen.height() - 140.0).max(MIN_HEIGHT)));
    }
    let area = shown.response.rect;
    app.hit.add("syncPanel", 0, area);
    // Scrolled to the first problem once; after that, it scrolls as the owner wants.
    if scrolled && let Some(open) = &mut app.sync_panel {
        open.to_problem = false;
    }
    if stop {
        let _ = app.run("ui.cancelSync", json!({}));
    }
    if let Some(account) = skip
        && let Err(e) = app.run("ui.skipSync", json!({"account": account}))
    {
        app.status(e);
    }
    // A press anywhere else closes it; on what opens it, that does (a click there toggles it).
    let openers = [app.hit.rect("syncStatus", 0), app.hit.rect("syncProblemsLink", 0)];
    let away = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer.interact_pos().is_some_and(|pos| !area.contains(pos) && !openers.iter().flatten().any(|r| r.contains(pos)))
    });
    if close || away {
        app.sync_panel = None;
    }
    if syncing || rows.iter().any(Row::running) {
        crate::software::repaint_after(ctx, std::time::Duration::from_millis(100));
    }
}

/// An account's name and how it is doing: a bar when the total is known, else a spinner while
/// it runs; Skip on the right while it waits its turn (its response); and under it, for a
/// Microsoft account, how far its full messages have got.
fn account_row(ui: &mut Ui, t: &Tokens, row: &Row) -> Option<egui::Response> {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    // (Room on the right for what a waiting account can be told: Skip.)
    let room = r.width() - 90.0;
    crate::theme::paint_clipped(ui.painter(), pos2(r.min.x, r.center().y), room.max(40.0), &row.name, semibold(13.0), t.text);
    let top = r;
    let line = row.line();
    let color = match &row.run {
        Some(run) if run.state == RunState::Running => t.text,
        _ if !row.problems.is_empty() => t.red,
        _ => t.text_dim,
    };
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    let mut x = r.min.x;
    if row.running() && row.fraction().is_none() {
        let spin = Rect::from_center_size(pos2(x + 7.0, r.center().y), vec2(12.0, 12.0));
        crate::software::spinner_at(ui, spin, Some(t.accent));
        x += 20.0;
    }
    crate::theme::paint_clipped(ui.painter(), pos2(x, r.center().y), (r.max.x - x - 90.0).max(40.0), &line, regular(12.5), color);
    if row.run.as_ref().is_some_and(|run| run.kept) {
        resp.on_hover_text(tl!("What the Send/Receive before came to: the last one brought nothing new"));
    }
    if let Some(done) = row.fraction() {
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width() - 90.0, 6.0), Sense::hover());
        ui.painter().rect_filled(bar, 3.0, t.border);
        let filled = Rect::from_min_size(bar.min, vec2((bar.width() * done).max(6.0), bar.height()));
        ui.painter().rect_filled(filled, 3.0, t.accent);
    }
    if let Some(phases) = row.phases_line() {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
        crate::theme::paint_clipped(ui.painter(), pos2(r.min.x, r.center().y), r.width().max(40.0), &phases, regular(12.0), t.text_dim);
    }
    if let Some(full) = row.full_line() {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
        crate::theme::paint_clipped(ui.painter(), pos2(r.min.x, r.center().y), r.width().max(40.0), &full, regular(12.0), t.text_dim);
    }
    if !row.waiting() {
        return None;
    }
    // Skip, over the two lines' right end; placed, not put, so that what is under those two
    // lines (the Inbox pass's time, Full messages) keeps its room.
    let at = Rect::from_center_size(pos2(top.max.x - 36.0, top.max.y + 2.0), vec2(72.0, 26.0));
    let skip = ui
        .place(at, egui::Button::new(RichText::new(tl!("Skip")).font(regular(12.5))))
        .on_hover_text(tl!("Leave this account out of this Send/Receive"));
    Some(skip)
}

/// A problem under its account's row, wrapped, with a red mark before it. Its rect.
fn problem_line(ui: &mut Ui, t: &Tokens, problem: &str) -> Rect {
    let response = ui
        .horizontal_top(|ui| {
            let (mark, _) = ui.allocate_exact_size(vec2(10.0, 16.0), Sense::hover());
            ui.painter().circle_filled(pos2(mark.center().x, mark.min.y + 8.0), 2.5, t.red);
            ui.add(egui::Label::new(RichText::new(problem).font(regular(12.0)).color(t.text)).selectable(true).wrap());
        })
        .response;
    response.rect
}

#[cfg(test)]
#[path = "sync_panel_tests.rs"]
mod tests;
