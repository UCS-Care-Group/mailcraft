// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The filters of the owner's own Sieve script, as a webmail of the RainLoop kind (RainLoop,
//! SnappyMail) writes them, changed with the rule commands (#2057): `rules.edit`, `rules.enable`,
//! `rules.move` and `rules.remove` on the rule a filter is shown as, and `rules.add` with
//! `"webmail": true` for a new one.
//!
//! Each command reads the script as last read from the server ([`super::sieve_elsewhere`]),
//! changes the bytes of the filters it is about and nothing else, and says what a host is to
//! write as `sieve.putOwnScript` does ([`super::sieve_own::own_put`]): the whole new text, checked
//! as a whole script, written only while the server's script is still the one read. A changed
//! filter has its settings (the webmail's JSON, in base64) and its Sieve written again together,
//! in the webmail's own shape ([`crate::sieve_rainloop`]), so that the webmail shows the change;
//! a move or a delete takes whole filters, byte for byte; `require` is given an extension a new
//! filter needs, in the shape of its list.
//!
//! A filter whose settings this version doesn't all know, or whose Sieve was changed by hand, is
//! read-only ([`ReadOnly`]). The mailbox changes in none of this: the rules shown follow the new
//! text when the host reads the server again.
//!
//! **The script is data.** It comes from the server, and its filters' names and values are shown
//! as text and written back as data, never read as instructions.

use mailcraft_i18n::Arg;
use mailcraft_model::{Action, Condition, Id, Rule};
use serde_json::{Value, json};

use super::sieve_elsewhere::{Block, Elsewhere, webmail_conditions};
use super::sieve_own::{OWN_CHANGED, Webmail, own_put, own_script, webmail};
use super::{bad, fail};
use crate::sieve::digest;
use crate::sieve_rainloop::{self, MAX_CONDITIONS, ReadOnly, Settings, header_name, line_end, new_id, value_ok, with_require};
use crate::{CmdError, CmdResult, Session, p};

/// Why a rule can't be written as a filter of the webmail's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotWebmail {
    /// One condition, of a recipient or a header, and no exception.
    Condition,
    /// One move into a folder of its account, stopping the rules after it, with no alert.
    Action,
}

impl NotWebmail {
    /// Why, in words.
    pub fn said(self) -> &'static str {
        match self {
            NotWebmail::Condition => "A filter of your webmail's has one condition here, on a recipient or a message header, and no exception.",
            NotWebmail::Action => {
                "A filter of your webmail's moves mail to one folder of its account and stops the filters after it, with no desktop alert."
            }
        }
    }
}

/// A filter of the owner's own script, found by the rule it is shown as.
pub struct Found<'a> {
    pub account: Id,
    pub script: &'a Elsewhere,
    /// Where it stands among the script's filters.
    pub index: usize,
    pub block: &'a Block,
}

/// The filter the rule `rule` is shown as, in the owner's own script that the server runs.
pub fn find(s: &Session, rule: Id) -> Option<Found<'_>> {
    s.sieve.iter().find_map(|(account, k)| {
        let e = k.elsewhere.as_ref().filter(|e| !e.ours)?;
        let (index, block) = e.blocks.iter().enumerate().find(|(_, b)| b.rule == Some(rule))?;
        // Only while it is the script the server runs.
        (own_script(s, *account).map(|(name, _)| name) == Some(e.script.as_str())).then_some(Found { account: *account, script: e, index, block })
    })
}

/// Why the rule `rule`, a filter of the webmail's, can't be changed here, when it is one and can't.
pub fn read_only(s: &Session, rule: Id) -> Option<ReadOnly> {
    find(s, rule).and_then(|f| f.block.settings.as_ref().err().copied())
}

/// Whether new filters can be added to `account`'s script: the owner's own, the webmail's.
pub fn takes_new(s: &Session, account: Id) -> bool {
    own_script(s, account).is_some_and(|(_, text)| webmail(text) == Some(Webmail::RainLoop))
}

