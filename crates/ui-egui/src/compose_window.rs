// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message being written in a window of its own (#286).
//!
//! The engine holds every message being written, each with an id, and one of them is current: the
//! one a command that names none acts on ([`mailcraft_engine::Session::show_compose`]). Here is
//! where each of them is shown: in a window of its own, in the reading pane (at most one,
//! [`pane_compose`]), or nowhere for now (one put aside for another in the reading pane, saved to
//! Drafts first; it is shown again once it is current, a click on its draft, say).
//!
//! The current one is the one in the window that has the keyboard, or else the reading pane's
//! ([`follow_focus`]). While a window is laid out its message is current, and while the main
//! window is, the reading pane's is ([`front_pane`]): what each runs acts on its own message.
//!
//! What opens in a window ([`place`]): New Email; a message being written that Pop Out moves out
//! of the reading pane (`compose.popOut`); a draft opened from the list by a double click; a reply
//! or a forward started in a message's own window, or anywhere once File › Options says so; and
//! every message an agent starts, behind the owner's window and without the keyboard. At the
//! bound of windows ([`super::MAX_WINDOWS`]) a reply or forward stays in the reading pane, and
//! anything else is refused with a line in the status bar.

use egui::{Key, Ui};
use serde_json::{Value, json};

use super::{Open, Opens, What, embedded_layer, viewport_of};
use crate::forms::Verdict;
use crate::{MailApp, theme::Tokens};

/// The size a window of a message being written opens at, in points.
pub(super) const SIZE: [f32; 2] = [980.0, 720.0];

/// The tabs of its ribbon, in order.
pub(crate) const TABS: [&str; 4] = ["Message", "Insert", "Format Text", "Review"];

/// The commands that answer a message: the reading pane is theirs, and a window only from a
/// message's own window or when File › Options says so; at the bound they stay in the pane.
pub(crate) const ANSWERS: [&str; 5] = ["mail.reply", "mail.replyAll", "mail.replyAndDelete", "mail.forward", "mail.forwardAsAttachment"];

/// What the window says when there is no room for another window.
fn too_many() -> String {
    tl!("Too many windows are open: close one first").to_string()
}

/// The message being written that the reading pane shows, by its id: open, and in no window.
pub(crate) fn pane_compose(app: &MailApp) -> Option<u64> {
    app.windows.pane.filter(|p| app.session.composing_by_id(*p).is_some() && app.windows.of_compose(*p).is_none())
}

/// Settle which message the reading pane shows: the current one when it has no window; else the
/// one it showed, while that is open and has none; else the one put aside last that has none. An
/// agent's command never takes the reading pane from the message it shows.
pub(crate) fn settle_pane(app: &mut MailApp) {
    let windowed = |c: u64| app.windows.of_compose(c).is_some();
    let s = &app.session;
    let held = s.agent.is_some() && pane_compose(app).is_some();
    let now = match s.compose_id() {
        Some(c) if !windowed(c) && !held => Some(c),
        _ => pane_compose(app).or_else(|| s.composing.iter().rev().map(|c| c.id).find(|c| !windowed(*c))),
    };
    app.windows.pane = now;
}

/// Where a command the owner runs opens a message being written, unless the caller said
/// ([`Opens`]): New Email in a window of its own; a reply or a forward too when File › Options
/// says so ("Open replies and forwards in a new window").
pub(crate) fn hint(app: &MailApp, id: &str) -> Option<Opens> {
    let window = Opens::Window { behind: false };
    match id {
        "mail.new" => Some(window),
        _ if ANSWERS.contains(&id) && app.ui.replies_in_window => Some(window),
        _ => None,
    }
}

