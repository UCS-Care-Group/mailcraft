// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Colour categories.
//!
//! A Microsoft account's list is kept in step with its server both ways (#661): what is made,
//! renamed, recoloured or deleted here is the difference between the list and the server's as
//! last synced, and goes with the next job ([`super::categories_remote`]). Its colours are the
//! server's 25 ([`mailcraft_model::outlook_colors`]). A list whose server MailCraft may not change
//! is read-only here, and says so.

use mailcraft_model::outlook_colors::{OUTLOOK_COLORS, outlook_preset};
use mailcraft_model::{GREY_CATEGORY, Id, Rgb};
use serde_json::{Value, json};

use super::{bad, done};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("category.set", "New Category", "Home › Tags", set)
            .params(r##"{"name": string, "color": "#RRGGBB", "account"?: number} — on the list that account's items use (default: the list the accounts without their own share). A Microsoft account's list takes one of the colours `category.list` gives for it, and refuses any change while it is read-only"##),
        CommandSpec::new("category.rename", "Rename Category", "Home › Tags", rename).params(r#"{"from": string, "to": string, "account"?: number}"#),
        CommandSpec::new("category.remove", "Delete Category", "Home › Tags", remove).params(r#"{"name": string, "account"?: number}"#),
        CommandSpec::new("category.addToOutlook", "Add to Outlook", "Home › Tags", add_to_outlook).params(
            r#"{"name": string, "account": number} — a category of a Microsoft account's list that is kept on this computer only ("outlook": false in `category.list`) goes to its server with the next job, in the nearest of the server's colours"#,
        ),
        CommandSpec::new("category.list", "All Categories", "Home › Tags", list).pure().params(
            r##"{"account"?: number} — answers {"categories": [{"name", "color", "outlook"}], "accounts": the accounts with a list of their own; for a Microsoft account's list also "readOnly": bool (MailCraft may not change its server's list) and "colors": ["#RRGGBB"] (the colours it takes)}"##,
        ),
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

/// Most categories the owner makes on one list.
const MAX_MADE: usize = 200;

fn set(s: &mut Session, v: &Value) -> CmdResult {
    let color = Rgb::parse(p::req_str(v, "color")?).ok_or_else(|| bad("`color` looks like #2E8B57"))?;
    let account = list_account(s, v)?;
    let name = p::req_str(v, "name")?;
    if s.mailbox.category_of(account, name).is_none() {
        check_room(s, account)?;
    }
    // A list kept in step with a Microsoft account's server has the server's colours (#661).
    if s.mailbox.synced_list_owner(account).is_some() && outlook_preset(color).is_none() {
        return Err(bad("a Microsoft account's category takes one of the colours `category.list` gives for its list"));
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

/// Refuses a new category when the list `account`'s items use is full. (A list that is read-only
/// here says that instead, when it is asked to change.)
fn check_room(s: &Session, account: Id) -> Result<(), CmdError> {
    if s.mailbox.categories_read_only(account).is_none() && s.mailbox.categories_of(account).len() >= MAX_MADE {
        return Err(super::fail("there are too many categories"));
    }
    Ok(())
}

/// The names on Outlook's list of categories that `account`'s items use, as its server last gave
/// them; `None` while no such list was read.
fn outlooks_list(s: &Session, account: Id) -> Option<&Vec<String>> {
    s.mailbox.category_list_owner(account).and_then(|owner| s.mailbox.server_categories.get(&owner))
}

/// Whether `name` is on Outlook's list of categories, as the server of `account` last gave it.
pub fn on_outlooks_list(s: &Session, account: Id, name: &str) -> bool {
    outlooks_list(s, account).is_some_and(|names| names.iter().any(|c| mailcraft_model::sync::same_category(c, name.trim())))
}

/// Whether `name`, on the list `account`'s items use, is a Microsoft account's that is kept in
/// step with its server (#661): on Outlook's list, or made here and to go there. `None` for a
/// list that isn't kept in step with a server.
pub fn in_outlook(s: &Session, account: Id, name: &str) -> Option<bool> {
    let owner = s.mailbox.synced_list_owner(account)?;
    Some(!s.mailbox.category_is_local(owner, name))
}

fn rename(s: &mut Session, v: &Value) -> CmdResult {
    let account = list_account(s, v)?;
    let before = categorized_events(s);
    s.mailbox.rename_category(account, p::req_str(v, "from")?, p::req_str(v, "to")?)?;
    tell_events(s, &before)?;
    done()
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let account = list_account(s, v)?;
    let before = categorized_events(s);
    s.mailbox.remove_category(account, p::req_str(v, "name")?)?;
    tell_events(s, &before)?;
    done()
}

/// The events that carry a category, as they are before a change of the list.
fn categorized_events(s: &Session) -> Vec<mailcraft_model::Event> {
    s.mailbox.events.values().filter(|e| !e.categories.is_empty()).map(|e| (**e).clone()).collect()
}

/// The events whose categories a rename or a deletion changed (`before`: as they were) are told
/// to their servers as an edit of each would be (#661). Mail and to-dos need nothing of the
/// kind: what differs from the server's copy goes by itself. An event of a calendar this account
/// may only read is not: it keeps the old name there.
fn tell_events(s: &mut Session, before: &[mailcraft_model::Event]) -> Result<(), CmdError> {
    let offset_at = super::remote::offset_of(s);
    for e in before {
        if super::remote::writable(&s.mailbox, e.calendar).is_err() {
            continue;
        }
        if s.mailbox.event(e.id).is_some_and(|now| now.categories != e.categories) {
            super::remote::note_changed(&mut s.mailbox, e, &offset_at)?;
        }
    }
    Ok(())
}

fn add_to_outlook(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::req_id(v, "account")?;
    s.mailbox.account(account).ok_or_else(|| bad("`account` isn't an account"))?;
    let name = p::req_str(v, "name")?;
    if s.mailbox.synced_list_owner(account).is_none() {
        return Err(super::fail("this account's categories aren't kept in step with a server"));
    }
    if !s.mailbox.send_category(account, name)? {
        return Err(super::fail(format!("\"{}\" isn't a category kept on this computer only", name.trim())));
    }
    done()
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    let account = list_account(s, v)?;
    // `outlook`: kept in step with Outlook's list (its colour is Outlook's); false for one kept on
    // this computer only; `null` while no such list was read.
    let synced = s.mailbox.synced_list_owner(account).is_some();
    let read = outlooks_list(s, account).is_some();
    let listed = |name: &str| if synced { in_outlook(s, account, name) } else { read.then(|| on_outlooks_list(s, account, name)) };
    let categories = s.mailbox.categories_of(account).iter().map(|c| json!({"name": c.name, "color": c.color.hex(), "outlook": listed(&c.name)}));
    let own = s.mailbox.accounts.iter().filter(|a| s.mailbox.account_categories.contains_key(&a.id));
    let mut answer =
        json!({"categories": categories.collect::<Vec<_>>(), "accounts": own.map(|a| json!({"id": a.id, "email": a.email})).collect::<Vec<_>>()});
    if synced && let Some(answer) = answer.as_object_mut() {
        answer.insert("readOnly".into(), s.mailbox.categories_read_only(account).is_some().into());
        answer.insert("colors".into(), OUTLOOK_COLORS.iter().map(|c| c.hex()).collect::<Vec<_>>().into());
    }
    Ok(answer)
}