/// `v`'s text `k`, when it is given: a text, or refused.
fn opt_text<'a>(v: &'a Value, k: &str) -> Result<Option<&'a str>, CmdError> {
    match v.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(t)) => Ok(Some(t.as_str())),
        Some(_) => Err(bad(format!("`{k}` is a text"))),
    }
}

/// The filter `v`'s `id` names, when it can be changed: refused with why, else.
fn changeable<'a>(s: &'a Session, v: &Value) -> Result<(Found<'a>, &'a Settings), CmdError> {
    let id = p::req_id(v, "id")?;
    let Some(found) = find(s, id) else {
        // (Not a filter: a rule of a script made elsewhere, never changed.)
        super::rules::elsewhere(s, id)?;
        return Err(failed!("no rule with id {id}", id));
    };
    if let Some(read) = opt_text(v, "digest")?
        && !read.eq_ignore_ascii_case(&digest(&found.script.text))
    {
        return Err(fail(OWN_CHANGED));
    }
    let block = found.block;
    match &block.settings {
        Ok(settings) => Ok((found, settings)),
        Err(why) => Err(failed!("This filter of your webmail's can't be changed here: {why}.", why = Arg::words(why.said()))),
    }
}

/// The settings a rule given as `rules.add` takes it (`v`) is written as on `account`, with the
/// id `id`, switched on or off as `enabled`; the conditions of `old` kept when the rule's
/// condition is still theirs.
fn wanted(s: &Session, v: &Value, account: Id, id: &str, enabled: bool, old: Option<&Settings>) -> Result<Settings, CmdError> {
    let parts = super::rules::rule_parts(s, v)?;
    if p::id_strict(v, "account")?.is_some_and(|a| a != account) {
        return Err(fail("a rule stays with its account: delete it and create one for the other account"));
    }
    if p::opt_bool(v, "server")? == Some(false) {
        return Err(fail("A filter of your webmail's is kept in your Sieve script on the server."));
    }
    let condition = match (parts.conditions.as_slice(), parts.exceptions.is_empty()) {
        ([one], true) => one.clone(),
        _ => return Err(CmdError::Failed(NotWebmail::Condition.said().to_string())),
    };
    // (Stopping is what the webmail's filters do; a rule asked for without saying, does too.)
    let stops = p::opt_bool(v, "stop")?.unwrap_or(true);
    let folder = match parts.actions.as_slice() {
        [Action::MoveTo(f)] if stops && !parts.alert => s.mailbox.folder(*f).filter(|f| f.account == account),
        _ => None,
    };
    let folder = folder.map(|f| f.remote.clone()).filter(|r| value_ok(r)).ok_or_else(|| CmdError::Failed(NotWebmail::Action.said().to_string()))?;
    let kept = old.filter(|o| webmail_conditions(&o.conditions).as_ref() == Some(&condition));
    let conditions = match kept {
        Some(o) => o.conditions.clone(),
        None => conditions(&condition)?,
    };
    let settings = Settings { id: id.to_string(), enabled, name: parts.name, conditions, folder };
    // As the script is read again: settings too long to read would make the filter read-only (#2058).
    if settings.json().len() > sieve_rainloop::MAX_JSON {
        return Err(fail("A filter of your webmail's has too much text to look for."));
    }
    Ok(settings)
}

