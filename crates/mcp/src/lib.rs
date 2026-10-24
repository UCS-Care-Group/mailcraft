// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft's MCP server (Model Context Protocol, JSON-RPC 2.0 over stdio).
//!
//! Two backends: [`Headless`] runs an in-process session on a mailbox (no window);
//! [`backend::Remote`] talks to a running MailCraft app through its control channel
//! (`mailcraft --control PORT`), so agents can also click, type and screenshot the real UI.
//!
//! Agents get only what the owner allows (`mailcraft_engine::agent`): every command has a class,
//! no file is named by an agent unless the owner gave a folder for it ([`files`]), and what needs
//! the owner is refused headless (unless allowed with `--allow`) or asked about in the window.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod backend;
pub mod files;
pub mod server;
pub mod tools;

pub use backend::{Backend, Remote};
pub use mailcraft_engine::agent::Class;
pub use server::Server;

use mailcraft_engine::Session;
use mailcraft_engine::agent::{self, Verdict};
use serde_json::{Value, json};

/// Methods that need the desktop app.
pub const NEEDS_APP: &str = "needs the desktop app: start `mailcraft --control 7991` and run `mailcraft-cli mcp --connect 127.0.0.1:7991`";

/// An in-process session, driven by an agent: every command goes through the gate, and what
/// needs the owner runs only when it is in `allow` (`--allow`).
pub struct Headless {
    pub session: Session,
    pub allow: Vec<Class>,
}

impl Default for Headless {
    fn default() -> Self {
        Headless { session: Session::empty(), allow: Vec::new() }
    }
}

impl Headless {
    pub fn new(session: Session, allow: Vec<Class>) -> Self {
        Headless { session, allow }
    }

    fn run(&mut self, id: &str, params: &Value) -> Result<Value, String> {
        match agent::check(&self.session, id, params, &self.allow) {
            Verdict::Run => {
                self.session.agent = Some(self.allow.clone());
                let r = self.session.run(id, params).map_err(|e| e.to_string());
                self.session.agent = None;
                r
            }
            Verdict::Ask(class, what) => Err(agent::needs_owner(id, class, &what)),
            Verdict::Refuse(why) => Err(why),
        }
    }
}

impl Backend for Headless {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "engine.execute" | "command" => {
                let id = params.get("command").and_then(Value::as_str).ok_or("missing `command`")?.to_string();
                let p = params.get("params").cloned().filter(|v| !v.is_null()).unwrap_or(json!({}));
                self.run(&id, &p)
            }
            "engine.commands" => Ok(self.session.registry.describe()),
            "mailbox.inspect" => self.session.run("file.info", &json!({})).map_err(|e| e.to_string()),
            "ui.parity" => Ok(mailcraft_engine::catalog::parity(&self.session.registry)),
            m if m.starts_with("ui.") || m.starts_with("app.") => Err(format!("`{m}` {NEEDS_APP}")),
            other => self.run(other, &params),
        }
    }
    fn has_ui(&self) -> bool {
        false
    }
    fn describe(&self) -> String {
        "headless session (no window)".into()
    }
}

#[cfg(test)]
mod files_tests;
#[cfg(test)]
mod gate_tests;
#[cfg(test)]
mod tests;
