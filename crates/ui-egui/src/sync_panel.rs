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
    let problems = |account: Id| app.sync_problems.iter().find(|(a, ..)| *a == account).map(|(.., lines)| lines.clone()).unwrap_or_default();
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
            rows.push(Row { account: *account, name, run: None, problems: lines.clone(), full: full(*account), paused });
        }
    }
    rows
}

/// Escape closes the panel, before anything else hears it (a search, the selection). With a menu
/// open, Escape closes only the menu. A held Escape is one press (#393).
pub(crate) fn escape(app: &mut MailApp, ctx: &egui::Context) {
    if app.sync_panel.is_some()
        && app.dialog.is_none()
        && !egui::Popup::is_any_open(ctx)
        && ctx.input_mut(|i| crate::keys::take_once(i, egui::Key::Escape))
    {
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
    let shown = egui::Area::new(egui::Id::unique("sync_progress_panel"))
        .order(egui::Order::Foreground)
        .anchor(Align2::RIGHT_BOTTOM, vec2(-12.0, -32.0))
        .show(ctx, |ui| {
            let framed =
                egui::Frame::popup(ui.style()).fill(t.surface).stroke(egui::Stroke::new(1.0, t.border_strong)).inner_margin(12).show(ui, |ui| {
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
                        let bar =
                            Rect::from_min_max(pos2(out.inner_rect.max.x - ui.spacing().scroll.bar_width, out.inner_rect.min.y), out.inner_rect.max);
                        app.hit.add("syncScrollBar", 0, bar);
                    }
                });
            // The panel stands on the window's foot at the right: it is made wider by its left
            // edge and taller by its top edge, or both at its top left corner.
            let r = framed.response.rect;
            let grip = 5.0;
            let edges = [
                (
                    "syncPanelLeft",
                    Rect::from_min_max(pos2(r.min.x, r.min.y + grip), pos2(r.min.x + grip, r.max.y)),
                    egui::CursorIcon::ResizeHorizontal,
                ),
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
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
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
        egui::Spinner::new().size(12.0).color(t.accent).paint_at(ui, spin);
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
mod tests {
    use std::sync::{Arc, Mutex, mpsc};

    use egui_kittest::Harness;
    use mailcraft_engine::Session;
    use mailcraft_model::sync::{FolderDelta, SyncRequest, SyncResult, SyncStage, SyncStep};
    use mailcraft_model::{Address, FolderKind, Message};
    use serde_json::json;

    use super::*;
    use crate::Services;

    /// Wednesday 2026-10-14 16:00 UTC.
    const NOW: i64 = 1_791_993_600;

    /// The pipe the pieces of the run under way come through.
    type Pipe = Arc<Mutex<Option<mpsc::Sender<SyncResult>>>>;

    /// A mailbox with three accounts on mail servers (passwords at hand), and a host
    /// whose runs the test hands the pieces of.
    fn app() -> (MailApp, Vec<Id>, Pipe) {
        // (Not the sample: its reminders would come up over the window.)
        let mut a =
            MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("Noor Haddad", "noor@example.org"), NOW, 0), Services::default());
        a.autosave = false;
        a.auto_sync = false;
        let pipe: Pipe = Default::default();
        let to = pipe.clone();
        a.services.sync = Some(Box::new(move |_: SyncRequest| {
            let (tx, rx) = mpsc::channel();
            if let Ok(mut pipe) = to.lock() {
                *pipe = Some(tx);
            }
            rx
        }));
        let mut ids = Vec::new();
        for who in ["avery", "blake", "casey"] {
            let added = a.execute("account.add", json!({"email": format!("{who}@mail.example"), "incoming": {"host": "imap.mail.example"}}));
            let id = added.unwrap()["id"].as_u64().unwrap();
            a.passwords.insert(id, "pw".into());
            ids.push(id);
        }
        (a, ids, pipe)
    }

    fn send(pipe: &Pipe, piece: SyncResult) {
        pipe.lock().unwrap().as_ref().unwrap().send(piece).unwrap();
    }

    /// New mail for the account's Inbox.
    fn mail(a: &MailApp, account: Id, uids: &[u32], partial: bool) -> SyncResult {
        let inbox = a.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
        let new = uids
            .iter()
            .map(|uid| Message {
                uid: *uid,
                subject: format!("Message {uid}"),
                from: Address::new("", "x@example.org"),
                date: NOW,
                ..Default::default()
            })
            .collect();
        let delta =
            FolderDelta { id: Some(inbox), remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 9, new, ..Default::default() };
        SyncResult { account, partial, folders: vec![delta], ..Default::default() }
    }

    fn lines(a: &MailApp) -> Vec<String> {
        rows(a).iter().map(Row::line).collect()
    }

    fn window(app: MailApp, height: f32) -> Harness<'static, MailApp> {
        window_sized(app, 1280.0, height)
    }

    fn window_sized(app: MailApp, width: f32, height: f32) -> Harness<'static, MailApp> {
        let mut harness =
            Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(width, height)).build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        harness.run_steps(3);
        harness
    }

    fn click_at(harness: &mut Harness<'static, MailApp>, pos: egui::Pos2) {
        harness.event(egui::Event::PointerMoved(pos));
        harness.step();
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            harness.step();
        }
        harness.run_steps(2);
    }

    fn escape(harness: &mut Harness<'static, MailApp>) {
        for pressed in [true, false] {
            let key = egui::Key::Escape;
            harness.event(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
            harness.step();
        }
        harness.run_steps(2);
    }

    /// A round over three accounts is one run: the ones still to go wait, the one running says
    /// its phase and counts, each one done says what it came to, and Stop stops the rest.
    #[test]
    fn a_round_of_accounts_waits_runs_and_ends_row_by_row() {
        let (mut a, ids, pipe) = app();
        assert!(rows(&a).is_empty());
        a.execute("ui.sync", json!({})).unwrap();
        assert_eq!(rows(&a).iter().map(|r| r.account).collect::<Vec<_>>(), ids, "every account, in the run's order");
        assert_eq!(lines(&a), ["Waiting", "Waiting", "Waiting"]);

        // The first account: a folder's new mail, counted; a bar, as the total is known.
        let step = Some(SyncStep::new(SyncStage::Reading, "Inbox", 150, 500));
        send(&pipe, SyncResult { step, ..mail(&a, ids[0], &[1, 2], true) });
        a.poll_sync();
        let first = rows(&a).remove(0);
        assert_eq!((first.line(), first.fraction()), ("New mail: Inbox, 150 of 500".to_string(), Some(0.3)));
        // Sending is counted, never named; a step with no total yet has no bar.
        send(&pipe, SyncResult { account: ids[0], partial: true, step: Some(SyncStep::new(SyncStage::Sending, "", 1, 3)), ..Default::default() });
        a.poll_sync();
        assert_eq!(rows(&a)[0].line(), "Sending: 1 of 3");
        send(
            &pipe,
            SyncResult { account: ids[0], partial: true, step: Some(SyncStep::new(SyncStage::Checking, "Archive", 2, 5)), ..Default::default() },
        );
        a.poll_sync();
        assert_eq!((rows(&a)[0].line(), rows(&a)[0].fraction()), ("New mail: Archive".to_string(), None));
        send(&pipe, mail(&a, ids[0], &[3], false));
        a.poll_sync();
        assert_eq!(lines(&a), ["Done: 3 new", "Waiting", "Waiting"]);

        // The next one starts by itself; Stop ends it and the one still waiting.
        a.auto_sync();
        send(&pipe, SyncResult { account: ids[1], partial: true, step: Some(SyncStep::new(SyncStage::Connecting, "", 0, 0)), ..Default::default() });
        a.poll_sync();
        assert_eq!(lines(&a), ["Done: 3 new", "Folders", "Waiting"]);
        a.execute("ui.cancelSync", json!({})).unwrap();
        assert_eq!(lines(&a), ["Done: 3 new", "Folders", "Stopped"]);
        send(&pipe, SyncResult { account: ids[1], stopped: true, ..Default::default() });
        a.poll_sync();
        a.auto_sync();
        assert_eq!(lines(&a), ["Done: 3 new", "Stopped", "Stopped"]);
        assert!(!a.session.sync_progress.running() && !a.syncing());

        // A run whose account never starts (its password isn't at hand) leaves it out.
        a.passwords.remove(&ids[2]);
        a.execute("ui.sync", json!({"account": ids[0]})).unwrap();
        a.session.sync_progress.begin(&[ids[0], ids[2]]);
        a.sync_queue.push_back((ids[2], true));
        send(&pipe, mail(&a, ids[0], &[4], false));
        a.poll_sync();
        a.auto_sync();
        a.auto_sync();
        assert_eq!(rows(&a).iter().map(|r| r.account).collect::<Vec<_>>(), [ids[0]]);
        assert!(!a.session.sync_progress.running());
    }

    /// It opens from the status bar's Send/Receive text and from Send / Receive › Show Progress;
    /// Escape closes it, and so does a click anywhere else. It never opens by itself.
    #[test]
    fn the_panel_opens_when_asked_and_closes_with_escape_or_a_click_elsewhere() {
        let (mut a, ids, pipe) = app();
        a.execute("ui.sync", json!({"account": ids[0]})).unwrap();
        send(&pipe, SyncResult { step: Some(SyncStep::new(SyncStage::Reading, "Inbox", 2, 8)), ..mail(&a, ids[0], &[1, 2], true) });
        let mut h = window(a, 800.0);
        assert!(h.state().sync_panel.is_none(), "a run doesn't open it");
        let status = h.state().hit.rect("syncStatus", 0).expect("the status bar says what Send/Receive is doing");
        click_at(&mut h, status.center());
        assert!(h.state().sync_panel.is_some());
        let panel = h.state().hit.rect("syncPanel", 0).unwrap();
        assert!(h.state().hit.rect("syncStop", 0).is_some_and(|r| panel.contains(r.center())), "Stop, while the run goes on");
        assert!(h.state().hit.rect("syncRow", ids[0]).is_some());
        // A click on it doesn't close it; Escape does.
        click_at(&mut h, panel.center());
        assert!(h.state().sync_panel.is_some());
        escape(&mut h);
        assert!(h.state().sync_panel.is_none());
        // Stop in the panel is Cancel Send/Receive.
        h.state_mut().run("ui.dialog", json!({"name": "syncProgress"})).unwrap();
        h.run_steps(2);
        let stop = h.state().hit.rect("syncStop", 0).unwrap();
        click_at(&mut h, stop.center());
        assert!(h.state().services.sync_cancel.load(std::sync::atomic::Ordering::Relaxed));
        send(&pipe, SyncResult { account: ids[0], stopped: true, ..Default::default() });
        h.run_steps(3);
        assert!(h.state().hit.rect("syncStop", 0).is_none(), "nothing to stop once it has");
        assert_eq!(lines(h.state()), ["Stopped"]);

        // From the ribbon; then a click elsewhere in the window closes it.
        h.state_mut().sync_panel = None;
        h.state_mut().run("ui.tab", json!({"tab": "Send / Receive"})).unwrap();
        h.run_steps(2);
        let button = h.state().hit.rect("ribbon", crate::ribbon::hit_id("sync.progress")).expect("Show Progress");
        click_at(&mut h, button.center());
        assert!(h.state().sync_panel.is_some());
        click_at(&mut h, egui::pos2(400.0, 400.0));
        assert!(h.state().sync_panel.is_none());
        // A dialog opened takes its place.
        h.state_mut().run("ui.dialog", json!({"name": "syncProgress"})).unwrap();
        h.state_mut().run("ui.dialog", json!({"name": "goToDate"})).unwrap();
        assert!(h.state().sync_panel.is_none());
    }

    /// What the status bar shows of the run: a label and how far, or words.
    fn bar(h: &Harness<'static, MailApp>) -> Option<(String, f32)> {
        match crate::chrome::sync_shown(h.state()) {
            crate::chrome::SyncShown::Bar { label, fraction } => Some((label, fraction)),
            _ => None,
        }
    }

    /// While a run goes on, the status bar fills a bar under a label that changes only from one
    /// account to the next, never the name of each folder as it goes by; a click on it still
    /// opens Send/Receive Progress; Stop says so; once it is over, what it came to is in words.
    #[test]
    fn the_status_bar_fills_a_bar_instead_of_naming_each_folder() {
        let (mut a, ids, pipe) = app();
        a.execute("ui.sync", json!({"account": ids[0]})).unwrap();
        let step = |stage, folder: &str, done, total| SyncResult {
            account: ids[0],
            partial: true,
            step: Some(SyncStep::new(stage, folder, done, total)),
            ..Default::default()
        };
        send(&pipe, step(SyncStage::Checking, "Inbox", 1, 4));
        let mut h = window(a, 800.0);
        let name = h.state().session.mailbox.account(ids[0]).unwrap().name.clone();
        let (label, first) = bar(&h).expect("a bar while the run goes on");
        assert_eq!(label, format!("Updating {name}"));
        let (track, status) = (h.state().hit.rect("syncBar", 0).unwrap(), h.state().hit.rect("syncStatus", 0).unwrap());
        assert!(status.contains_rect(track), "the bar is what a click opens the progress from");
        assert!(track.width() >= 60.0 && track.height() <= 8.0, "{track:?}");
        let mut last = first;
        for piece in
            [step(SyncStage::Reading, "Inbox", 40, 80), step(SyncStage::Checking, "Archive", 2, 4), step(SyncStage::Reading, "Archive", 5, 10)]
        {
            send(&pipe, piece);
            h.run_steps(3);
            let (label, now) = bar(&h).unwrap();
            assert_eq!(label, format!("Updating {name}"), "steady, whatever folder it is at");
            assert!(!label.contains("Archive") && now > last, "{now} after {last}");
            last = now;
        }
        // A click shows Send/Receive Progress, as the words did.
        let status = h.state().hit.rect("syncStatus", 0).unwrap();
        click_at(&mut h, status.center());
        assert!(h.state().sync_panel.is_some());
        escape(&mut h);
        // Stop: the bar says it is stopping, and the run's end says what it came to in words.
        h.state_mut().run("ui.cancelSync", json!({})).unwrap();
        h.run_steps(2);
        assert_eq!(bar(&h).map(|(label, _)| label), Some("Stopping…".to_string()));
        send(&pipe, SyncResult { account: ids[0], stopped: true, ..Default::default() });
        h.run_steps(3);
        assert!(bar(&h).is_none() && h.state().hit.rect("syncBar", 0).is_none());
        assert!(matches!(crate::chrome::sync_shown(h.state()), crate::chrome::SyncShown::Text(_)));
        assert!(h.state().hit.rect("syncStatus", 0).is_some(), "the words open it too");
    }

    /// A round of several accounts fills one bar, account by account, and a long problem in a
    /// narrow window ends in an ellipsis rather than running over what is at the bar's left.
    #[test]
    fn a_round_fills_one_bar_and_long_words_keep_to_their_room() {
        let (mut a, ids, pipe) = app();
        a.execute("ui.sync", json!({})).unwrap();
        assert_eq!(a.session.sync_progress.fraction(), Some(0.0));
        send(&pipe, mail(&a, ids[0], &[1], false));
        a.poll_sync();
        a.auto_sync();
        let mut h = window(a, 700.0);
        let (label, fraction) = bar(&h).expect("the round goes on");
        assert!((fraction - 1.0 / 3.0).abs() < 0.05, "one account of three: {fraction}");
        assert!(label == "Send/Receive" || label.starts_with("Updating "), "{label}");
        h.state_mut().run("ui.cancelSync", json!({})).unwrap();
        send(&pipe, SyncResult { account: ids[1], stopped: true, ..Default::default() });
        h.run_steps(4);

        // 1100 wide: a long problem and a message at the left.
        let mut h = window_sized(app().0, 1100.0, 700.0);
        h.state_mut().session.sync_status = "Couldn't connect to imap.mail.example: the server closed the connection before it said hello, which usually means the port or the security setting is wrong".repeat(2);
        h.state_mut().status("Moved 3 messages to the folder called Projects that you made last week, and marked them read");
        h.run_steps(3);
        let said = h.state().hit.rect("syncStatus", 0).unwrap();
        let left = h.state().hit.rect("statusMessage", 0).unwrap();
        assert!(left.max.x < said.min.x, "{left:?} runs into {said:?}");
        assert!(said.min.x >= 0.0 && said.max.x <= 1100.0);
    }

    /// `syncProblems` opens the panel at the first problem, however far down its account is.
    #[test]
    fn sync_problems_opens_the_panel_at_the_first_problem() {
        let (mut a, _, _) = app();
        let mut ids = Vec::new();
        for n in 0..10 {
            let added = a.execute("account.add", json!({"email": format!("owner{n}@mail.example"), "incoming": {"host": "imap.mail.example"}}));
            ids.push(added.unwrap()["id"].as_u64().unwrap());
        }
        a.session.sync_progress.begin(&ids);
        for id in &ids {
            a.session.sync_progress.end(*id, false);
        }
        let last = *ids.last().unwrap();
        a.keep_sync_problems(last, &["Couldn't open \"Projects\": the server said no".to_string(), "Another one".to_string()]);
        assert_eq!(lines(&a).last().map(String::as_str), Some("2 problems"));
        a.execute("ui.dialog", json!({"name": "syncProblems"})).unwrap();
        let mut h = window(a, 600.0);
        h.run_steps(4);
        let panel = h.state().hit.rect("syncPanel", 0).unwrap();
        let first = h.state().hit.rect("syncProblem", 0).unwrap();
        assert!(panel.contains(first.center()), "the first problem is in view: {first:?} in {panel:?}");
        let top = h.state().hit.rect("syncRow", ids[0]).unwrap();
        assert!(!panel.contains(top.center()), "scrolled past the accounts above it");
        assert!(h.state().sync_panel.as_ref().is_some_and(|p| !p.to_problem), "once");
        // The red link in the status bar opens it there too.
        h.state_mut().sync_panel = None;
        h.run_steps(2);
        let link = h.state().hit.rect("syncProblemsLink", 0).expect("the red link stays");
        click_at(&mut h, link.center());
        assert!(h.state().sync_panel.is_some());
    }

    fn drag(harness: &mut Harness<'static, MailApp>, from: egui::Pos2, by: egui::Vec2) {
        harness.event(egui::Event::PointerMoved(from));
        harness.step();
        let button =
            |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        harness.event(button(from, true));
        harness.step();
        for n in 1..=4 {
            harness.event(egui::Event::PointerMoved(from + by * (n as f32 / 4.0)));
            harness.step();
        }
        harness.event(button(from + by, false));
        harness.run_steps(3);
    }

    /// Ten accounts done, the last with problems.
    fn many(a: &mut MailApp) {
        let mut ids = Vec::new();
        for n in 0..10 {
            let added = a.execute("account.add", json!({"email": format!("owner{n}@mail.example"), "incoming": {"host": "imap.mail.example"}}));
            ids.push(added.unwrap()["id"].as_u64().unwrap());
        }
        a.session.sync_progress.begin(&ids);
        for id in &ids {
            a.session.sync_progress.end(*id, false);
        }
        let last = *ids.last().unwrap();
        a.keep_sync_problems(last, &["Couldn't open \"Projects\": the server said no".to_string()]);
    }

    /// #145: the panel is made wider by its left edge and taller by its top edge (it stands on the
    /// window's foot at the right); the size is kept with the window's other sizes, within limits;
    /// and a list longer than it has a scroll bar that can be seen.
    #[test]
    fn the_panel_is_resized_by_its_edges_and_shows_its_scroll_bar() {
        let (mut a, _, _) = app();
        many(&mut a);
        a.execute("ui.dialog", json!({"name": "syncProgress"})).unwrap();
        let mut h = window(a, 600.0);
        let panel = h.state().hit.rect("syncPanel", 0).unwrap();
        assert!((panel.width() - (WIDTH + 24.0)).abs() < 4.0, "{panel:?}");
        let bar = h.state().hit.rect("syncScrollBar", 0).expect("more than it shows: a scroll bar");
        assert!(panel.contains(bar.center()) && bar.width() >= 8.0, "{bar:?}");

        // Wider by the left edge: the right edge stays where it is.
        let left = h.state().hit.rect("syncPanelLeft", 0).unwrap();
        drag(&mut h, left.center(), egui::vec2(-120.0, 0.0));
        assert!(h.state().sync_panel.is_some(), "dragging an edge doesn't close it");
        assert!((h.state().ui.sync_panel_width - (WIDTH + 120.0)).abs() < 2.0, "{}", h.state().ui.sync_panel_width);
        let wider = h.state().hit.rect("syncPanel", 0).unwrap();
        assert!((wider.max.x - panel.max.x).abs() < 1.0 && wider.width() > panel.width() + 100.0, "{wider:?}");

        // Lower by the top edge.
        let list = h.state().hit.rect("syncList", 0).unwrap();
        let top = h.state().hit.rect("syncPanelTop", 0).unwrap();
        drag(&mut h, top.center(), egui::vec2(0.0, 100.0));
        let kept = h.state().ui.sync_panel_height;
        assert!((kept - (list.height() - 100.0)).abs() < 2.0, "{kept} from {}", list.height());
        let lower = h.state().hit.rect("syncList", 0).unwrap();
        assert!((lower.height() - kept).abs() < 2.0, "{lower:?}");
        assert!(h.state().hit.rect("syncScrollBar", 0).is_some(), "still more than it shows");

        // Never past its limits, nor out of the window.
        let corner = h.state().hit.rect("syncPanelCorner", 0).unwrap();
        drag(&mut h, corner.center(), egui::vec2(-2000.0, -2000.0));
        let (w, ht) = (h.state().ui.sync_panel_width, h.state().ui.sync_panel_height);
        assert!(w <= 1280.0 - 48.0 + 0.5 && (MIN_HEIGHT..=600.0 - 140.0 + 0.5).contains(&ht), "{w} × {ht}");
        let corner = h.state().hit.rect("syncPanelCorner", 0).unwrap();
        drag(&mut h, corner.center(), egui::vec2(2000.0, 2000.0));
        assert_eq!((h.state().ui.sync_panel_width, h.state().ui.sync_panel_height), (MIN_WIDTH, MIN_HEIGHT));

        // Kept for the next run with the window's other sizes; nonsense in the file is put right.
        h.state_mut().ui.sync_panel_width = 600.0;
        h.state_mut().ui.sync_panel_height = 300.0;
        let saved = serde_json::to_value(h.state().prefs()).unwrap();
        assert_eq!((saved["syncPanelWidth"].as_f64(), saved["syncPanelHeight"].as_f64()), (Some(600.0), Some(300.0)));
        let (mut b, _, _) = app();
        b.apply_prefs(serde_json::from_value(saved).unwrap());
        assert_eq!((b.ui.sync_panel_width, b.ui.sync_panel_height), (600.0, 300.0));
        for (width, height, kept) in [(f32::NAN, f32::INFINITY, (WIDTH, 0.0)), (-5.0, -5.0, (MIN_WIDTH, 0.0)), (1e9, 5.0, (MAX_WIDTH, MIN_HEIGHT))] {
            b.apply_prefs(crate::UiState { sync_panel_width: width, sync_panel_height: height, ..Default::default() });
            assert_eq!((b.ui.sync_panel_width, b.ui.sync_panel_height), kept);
        }

        // A list that fits has no bar.
        let (mut c, ids, _) = app();
        c.execute("ui.dialog", json!({"name": "syncProgress"})).unwrap();
        c.session.sync_progress.begin(&ids);
        for id in &ids {
            c.session.sync_progress.end(*id, false);
        }
        let mut h = window(c, 800.0);
        h.run_steps(6);
        assert!(h.state().hit.rect("syncList", 0).is_some(), "{:?}", lines(h.state()));
        assert!(h.state().hit.rect("syncScrollBar", 0).is_none(), "{:?}", h.state().hit.rect("syncScrollBar", 0));
    }

    /// Skip leaves a waiting account out of the run, and its row says so; the account running
    /// is for Stop, and one not waiting has nothing to skip.
    #[test]
    fn skip_leaves_a_waiting_account_out_of_the_run() {
        let (mut a, ids, pipe) = app();
        a.execute("ui.sync", json!({})).unwrap();
        // The first is running (its run started, though nothing has come from it yet).
        let refused = a.execute("ui.skipSync", json!({"account": ids[0]})).unwrap_err();
        assert!(refused.contains("Use Stop"), "{refused}");
        assert_eq!(a.execute("ui.skipSync", json!({"account": ids[2]})).unwrap(), json!({"skipped": ids[2]}));
        assert_eq!(lines(&a), ["Waiting", "Waiting", "Skipped"]);
        // Skipped once is not waiting any more; nor is an account that isn't in the run, or none.
        assert!(a.execute("ui.skipSync", json!({"account": ids[2]})).unwrap_err().contains("isn't waiting"));
        let local = a.session.mailbox.accounts[0].id;
        assert!(a.execute("ui.skipSync", json!({"account": local})).unwrap_err().contains("isn't waiting"));
        for junk in [json!({}), json!({"account": "x"}), json!({"account": 424_242}), json!(null), json!([1])] {
            assert!(a.execute("ui.skipSync", junk).is_err());
        }
        send(&pipe, mail(&a, ids[0], &[1], false));
        a.poll_sync();
        a.auto_sync();
        let r = a.execute("ui.skipSync", json!({"account": ids[1]})).unwrap_err();
        assert!(r.contains("Use Stop"), "the second one runs now: {r}");
        send(&pipe, mail(&a, ids[1], &[2], false));
        a.poll_sync();
        a.auto_sync();
        a.auto_sync();
        // The skipped one was never started: the run is over without it.
        assert_eq!(lines(&a), ["Done: 1 new", "Done: 1 new", "Skipped"]);
        assert!(!a.session.sync_progress.running() && !a.syncing() && a.sync_queue.is_empty());
        let said = a.execute("sync.progress", json!({})).unwrap();
        assert_eq!(said["accounts"][2]["state"], json!("skipped"));
    }

    /// Skip on a waiting row in the panel.
    #[test]
    fn the_skip_button_is_on_waiting_rows() {
        let (mut a, ids, _) = app();
        a.execute("ui.sync", json!({})).unwrap();
        a.execute("ui.dialog", json!({"name": "syncProgress"})).unwrap();
        let mut h = window(a, 800.0);
        let panel = h.state().hit.rect("syncPanel", 0).unwrap();
        for id in &ids[1..] {
            let skip = h.state().hit.rect("syncSkip", *id).expect("Skip on a waiting row");
            assert!(panel.contains(skip.center()) && h.state().hit.rect("syncRow", *id).is_some_and(|r| r.contains(skip.center())));
        }
        let skip = h.state().hit.rect("syncSkip", ids[1]).unwrap();
        click_at(&mut h, skip.center());
        assert_eq!(lines(h.state())[1], "Skipped");
        assert!(h.state().sync_panel.is_some(), "the panel stays open");
        h.run_steps(2);
        assert!(h.state().hit.rect("syncSkip", ids[1]).is_none(), "nothing to skip once skipped");
        assert!(h.state().hit.rect("syncSkip", ids[2]).is_some());
    }

    /// A waiting row with lines under it (an Inbox pass's time, Full messages) keeps their room:
    /// the next row starts below them, not over them.
    #[test]
    fn a_waiting_rows_lines_are_not_covered_by_the_next_row() {
        use mailcraft_model::sync::{SyncPhase, Timings};
        let (mut a, ids, _) = app();
        a.execute("ui.sync", json!({})).unwrap();
        let mut timings = Timings::default();
        timings.add(SyncPhase::Inbox, 300, 1);
        for run in a.session.sync_progress.accounts.iter_mut().filter(|r| r.account != ids[0]) {
            run.timings = timings.clone();
        }
        a.session.sync_progress.set_full(ids[1], 10, 50);
        a.execute("ui.dialog", json!({"name": "syncProgress"})).unwrap();
        let h = window(a, 800.0);
        let row = |id| h.state().hit.rect("syncRow", id).unwrap();
        assert!(h.state().hit.rect("syncSkip", ids[1]).is_some(), "the row waits");
        // Name, Waiting, the Inbox time and Full messages: four lines of room.
        assert!(row(ids[1]).height() >= 4.0 * 16.0, "{:?}", row(ids[1]));
        assert!(row(ids[2]).min.y >= row(ids[1]).max.y, "{:?} over {:?}", row(ids[2]), row(ids[1]));
    }

    /// A Microsoft account's full messages: counted under its row as they come in; Stop pauses
    /// them, and the next run, the owner's or one by itself, has them go on.
    #[test]
    fn full_messages_pause_with_stop_and_go_on_with_the_next_run() {
        use mailcraft_engine::cmd::graph_mail;
        use mailcraft_model::sync::{GraphFetchResult, GraphFetched, ServerMessage, Tokens};

        use crate::graph_fetch::GraphFetchJob;

        let (mut a, ids, pipe) = app();
        let ms = a.execute("account.add", json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        let inbox = a.session.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
        for n in 0..50 {
            let server = ServerMessage { id: format!("AAM{n}"), folder: "inbox".into(), ..Default::default() };
            let m = Message {
                account: ms,
                folder: inbox,
                subject: format!("Note {n}"),
                date: NOW - n,
                received: NOW - n,
                unfetched: true,
                server: Some(server),
                ..Default::default()
            };
            a.session.mailbox.add_message(m).unwrap();
        }
        a.seed_graph_token(
            ms,
            Tokens { access: "stand-in".into(), refresh: "r".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() },
        );
        let jobs: Arc<Mutex<Vec<(GraphFetchJob, mpsc::Sender<GraphFetchResult>)>>> = Default::default();
        let asked = jobs.clone();
        a.services.graph_fetch = Some(Box::new(move |job: GraphFetchJob| {
            let (tx, rx) = mpsc::channel();
            asked.lock().unwrap().push((job, tx));
            rx
        }));
        // Every job is answered with the messages it asked for.
        let answer = |a: &mut MailApp| {
            let pending: Vec<_> = std::mem::take(&mut *jobs.lock().unwrap());
            for (job, tx) in pending {
                let fetched =
                    job.request.ids.iter().map(|id| GraphFetched { id: id.clone(), mime: b"Subject: Note\r\n\r\nWords.\r\n".to_vec() }).collect();
                tx.send(GraphFetchResult { account: ms, fetched, ..Default::default() }).unwrap();
            }
            a.drive_fetch();
        };
        let asked_for = || jobs.lock().unwrap().len();
        let full = |a: &MailApp| rows(a).into_iter().find(|r| r.account == ms).and_then(|r| r.full_line());

        // A round of every account, the Microsoft one among them.
        a.execute("ui.sync", json!({})).unwrap();
        a.drive_fetch();
        assert_eq!(asked_for(), 1, "the background fetches them");
        answer(&mut a);
        assert_eq!(full(&a).as_deref(), Some("Full messages: 10 of 50"));
        let said = a.execute("sync.progress", json!({})).unwrap();
        let row = said["accounts"].as_array().unwrap().iter().find(|r| r["id"] == json!(ms)).unwrap().clone();
        assert_eq!(row["fullMessages"], json!({"done": 10, "total": 50, "paused": false}));
        assert_eq!(said["accounts"][0]["fullMessages"], json!(null), "a mail server's account has none");

        // Stop: the batch in hand is called off, and no other is asked for.
        a.execute("ui.cancelSync", json!({})).unwrap();
        assert!(jobs.lock().unwrap().iter().all(|(job, _)| job.cancel.load(std::sync::atomic::Ordering::Relaxed)));
        answer(&mut a);
        for _ in 0..3 {
            a.drive_fetch();
        }
        assert_eq!(asked_for(), 0);
        assert_eq!(full(&a).as_deref(), Some("Full messages: 20 of 50 · Paused until the next Send/Receive"));
        assert_eq!(graph_mail::graph_fetch_request(&a.session, ms, None, 50).unwrap().ids.len(), 30, "thirty still to go");
        send(&pipe, SyncResult { account: ids[0], stopped: true, ..Default::default() });
        a.poll_sync();
        a.auto_sync();
        assert!(!a.syncing());
        a.drive_fetch();
        assert_eq!(asked_for(), 0, "still paused once the run has ended");

        // The owner's next Send/Receive: they go on.
        a.execute("ui.sync", json!({"account": ids[1]})).unwrap();
        a.drive_fetch();
        assert_eq!(asked_for(), 1);
        answer(&mut a);
        // (The Microsoft account isn't in this run: it has no row, and its numbers go on.)
        let counted = |a: &MailApp| (a.session.sync_progress.full(ms), a.session.sync_progress.full_paused);
        assert_eq!(counted(&a), (Some(FullMessages { done: 30, total: 50 }), false));
        a.execute("ui.cancelSync", json!({})).unwrap();
        answer(&mut a);
        assert_eq!(counted(&a), (Some(FullMessages { done: 40, total: 50 }), true));
        send(&pipe, SyncResult { account: ids[1], stopped: true, ..Default::default() });
        a.poll_sync();
        a.auto_sync();

        // A Send/Receive that starts by itself does too.
        a.auto_sync = true;
        a.session.prefs.sync_minutes = 5;
        a.last_auto_sync = 0.0;
        a.auto_sync();
        a.auto_sync();
        assert!(a.syncing(), "a round started by itself");
        a.drive_fetch();
        assert_eq!(asked_for(), 1);
        answer(&mut a);
        assert_eq!(full(&a), None, "all of them are here: no line");
        assert!(a.session.mailbox.messages.values().all(|m| !m.unfetched));
    }

    /// #276: a span of time is short, from a tenth of a second to minutes, and never overflows.
    #[test]
    fn a_span_of_time_is_short() {
        assert_eq!(took(40), "0.0 s");
        assert_eq!(took(400), "0.4 s");
        assert_eq!(took(9_940), "9.9 s");
        assert_eq!(took(12_400), "12 s");
        assert_eq!(took(59_400), "59 s");
        assert_eq!(took(125_000), "2 min 05 s");
        assert!(!took(u64::MAX).is_empty());
    }

    /// #276: a done row says how long it took, and under it how long each phase did; a running
    /// one names only the phases that are through.
    #[test]
    fn a_row_says_how_long_it_took_and_each_phase() {
        use mailcraft_model::sync::{SyncPhase, Timings};
        let mut timings = Timings::default();
        timings.add(SyncPhase::Sending, 400, 2);
        timings.add(SyncPhase::Folders, 1_200, 4);
        timings.add(SyncPhase::NewMail, 33_000, 25);
        let row = |run: AccountRun| Row { account: 1, name: "A".into(), run: Some(run), problems: Vec::new(), full: None, paused: false };
        let done = row(AccountRun { state: RunState::Done, new: 34, timings: timings.clone(), ..Default::default() });
        assert_eq!(done.line(), "Done: 34 new · 35 s");
        assert_eq!(done.phases_line().as_deref(), Some("Sending 0.4 s · Folders 1.2 s · New mail 33 s"));
        let up_to_date = row(AccountRun { state: RunState::Done, timings: timings.clone(), ..Default::default() });
        assert_eq!(up_to_date.line(), "Done: up to date · 35 s");
        let stopped = row(AccountRun { state: RunState::Stopped, timings: timings.clone(), ..Default::default() });
        assert_eq!(stopped.line(), "Stopped · 35 s");
        // Running: the phase it is in is not through yet.
        let running = row(AccountRun { state: RunState::Running, phase: Some(SyncPhase::NewMail), timings, ..Default::default() });
        assert_eq!(running.phases_line().as_deref(), Some("Sending 0.4 s · Folders 1.2 s"));
        // Nothing measured: nothing said (a host that doesn't time, a run from before).
        let none = row(AccountRun { state: RunState::Done, new: 3, ..Default::default() });
        assert_eq!((none.line().as_str(), none.phases_line()), ("Done: 3 new", None));
    }

    /// #276: Copy Diagnostics adds the last run's time per phase, requests and folders skipped
    /// for each account, with no address or name in it.
    #[test]
    fn copy_diagnostics_has_each_accounts_timings_and_no_addresses() {
        use mailcraft_model::sync::{SyncPhase, Timings};
        let (mut a, ids, _) = app();
        assert!(a.execute("ui.copyDiagnostics", json!({})).unwrap()["text"].as_str().unwrap().contains("- none yet"));
        let mut timings = Timings::default();
        timings.add(SyncPhase::Folders, 1_200, 4);
        timings.add(SyncPhase::NewMail, 8_300, 21);
        timings.skipped = 3;
        let piece = SyncResult { account: ids[0], timings, ..Default::default() };
        a.run("sync.apply", json!({"result": piece})).unwrap();
        let text = a.execute("ui.copyDiagnostics", json!({})).unwrap()["text"].as_str().unwrap().to_string();
        assert!(text.contains("Send/Receive timings"), "{text}");
        assert!(
            text.contains("- Account 1 (mail server): done, 9500 ms in all, 25 requests, 3 folders skipped; Folders 1200 ms (4 requests), New mail 8300 ms (21 requests)"),
            "{text}"
        );
        assert!(!text.contains("mail.example") && !text.contains("avery") && !text.contains("Inbox"), "{text}");
    }

    /// What a row says, whatever the numbers.
    #[test]
    fn a_row_says_where_it_is_in_one_line() {
        use mailcraft_model::sync::SyncPhase;
        let row = |run: AccountRun, problems: usize| Row {
            account: 1,
            name: "A".into(),
            run: Some(run),
            problems: vec!["x".into(); problems],
            full: None,
            paused: false,
        };
        let running = |phase, folder: &str, done, total| AccountRun {
            state: RunState::Running,
            phase: Some(phase),
            folder: folder.into(),
            done,
            total,
            ..Default::default()
        };
        assert_eq!(row(running(SyncPhase::NewMail, "Inbox", 150, 500), 0).line(), "New mail: Inbox, 150 of 500");
        assert_eq!(row(running(SyncPhase::NewMail, "Inbox", 40, 0), 0).line(), "New mail: Inbox, 40");
        assert_eq!(row(running(SyncPhase::CalendarAndContacts, "", 0, 0), 0).line(), "Calendar and contacts");
        assert_eq!(row(AccountRun { state: RunState::Running, ..Default::default() }, 0).line(), "Starting…");
        // (A count past its total is a full bar, not more.)
        assert_eq!(row(running(SyncPhase::NewMail, "Inbox", u32::MAX, 3), 0).fraction(), Some(1.0));
        let done = |new, sent| AccountRun { state: RunState::Done, new, sent, ..Default::default() };
        assert_eq!(row(done(34, 2), 0).line(), "Done: 34 new, 2 sent");
        assert_eq!(row(done(1, 0), 0).line(), "Done: 1 new");
        assert_eq!(row(done(0, 0), 0).line(), "Done: up to date");
        assert_eq!(row(done(3, 0), 1).line(), "1 problem");
        assert_eq!(row(AccountRun { state: RunState::Stopped, new: 4, ..Default::default() }, 0).line(), "Stopped");
        let none = Row { account: 1, name: "A".into(), run: None, problems: vec!["x".into(); 2], full: None, paused: false };
        assert_eq!((none.line(), none.full_line()), ("2 problems".to_string(), None));
        assert_eq!(row(AccountRun { state: RunState::Skipped, ..Default::default() }, 1).line(), "Skipped");
        // Full messages, while some are still to be fetched; paused by Stop.
        let full = |done, total, paused| Row { full: Some(FullMessages { done, total }), paused, ..none.clone() }.full_line();
        assert_eq!(full(40, 900, false).as_deref(), Some("Full messages: 40 of 900"));
        assert_eq!(full(40, 900, true).as_deref(), Some("Full messages: 40 of 900 · Paused until the next Send/Receive"));
        assert_eq!(full(900, 900, true), None, "all of them are here");
    }
}
