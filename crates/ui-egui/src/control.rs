// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Programmatic control of the running app (agents, tests, MCP bridge).
//!
//! Methods (JSON lines over the host's transport, see `docs/control-protocol.md`):
//! - `engine.execute {command, params}`: run any command (engine or `ui.*`)
//! - `engine.commands`: every command with its enablement
//! - `mailbox.inspect`: accounts, folders and counts; `ui.inspect`: UI state, what is on screen
//!   and where (`items`), perf
//! - `ui.click {x, y, button?, count?, shift?, cmd?}`, `ui.move {x, y}`, `ui.drag {x, y, toX, toY}`:
//!   real pointer input in screen points (reaches every widget)
//! - `ui.key {key, shift?, alt?, cmd?}`, `ui.text {text}`: keyboard input through egui
//! - `ui.screenshot {path?, inline?}`: PNG of the window (`inline`: as base64 `data` in the answer)
//! - `app.status`: whether the mailbox is open; while the window still opens it, the one request
//!   answered (every other is refused, `crate::opening::STILL_OPENING`)
//!
//! A request the MCP bridge marked as an agent's (`agent`) goes through [`crate::agent_gate`].
//! - `ui.parity`: feature catalog parity

use std::sync::mpsc::Sender;

use serde_json::{Value, json};

use crate::MailApp;

pub struct ControlRequest {
    pub method: String,
    pub params: Value,
    pub reply: Sender<Value>,
    /// From an agent (the MCP bridge marks its requests), with what the owner allowed it in
    /// advance; `None`: the owner's own script.
    pub agent: Option<Vec<mailcraft_engine::agent::Class>>,
}

impl ControlRequest {
    pub fn new(method: impl Into<String>, params: Value) -> (Self, std::sync::mpsc::Receiver<Value>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Self { method: method.into(), params, reply: tx, agent: None }, rx)
    }

    /// The same request, as an agent's that was allowed `allow`.
    pub fn from_agent(mut self, allow: Vec<mailcraft_engine::agent::Class>) -> Self {
        self.agent = Some(allow);
        self
    }
}

/// What the `agent` field of a control-channel request allows (`{"allow": ["send", …]}`;
/// names it doesn't know allow nothing).
pub fn agent_allow(v: &Value) -> Vec<mailcraft_engine::agent::Class> {
    v.get("allow")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).filter_map(mailcraft_engine::agent::Class::parse).collect())
        .unwrap_or_default()
}

pub enum Outcome {
    Done(Value),
    Screenshot {
        path: Option<String>,
        inline: bool,
    },
    /// The owner is asked first ([`crate::agent_gate`]); the answer goes back from there.
    Pending,
}

