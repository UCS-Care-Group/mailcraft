//! Undo and redo.

use serde_json::{Value, json};

use crate::{CmdResult, CommandSpec, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("edit.undo", "Undo", "Quick Access", undo)
            .key("Mod+Z")
            .when(|s| if s.can_undo() { None } else { Some(s.sealed_history().unwrap_or("nothing to undo")) }),
        CommandSpec::new("edit.redo", "Redo", "Quick Access", redo)
            .key("Mod+Y / Mod+Shift+Z")
            .when(|s| if s.can_redo() { None } else { Some("nothing to redo") }),
    ]
}

fn undo(s: &mut Session, _: &Value) -> CmdResult {
    let label = s.undo_label();
    s.undo();
    Ok(json!({"undone": label}))
}

fn redo(s: &mut Session, _: &Value) -> CmdResult {
    let label = s.redo_label();
    s.redo();
    Ok(json!({"redone": label}))
}
