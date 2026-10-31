// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! How the mail looks on screen: the list's ready-made views, its tighter spacing, the zoom of the
//! message being read, and the searches run lately. All of it is this computer's settings
//! ([`crate::Prefs`]) and the view: nothing here changes the mailbox or goes to a server.

use serde_json::{Value, json};

use crate::cmd::bad;
use crate::{CmdResult, CommandSpec, Pane, Prefs, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("view.changeView", "Change View", "View › Current View", change_view).pure().params(
            r#"{"value"?: "compact"|"single"|"preview"} — compact: rows of two lines with a line of their text, the reading pane on the right; single: rows of one line, the reading pane on the right; preview: rows with three lines of their text and no reading pane. Omit to read which the list is in ("custom": none of them)"#,
        ),
        CommandSpec::new("view.density", "Tighter Spacing", "View › Messages", density)
            .pure()
            .params(r#"{"value"?: bool} (default: toggle) — the message list's rows closer together"#),
        CommandSpec::new("view.zoom", "Zoom", "View › Zoom", zoom).pure().params(
            r#"{"value"?: 50-300 (percent), "step"?: "in"|"out"|"reset"} — how large the text of the message being read is (in the reading pane and in a window of its own); omit both to read it"#,
        ),
        CommandSpec::new("view.zoomIn", "Zoom In", "View › Zoom", zoom_in).pure().key("Mod+Plus"),
        CommandSpec::new("view.zoomOut", "Zoom Out", "View › Zoom", zoom_out).pure().key("Mod+Minus"),
        CommandSpec::new("view.zoomReset", "Actual Size", "View › Zoom", zoom_reset).pure().key("Mod+0"),
        CommandSpec::new("view.recentSearches", "Recent Searches", "Search", recent_searches).pure().params(
            r#"{"remember"?: string, "clear"?: bool} — the searches run lately, the latest first (10 at most), kept in this computer's settings and never sent anywhere. "remember" puts one first; "clear": true forgets them all. Answers the list"#,
        ),
    ]
}

/// The list's ready-made views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListView {
    /// Rows of two lines with a line of their text; the reading pane on the right.
    Compact,
    /// Rows of one line; the reading pane on the right.
    Single,
    /// Rows with three lines of their text; no reading pane.
    Preview,
}

impl ListView {
    pub const ALL: [ListView; 3] = [ListView::Compact, ListView::Single, ListView::Preview];

    pub fn code(self) -> &'static str {
        match self {
            ListView::Compact => "compact",
            ListView::Single => "single",
            ListView::Preview => "preview",
        }
    }

    /// Its name in a menu (English; the window shows it through `tl!`).
    pub fn label(self) -> &'static str {
        match self {
            ListView::Compact => "Compact",
            ListView::Single => "Single Line",
            ListView::Preview => "Preview",
        }
    }

    fn parse(code: &str) -> Option<ListView> {
        ListView::ALL.into_iter().find(|v| v.code() == code)
    }

    /// The view the list is in now, when it is one of these.
    pub fn of(s: &Session) -> Option<ListView> {
        let (one, lines, pane) = (s.prefs.one_line, s.prefs.preview_lines.min(3), s.view.reading_pane);
        ListView::ALL.into_iter().find(|v| match v {
            ListView::Compact => !one && lines == 1 && pane == Pane::Right,
            ListView::Single => one && pane == Pane::Right,
            ListView::Preview => !one && lines == 3 && pane == Pane::Off,
        })
    }

    fn apply(self, s: &mut Session) {
        let (one, lines, pane) = match self {
            ListView::Compact => (false, 1, Pane::Right),
            // A single-line row shows no text of the message: the lines set before are kept for
            // when the rows have two lines again.
            ListView::Single => (true, s.prefs.preview_lines.min(3), Pane::Right),
            ListView::Preview => (false, 3, Pane::Off),
        };
        (s.prefs.one_line, s.prefs.preview_lines, s.view.reading_pane) = (one, lines, pane);
    }
}

fn change_view(s: &mut Session, v: &Value) -> CmdResult {
    if let Some(code) = v.get("value").filter(|x| !x.is_null()) {
        let view = code.as_str().and_then(ListView::parse).ok_or_else(|| bad("`value` is compact, single or preview"))?;
        view.apply(s);
    }
    Ok(json!({
        "value": ListView::of(s).map_or("custom", ListView::code),
        "oneLine": s.prefs.one_line,
        "previewLines": s.prefs.preview_lines,
        "readingPane": s.view.reading_pane,
    }))
}