/// Where things were drawn this frame: message rows, folders, events, contacts, tasks. Agents and
/// tests click by asking `ui.inspect` for an item's rectangle instead of guessing coordinates.
#[derive(Default)]
pub struct HitMap {
    items: Vec<(&'static str, u64, egui::Rect)>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Record that the `kind` (`message`, `messageRead` for a row's read/unread switch, `folder`,
    /// `event`, `contact`, `task`) with `id` is at `rect`.
    pub fn add(&mut self, kind: &'static str, id: u64, rect: egui::Rect) {
        // A list far longer than a screen can show isn't worth recording.
        if self.items.len() < 4000 {
            self.items.push((kind, id, rect));
        }
    }

    pub fn rect(&self, kind: &str, id: u64) -> Option<egui::Rect> {
        self.items.iter().find(|(k, i, _)| *k == kind && *i == id).map(|(_, _, r)| *r)
    }

    fn to_json(&self) -> Value {
        Value::Array(
            self.items.iter().map(|(kind, id, r)| json!({"kind": kind, "id": id, "rect": [r.left(), r.top(), r.width(), r.height()]})).collect(),
        )
    }
}

fn ok(v: Value) -> Outcome {
    Outcome::Done(json!({"ok": true, "result": v}))
}
fn err(e: impl std::fmt::Display) -> Outcome {
    Outcome::Done(json!({"ok": false, "error": e.to_string()}))
}

pub fn inspect(app: &mut MailApp, ctx: &egui::Context) -> Value {
    let r = ctx.content_rect();
    json!({
        "ui": app.prefs(),
        "view": app.session.view,
        "selection": app.session.sel,
        "composing": app.session.compose.is_some(),
        "dialog": app.dialog.as_ref().map(|d| d.name()),
        // Send/Receive Progress, a panel in the window rather than a dialog.
        "panel": app.sync_panel.as_ref().map(|_| "syncProgress"),
        "agentQuestion": app.asking.front().map(|a| json!({"command": a.id, "needs": a.class.as_str(), "what": a.what})),
        "windows": crate::windows::inspect(app),
        "window": [r.width(), r.height()],
        "items": app.hit.to_json(),
        "title": app.title_stem(),
        "dirty": app.session.dirty,
        "syncing": app.syncing(),
        "saving": app.saving_inspect(),
        // A mailbox being read in the background (File › Open).
        "opening": crate::opening::ahead_inspect(app),
        "status": app.status_msg.as_ref().map(|(m, _)| m.clone()),
        "textSelection": crate::reading::selection_inspect(app, ctx),
        "perf": {"frameMs": app.frame_ms},
        "input": {
            "windowFocused": ctx.input(|i| i.focused),
            "focus": ctx.memory(|m| m.focused()).map(|id| format!("{id:?}")),
            "bodyFocused": crate::compose::editor::has_focus(ctx),
            "trail": app.input_trail,
        },
    })
}

/// Most entries of [`MailApp::input_trail`].
const TRAIL: usize = 60;

/// Note this frame's input in the trail, by kind: no text, no positions.
pub(crate) fn trail(app: &mut MailApp, ctx: &egui::Context) {
    use egui::{Event, ImeEvent};
    for e in ctx.input(|i| i.raw.events.clone()) {
        let entry = match e {
            Event::Text(t) => format!("text {}", t.chars().count()),
            Event::Key { key, pressed, modifiers, .. } => {
                format!("key {} {}{}", key.name(), if pressed { "down" } else { "up" }, if modifiers.any() { " +mods" } else { "" })
            }
            Event::PointerButton { pressed, .. } => format!("button {}", if pressed { "down" } else { "up" }),
            Event::WindowFocused(f) => format!("windowFocused {f}"),
            Event::Ime(ImeEvent::Preedit { text, .. }) => format!("ime preedit {}", text.chars().count()),
            Event::Ime(ImeEvent::Commit(t)) => format!("ime commit {}", t.chars().count()),
            Event::Paste(_) => "paste".into(),
            Event::PointerGone => "pointer gone".into(),
            _ => continue,
        };
        app.input_trail.push_back(format!("{} {entry}", ctx.cumulative_pass_nr()));
        while app.input_trail.len() > TRAIL {
            app.input_trail.pop_front();
        }
    }
}

fn key_from(name: &str) -> Option<egui::Key> {
    egui::Key::from_name(name).or(match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => Some(egui::Key::Enter),
        "esc" | "escape" => Some(egui::Key::Escape),
        "delete" => Some(egui::Key::Delete),
        "backspace" => Some(egui::Key::Backspace),
        "left" => Some(egui::Key::ArrowLeft),
        "right" => Some(egui::Key::ArrowRight),
        "up" => Some(egui::Key::ArrowUp),
        "down" => Some(egui::Key::ArrowDown),
        "space" => Some(egui::Key::Space),
        "tab" => Some(egui::Key::Tab),
        "home" => Some(egui::Key::Home),
        "end" => Some(egui::Key::End),
        _ => None,
    })
}

fn mods(p: &Value) -> egui::Modifiers {
    let b = |k: &str| p.get(k).and_then(Value::as_bool).unwrap_or(false);
    let cmd = b("cmd") || b("command") || b("ctrl");
    egui::Modifiers {
        alt: b("alt"),
        ctrl: cmd && !cfg!(target_os = "macos"),
        shift: b("shift"),
        mac_cmd: cmd && cfg!(target_os = "macos"),
        command: cmd,
    }
}

