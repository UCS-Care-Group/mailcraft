// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Colour categories.

use mailcraft_model::{Id, Rgb};
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
fn list_account(s: &Session, v: &Value) -> Result<Id, CmdError> {
    match v.get("account") {
        None | Some(Value::Null) => Ok(0),
        Some(_) => s.mailbox.account(p::req_id(v, "account")?).map(|a| a.id).ok_or_else(|| bad("`account` isn't an account")),
    }
}

fn set(s: &mut Session, v: &Value) -> CmdResult {
    let color = Rgb::parse(p::req_str(v, "color")?).ok_or_else(|| bad("`color` looks like #2E8B57"))?;
    let account = list_account(s, v)?;
    let name = p::req_str(v, "name")?;
    if s.mailbox.categories_of(account).len() >= 200 && s.mailbox.category_of(account, name).is_none() {
        return Err(super::fail("there are too many categories"));
    }
    // A category on Outlook's list has Outlook's colour (R3 spec S18): nothing changes here or
    // on the server.
    if on_outlooks_list(s, account, name) && s.mailbox.category_of(account, name).is_some_and(|c| c.color != color) {
        return Err(super::fail(COLOUR_IN_OUTLOOK));
    }
    s.mailbox.set_category(account, name, color)?;
    done()
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