/// Before a command runs ([`MailApp::execute`]): one that would open a message in a window of
/// its own when there is no room for one is refused, unless the owner answers a message (that one
/// stays in the reading pane). An agent's are all refused: the reading pane is the owner's. Pop
/// Out too, for a message that has no window yet.
pub(crate) fn before_run(app: &MailApp, id: &str, params: &Value) -> Result<(), String> {
    if !app.windows.full() {
        return Ok(());
    }
    let starting = mailcraft_engine::cmd::compose::STARTING.contains(&id);
    let agent = app.session.agent.is_some();
    let window = matches!(app.windows.place, Some(Opens::Window { .. })) || agent;
    if starting && window && (agent || !ANSWERS.contains(&id)) {
        // (A draft being written already only comes forward, in its window or the reading pane.)
        let open = id == "mail.editDraft" && being_written(app, params).is_some();
        if !open {
            return Err(too_many());
        }
    }
    if id == "compose.popOut" && app.session.compose_named(params).is_some_and(|c| app.windows.of_compose(c.id).is_none()) {
        return Err(too_many());
    }
    Ok(())
}

/// The message being written of the draft `mail.editDraft` with `params` would open, when it is
/// being written already.
fn being_written(app: &MailApp, params: &Value) -> Option<u64> {
    let draft = mailcraft_engine::p::id(params, "id").or_else(|| app.session.sel.focus())?;
    Some(app.session.all_composing().find(|c| c.draft == Some(draft))?.id)
}

/// After a command ran ([`MailApp::execute`]), with the ids of the messages being written before
/// it: each it opened goes where it is to ([`hint`], [`Opens`]); an agent's into a window behind
/// the owner's. Pop Out moves its message into a window. The answer names the window (`window`).
pub(crate) fn after_run(app: &mut MailApp, id: &str, before: &[u64], r: Result<Value, String>) -> Result<Value, String> {
    let place = app.windows.place.take();
    let mut answer = match r {
        Ok(answer) => answer,
        Err(e) => {
            settle_pane(app);
            return Err(e);
        }
    };
    let mut window = None;
    let agent = app.session.agent.is_some();
    // (What it made current goes back to the owner's on the next frame, [`follow_focus`].)
    app.windows.restore |= agent;
    if id == "compose.popOut"
        && let Some(compose) = answer.get("compose").and_then(Value::as_u64)
    {
        // An agent's opens behind the owner's window, without the keyboard.
        window = Some(put_in_window(app, compose, agent)?);
    }
    let opened: Vec<u64> = app.session.all_composing().map(|c| c.id).filter(|c| !before.contains(c)).collect();
    for compose in &opened {
        let want = if agent { Opens::Window { behind: true } } else { place.unwrap_or(Opens::Pane) };
        match want {
            Opens::Window { behind } if !app.windows.full() => window = Some(put_in_window(app, *compose, behind)?),
            // (An agent's never takes the reading pane: at the bound it was refused.)
            _ if agent => {}
            // At the bound, it stays in the reading pane (only a reply or forward gets this far).
            _ => app.windows.pane = Some(*compose),
        }
    }
    // A draft that was open already, asked for in a window: its window, or one for it (at the
    // bound, it stays where it is).
    if opened.is_empty()
        && let Some(Opens::Window { behind }) = place
        && id == "mail.editDraft"
        && let Some(compose) = answer.get("compose").and_then(Value::as_u64)
        && (!app.windows.full() || app.windows.of_compose(compose).is_some())
    {
        window = Some(put_in_window(app, compose, behind || agent)?);
    }

    if let (Some(window), Some(fields)) = (window, answer.as_object_mut()) {
        fields.insert("window".into(), json!(window));
    }
    settle_pane(app);
    Ok(answer)
}

/// Put the message being written `compose` in a window of its own: the one it has (brought
/// forward, unless `behind`), or a new one. Its number.
fn put_in_window(app: &mut MailApp, compose: u64, behind: bool) -> Result<u64, String> {
    if app.session.composing_by_id(compose).is_none() {
        return Err(format!("no message being written has the id {compose}"));
    }
    if let Some(window) = app.windows.of_compose(compose) {
        if !behind {
            app.windows.raise(window);
        }
        return Ok(window);
    }
    if app.windows.full() {
        return Err(too_many());
    }
    let window = app.windows.add(What::Compose { compose, tab: "Message" });
    if let Some(open) = app.windows.open.iter_mut().find(|w| w.id == window) {
        open.behind = behind;
        open.raise = !behind;
    }
    // The owner's goes in front, with the keyboard: its message is the current one.
    if !behind {
        app.session.show_compose(compose);
        app.windows.focus = Some(compose);
    }
    if app.windows.pane == Some(compose) {
        app.windows.pane = None;
    }
    Ok(window)
}