fn click_events(app: &mut MailApp, pos: egui::Pos2, button: egui::PointerButton, count: u64, m: egui::Modifiers) {
    app.synthetic.push(egui::Event::PointerMoved(pos));
    for _ in 0..count.clamp(1, 3) {
        app.synthetic.push(egui::Event::PointerButton { pos, button, pressed: true, modifiers: m });
        app.synthetic.push(egui::Event::PointerButton { pos, button, pressed: false, modifiers: m });
    }
}

/// Every command, the engine's and the window's, with whether it can run now.
pub fn commands(app: &MailApp) -> Value {
    let reg = app.session.registry.clone();
    let mut all: Vec<Value> = reg
        .all()
        .iter()
        .map(|c| {
            let mut v = json!({"id": c.id, "label": c.label, "location": c.location, "shortcut": c.shortcut, "params": c.params, "enabled": (c.enabled)(&app.session).is_none()});
            mailcraft_engine::agent_fields(&mut v, c.id);
            v
        })
        .collect();
    all.extend(crate::UI_COMMANDS.iter().map(|(id, label, location, params)| {
        let mut v = json!({"id": id, "label": label, "location": location, "shortcut": "", "params": params, "enabled": app.enabled(id)});
        mailcraft_engine::agent_fields(&mut v, id);
        v
    }));
    Value::Array(all)
}

