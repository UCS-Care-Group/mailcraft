//! Quick Steps: named runs of actions on the selected messages, from the Home tab. Each action
//! is one of the mail commands (move, copy, mark, flag, categorize, delete, archive, reply,
//! forward), run in order as one undo step.

use mailcraft_model::{Id, MAX_SAVED, MAX_STEP_ACTIONS, QuickStep, StepAction, clean_name};
use serde_json::{Value, json};

use super::{bad, fail, needs_message, targets};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// What `actions` holds, for the params of the commands.
macro_rules! actions_doc {
    () => {
        r#"[{"type": "moveTo"|"copyTo", "value": folder id} | {"type": "markRead"|"markUnread"|"flag"|"clearFlag"|"markComplete"|"clearCategories"|"delete"|"archive"|"reply"|"replyAll"} | {"type": "categorize", "value": category} | {"type": "forward", "value"?: "a@example.org; b@example.org"}]"#
    };
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("quickSteps.new", "Create Quick Step", "Home › Quick Steps", new).params(concat!(
            r#"{"name": string, "actions": "#,
            actions_doc!(),
            r#" (at most 20; reply, reply all and forward start a message for the owner to finish, on the message in the reading pane)} — answers {"id"}"#
        )),
        CommandSpec::new("quickSteps.edit", "Edit Quick Step", "Home › Quick Steps", edit)
            .params(concat!(r#"{"id": number, "name"?: string, "actions"?: "#, actions_doc!(), "}"))
            .when(has_steps),
        CommandSpec::new("quickSteps.delete", "Delete Quick Step", "Home › Quick Steps", delete).params(r#"{"id": number}"#).when(has_steps),
        CommandSpec::new("quickSteps.reorder", "Move Quick Step", "Home › Quick Steps", reorder)
            .params(r#"{"id": number, "index": number (its place in the list, from 0)}"#)
            .when(has_steps),
        CommandSpec::new("quickSteps.manage", "Manage Quick Steps", "Home › Quick Steps", manage)
            .pure()
            .params(r#"{} — answers {"quickSteps": [{"id", "name", "text", "actions"}]}"#),
        CommandSpec::new("quickSteps.run", "Run Quick Step", "Home › Quick Steps", run)
            .params(r#"{"step": number (the Quick Step), "ids"?: [number] (default: the selected messages)} — one undo step; answers {"ran": number of actions, "composing": bool}"#)
            .selection(needs_message),
        CommandSpec::new("mail.replyAndDelete", "Reply and Delete", "Home › Quick Steps", reply_and_delete)
            .params(r#"{"id"?: number} (default: the selected message) — a reply starts, and the message goes to Deleted Items"#)
            .selection(needs_message),
    ]
}

fn has_steps(s: &Session) -> Option<&'static str> {
    if s.mailbox.quick_steps.is_empty() { Some("there are no Quick Steps") } else { None }
}

fn action(s: &Session, v: &Value) -> Result<StepAction, CmdError> {
    let folder = |v: &Value| -> Result<Id, CmdError> {
        let id = p::req_id(v, "value")?;
        s.mailbox.folder(id).map(|f| f.id).ok_or_else(|| fail(format!("no folder with id {id}")))
    };
    Ok(match p::req_str(v, "type")? {
        "moveTo" => StepAction::MoveTo(folder(v)?),
        "copyTo" => StepAction::CopyTo(folder(v)?),
        "markRead" => StepAction::MarkRead,
        "markUnread" => StepAction::MarkUnread,
        "flag" => StepAction::Flag,
        "clearFlag" => StepAction::ClearFlag,
        "markComplete" => StepAction::MarkComplete,
        "categorize" => StepAction::Categorize(clean_name(p::str(v, "value").unwrap_or("")).ok_or_else(|| bad("categorize needs a category name"))?),
        "clearCategories" => StepAction::ClearCategories,
        "delete" => StepAction::Delete,
        "archive" => StepAction::Archive,
        "reply" => StepAction::Reply,
        "replyAll" => StepAction::ReplyAll,
        "forward" => StepAction::Forward(p::str(v, "value").unwrap_or("").replace(['\r', '\n', '\0'], " ").trim().chars().take(2_000).collect()),
        other => return Err(bad(format!("unknown action type \"{other}\""))),
    })
}

fn actions(s: &Session, v: &Value) -> Result<Vec<StepAction>, CmdError> {
    let list = v.get("actions").and_then(Value::as_array).ok_or_else(|| bad("`actions` (a list) is required"))?;
    if list.len() > MAX_STEP_ACTIONS {
        return Err(bad(format!("a Quick Step does at most {MAX_STEP_ACTIONS} things")));
    }
    let out = list.iter().map(|a| action(s, a)).collect::<Result<Vec<_>, _>>()?;
    if out.is_empty() {
        return Err(bad("a Quick Step needs at least one action"));
    }
    Ok(out)
}

fn new(s: &mut Session, v: &Value) -> CmdResult {
    let name = clean_name(p::req_str(v, "name")?).ok_or_else(|| bad("`name` is empty"))?;
    let actions = actions(s, v)?;
    if s.mailbox.quick_steps.len() >= MAX_SAVED {
        return Err(fail("there are too many Quick Steps"));
    }
    let id = s.mailbox.alloc_id();
    s.mailbox.quick_steps.push(QuickStep { id, name, actions });
    Ok(json!({"id": id}))
}

fn position(s: &Session, v: &Value) -> Result<usize, CmdError> {
    let id = p::req_id(v, "id")?;
    s.mailbox.quick_steps.iter().position(|q| q.id == id).ok_or_else(|| fail(format!("no Quick Step with id {id}")))
}

fn edit(s: &mut Session, v: &Value) -> CmdResult {
    let at = position(s, v)?;
    let name = match p::str(v, "name") {
        Some(n) => Some(clean_name(n).ok_or_else(|| bad("`name` is empty"))?),
        None => None,
    };
    let actions = if v.get("actions").is_some() { Some(actions(s, v)?) } else { None };
    let q = s.mailbox.quick_steps.get_mut(at).ok_or_else(|| fail("the Quick Step is gone"))?;
    if let Some(n) = name {
        q.name = n;
    }
    if let Some(a) = actions {
        q.actions = a;
    }
    Ok(json!({"id": q.id, "name": q.name, "text": q.describe()}))
}

fn delete(s: &mut Session, v: &Value) -> CmdResult {
    let at = position(s, v)?;
    let gone = s.mailbox.quick_steps.remove(at);
    Ok(json!({"deleted": gone.id}))
}

fn reorder(s: &mut Session, v: &Value) -> CmdResult {
    let at = position(s, v)?;
    let index = p::u64(v, "index").ok_or_else(|| bad("`index` (number) is required"))?;
    let step = s.mailbox.quick_steps.remove(at);
    let to = usize::try_from(index).unwrap_or(usize::MAX).min(s.mailbox.quick_steps.len());
    s.mailbox.quick_steps.insert(to, step);
    Ok(json!({"index": to}))
}

fn manage(s: &mut Session, _: &Value) -> CmdResult {
    let steps: Vec<Value> =
        s.mailbox.quick_steps.iter().map(|q| json!({"id": q.id, "name": q.name, "text": q.describe(), "actions": q.actions})).collect();
    Ok(json!({"quickSteps": steps}))
}

/// Run one command of the registry inside this one (so that the whole is one undo step).
fn call(s: &mut Session, id: &str, params: &Value) -> Result<Value, CmdError> {
    let registry = s.registry.clone();
    let spec = registry.get(id).ok_or_else(|| CmdError::Unknown(id.to_string()))?;
    (spec.run)(s, params)
}

fn run(s: &mut Session, v: &Value) -> CmdResult {
    let step_id = p::req_id(v, "step")?;
    let at = s.mailbox.quick_steps.iter().position(|q| q.id == step_id).ok_or_else(|| fail(format!("no Quick Step with id {step_id}")))?;
    let step = s.mailbox.quick_steps.get(at).cloned().ok_or_else(|| fail("the Quick Step is gone"))?;
    let ids = targets(s, v)?;
    // A reply or forward is written to the message in the reading pane (one message is written
    // at a time).
    let focus = s.sel.focus().filter(|f| ids.contains(f)).or_else(|| ids.last().copied()).unwrap_or(0);
    let mut composing = false;
    for a in &step.actions {
        let all = json!({"ids": ids});
        match a {
            StepAction::MoveTo(folder) => call(s, "mail.move", &json!({"ids": ids, "folder": folder}))?,
            StepAction::CopyTo(folder) => call(s, "mail.copy", &json!({"ids": ids, "folder": folder}))?,
            StepAction::MarkRead => call(s, "mail.markRead", &all)?,
            StepAction::MarkUnread => call(s, "mail.markUnread", &all)?,
            StepAction::Flag => call(s, "mail.flag", &all)?,
            StepAction::ClearFlag => call(s, "mail.clearFlag", &all)?,
            StepAction::MarkComplete => call(s, "mail.complete", &all)?,
            StepAction::Categorize(c) => call(s, "mail.categorize", &json!({"ids": ids, "category": c, "value": true}))?,
            StepAction::ClearCategories => call(s, "mail.clearCategories", &all)?,
            StepAction::Delete => call(s, "mail.delete", &all)?,
            StepAction::Archive => call(s, "mail.archive", &all)?,
            StepAction::Reply => call(s, "mail.reply", &json!({"id": focus}))?,
            StepAction::ReplyAll => call(s, "mail.replyAll", &json!({"id": focus}))?,
            StepAction::Forward(to) => {
                call(s, "mail.forward", &json!({"id": focus}))?;
                if !to.trim().is_empty() {
                    call(s, "compose.set", &json!({"to": to}))?;
                }
                Value::Null
            }
        };
        composing |= a.writes();
    }
    Ok(json!({"ran": step.actions.len(), "composing": composing && s.compose.is_some()}))
}

fn reply_and_delete(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    call(s, "mail.reply", &json!({"id": id}))?;
    call(s, "mail.delete", &json!({"ids": [id]}))?;
    Ok(json!({"composing": true, "deleted": id}))
}

#[cfg(test)]
mod tests {
    use mailcraft_model::{FlagStatus, FolderKind};
    use serde_json::json;

    use crate::{Session, sample};

    /// The sample, without its own Quick Steps.
    fn session() -> Session {
        let now = 1_791_993_600;
        let mut s = Session::at(sample::mailbox(now, 0), now, 0);
        s.mailbox.quick_steps.clear();
        s
    }

    #[test]
    fn the_samples_done_step_files_the_message() {
        let now = 1_791_993_600;
        let mut s = Session::at(sample::mailbox(now, 0), now, 0);
        let done = s.mailbox.quick_steps.iter().find(|q| q.name == "Done").map(|q| q.id).unwrap();
        let target = s.rows()[0].id;
        s.run("quickSteps.run", &json!({"step": done, "ids": [target]})).unwrap();
        let m = s.mailbox.message(target).unwrap();
        assert_eq!(s.mailbox.folder(m.folder).unwrap().kind, FolderKind::Archive);
        assert!(m.read && m.flag == FlagStatus::Complete);
    }

    #[test]
    fn a_quick_step_runs_its_actions_in_order_as_one_undo_step() {
        let mut s = session();
        let account = s.mailbox.accounts[0].id;
        let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
        let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
        let id = s
            .run(
                "quickSteps.new",
                &json!({"name": "Done", "actions": [{"type": "markRead"}, {"type": "flag"}, {"type": "markComplete"}, {"type": "categorize", "value": "Green category"}, {"type": "moveTo", "value": archive}]}),
            )
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let listed = s.run("quickSteps.manage", &json!({})).unwrap();
        assert_eq!(listed["quickSteps"][0]["text"], "Mark as read, flag, mark complete, categorize as \"Green category\", move to a folder");
        let target = s.mailbox.messages_in(inbox).find(|m| !m.read).map(|m| m.id).unwrap();
        s.run("mail.select", &json!({"id": target})).unwrap();
        // (Selecting it marked it read: undo that so the step does it.)
        s.mailbox.message_mut(target).unwrap().read = false;
        let before = s.mailbox.clone();
        let ran = s.run("quickSteps.run", &json!({"step": id, "ids": [target]})).unwrap();
        assert_eq!(ran["ran"], 5);
        let m = s.mailbox.message(target).unwrap();
        assert!(m.read && m.flag == FlagStatus::Complete && m.folder == archive && m.has_category("Green category"));
        assert_eq!(s.undo_label(), Some("Run Quick Step"));
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(s.mailbox, before, "one undo puts everything back");
    }

    #[test]
    fn a_step_that_fails_half_way_changes_nothing() {
        let mut s = session();
        let account = s.mailbox.accounts[0].id;
        let folder = s.mailbox.add_folder(account, None, "Projects").unwrap();
        let id = s
            .run("quickSteps.new", &json!({"name": "File it", "actions": [{"type": "markRead"}, {"type": "moveTo", "value": folder}]}))
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        // The folder goes after the step was made.
        s.mailbox.forget_folder(folder).unwrap();
        let target = *s.mailbox.messages.keys().next().unwrap();
        s.mailbox.message_mut(target).unwrap().read = false;
        let before = s.mailbox.clone();
        assert!(s.run("quickSteps.run", &json!({"step": id, "ids": [target]})).is_err());
        assert_eq!(s.mailbox, before);
    }

    #[test]
    fn writing_steps_open_a_message_and_reply_and_delete_does_both() {
        let mut s = session();
        let target = s.rows()[0].id;
        s.run("mail.select", &json!({"id": target})).unwrap();
        let id = s
            .run(
                "quickSteps.new",
                &json!({"name": "To the team", "actions": [{"type": "forward", "value": "team@example.org\r\nBcc: x@example.org"}]}),
            )
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let ran = s.run("quickSteps.run", &json!({"step": id})).unwrap();
        assert_eq!(ran["composing"], true);
        let c = s.compose.clone().unwrap();
        assert!(c.subject.starts_with("FW:") && c.to.contains("team@example.org") && !c.to.contains('\n'));
        s.compose = None;
        let deleted = s.mailbox.folder_of_kind(s.mailbox.message(target).unwrap().account, FolderKind::Deleted).unwrap();
        s.run("mail.replyAndDelete", &json!({"id": target})).unwrap();
        assert!(s.compose.as_ref().is_some_and(|c| c.subject.starts_with("RE:")));
        assert_eq!(s.mailbox.message(target).unwrap().folder, deleted);
    }

    #[test]
    fn steps_are_edited_reordered_and_deleted_and_refuse_junk() {
        let mut s = session();
        let a = s.run("quickSteps.new", &json!({"name": "A", "actions": [{"type": "markRead"}]})).unwrap()["id"].as_u64().unwrap();
        let b = s.run("quickSteps.new", &json!({"name": "B", "actions": [{"type": "delete"}]})).unwrap()["id"].as_u64().unwrap();
        s.run("quickSteps.reorder", &json!({"id": b, "index": 0})).unwrap();
        assert_eq!(s.mailbox.quick_steps.iter().map(|q| q.id).collect::<Vec<_>>(), [b, a]);
        s.run("quickSteps.reorder", &json!({"id": b, "index": 1e15})).unwrap();
        assert_eq!(s.mailbox.quick_steps.iter().map(|q| q.id).collect::<Vec<_>>(), [a, b]);
        s.run("quickSteps.edit", &json!({"id": a, "name": "Read it", "actions": [{"type": "markUnread"}]})).unwrap();
        assert_eq!(s.mailbox.quick_steps[0].name, "Read it");
        for junk in [
            json!({"name": "x", "actions": []}),
            json!({"name": "", "actions": [{"type": "flag"}]}),
            json!({"name": "x", "actions": [{"type": "moveTo", "value": 999_999}]}),
            json!({"name": "x", "actions": [{"type": "categorize", "value": "\u{0}"}]}),
            json!({"name": "x", "actions": [{"type": "runCommand", "value": "file.open"}]}),
            json!({"name": "x", "actions": vec![json!({"type": "flag"}); 21]}),
        ] {
            assert!(s.run("quickSteps.new", &junk).is_err(), "{junk}");
        }
        s.run("quickSteps.delete", &json!({"id": a})).unwrap();
        assert_eq!(s.mailbox.quick_steps.len(), 1);
        assert!(s.run("quickSteps.delete", &json!({"id": a})).is_err());
    }
}
