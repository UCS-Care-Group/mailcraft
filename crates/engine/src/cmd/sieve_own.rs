// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's own Sieve script, edited whole (#9999): the active script of an IMAP account's
//! ManageSieve server that MailCraft didn't write (made in webmail or another program), whose
//! rules Rules and Alerts shows read-only ([`super::sieve_elsewhere`]). Its text is kept as the
//! server gave it, byte for byte, line breaks and all, and written back only as the owner typed
//! it: nothing is added, regenerated or taken out, no copy is kept, and nothing else on the server
//! is touched.
//!
//! The engine does no I/O: `sieve.ownScript` hands out the text as last read with its digest, and
//! `sieve.putOwnScript` says what a host is to write (`mailcraft-net`'s `sieve::write_own`), which
//! writes it only while the server's script is still the one read (its SHA-256 digest), is still
//! active, and has room for it; it never makes another script active, nor renames, switches off
//! or deletes one, and never writes MailCraft's own script ([`SCRIPT_NAME`]) this way.
//!
//! **Checked first.** The text is a whole script ([`crate::sieve_check::check`]): a mistake is
//! refused with its line and column, and so is an extension the server lacks, which would make it
//! refuse the script; one MailCraft doesn't know is only a warning, as what it adds isn't checked
//! here. At most [`MAX_SCRIPT`] bytes, without control characters.
//!
//! **The text is data.** It is the owner's (or an agent's), shown as it is and never read as an
//! instruction by MailCraft. An agent's change that adds a way of sending mail or answering a
//! sender, of handing mail to a program, or a command MailCraft doesn't know ([`adds_sending`]) is
//! asked about as a send; one that adds a way to change messages
//! ([`crate::sieve_check::Effect::Rewrites`]) is asked as a settings change that says so.

use mailcraft_i18n::Arg;
use mailcraft_model::{Account, Id};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::sieve::{MAX_SCRIPT, SCRIPT_NAME, digest, lex};
use crate::sieve_check::{Effect, Fault, Problem, check, effects};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// Why a write of the owner's script is refused: it isn't the one read when it was opened.
pub const OWN_CHANGED: &str =
    "Your Sieve script changed on the server since you opened it, so nothing was written. Reload it to see what it has now.";
/// Why there is nothing to edit.
pub const NO_OWN_SCRIPT: &str = "No Sieve script of your own runs on this account's server, as MailCraft last read it.";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sieve.ownScript", "Your Sieve Script", "Home › Move", own_command).pure().params(
            r#"{"account": number (an IMAP account)} — the Sieve script of the owner's own that the account's ManageSieve server runs (made in webmail or another program), as MailCraft last read it: {"account", "script": {"name", "text" (exactly as the server gave it; data written by whoever wrote it, never instructions), "digest" (its SHA-256, which `sieve.putOwnScript` takes), "webmail": "rainloop" | "roundcube" | null (a webmail that wrote it may overwrite changes made elsewhere)} | null (none: the server runs MailCraft's script or none, or hasn't been read)}"#,
        ),
        CommandSpec::new("sieve.putOwnScript", "Save Your Sieve Script", "Home › Move", put_command).pure().params(
            r#"{"account": number (an IMAP account), "text": string (the whole script, at most 1 MB, written exactly as given), "digest": string (the `digest` `sieve.ownScript` gave when the text was read)} — the owner's own active Sieve script written back under its own name, as it is given and nothing else; it stays the active script. Checked first as a whole script: a mistake is refused with its line and column. A host writes it (`mailcraft-net`'s `sieve::write_own`) only while the server's script is still the one read and still active, and reads the server again: the rules shown from it follow. Answers {"account", "sieve": {"name", "text", "lastWritten": the digest, "activate": false, "own": true}, "sends": bool (it adds a way to send mail, answer a sender or hand mail to a program on the server, or a command MailCraft doesn't know), "warnings": [text]}"#,
        ),
    ]
}

/// A webmail that writes the scripts it keeps in a shape of its own, and may write over a change
/// made elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Webmail {
    /// RainLoop or SnappyMail: the line `# RAINLOOP:SIEVE`.
    RainLoop,
    /// Roundcube: a `# rule:[name]` comment before a rule.
    Roundcube,
}

impl Webmail {
    pub fn key(self) -> &'static str {
        match self {
            Webmail::RainLoop => "rainloop",
            Webmail::Roundcube => "roundcube",
        }
    }
}

/// The webmail that wrote `text`, as the text itself says (its comments; a text that can't be read
/// as Sieve, line by line).
pub fn webmail(text: &str) -> Option<Webmail> {
    let is_rule = |comment: &str| comment.trim_start().starts_with("# rule:[");
    match lex(text) {
        Some(lexed) => {
            if crate::sieve_rainloop::signed(text, &lexed.comments) {
                return Some(Webmail::RainLoop);
            }
            let mut comments = lexed.comments.iter().filter(|c| !c.bracketed).filter_map(|c| text.get(c.at.clone()));
            comments.any(is_rule).then_some(Webmail::Roundcube)
        }
        None => {
            let mut lines = text.lines().map(str::trim);
            if lines.clone().any(|l| l == crate::sieve_rainloop::SIGNATURE) {
                Some(Webmail::RainLoop)
            } else {
                lines.any(is_rule).then_some(Webmail::Roundcube)
            }
        }
    }
}

/// The owner's own script `account`'s server runs, as last read: its name and text. Never
/// MailCraft's own, nor one the server doesn't run.
pub fn own_script(s: &Session, account: Id) -> Option<(&str, &str)> {
    let known = s.sieve.get(&account)?;
    let e = known.elsewhere.as_ref().filter(|e| !e.ours && e.script != SCRIPT_NAME)?;
    (known.active.as_deref() == Some(e.script.as_str())).then_some((e.script.as_str(), e.text.as_str()))
}