/// `ui.openWindow {"compose": id}`: as Pop Out does, for that message.
pub(crate) fn pop_out(app: &mut MailApp, compose: u64) -> Result<Value, String> {
    app.execute("compose.popOut", json!({"compose": compose}))
}

/// `ui.openWindow` of a draft, as a double click on it in the list does: it opens in a window of
/// its own, ready to be written in (or its window comes forward).
pub(crate) fn open_draft(app: &mut MailApp, draft: mailcraft_model::Id) -> Result<Value, String> {
    let agent = app.session.agent.is_some();
    app.windows.place = Some(Opens::Window { behind: agent });
    let r = app.execute("mail.editDraft", json!({"id": draft}));
    app.windows.place = None;
    r
}

/// Close the message being written `compose` with its window: saved to Drafts first when it has
/// changes, or (`discard`) as it was last saved. `false` when it couldn't be, and why is said.
pub(crate) fn close_message(app: &mut MailApp, compose: u64, discard: bool) -> bool {
    if app.session.composing_by_id(compose).is_none() {
        return true;
    }
    let mut params = json!({"compose": compose});
    if discard {
        params["discard"] = json!(true);
    }
    match app.execute("compose.close", params) {
        Ok(_) => true,
        Err(e) => {
            app.status(e);
            false
        }
    }
}

/// `ui.closeWindow` of windows that show a message being written: each closes with its message,
/// which the call says how: `save` (to Drafts) or `discard` (as it was last saved). Neither, or
/// both, is an error, and nothing is closed (no command asks the owner anything).
pub(crate) fn close_by_command(app: &mut MailApp, closing: &[u64], p: &Value) -> Result<(), String> {
    let composes: Vec<u64> = app.windows.open.iter().filter(|w| closing.contains(&w.id)).filter_map(|w| w.what.compose()).collect();
    if composes.is_empty() {
        return Ok(());
    }
    let said = |key: &str| match p.get(key).filter(|v| !v.is_null()) {
        None => Ok(false),
        Some(v) => v.as_bool().ok_or_else(|| format!("`{key}` is true or false")),
    };
    let (save, discard) = (said("save")?, said("discard")?);
    if save == discard {
        return Err(
            "a window with a message being written closes with it: say `save: true` (it goes to Drafts) or `discard: true` (it closes as it was last saved)"
                .to_string(),
        );
    }
    for compose in composes {
        let mut params = json!({"compose": compose});
        if discard {
            params["discard"] = json!(true);
        }
        app.execute("compose.close", params)?;
    }
    Ok(())
}

/// What an agent's `ui.closeWindow` with `discard` would throw away of what the owner wrote
/// (#494): each of the windows it closes whose message the owner wrote in and that isn't all in
/// Drafts, as the owner's question names it.
pub(crate) fn discarding(app: &MailApp, closing: &[u64]) -> Vec<Value> {
    let s = &app.session;
    app.windows
        .open
        .iter()
        .filter(|w| closing.contains(&w.id))
        .filter_map(|w| w.what.compose())
        .filter_map(|id| s.composing_by_id(id))
        .filter(|c| c.owner_changed && mailcraft_engine::cmd::compose::unsaved(s, c))
        .map(|c| json!({"form": "message", "title": c.subject}))
        .collect()
}

/// The main window closes (#286): every message being written that isn't all in Drafts is saved
/// there, unasked, the ones in windows of their own and the reading pane's. `false`, with why
/// said, at the first that can't be: the owner is asked then, as before.
pub(crate) fn save_all_for_quit(app: &mut MailApp) -> bool {
    let s = &app.session;
    let unsaved: Vec<u64> = s.all_composing().filter(|c| c.dirty || mailcraft_engine::cmd::compose::draft_gone(s, c)).map(|c| c.id).collect();
    unsaved.into_iter().all(|compose| match app.execute("compose.saveDraft", json!({"compose": compose})) {
        Ok(_) => true,
        Err(e) => {
            app.status(e);
            false
        }
    })
}

