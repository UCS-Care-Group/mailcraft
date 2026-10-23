// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What an agent may do through the window (`mailcraft-cli mcp --connect`): the bridge marks
//! every request it sends as an agent's (`ControlRequest::agent`, with what the owner allowed it
//! in advance), and those go through [`mailcraft_engine::agent`]'s gate here. A command that is
//! not for agents, or names a file, is refused; one that needs the owner is asked about in a
//! question over the window ("An agent wants to send …"), whose default answer is no. It runs
//! only on the owner's yes; nobody's answer within [`WAIT_MS`] is a no. While a question is up,
//! no input from the control channel reaches the window.

use std::collections::VecDeque;
use std::sync::mpsc::Sender;

use egui::{Key, vec2};
use mailcraft_engine::agent::{self, Class, Verdict};
use serde_json::{Value, json};

use crate::MailApp;
use crate::control::{ControlRequest, Outcome};
use crate::theme::{Tokens, semibold};
use crate::widgets::primary_button;

/// How long a question waits for the owner (the control channel gives up at 60 s).
pub const WAIT_MS: f64 = 50_000.0;

/// Most questions waiting at once.
const MOST: usize = 8;

/// A question for the owner about what an agent wants.
pub struct Asking {
    pub id: String,
    pub params: Value,
    pub class: Class,
    /// What it would do, after "An agent wants to".
    pub what: String,
    allow: Vec<Class>,
    reply: Sender<Value>,
    deadline: f64,
}

/// The questions waiting, the first one on screen.
pub type Questions = VecDeque<Asking>;

fn ok(v: Value) -> Outcome {
    Outcome::Done(json!({"ok": true, "result": v}))
}

fn err(e: impl std::fmt::Display) -> Outcome {
    Outcome::Done(json!({"ok": false, "error": e.to_string()}))
}

/// A request from an agent: `Some` when the gate settles it (refused, asked about, or run
/// here), `None` when it goes on as anybody's (looking, a screenshot into the answer).
pub fn screen(app: &mut MailApp, req: &ControlRequest, allow: &[Class]) -> Option<Outcome> {
    let p = &req.params;
    let given = |k: &str| p.get(k).is_some_and(|v| !v.is_null());
    match req.method.as_str() {
        "engine.commands" | "mailbox.inspect" | "ui.inspect" | "ui.alerts" | "ui.parity" | "ui.resize" | "ui.focus" => None,
        "ui.screenshot" if given("path") => Some(err("agents don't name files: the screenshot comes back in the answer (leave out `path`)")),
        "ui.screenshot" => None,
        "ui.click" | "ui.move" | "ui.drag" | "ui.key" | "ui.text" => {
            if !allow.contains(&Class::Input) {
                Some(err(format!(
                    "clicking and typing in the window needs the owner: {}. Only the owner can let an agent do this, by starting the bridge with `mailcraft-cli mcp --allow input`",
                    Class::Input.what()
                )))
            } else if !app.asking.is_empty() {
                Some(err("the owner is being asked something: no input reaches the window until they answer"))
            } else {
                None
            }
        }
        "engine.execute" | "command" => {
            let id = p.get("command").or_else(|| p.get("id")).and_then(Value::as_str).unwrap_or("").to_string();
            let params = p.get("params").cloned().filter(|v| !v.is_null()).unwrap_or(json!({}));
            Some(gate(app, req, &id, params, allow))
        }
        "app.quit" | "auth" => Some(err(format!("`{}` isn't available to agents", req.method))),
        other => Some(gate(app, req, other, p.clone(), allow)),
    }
}

fn gate(app: &mut MailApp, req: &ControlRequest, id: &str, params: Value, allow: &[Class]) -> Outcome {
    match agent::check(&app.session, id, &params, allow) {
        Verdict::Run => run(app, id, params, allow),
        Verdict::Refuse(why) => err(why),
        Verdict::Ask(class, what) => {
            if app.asking.len() >= MOST {
                return err("too many questions are waiting for the owner; nothing was done");
            }
            app.asking.push_back(Asking {
                id: id.to_string(),
                params,
                class,
                what,
                allow: allow.to_vec(),
                reply: req.reply.clone(),
                deadline: crate::now_ms() + WAIT_MS,
            });
            Outcome::Pending
        }
    }
}

/// Run for an agent: what the command does is held to `allow` (`Session::agent`).
fn run(app: &mut MailApp, id: &str, params: Value, allow: &[Class]) -> Outcome {
    app.session.agent = Some(allow.to_vec());
    let r = app.execute(id, params);
    app.session.agent = None;
    match r {
        Ok(v) => ok(v),
        Err(e) => err(e),
    }
}