/// A rule's condition as the webmail's conditions, any one of which is enough.
fn conditions(c: &Condition) -> Result<Vec<sieve_rainloop::Condition>, CmdError> {
    let refuse = || CmdError::Failed(NotWebmail::Condition.said().to_string());
    let mut out = Vec::new();
    match c {
        Condition::ToContains(words) => {
            for w in words {
                out.push(sieve_rainloop::Condition::new("Recipient", "Contains", w, "").ok_or_else(refuse)?);
            }
        }
        Condition::Header { names, words, exact } => {
            let kind = if *exact { "EqualTo" } else { "Contains" };
            for n in names {
                if !header_name(n) {
                    return Err(bad("header's `names` are header names, such as X-Spam-Flag"));
                }
                for w in words {
                    out.push(sieve_rainloop::Condition::new("Header", kind, w, n).ok_or_else(refuse)?);
                }
            }
        }
        _ => return Err(refuse()),
    }
    if out.len() > MAX_CONDITIONS {
        return Err(failed!("A filter of your webmail's looks for at most {n} things.", n = MAX_CONDITIONS));
    }
    if !out.iter().all(|c| value_ok(&c.value)) {
        return Err(bad("a word to look for has a control character, or is too long"));
    }
    Ok(out)
}

/// The text `text` with its bytes `at` put in place of `with`.
fn spliced(text: &str, at: &std::ops::Range<usize>, with: &str) -> Result<String, CmdError> {
    let (before, after) = text.get(..at.start).zip(text.get(at.end..)).ok_or_else(|| fail(OWN_CHANGED))?;
    Ok(format!("{before}{with}{after}"))
}

/// What `rules.*` answers for the new text `text` of the script of `account`: what a host is to
/// write, as `sieve.putOwnScript` says it, with `answer`'s own fields.
fn write(s: &Session, account: Id, text: String, mut answer: Value) -> CmdResult {
    let read = s.sieve.get(&account).and_then(|k| k.elsewhere.as_ref()).map(|e| digest(&e.text)).unwrap_or_default();
    let put = own_put(s, &json!({"account": account, "text": text, "digest": read}))?;
    let warnings: Vec<String> = put.warnings.iter().map(|p| format!("{}:{}: {}", p.line, p.column, p.what())).collect();
    answer["account"] = json!(account);
    answer["webmail"] = json!(true);
    answer["sieve"] = json!({"name": put.name, "text": put.text, "lastWritten": put.digest, "activate": false, "own": true});
    answer["sends"] = json!(false);
    answer["warnings"] = json!(warnings);
    Ok(answer)
}

/// The new text of the script for `rules.edit` `v` (shared with the agent's question): the
/// filter's account, its rule as it will be shown, and the text.
pub(crate) fn edited(s: &Session, v: &Value) -> Result<(Id, Rule, String), CmdError> {
    p::only(v, &["id", "name", "conditions", "exceptions", "actions", "stop", "alert", "account", "server", "digest"])?;
    let (found, old) = changeable(s, v)?;
    let new = wanted(s, v, found.account, &old.id, old.enabled, Some(old))?;
    let at = found.block.at.clone().ok_or_else(|| fail(OWN_CHANGED))?;
    // The same settings: the filter's bytes stay as they are, their spacing too (#2058).
    let text = if new == *old { found.script.text.clone() } else { spliced(&found.script.text, &at, &new.block(line_end(&found.script.text)))? };
    Ok((found.account, shown(s, found.account, &new, found.block.rule.unwrap_or_default()), text))
}

/// The rule the settings `new` will be shown as.
fn shown(s: &Session, account: Id, new: &Settings, id: Id) -> Rule {
    let (condition, folder) = super::sieve_elsewhere::webmail_rule(&s.mailbox, account, new).unwrap_or((Condition::ToContains(Vec::new()), 0));
    Rule {
        id,
        name: new.name.clone(),
        enabled: new.enabled,
        account,
        conditions: vec![condition],
        actions: vec![Action::MoveTo(folder)],
        stop: true,
        keep: mailcraft_model::Keep::Server,
        ..Default::default()
    }
}

pub(crate) fn edit(s: &mut Session, v: &Value) -> CmdResult {
    let (account, rule, text) = edited(s, v)?;
    write(s, account, text, json!({"id": rule.id, "keep": "server"}))
}

