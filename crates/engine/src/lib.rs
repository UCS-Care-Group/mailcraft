//! MailCraft engine: the session and its commands.
//!
//! **Everything is a command.** Each user-visible action is a [`CommandSpec`] with a stable id
//! (`mail.reply`, `calendar.new`), a label, the ribbon/menu location, a default shortcut, a
//! params description and a `run` function. The ribbon, shortcuts, command search, the CLI, the
//! JSON control channel and the MCP server all call [`Session::run`].
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod catalog;
pub mod cmd;
pub mod io;
pub mod list;
pub mod sample;
pub mod security;
mod session;
pub mod spell;
#[cfg(not(target_arch = "wasm32"))]
pub mod store;

use std::collections::HashMap;

use serde_json::Value;

pub use mailcraft_html as html;
pub use mailcraft_model as model;
pub use mailcraft_print as print;
pub use mailcraft_time as time;
pub use session::{
    CalView, Clock, Compose, ComposeMode, Filter, Module, Pane, Part, Prefs, ReceiptPolicy, Scope, Selection, Session, SortBy, TaskFilter, ViewState,
};

/// What a message says instead of text that couldn't be read back from where the mailbox keeps
/// it (`store::UNAVAILABLE`; the same words in every build).
pub fn store_placeholder() -> &'static str {
    "[This message's text is unavailable: the file that keeps it is missing from the mailbox's content folder, or damaged.]"
}

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum CmdError {
    #[error("unknown command `{0}`")]
    Unknown(String),
    #[error("bad parameters: {0}")]
    Params(String),
    #[error("not available: {0}")]
    Disabled(String),
    #[error("{0}")]
    Failed(String),
}

impl From<mailcraft_model::ModelError> for CmdError {
    fn from(e: mailcraft_model::ModelError) -> Self {
        CmdError::Failed(e.to_string())
    }
}

pub type CmdResult = Result<Value, CmdError>;

/// A registered command.
#[derive(Clone)]
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Where it lives in the UI: `Home › Respond`, `Folder › New`, `File`…
    pub location: &'static str,
    /// Default shortcut in a portable form (`Mod+R`, `Mod+Shift+M`, `F9`). `Mod` = Ctrl, or ⌘ on macOS.
    pub shortcut: &'static str,
    /// Parameter description for agents (`{"id": number}`).
    pub params: &'static str,
    /// Changes the mailbox (so it is undoable and marks the mailbox as edited).
    pub mutates: bool,
    /// `None` = enabled; `Some(reason)` = disabled.
    pub enabled: fn(&Session) -> Option<&'static str>,
    /// `enabled` only asks for a selection: the command still runs for a caller that names what
    /// to act on (`id` / `ids` in its params), whatever is selected.
    pub selection: bool,
    pub run: fn(&mut Session, &Value) -> CmdResult,
}

fn always(_: &Session) -> Option<&'static str> {
    None
}

impl CommandSpec {
    pub const fn new(id: &'static str, label: &'static str, location: &'static str, run: fn(&mut Session, &Value) -> CmdResult) -> Self {
        CommandSpec { id, label, location, shortcut: "", params: "{}", mutates: true, enabled: always, selection: false, run }
    }
    pub const fn key(mut self, s: &'static str) -> Self {
        self.shortcut = s;
        self
    }
    pub const fn params(mut self, p: &'static str) -> Self {
        self.params = p;
        self
    }
    /// Doesn't change the mailbox (navigation, view, export, composing).
    pub const fn pure(mut self) -> Self {
        self.mutates = false;
        self
    }
    pub const fn when(mut self, f: fn(&Session) -> Option<&'static str>) -> Self {
        self.enabled = f;
        self
    }
    /// Enabled when something is selected (`f` says what is missing). Buttons grey out without
    /// a selection; scripts and agents may name the target in the params instead.
    pub const fn selection(mut self, f: fn(&Session) -> Option<&'static str>) -> Self {
        self.enabled = f;
        self.selection = true;
        self
    }
}

/// All commands by id.
pub struct Registry {
    specs: Vec<CommandSpec>,
    by_id: HashMap<&'static str, usize>,
}