/// The message being written current while the window that has the keyboard is laid out: once
/// a frame, before the windows are shown. When another window takes the keyboard (or the main
/// window does), its message is made current (the main window's: the reading pane's). Nothing
/// changes while a dialog or an agent's question is open: what it was opened for stays current.
pub(super) fn follow_focus(app: &mut MailApp, ctx: &egui::Context) {
    if app.dialog.is_some() || !app.asking.is_empty() {
        return;
    }
    let target = if ctx.embed_viewports() {
        let active = crate::keys::active(ctx);
        Some(app.windows.open.iter().find(|w| embedded_layer(w.id) == active).and_then(|w| w.what.compose()))
    } else {
        let focused = |v: egui::ViewportId| ctx.input(|i| i.raw.viewports.get(&v).and_then(|info| info.focused)).unwrap_or(false);
        match app.windows.open.iter().find(|w| focused(viewport_of(w.id))) {
            Some(w) => Some(w.what.compose()),
            None if focused(egui::ViewportId::ROOT) => Some(None),
            // (Another program has the keyboard: what was current stays.)
            None => None,
        }
    };
    let mut changed = false;
    if let Some(target) = target
        && target != app.windows.focus
    {
        app.windows.focus = target;
        changed = true;
        if let Some(compose) = target
            && let Some(w) = app.windows.open.iter_mut().find(|w| w.what.compose() == Some(compose))
        {
            // The owner went to it: it is theirs now.
            w.behind = false;
        }
    }
    // When the keyboard moved, and after an agent's command: what an agent opened behind doesn't
    // stay current, so the menu bar, a key and a dropped file act on the message of the window
    // that has the keyboard. Not otherwise: a message a script just started stays current for
    // the script's next call, until the owner moves (#286).
    if (changed || std::mem::take(&mut app.windows.restore))
        && let Some(want) = self::target(app)
    {
        app.session.show_compose(want);
    }
}

/// The message being written that the window with the keyboard acts on: its window's, or (the
/// main window has it) the reading pane's. `None` when it has none.
pub(crate) fn target(app: &MailApp) -> Option<u64> {
    match app.windows.focus {
        Some(c) if app.windows.of_compose(c).is_some() => Some(c),
        _ => pane_compose(app),
    }
}

/// Whether the command `id`, run from outside a window's own layout (the menu bar, Search
/// commands), acts on the message the window with the keyboard writes: one that acts on a message
/// being written doesn't from the main window when its reading pane has none, rather than act on
/// a message in a window behind it.
pub(crate) fn acts_here(app: &MailApp, id: &str) -> bool {
    let id = if id == "ui.attach" { "compose.attach" } else { id };
    let writing = app.session.registry.all().iter().any(|c| c.id == id && c.writing);
    !writing || app.session.compose_id().is_some_and(|c| target(app) == Some(c))
}

/// What [`front_pane`] changed, for [`back`] to put back.
pub(crate) struct Fronted {
    was: Option<u64>,
    to: u64,
    ids: Vec<u64>,
    dialog: bool,
}

/// The main window is laid out: the reading pane's message is current meanwhile, so that its
/// ribbon, its keys and its composer act on it. Answers what to put back after ([`back`]).
pub(crate) fn front_pane(app: &mut MailApp) -> Option<Fronted> {
    settle_pane(app);
    let to = pane_compose(app)?;
    let was = app.session.compose_id();
    if was == Some(to) {
        return None;
    }
    app.session.show_compose(to);
    Some(Fronted { was, to, ids: app.session.all_composing().map(|c| c.id).collect(), dialog: app.dialog.is_some() })
}

/// The main window was laid out: the message current before it is current again, unless what was
/// done there opened another (it is current), or a dialog for the reading pane's message (that
/// one stays current until the dialog is answered).
pub(crate) fn back(app: &mut MailApp, fronted: Option<Fronted>) {
    let Some(f) = fronted else { return };
    if app.session.all_composing().any(|c| !f.ids.contains(&c.id)) {
        return;
    }
    if !f.dialog && app.dialog.is_some() {
        app.windows.focus = None;
        return;
    }
    if app.session.compose_id() == Some(f.to)
        && let Some(was) = f.was
    {
        app.session.show_compose(was);
    }
}