/// The new text of the script for `rules.add` `v` with `"webmail": true` (shared with the agent's
/// question): the account, the new filter as it will be shown, and the text.
pub(crate) fn added(s: &Session, v: &Value) -> Result<(Id, Rule, String), CmdError> {
    p::only(v, &["name", "conditions", "exceptions", "actions", "stop", "alert", "account", "server", "webmail", "digest"])?;
    if p::opt_bool(v, "webmail")? != Some(true) {
        return Err(bad("`webmail` is true, or left out"));
    }
    let account = p::req_id(v, "account")?;
    s.mailbox.account(account).ok_or_else(|| failed!("no account with id {account}", account))?;
    if !takes_new(s, account) {
        return Err(fail("Your webmail's Sieve script isn't the one this account's server runs, as MailCraft last read it."));
    }
    let e = s.sieve.get(&account).and_then(|k| k.elsewhere.as_ref()).ok_or_else(|| fail(super::sieve_own::NO_OWN_SCRIPT))?;
    if let Some(read) = opt_text(v, "digest")?
        && !read.eq_ignore_ascii_case(&digest(&e.text))
    {
        return Err(fail(OWN_CHANGED));
    }
    if e.blocks.len() >= sieve_rainloop::MAX_FILTERS {
        return Err(fail("there are too many rules"));
    }
    let text = &e.text;
    let taken: Vec<&str> = e.blocks.iter().map(|b| b.id.as_str()).collect();
    let name = opt_text(v, "name")?.unwrap_or_default();
    let id = new_id(&format!("{}\u{0}{name}\u{0}{}", digest(text), s.now()), &taken);
    let new = wanted(s, v, account, &id, true, None)?;
    let nl = line_end(text);
    let block = new.block(nl);
    // After the last filter, set apart from it as the filters before are; else at the end.
    let ends: Vec<std::ops::Range<usize>> = e.blocks.iter().filter_map(|b| b.at.clone()).collect();
    let new_text = match ends.as_slice() {
        [.., before, last] => {
            let gap = text.get(before.end..).and_then(|t| t.get(..t.len().saturating_sub(t.trim_start().len()))).filter(|g| !g.is_empty());
            let gap = gap.unwrap_or("").to_string();
            let gap = if gap.is_empty() { format!("{nl}{nl}") } else { gap };
            spliced(text, &(last.end..last.end), &format!("{gap}{block}"))?
        }
        [last] => spliced(text, &(last.end..last.end), &format!("{nl}{nl}{block}"))?,
        // No filter yet: at the end, only of a script with nothing but its `require` and
        // comments (the webmail's empty one). After commands of the owner's own a new filter
        // could never run (a `stop;` for every message), and where else it could go isn't known.
        [] if !e.rules.is_empty() || e.replies || e.unreadable => {
            return Err(fail(
                "Your webmail's Sieve script has commands of its own and no filter to put a new one after: add the filter in your webmail, or change the whole script.",
            ));
        }
        [] => {
            let end = text.trim_end().len();
            let tail = text.get(end..).filter(|t| !t.is_empty()).unwrap_or(nl);
            format!("{}{nl}{nl}{block}{tail}", text.get(..end).unwrap_or_default())
        }
    };
    let new_text = with_require(&new_text, "fileinto", nl).ok_or_else(|| fail(OWN_CHANGED))?;
    let rule = shown(s, account, &new, 0);
    Ok((account, rule, new_text))
}

pub(crate) fn add(s: &mut Session, v: &Value) -> CmdResult {
    let (account, _, text) = added(s, v)?;
    write(s, account, text, json!({"keep": "server"}))
}

/// The new text for `rules.remove` `v`: the filter's bytes taken out, with the white space that
/// set it apart from the next one (or, the last, from the one before).
pub(crate) fn removed(s: &Session, v: &Value) -> Result<(Id, Rule, String), CmdError> {
    p::only(v, &["id", "digest"])?;
    let (found, old) = changeable(s, v)?;
    let text = &found.script.text;
    let at = found.block.at.clone().ok_or_else(|| fail(OWN_CHANGED))?;
    let after = text.get(at.end..).unwrap_or_default();
    let gap_after = after.len().saturating_sub(after.trim_start().len());
    let cut = if after.trim().is_empty() {
        let before = text.get(..at.start).unwrap_or_default();
        before.trim_end().len()..at.end
    } else {
        at.start..at.end.saturating_add(gap_after)
    };
    let new_text = spliced(text, &cut, "")?;
    Ok((found.account, shown(s, found.account, old, found.block.rule.unwrap_or_default()), new_text))
}

