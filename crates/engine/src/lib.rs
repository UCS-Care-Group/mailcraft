// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft engine: the session and its commands.
//!
//! **Everything is a command.** Each user-visible action is a [`CommandSpec`] with a stable id
//! (`mail.reply`, `calendar.new`), a label, the ribbon/menu location, a default shortcut, a
//! params description and a `run` function. The ribbon, shortcuts, command search, the CLI, the
//! JSON control channel and the MCP server all call [`Session::run`].
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// A text with values (`mailcraft_i18n::Text`): `said!("no folder named {name}", name)`, or
/// `said!("{name} is larger than {mb} MB", name, mb = MAX >> 20)`. The template is a literal, so
/// `cargo xtask i18n` finds it; a value that is interface text itself is `mailcraft_i18n::Arg::words(…)`.
macro_rules! said {
    (@arg $k:ident) => { mailcraft_i18n::ToArg::to_arg(&$k) };
    (@arg $k:ident = $v:expr) => { mailcraft_i18n::ToArg::to_arg(&$v) };
    ($t:literal $(, $k:ident $(= $v:expr)?)* $(,)?) => {
        mailcraft_i18n::Text::new($t, vec![$((stringify!($k), said!(@arg $k $(= $v)?))),*])
    };
}

/// A command that failed, saying why with values in it ([`CmdError::Said`]); as [`said!`].
macro_rules! failed {
    ($($t:tt)*) => { $crate::CmdError::Said(said!($($t)*)) };
}

pub mod agent;
pub mod arrivals;
pub mod catalog;
pub mod cmd;
pub mod import_tree;
pub mod io;
#[cfg(test)]
mod launchable_tests;
pub mod list;
pub mod mark;
#[cfg(test)]
mod mark_tests;
pub mod opening;
pub mod sample;
mod session;
#[cfg(not(target_arch = "wasm32"))]
pub mod store;
#[cfg(not(target_arch = "wasm32"))]
pub mod store_before;
mod undo;
#[cfg(not(target_arch = "wasm32"))]
pub mod words;

use std::collections::HashMap;

use serde_json::Value;

pub use mailcraft_html as html;
pub use mailcraft_mime as mime;
pub use mailcraft_model as model;
pub use mailcraft_time as time;
pub use session::{
    BodyFormat, CalView, Clock, Compose, ComposeMode, Doc, Filter, InsertedSignature, Module, Pane, Part, PeopleSort, Prefs, Quote, Scope, Selection,
    Session, SignatureEdit, SortBy, Stays, TaskFilter, TaskSort, ViewState, WordIndex,
};

/// What a message says instead of text that couldn't be read back from where the mailbox keeps
/// it (`store::UNAVAILABLE`; the same words in every build).
pub fn store_placeholder() -> &'static str {
    "[This message's text is unavailable: the file that keeps it is missing from the mailbox's content folder, or damaged.]"
}

/// Why a command didn't run. Its `Display` is English (for the CLI, MCP and the control
/// channel); [`CmdError::in_lang`] is what the window says.
#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum CmdError {
    #[error("unknown command `{0}`")]
    Unknown(String),
    /// For whoever wrote the parameters (an agent, a script): English.
    #[error("bad parameters: {0}")]
    Params(String),
    #[error("not available: {0}")]
    Disabled(String),
    /// A fixed English text, looked up whole in the catalogue.
    #[error("{0}")]
    Failed(String),
    /// A text with values in it ([`failed!`]): its template is looked up, then filled.
    #[error("{0}")]
    Said(mailcraft_i18n::Text),
}

impl CmdError {
    /// What the window says of it in `lang`, without the "bad parameters:" / "not available:"
    /// that the English starts with for an agent.
    pub fn in_lang(&self, lang: mailcraft_i18n::Lang) -> String {
        match self {
            CmdError::Unknown(_) => self.to_string(),
            CmdError::Params(s) | CmdError::Disabled(s) | CmdError::Failed(s) => lang.t(s).to_string(),
            CmdError::Said(t) => t.in_lang(lang),
        }
    }
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
                .map(|s| {
                    let mut v = serde_json::json!({"id": s.id, "label": s.label, "location": s.location, "shortcut": s.shortcut, "params": s.params, "mutates": s.mutates});
                    agent_fields(&mut v, s.id);
                    v
                })
                .collect(),
        )
    }
    /// The command bound to a shortcut, if any.
    pub fn by_shortcut(&self, key: &str) -> Option<&CommandSpec> {
        self.specs.iter().find(|s| !s.shortcut.is_empty() && s.shortcut.split(" / ").any(|k| k.eq_ignore_ascii_case(key)))
    }
}

/// Add `agent` (`"safe"`, `"owner"`, `"never"`) and, for `"owner"`, `needs` (the `--allow`
/// class) to a command's description.
pub fn agent_fields(v: &mut Value, id: &str) {
    let a = agent::access(id).unwrap_or(agent::Access::Never);
    if let Some(o) = v.as_object_mut() {
        o.insert("agent".into(), Value::from(a.label()));
        if let agent::Access::Owner(c) = a {
            o.insert("needs".into(), Value::from(c.as_str()));
        }
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
mod agent_gate_tests;
#[cfg(test)]
mod agent_tests;
#[cfg(test)]
mod category_rows_tests;
#[cfg(test)]
mod focus_stays_tests;
#[cfg(test)]
mod import_tests;
#[cfg(test)]
mod import_tree_progress_tests;
#[cfg(test)]
mod import_tree_tests;
#[cfg(test)]
mod tests;