/// The keys of a window of a message being written that has the keyboard, taken out of what was
/// pressed: the registry's for the commands that act on a message being written (Send, Save
/// Draft, Check Names…), run on its message; Escape and Ctrl+W are the window's own (they close
/// it, [`super::keys_of`]), and the body's chords (Ctrl+B, Z…) are the body's.
fn run_keys(app: &mut MailApp, ctx: &egui::Context) {
    let in_body = crate::compose::editor::has_focus(ctx);
    let pressed: Vec<(Key, egui::Modifiers)> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            })
            .take(8)
            .collect()
    });
    for (key, modifiers) in pressed {
        let Some(name) = crate::keys::key_name(key) else { continue };
        let function = name.len() > 1 && name.starts_with('F') && name[1..].chars().all(|c| c.is_ascii_digit());
        if !(modifiers.command || modifiers.alt || function) || (in_body && modifiers.command && crate::compose::editor::owns_chord(key)) {
            continue;
        }
        let combo = crate::keys::combo(modifiers, name);
        let bound = |c: &&mailcraft_engine::CommandSpec| {
            c.writing && !c.shortcut.is_empty() && c.shortcut.split(" / ").any(|k| k.eq_ignore_ascii_case(&combo) && k != "Escape")
        };
        let found = app.session.registry.all().iter().filter(bound).find(|c| (c.enabled)(&app.session).is_none()).map(|c| c.id);
        if let Some(id) = found {
            ctx.input_mut(|i| {
                i.events.retain(|e| !matches!(e, egui::Event::Key { key: k, pressed: true, modifiers: m, .. } if *k == key && *m == modifiers))
            });
            let _ = app.run(id, json!({}));
        }
    }
}

/// What is in a window of a message being written: its ribbon, and under it the composer the
/// reading pane has, for its message. `keyboard`: the composer may take the keyboard when nothing
/// has it (not for one that opened behind); `keys`: the window has the keyboard, and its keys are
/// its own to act on.
pub(super) fn contents(app: &mut MailApp, ui: &mut Ui, window: &mut Open, keyboard: bool, keys: bool) -> Verdict {
    let id = window.id;
    let What::Compose { compose, tab } = &mut window.what else { return Verdict::Stay };
    let compose = *compose;
    if app.session.composing_by_id(compose).is_none() {
        return Verdict::Close;
    }
    let t = Tokens::get(ui.ctx());
    let before: Vec<u64> = app.session.all_composing().map(|c| c.id).collect();
    let dialog = app.dialog.is_some();
    let was = app.session.compose_id();
    app.session.show_compose(compose);
    let scope = app.hit.scope(id);
    if keys {
        run_keys(app, ui.ctx());
    }
    crate::ribbon::show_window(app, ui, id, tab);
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.surface)).show(ui, |ui| {
        // Only while it is still open: what was done in its ribbon may have sent or closed it.
        if app.session.compose_id() == Some(compose) {
            ui.push_id(("compose", compose), |ui| crate::compose::show_in(app, ui, Some(id), keyboard));
        }
    });
    app.hit.scope(scope);
    let gone = app.session.composing_by_id(compose).is_none();
    let opened = app.session.all_composing().any(|c| !before.contains(&c.id));
    if !dialog && app.dialog.is_some() {
        // A dialog for this message (Insert Link, Word Count…), in the main window: the message
        // stays current until it is answered, and the main window comes forward for it.
        app.windows.focus = Some(compose);
        if !ui.ctx().embed_viewports() {
            ui.ctx().send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        }
    } else if !opened
        && let Some(was) = was
        && was != compose
    {
        app.session.show_compose(was);
    }
    if gone { Verdict::Close } else { Verdict::Stay }
}

#[cfg(test)]
#[path = "compose_window_tests.rs"]
mod compose_window_tests;