fn density(s: &mut Session, v: &Value) -> CmdResult {
    s.prefs.tight_spacing = p::bool(v, "value").unwrap_or(!s.prefs.tight_spacing);
    Ok(json!({"value": s.prefs.tight_spacing}))
}

/// The zooms In and Out step through, in percent.
pub const ZOOM_STEPS: [u16; 12] = [50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 300];

/// The next step above (`up`) or below `now`; the end of the range where there is none.
pub fn zoom_step(now: u16, up: bool) -> u16 {
    let next = if up { ZOOM_STEPS.iter().copied().find(|z| *z > now) } else { ZOOM_STEPS.iter().rev().copied().find(|z| *z < now) };
    next.unwrap_or(if up { Prefs::ZOOM_MAX } else { Prefs::ZOOM_MIN })
}

fn set_zoom(s: &mut Session, percent: u16) -> CmdResult {
    s.prefs.zoom = percent.clamp(Prefs::ZOOM_MIN, Prefs::ZOOM_MAX);
    Ok(json!({"value": s.prefs.zoom}))
}

fn zoom(s: &mut Session, v: &Value) -> CmdResult {
    let now = s.prefs.zoom_percent();
    if let Some(step) = v.get("step").filter(|x| !x.is_null()) {
        return match step.as_str() {
            Some("in") => set_zoom(s, zoom_step(now, true)),
            Some("out") => set_zoom(s, zoom_step(now, false)),
            Some("reset") => set_zoom(s, Prefs::ZOOM_DEFAULT),
            _ => Err(bad("`step` is in, out or reset")),
        };
    }
    match v.get("value").filter(|x| !x.is_null()) {
        None => set_zoom(s, now),
        Some(x) => {
            let percent = x.as_f64().filter(|f| f.is_finite()).ok_or_else(|| bad("`value` is a number of percent, 50 to 300"))?;
            // In range first, so the cast is exact.
            set_zoom(s, percent.round().clamp(f64::from(Prefs::ZOOM_MIN), f64::from(Prefs::ZOOM_MAX)) as u16)
        }
    }
}

fn zoom_in(s: &mut Session, _: &Value) -> CmdResult {
    let now = s.prefs.zoom_percent();
    set_zoom(s, zoom_step(now, true))
}

fn zoom_out(s: &mut Session, _: &Value) -> CmdResult {
    let now = s.prefs.zoom_percent();
    set_zoom(s, zoom_step(now, false))
}

fn zoom_reset(s: &mut Session, _: &Value) -> CmdResult {
    set_zoom(s, Prefs::ZOOM_DEFAULT)
}

/// `query` put first in the list of recent searches: without control characters, cut to
/// [`Prefs::RECENT_SEARCH_CHARS`], once (the same search run before moves up), the list cut to
/// [`Prefs::RECENT_SEARCHES`]. An empty search is not kept.
pub fn remember_search(prefs: &mut Prefs, query: &str) {
    let query: String = query.chars().filter(|c| !c.is_control()).take(Prefs::RECENT_SEARCH_CHARS).collect();
    let query = query.trim();
    if query.is_empty() {
        return;
    }
    let mut list: Vec<String> = vec![query.to_string()];
    list.extend(prefs.recent_searches().filter(|q| q.trim() != query).map(str::to_string));
    list.truncate(Prefs::RECENT_SEARCHES);
    prefs.recent_searches = list;
}

fn recent_searches(s: &mut Session, v: &Value) -> CmdResult {
    if p::bool(v, "clear") == Some(true) {
        s.prefs.recent_searches.clear();
    }
    if let Some(query) = v.get("remember").filter(|x| !x.is_null()) {
        let query = query.as_str().ok_or_else(|| bad("`remember` is the search (a string)"))?;
        remember_search(&mut s.prefs, query);
    }
    let list: Vec<&str> = s.prefs.recent_searches().collect();
    Ok(json!({"searches": list}))
}

#[cfg(test)]
#[path = "display_tests.rs"]
mod display_tests;