pub(crate) fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let (account, rule, text) = removed(s, v)?;
    write(s, account, text, json!({"deleted": rule.id}))
}

/// The new text for `rules.enable` `v`, and whether the filter is to be on; `None` for the text
/// when it is already as asked.
pub(crate) fn enabled(s: &Session, v: &Value) -> Result<(Id, Rule, bool, Option<String>), CmdError> {
    p::only(v, &["id", "value", "digest"])?;
    let (found, old) = changeable(s, v)?;
    let on = p::opt_bool(v, "value")?.unwrap_or(!old.enabled);
    let rule = shown(s, found.account, old, found.block.rule.unwrap_or_default());
    if on == old.enabled {
        return Ok((found.account, rule, on, None));
    }
    let new = Settings { enabled: on, ..old.clone() };
    let at = found.block.at.clone().ok_or_else(|| fail(OWN_CHANGED))?;
    let text = spliced(&found.script.text, &at, &new.block(line_end(&found.script.text)))?;
    Ok((found.account, rule, on, Some(text)))
}

pub(crate) fn enable(s: &mut Session, v: &Value) -> CmdResult {
    let (account, rule, on, text) = enabled(s, v)?;
    let answer = json!({"id": rule.id, "enabled": on});
    match text {
        Some(text) => write(s, account, text, answer),
        None => Ok(answer),
    }
}

/// The new text for `rules.move` `v`: the filter and the one before it (up) or after it (down)
/// change places, each byte for byte; `None` when there is none there.
pub(crate) fn moved(s: &Session, v: &Value) -> Result<(Id, Rule, bool, Option<String>), CmdError> {
    p::only(v, &["id", "direction", "account", "digest"])?;
    let up = match p::req_str(v, "direction")? {
        "up" => true,
        "down" => false,
        _ => return Err(bad("`direction` is \"up\" or \"down\"")),
    };
    let (found, old) = changeable(s, v)?;
    if p::id_strict(v, "account")?.is_some_and(|a| a != found.account) {
        return Err(fail("a rule stays with its account: delete it and create one for the other account"));
    }
    let rule = shown(s, found.account, old, found.block.rule.unwrap_or_default());
    let other = if up { found.index.checked_sub(1) } else { found.index.checked_add(1) };
    let Some(other) = other.and_then(|i| found.script.blocks.get(i)) else { return Ok((found.account, rule, up, None)) };
    let (Some(a), Some(b)) = (found.block.at.clone(), other.at.clone()) else {
        return Err(fail("the filter next to it in your webmail's script has no end, so it can't be moved"));
    };
    let (first, second) = if a.start < b.start { (a, b) } else { (b, a) };
    let text = &found.script.text;
    let piece = |r: &std::ops::Range<usize>| text.get(r.clone()).map(str::to_string);
    let (Some(one), Some(two), Some(between)) = (piece(&first), piece(&second), piece(&(first.end..second.start))) else {
        return Err(fail(OWN_CHANGED));
    };
    let new_text = spliced(text, &(first.start..second.end), &format!("{two}{between}{one}"))?;
    Ok((found.account, rule, up, Some(new_text)))
}

pub(crate) fn move_filter(s: &mut Session, v: &Value) -> CmdResult {
    let (account, rule, _, text) = moved(s, v)?;
    match text {
        Some(text) => write(s, account, text, json!({"id": rule.id, "moved": true})),
        None => Ok(json!({"id": rule.id, "moved": false})),
    }
}