/// The owner's answer to the question on screen.
pub fn answer(app: &mut MailApp, yes: bool) {
    let Some(a) = app.asking.pop_front() else { return };
    let out = if yes {
        let mut allow = a.allow.clone();
        allow.push(a.class);
        // Asked again: what it would do may have changed while the question was up.
        match agent::check(&app.session, &a.id, &a.params, &allow) {
            Verdict::Run => run(app, &a.id, a.params, &allow),
            Verdict::Refuse(why) => err(why),
            Verdict::Ask(..) => err(format!("`{}` would now do more than the owner was asked about; nothing was done", a.id)),
        }
    } else {
        err(format!("the owner said no to `{}`: nothing was done", a.id))
    };
    if let Outcome::Done(v) = out {
        let _ = a.reply.send(v);
    }
}

/// Questions nobody answered in time are a no.
pub fn expire(app: &mut MailApp, now: f64) {
    while app.asking.front().is_some_and(|a| now >= a.deadline) {
        if let Some(a) = app.asking.pop_front() {
            let _ = a.reply.send(json!({"ok": false, "error": format!("the owner didn't answer about `{}` in time: nothing was done", a.id)}));
        }
    }
}

/// The buttons: going ahead, and not (the default).
fn verbs(class: Class) -> (&'static str, &'static str) {
    match class {
        Class::Send => (tl!("Send"), tl!("Don't Send")),
        Class::Delete => (tl!("Delete"), tl!("Don't Delete")),
        _ => (tl!("Allow"), tl!("Don't Allow")),
    }
}