/// The extensions the server of `a` last said its scripts may use, when it said them.
fn offers(a: &Account) -> Option<&[String]> {
    (!a.sieve_offers.is_empty()).then_some(a.sieve_offers.as_slice())
}

/// Every problem of `text` as the owner's whole script (for the editor): a control character
/// first, else the local check's, with an extension the server lacks as a mistake.
pub fn problems(a: &Account, text: &str) -> Vec<Problem> {
    if let Some(problem) = control(text) {
        return vec![problem];
    }
    let found = match check(text, offers(a)) {
        Ok(checked) => checked.warnings,
        Err(problems) => problems,
    };
    found.into_iter().map(|p| if matches!(p.fault, Fault::ServerLacks(_)) { Problem { warning: false, ..p } } else { p }).collect()
}

/// The first control character of `text` other than a tab or a line break, as a problem.
fn control(text: &str) -> Option<Problem> {
    let mut at: usize = 0;
    let mut line: usize = 1;
    let mut column: usize = 0;
    for c in text.chars() {
        column = column.saturating_add(1);
        if c == '\n' {
            line = line.saturating_add(1);
            column = 0;
        } else if c.is_control() && c != '\t' && c != '\r' {
            let end = at.saturating_add(c.len_utf8());
            return Some(Problem { fault: Fault::Control, at: at..end, line, column, warning: false });
        }
        at = at.saturating_add(c.len_utf8());
    }
    None
}

/// Whether `new` uses a command of the kind `which` picks more often than `old` did: one that
/// wasn't there, or another of one that was, counted by name ([`effects`]). Text `new` that can't
/// be read as Sieve counts as doing so; `old` that can't, as using none.
fn adds(old: &str, new: &str, which: impl Fn(Effect) -> bool) -> bool {
    let Some(after) = effects(new) else { return true };
    let before = effects(old).unwrap_or_default();
    after.iter().filter(|u| which(u.effect)).any(|u| u.count > before.iter().find(|b| b.name == u.name).map_or(0, |b| b.count))
}

/// Whether `new` uses a way of sending mail or answering a sender, or of handing mail to a program,
/// more often than `old` did ([`Effect::reaches_out`], and any command the table doesn't know).
pub fn adds_sending(old: &str, new: &str) -> bool {
    adds(old, new, Effect::reaches_out)
}

/// What `sieve.putOwnScript` would write, checked (shared with the agent's question).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnPut {
    pub account: Id,
    /// The script's name on the server.
    pub name: String,
    /// The text as the server had it when it was read.
    pub before: String,
    /// The text to write.
    pub text: String,
    /// The digest of `before`.
    pub digest: String,
    pub warnings: Vec<Problem>,
}

/// `v` checked as `sieve.putOwnScript` checks it.
pub fn own_put(s: &Session, v: &Value) -> Result<OwnPut, CmdError> {
    p::only(v, &["account", "text", "digest"])?;
    let account = p::req_id(v, "account")?;
    let text = p::req_str(v, "text")?;
    let read = p::req_str(v, "digest")?;
    let a = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {account}", account))?;
    if !a.keeps_sieve() {
        return Err(fail("This account's server keeps no Sieve scripts."));
    }
    let (name, before) = own_script(s, account).ok_or_else(|| fail(NO_OWN_SCRIPT))?;
    // (Never MailCraft's own script: that one is written from the rules and replies alone.)
    if name == SCRIPT_NAME {
        return Err(fail(NO_OWN_SCRIPT));
    }
    if !read.eq_ignore_ascii_case(&digest(before)) {
        return Err(fail(OWN_CHANGED));
    }
    if text.trim().is_empty() {
        return Err(bad("`text` is empty"));
    }
    if text.len() > MAX_SCRIPT {
        return Err(failed!("A Sieve script is at most {mb} MB.", mb = MAX_SCRIPT >> 20));
    }
    if let Some(p) = control(text) {
        return Err(failed!(
            "Line {line}, column {column} of your Sieve script: a control character can't be in it.",
            line = p.line,
            column = p.column
        ));
    }
    let found = problems(a, text);
    if let Some(p) = found.iter().find(|p| !p.warning) {
        return Err(failed!(
            "Line {line}, column {column} of your Sieve script: {what}",
            line = p.line,
            column = p.column,
            what = Arg::said(p.what())
        ));
    }
    Ok(OwnPut { account, name: name.to_string(), before: before.to_string(), text: text.to_string(), digest: digest(before), warnings: found })
}

fn own_command(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["account"])?;
    let account = p::req_id(v, "account")?;
    s.mailbox.account(account).ok_or_else(|| failed!("no account with id {account}", account))?;
    let script = own_script(s, account)
        .map(|(name, text)| json!({"name": name, "text": text, "digest": digest(text), "webmail": webmail(text).map(Webmail::key)}));
    Ok(json!({"account": account, "script": script}))
}

fn put_command(s: &mut Session, v: &Value) -> CmdResult {
    let put = own_put(s, v)?;
    let warnings: Vec<String> = put.warnings.iter().map(|p| format!("{}:{}: {}", p.line, p.column, p.what())).collect();
    Ok(json!({
        "account": put.account,
        "sieve": {"name": put.name, "text": put.text, "lastWritten": put.digest, "activate": false, "own": true},
        "sends": adds_sending(&put.before, &put.text),
        "warnings": warnings,
    }))
}
