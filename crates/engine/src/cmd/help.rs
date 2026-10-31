// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Help and the account page: where MailCraft's own pages are, and what this copy is.
//!
//! The engine opens nothing: each help command answers the address of a page on the project's
//! own repository, and the window opens it in the system's browser when the owner pressed the
//! button. The addresses carry nothing of the owner: no account, no address, no message.

use serde_json::{Value, json};

use crate::{CmdResult, CommandSpec, Session};

/// The project's repository, as the build names it (`https://github.com/…/mailcraft`).
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
/// This copy's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("help.help", "Help", "Help › Help", help).pure().params(
            r#"{} — answers {"url"}: MailCraft's own help pages (the project's read-me). The window opens it in the system's browser; nothing about the owner is in the address"#,
        ),
        CommandSpec::new("help.support", "Get Support", "Help › Help", support).pure().params(
            r#"{} — answers {"url"}: the project's list of issues, where questions are asked and answered in public. The window opens it in the system's browser; nothing about the owner is in the address"#,
        ),
        CommandSpec::new("help.feedback", "Feedback", "Help › Help", feedback).pure().params(
            r#"{} — answers {"url", "dialog": "reportProblem"}: the project's page for a new issue. The window opens its Report a Problem window instead, which says what goes with a report before anything is opened or sent"#,
        ),
        CommandSpec::new("help.whatsNew", "What's New", "Help › Help", whats_new).pure().params(
            r#"{} — answers {"url", "version"}: the release notes of this version, on the project's releases page. The window opens it in the system's browser"#,
        ),
        CommandSpec::new("file.account", "Account", "File", account).pure().params(
            r#"{} — answers {"version", "accounts": [{"id", "name", "email", "kind"}], "offline": bool}: this copy of MailCraft and the accounts it has. The window shows File › About"#,
        ),
    ]
}

/// `REPOSITORY` followed by `path`, however the build wrote it.
fn page(path: &str) -> String {
    format!("{}/{path}", REPOSITORY.trim_end_matches('/'))
}

fn help(_: &mut Session, _: &Value) -> CmdResult {
    Ok(json!({"url": page("#readme")}))
}

fn support(_: &mut Session, _: &Value) -> CmdResult {
    Ok(json!({"url": page("issues")}))
}

fn feedback(_: &mut Session, _: &Value) -> CmdResult {
    Ok(json!({"url": page("issues/new/choose"), "dialog": "reportProblem"}))
}

fn whats_new(_: &mut Session, _: &Value) -> CmdResult {
    Ok(json!({"url": page(&format!("releases/tag/v{VERSION}")), "version": VERSION}))
}

fn account(s: &mut Session, _: &Value) -> CmdResult {
    let accounts: Vec<Value> = s.mailbox.accounts.iter().map(|a| json!({"id": a.id, "name": a.name, "email": a.email, "kind": a.kind})).collect();
    Ok(json!({"version": VERSION, "accounts": accounts, "offline": s.prefs.offline}))
}

#[cfg(test)]
#[path = "help_tests.rs"]
mod help_tests;