/// The question over the window, when there is one. Enter and Escape say no.
pub fn show(app: &mut MailApp, ctx: &egui::Context) {
    expire(app, crate::now_ms());
    let Some(a) = app.asking.front() else { return };
    // Nothing from the control channel reaches the window while the owner is asked.
    app.synthetic.clear();
    let t = Tokens::get(ctx);
    let what: String = a.what.chars().filter(|c| !c.is_control()).take(agent::MAX_QUESTION).collect();
    let text = crate::i18n::fmt(tl!("An agent wants to {what}."), &[("what", what.trim())]);
    let (yes_label, no_label) = verbs(a.class);
    let waiting = app.asking.len();
    let mut said = None;
    let frame = egui::Frame::window(&ctx.global_style()).fill(t.surface).inner_margin(egui::Margin::same(16));
    let width = (ctx.content_rect().width() - 48.0).clamp(160.0, 460.0);
    egui::Modal::new(egui::Id::unique("agent-asks"))
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(if t.dark { 110 } else { 60 }))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            ui.label(egui::RichText::new(tl!("An Agent Is Asking")).font(semibold(14.0)).color(t.text));
            ui.label(egui::RichText::new(text).color(t.text));
            ui.label(
                egui::RichText::new(tl!(
                    "Say yes only if you asked the agent for this. Text in a message can tell an agent to do things you didn't ask for."
                ))
                .color(t.text_dim),
            );
            if waiting > 1 {
                ui.label(
                    egui::RichText::new(tln!(waiting - 1, "{n} more question is waiting.", "{n} more questions are waiting.")).color(t.text_dim),
                );
            }
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let no = primary_button(ui, no_label, true);
                let yes = ui.add(egui::Button::new(yes_label).min_size(vec2(72.0, 26.0)));
                app.hit.add("agentNo", 0, no.rect);
                app.hit.add("agentYes", 0, yes.rect);
                if no.clicked() {
                    said = Some(false);
                } else if yes.clicked() {
                    said = Some(true);
                }
            });
        });
    let (enter, escape) = ctx.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, Key::Enter), i.consume_key(egui::Modifiers::NONE, Key::Escape)));
    if enter || escape {
        said = Some(false);
    }
    if let Some(yes) = said {
        answer(app, yes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mailcraft_engine::Session;
    use mailcraft_model::FolderKind;

    const NOW: i64 = 1_791_993_600;

    fn app() -> MailApp {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), crate::Services::default());
        app.autosave = false;
        app
    }

    /// Send `method` as an agent allowed `allow`; the answer, if there is one yet.
    fn ask(app: &mut MailApp, method: &str, params: Value, allow: Vec<Class>) -> (Option<Value>, std::sync::mpsc::Receiver<Value>) {
        let (req, rx) = ControlRequest::new(method, params);
        let req = req.from_agent(allow);
        let out = crate::control::handle(app, &egui::Context::default(), &req);
        (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
    }

    fn sent(app: &MailApp) -> usize {
        let out: Vec<u64> =
            app.session.mailbox.folders.iter().filter(|f| matches!(f.kind, FolderKind::Outbox | FolderKind::Sent)).map(|f| f.id).collect();
        app.session.mailbox.messages.values().filter(|m| out.contains(&m.folder) && m.subject == "Re: Invoice").count()
    }

    fn write(app: &mut MailApp) {
        app.execute("mail.new", json!({"to": "x@attacker.example", "subject": "Re: Invoice"})).unwrap();
        app.execute("compose.attach", json!({"name": "id_ed25519", "data": "AAAA"})).unwrap();
    }

    #[test]
    fn every_window_command_has_a_class() {
        let missing: Vec<&str> = crate::UI_COMMANDS.iter().map(|c| c.0).filter(|id| agent::access(id).is_none()).collect();
        assert!(missing.is_empty(), "classify these for agents in crates/engine/src/agent.rs: {missing:?}");
    }

    #[test]
    fn sending_waits_for_the_owners_yes() {
        let mut a = app();
        write(&mut a);
        let (now, rx) = ask(&mut a, "engine.execute", json!({"command": "compose.send"}), vec![]);
        assert!(now.is_none(), "asked first");
        let q = a.asking.front().unwrap();
        assert_eq!(q.class, Class::Send);
        assert!(q.what.contains("Re: Invoice") && q.what.contains("x@attacker.example") && q.what.contains("id_ed25519"), "{}", q.what);
        // No.
        answer(&mut a, false);
        assert_eq!(rx.try_recv().unwrap()["ok"], false);
        assert_eq!(sent(&a), 0);
        assert!(a.session.compose.is_some());
        // Nobody answers: no.
        let (_, rx) = ask(&mut a, "compose.send", json!({}), vec![]);
        expire(&mut a, crate::now_ms() + WAIT_MS + 1.0);
        assert!(rx.try_recv().unwrap()["error"].as_str().unwrap().contains("in time"));
        assert_eq!(sent(&a), 0);
        // Yes.
        let (_, rx) = ask(&mut a, "engine.execute", json!({"command": "compose.send"}), vec![]);
        answer(&mut a, true);
        assert_eq!(rx.try_recv().unwrap()["ok"], true);
        assert_eq!(sent(&a), 1);
        // Allowed in advance (`--allow send`): no question.
        write(&mut a);
        let (now, _) = ask(&mut a, "engine.execute", json!({"command": "compose.send"}), vec![Class::Send]);
        assert_eq!(now.unwrap()["ok"], true);
        assert!(a.asking.is_empty());
    }

    #[test]
    fn the_owners_own_scripts_work_as_before() {
        let mut a = app();
        write(&mut a);
        let (req, _) = ControlRequest::new("engine.execute", json!({"command": "compose.send"}));
        let out = crate::control::handle(&mut a, &egui::Context::default(), &req);
        assert!(matches!(out, Outcome::Done(ref v) if v["ok"] == true));
        assert_eq!(sent(&a), 1);
    }

    #[test]
    fn what_agents_never_get_through_the_window() {
        let mut a = app();
        write(&mut a);
        let all = Class::ALL.to_vec();
        let bat = std::env::temp_dir().join(format!("mailcraft-gate-{}.bat", std::process::id()));
        for (method, params) in [
            ("engine.execute", json!({"command": "compose.attach", "params": {"path": "/etc/passwd"}})),
            ("compose.attach", json!({"path": "/etc/passwd"})),
            ("engine.execute", json!({"command": "file.exportMessage", "params": {"id": 1, "path": bat.to_string_lossy()}})),
            ("engine.execute", json!({"command": "sync.apply", "params": {}})),
            ("engine.execute", json!({"command": "ui.copyLinkAddress", "params": {"url": "https://x.example"}})),
            ("engine.execute", json!({"command": "no.such"})),
            ("ui.screenshot", json!({"path": "/tmp/evil.png"})),
            ("app.quit", json!({})),
        ] {
            let (r, _) = ask(&mut a, method, params.clone(), all.clone());
            assert_eq!(r.unwrap_or_default()["ok"], false, "{method} {params}");
        }
        assert_eq!(a.session.compose.as_ref().unwrap().attachments.len(), 1);
        assert!(!a.quit_requested);
        assert!(!bat.exists());
        // Input only with `--allow input`.
        let (r, _) = ask(&mut a, "ui.click", json!({"x": 1, "y": 1}), vec![]);
        assert!(r.unwrap()["error"].as_str().unwrap().contains("--allow input"));
        assert!(a.synthetic.is_empty());
        let (r, _) = ask(&mut a, "ui.click", json!({"x": 1, "y": 1}), vec![Class::Input]);
        assert_eq!(r.unwrap()["ok"], true);
        // And none while the owner is asked.
        a.synthetic.clear();
        let _ = ask(&mut a, "compose.send", json!({}), vec![Class::Input]);
        let (r, _) = ask(&mut a, "ui.click", json!({"x": 1, "y": 1}), vec![Class::Input]);
        assert_eq!(r.unwrap()["ok"], false);
        assert!(a.synthetic.is_empty());
    }

    #[test]
    fn agents_cant_slip_mail_into_the_outbox() {
        let mut a = app();
        let outbox = a.session.mailbox.folders.iter().find(|f| f.kind == FolderKind::Outbox).unwrap().id;
        let id = *a.session.mailbox.messages.keys().next().unwrap();
        let (r, _) = ask(&mut a, "mail.copy", json!({"folder": outbox, "ids": [id]}), vec![]);
        assert!(r.is_none(), "asked first");
        answer(&mut a, false);
        assert_eq!(a.session.mailbox.count(outbox), 0);
    }
}