impl Registry {
    pub fn new(specs: Vec<CommandSpec>) -> Self {
        let mut by_id = HashMap::new();
        for (i, s) in specs.iter().enumerate() {
            by_id.insert(s.id, i);
        }
        Registry { specs, by_id }
    }
    pub fn get(&self, id: &str) -> Option<&CommandSpec> {
        self.by_id.get(id).and_then(|i| self.specs.get(*i))
    }
    pub fn all(&self) -> &[CommandSpec] {
        &self.specs
    }
    /// Command list as JSON (for agents: `commands` / `list_commands`).
    pub fn describe(&self) -> Value {
        Value::Array(
            self.specs
                .iter()
                .map(|s| serde_json::json!({"id": s.id, "label": s.label, "location": s.location, "shortcut": s.shortcut, "params": s.params, "mutates": s.mutates}))
                .collect(),
        )
    }
    /// The command bound to a shortcut, if any.
    pub fn by_shortcut(&self, key: &str) -> Option<&CommandSpec> {
        self.specs.iter().find(|s| !s.shortcut.is_empty() && s.shortcut.split(" / ").any(|k| k.eq_ignore_ascii_case(key)))
    }
}

/// Parameter helpers.
pub mod p {
    use mailcraft_model::Id;
    use mailcraft_time::{Date, DateTime};
    use serde_json::Value;

    use crate::CmdError;

    /// Most ids one command takes (a hostile caller can't make a command walk forever).
    const MAX_IDS: usize = 100_000;

    pub fn str<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
        v.get(k).and_then(Value::as_str)
    }
    pub fn req_str<'a>(v: &'a Value, k: &str) -> Result<&'a str, CmdError> {
        str(v, k).ok_or_else(|| CmdError::Params(format!("`{k}` (string) is required")))
    }
    pub fn u64(v: &Value, k: &str) -> Option<u64> {
        v.get(k).and_then(|x| {
            x.as_u64().or_else(|| x.as_f64().filter(|f| *f >= 0.0 && f.is_finite() && f.fract() == 0.0 && *f < 9.1e15).map(|f| f as u64))
        })
    }
    pub fn i64(v: &Value, k: &str) -> Option<i64> {
        v.get(k).and_then(|x| x.as_i64().or_else(|| x.as_f64().filter(|f| f.is_finite() && f.abs() < 9.1e15).map(|f| f as i64)))
    }
    pub fn bool(v: &Value, k: &str) -> Option<bool> {
        v.get(k).and_then(Value::as_bool)
    }
    pub fn id(v: &Value, k: &str) -> Option<Id> {
        u64(v, k).filter(|i| *i != 0)
    }
    pub fn req_id(v: &Value, k: &str) -> Result<Id, CmdError> {
        id(v, k).ok_or_else(|| CmdError::Params(format!("`{k}` (id) is required")))
    }
    /// `ids` (an array) or `id` (one), when either is given.
    pub fn ids(v: &Value) -> Option<Vec<Id>> {
        if let Some(a) = v.get("ids").and_then(Value::as_array) {
            return Some(a.iter().take(MAX_IDS).filter_map(|x| x.as_u64()).filter(|i| *i != 0).collect());
        }
        id(v, "id").map(|i| vec![i])
    }
    /// A `YYYY-MM-DD` date.
    pub fn date(v: &Value, k: &str) -> Result<Option<Date>, CmdError> {
        match v.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
            Some(Value::String(s)) => Date::parse_iso(s).map(Some).ok_or_else(|| CmdError::Params(format!("`{k}` must be a date like 2026-10-11"))),
            Some(_) => Err(CmdError::Params(format!("`{k}` must be a date like 2026-10-11"))),
        }
    }
    /// A wall-clock time: `2026-10-11T09:30` (an offset, if any, is ignored).
    pub fn datetime(v: &Value, k: &str) -> Result<Option<DateTime>, CmdError> {
        let bad = || CmdError::Params(format!("`{k}` must be a date and time like 2026-10-11T09:30"));
        match v.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
            Some(Value::String(s)) => mailcraft_time::parse_iso(s).map(|(t, _)| Some(t)).ok_or_else(bad),
            Some(_) => Err(bad()),
        }
    }
    /// A list of strings from an array, or from one string split on commas.
    pub fn strings(v: &Value, k: &str) -> Option<Vec<String>> {
        match v.get(k)? {
            Value::Array(a) => Some(a.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).take(1000).collect()),
            Value::String(s) => Some(s.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).take(1000).collect()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod msg_tests;
#[cfg(test)]
mod security_tests;
#[cfg(test)]
mod tests;