pub fn handle(app: &mut MailApp, ctx: &egui::Context, req: &ControlRequest) -> Outcome {
    if let Some(allow) = &req.agent
        && let Some(settled) = crate::agent_gate::screen(app, req, allow)
    {
        return settled;
    }
    let p = &req.params;
    let s = |k: &str| p.get(k).and_then(Value::as_str);
    let f = |k: &str| p.get(k).and_then(Value::as_f64).map(|v| v as f32).filter(|v| v.is_finite());
    match req.method.as_str() {
        "engine.execute" | "command" => {
            let Some(id) = s("command").or(s("id")) else { return err("missing `command`") };
            let params = p.get("params").cloned().filter(|v| !v.is_null()).unwrap_or(json!({}));
            match app.execute(id, params) {
                Ok(v) => ok(v),
                Err(e) => err(e),
            }
        }
        "engine.commands" => ok(commands(app)),
        "mailbox.inspect" => match app.session.run("file.info", &json!({})) {
            Ok(v) => ok(v),
            Err(e) => err(e),
        },
        "ui.inspect" => ok(inspect(app, ctx)),
        // Whether the mailbox is open (while it opens, this alone is answered: `opening`), and
        // the one being read in the background, if any.
        "app.status" => ok(json!({"mailbox": "open", "opening": crate::opening::ahead_inspect(app)})),
        "ui.parity" => ok(mailcraft_engine::catalog::parity(&app.session.registry)),
        // The desktop alerts the window showed (read only: nothing can ask for one).
        "ui.alerts" => ok(crate::alerts::inspect(app, p)),
        "ui.click" => {
            // By coordinates, or by what to click (`{"kind": "message", "id": 12}`).
            let pos = match (f("x"), f("y")) {
                (Some(x), Some(y)) => egui::pos2(x, y),
                _ => match (s("kind"), p.get("id").and_then(Value::as_u64)) {
                    (Some(kind), Some(id)) => match app.hit.rect(kind, id) {
                        Some(r) => r.center(),
                        None => return err(format!("no {kind} {id} on screen")),
                    },
                    _ => return err("missing x/y (or kind/id)"),
                },
            };
            let button = match s("button") {
                Some("right") => egui::PointerButton::Secondary,
                _ => egui::PointerButton::Primary,
            };
            let count = p.get("count").and_then(Value::as_u64).unwrap_or(1);
            click_events(app, pos, button, count, mods(p));
            ok(json!({"queued": true, "at": [pos.x, pos.y]}))
        }
        "ui.move" => {
            let (Some(x), Some(y)) = (f("x"), f("y")) else { return err("missing x/y") };
            app.synthetic.push(egui::Event::PointerMoved(egui::pos2(x, y)));
            ok(json!({"queued": true}))
        }
        "ui.drag" => {
            let (Some(x), Some(y), Some(tx), Some(ty)) = (f("x"), f("y"), f("toX"), f("toY")) else { return err("missing x/y/toX/toY") };
            let m = mods(p);
            let steps = p.get("steps").and_then(Value::as_u64).unwrap_or(8).clamp(1, 100);
            app.synthetic.push(egui::Event::PointerMoved(egui::pos2(x, y)));
            app.synthetic.push(egui::Event::PointerButton {
                pos: egui::pos2(x, y),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: m,
            });
            for i in 1..=steps {
                let k = i as f32 / steps as f32;
                app.synthetic.push(egui::Event::PointerMoved(egui::pos2(x + (tx - x) * k, y + (ty - y) * k)));
            }
            app.synthetic.push(egui::Event::PointerButton {
                pos: egui::pos2(tx, ty),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: m,
            });
            ok(json!({"queued": true}))
        }
        "ui.key" => {
            let Some(k) = s("key").and_then(key_from) else { return err("unknown key") };
            let m = mods(p);
            app.synthetic.push(egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m });
            app.synthetic.push(egui::Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers: m });
            ok(json!({"queued": true}))
        }
        "ui.text" => {
            let Some(t) = s("text") else { return err("missing text") };
            app.synthetic.push(egui::Event::Text(t.to_string()));
            ok(json!({"queued": true}))
        }
        // An agent's comes back in the answer: nothing is written.
        "ui.screenshot" => Outcome::Screenshot {
            path: s("path").map(str::to_string),
            inline: req.agent.is_some() || p.get("inline").and_then(Value::as_bool) == Some(true),
        },
        "ui.resize" => {
            let (Some(w), Some(h)) = (f("width"), f("height")) else { return err("missing width/height") };
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w.clamp(200.0, 10_000.0), h.clamp(200.0, 10_000.0))));
            ok(json!({}))
        }
        "ui.focus" => {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ok(json!({}))
        }
        "app.quit" => {
            app.quit_requested = true;
            ok(json!({}))
        }
        other => {
            // Anything else: a command id.
            match app.execute(other, p.clone()) {
                Ok(v) => ok(v),
                Err(e) => err(e),
            }
        }
    }
}

/// The window as PNG, base64 in the answer (`data`).
pub fn inline_screenshot(image: &egui::ColorImage) -> Value {
    let [w, h] = image.size;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    let Some(buffer) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) else {
        return json!({"ok": false, "error": "the screenshot had no pixels"});
    };
    let mut png = std::io::Cursor::new(Vec::new());
    if let Err(e) = buffer.write_to(&mut png, image::ImageFormat::Png) {
        return json!({"ok": false, "error": e.to_string()});
    }
    json!({"ok": true, "result": {"data": mailcraft_engine::cmd::base64_encode(png.get_ref()), "width": w, "height": h}})
}

pub fn save_screenshot(image: &egui::ColorImage, path: Option<&str>) -> Value {
    let [w, h] = image.size;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    let Some(buffer) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) else {
        return json!({"ok": false, "error": "the screenshot had no pixels"});
    };
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (buffer, path);
        json!({"ok": false, "error": "screenshots can't be saved from the browser"})
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = match path {
            Some(p) => std::path::PathBuf::from(p),
            None => std::env::temp_dir().join("mailcraft-screenshot.png"),
        };
        if let Err(e) = buffer.save_with_format(&path, image::ImageFormat::Png) {
            return json!({"ok": false, "error": format!("{}: {e}", path.display())});
        }
        json!({"ok": true, "result": {"path": path.to_string_lossy(), "width": w, "height": h}})
    }
}
