// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Colour categories.

use mailcraft_model::{GREY_CATEGORY, Id, MAX_NAME, Rgb, clean_name};
use serde_json::{Value, json};

use super::{bad, done};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("category.set", "New Category", "Home › Tags", set)
            .params(r##"{"name": string, "color": "#RRGGBB", "account"?: number} — on the list that account's items use (default: the list the accounts without their own share)"##),
        CommandSpec::new("category.rename", "Rename Category", "Home › Tags", rename).params(r#"{"from": string, "to": string, "account"?: number}"#),
        CommandSpec::new("category.remove", "Delete Category", "Home › Tags", remove).params(r#"{"name": string, "account"?: number}"#),
        CommandSpec::new("category.list", "All Categories", "Home › Tags", list)
            .pure()
            .params(r#"{"account"?: number} — answers {"categories": [{"name", "color", "outlook"}], "accounts": the accounts with a list of their own}"#),
    ]
}

/// The account whose list a call is about: `account`, and with none the list that accounts
/// without their own share (0, as [`mailcraft_model::Mailbox::categories_of`] takes it). Anything
/// but an account's id is refused, and never taken for that shared list.
pub fn list_account(s: &Session, v: &Value) -> Result<Id, CmdError> {
    match v.get("account") {
        None | Some(Value::Null) => Ok(0),
        Some(_) => s.mailbox.account(p::req_id(v, "account")?).map(|a| a.id).ok_or_else(|| bad("`account` isn't an account")),
    }
}

fn set(s: &mut Session, v: &Value) -> CmdResult {
    let color = Rgb::parse(p::req_str(v, "color")?).ok_or_else(|| bad("`color` looks like #2E8B57"))?;
    let account = list_account(s, v)?;
    let name = p::req_str(v, "name")?;
    if s.mailbox.category_of(account, name).is_none() {
        check_room(s, account)?;
    }
    // A category on Outlook's list has Outlook's colour (R3 spec S18): nothing changes here or
    // on the server.
    if on_outlooks_list(s, account, name) && s.mailbox.category_of(account, name).is_some_and(|c| c.color != color) {
        return Err(super::fail(COLOUR_IN_OUTLOOK));
    }
    s.mailbox.set_category(account, name, color)?;
    done()
}

/// The spelling an item of `account` gives the category `name` it is put on: its list's own, or
/// the name as given for one the list doesn't have yet. With `add` (the category is being put on,
/// not taken off) that name joins the list in grey, so it can be given a colour.
pub(super) fn spelled(s: &mut Session, account: Id, name: &str, add: bool) -> Result<String, CmdError> {
    if let Some(c) = s.mailbox.category_of(account, name) {
        return Ok(c.name.clone());
    }
    if add {
        check_room(s, account)?;
        s.mailbox.set_category(account, name, GREY_CATEGORY)?;
    }
    Ok(name.to_string())
}

/// The categories an item of `account` carries when `given` is what a new or an edit says it
/// carries, and `had` what it carried before, as an item of `had_account`: each name [`spelled`]
/// as its list has it, a name given twice in other capitals kept once. A name it didn't carry
/// joins the list, as putting it on with Categorize does, and so does one that was on the list
/// of the account it was in (an event moved to another account's calendar). One it carried
/// already that neither list has stays as the item had it, so that an edit that changes nothing
/// of its categories needs no room on a full list. Names are compared as [`clean_name`] leaves
/// them, so that one the item came with that is longer than a typed name may be (from an
/// `.ics`, say) is still its own, and isn't cut and put on the list as a new one. Such a name is
/// first looked for by its whole spelling, so that two the item carries that differ only past
/// the cut stay two.
pub(super) fn spelled_all(s: &mut Session, account: Id, given: &[String], had: &[String], had_account: Id) -> Result<Vec<String>, CmdError> {
    use std::collections::{HashMap, HashSet};
    // Names are compared in ASCII capitals as `eq_ignore_ascii_case` does, through maps made once,
    // so that a long list given against a long list had is no slower than the two read once.
    let fold = |c: &str| c.to_ascii_lowercase();
    let given_whole: HashSet<String> = given.iter().map(|g| fold(g.trim())).collect();
    // What the item had, by its whole spelling and by its cleaned name; for the cleaned name,
    // not a long one that is given whole as well (that is then a name of its own).
    let mut had_whole: HashMap<String, &str> = HashMap::new();
    let mut had_clean: HashMap<String, &str> = HashMap::new();
    for c in had.iter().map(|c| c.trim()) {
        had_whole.entry(fold(c)).or_insert(c);
        if let Some(clean) = clean_name(c)
            && !given_whole.contains(&fold(c))
        {
            had_clean.entry(fold(&clean)).or_insert(c);
        }
    }
    // What each name carried is known by: the item's own spelling of one it had, whole, and the
    // cleaned name of any other.
    let mut keys: HashSet<String> = HashSet::new();
    let mut carried: Vec<String> = Vec::new();
    for raw in given {
        let Some(name) = clean_name(raw) else { continue };
        let whole = raw.trim();
        // A name that wasn't cut may be the cut beginning of a long one the item had, as given
        // back; one that was cut is the item's only by its whole spelling.
        let cut = whole.chars().filter(|c| !c.is_control()).nth(MAX_NAME).is_some();
        let had_as = had_whole.get(&fold(whole)).copied().or_else(|| (!cut).then(|| had_clean.get(&fold(&name)).copied()).flatten());
        let key = had_as.unwrap_or(&name);
        if !keys.insert(fold(key)) {
            continue;
        }
        // A name the item had is looked for on a list by its whole spelling: a long one is on
        // none, and its cut beginning (another name) is not it.
        let listed = |s: &Session, list: Id| s.mailbox.category_of(list, key).is_some();
        let spelling = match had_as {
            Some(c) if !listed(s, account) && !listed(s, had_account) => c.to_string(),
            // Else the list's spelling, or the item's as it joins the list.
            Some(c) => spelled(s, account, c, true)?,
            None => spelled(s, account, &name, true)?,
        };
        carried.push(spelling);
    }
    Ok(carried)
}

/// Refuses a new category when the list `account`'s items use is full.
fn check_room(s: &Session, account: Id) -> Result<(), CmdError> {
    if s.mailbox.categories_of(account).len() >= 200 {
        return Err(super::fail("there are too many categories"));
    }
    Ok(())
}

/// What changing the colour of a category on Outlook's list says.
pub const COLOUR_IN_OUTLOOK: &str = "The colour is set in Outlook";

/// The names on Outlook's list of categories that `account`'s items use, as its server last gave
/// them; `None` while no such list was read.
fn outlooks_list(s: &Session, account: Id) -> Option<&Vec<String>> {
    s.mailbox.category_list_owner(account).and_then(|owner| s.mailbox.server_categories.get(&owner))
}

/// Whether `name` is on Outlook's list of categories, as the server of `account` last gave it.
pub fn on_outlooks_list(s: &Session, account: Id, name: &str) -> bool {
    outlooks_list(s, account).is_some_and(|names| names.iter().any(|c| mailcraft_model::sync::same_category(c, name.trim())))
}

fn rename(s: &mut Session, v: &Value) -> CmdResult {
    let account = list_account(s, v)?;
    s.mailbox.rename_category(account, p::req_str(v, "from")?, p::req_str(v, "to")?)?;
    done()
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let account = list_account(s, v)?;
    s.mailbox.remove_category(account, p::req_str(v, "name")?)?;
    done()
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    let account = list_account(s, v)?;
    // `outlook`: on Outlook's list (its colour is Outlook's); `null` while no such list was read.
    let read = outlooks_list(s, account).is_some();
    let listed = |name: &str| read.then(|| on_outlooks_list(s, account, name));
    let categories = s.mailbox.categories_of(account).iter().map(|c| json!({"name": c.name, "color": c.color.hex(), "outlook": listed(&c.name)}));
    let own = s.mailbox.accounts.iter().filter(|a| s.mailbox.account_categories.contains_key(&a.id));
    Ok(json!({"categories": categories.collect::<Vec<_>>(), "accounts": own.map(|a| json!({"id": a.id, "email": a.email})).collect::<Vec<_>>()}))
}
