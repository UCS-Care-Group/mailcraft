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
use mailcraft_model::sync::same_category;
use mailcraft_model::{GREY_CATEGORY, Id, MAX_NAME, Mailbox, Rgb, category_key, clean_name};
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
    spelled_in(&mut s.mailbox, account, name, add)
}

fn spelled_in(mb: &mut Mailbox, account: Id, name: &str, add: bool) -> Result<String, CmdError> {
    if let Some(c) = mb.category_of(account, name) {
        return Ok(c.name.clone());
    }
    if add {
        room_in(mb, account)?;
        mb.set_category(account, name, GREY_CATEGORY)?;
    }
    Ok(name.to_string())
}

/// The names [`spelled_all`] would put on the list `account`'s items use (worked out on a copy,
/// as it would do it), when that list is one MailCraft keeps in step with Outlook and may change
/// there: what the next job makes in Outlook (#661), for the agent gate. Empty for any other list,
/// and for a call the command would refuse.
pub(crate) fn joining(s: &Session, account: Id, given: &[String], had: &[String], had_account: Id) -> Vec<String> {
    let mb = &s.mailbox;
    if mb.synced_list_owner(account).is_none() || mb.categories_read_only(account).is_some() {
        return Vec::new();
    }
    // (A copy only when a name is new to the list: most calls give names it has.)
    if given.iter().filter_map(|g| clean_name(g)).all(|name| mb.category_of(account, &name).is_some()) {
        return Vec::new();
    }
    let mut copy = mb.clone();
    if spelled_all_in(&mut copy, account, given, had, had_account).is_err() {
        return Vec::new();
    }
    let before = mb.categories_of(account);
    copy.categories_of(account).iter().filter(|c| !before.iter().any(|b| same_category(&b.name, &c.name))).map(|c| c.name.clone()).collect()
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
    spelled_all_in(&mut s.mailbox, account, given, had, had_account)
}

fn spelled_all_in(mb: &mut Mailbox, account: Id, given: &[String], had: &[String], had_account: Id) -> Result<Vec<String>, CmdError> {
    use std::collections::{HashMap, HashSet};
    // Names are compared as `same_category` compares them (any capitals, as the lists do), through
    // maps made once, so that a long list given against a long list had is no slower than the two
    // read once.
    let fold = category_key;
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
        let listed = |mb: &Mailbox, list: Id| mb.category_of(list, key).is_some();
        let spelling = match had_as {
            Some(c) if !listed(mb, account) && !listed(mb, had_account) => c.to_string(),
            // Else the list's spelling, or the item's as it joins the list.
            Some(c) => spelled_in(mb, account, c, true)?,
            None => spelled_in(mb, account, &name, true)?,
        };
        carried.push(spelling);
    }
    Ok(carried)
}

/// Refuses a new category when the list `account`'s items use is full. (A list that is read-only
/// here says that instead, when it is asked to change.)
fn check_room(s: &Session, account: Id) -> Result<(), CmdError> {
    room_in(&s.mailbox, account)
}

fn room_in(mb: &Mailbox, account: Id) -> Result<(), CmdError> {
    if mb.categories_read_only(account).is_none() && mb.categories_of(account).len() >= MAX_MADE {
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

/// The events that carry a category and that a change of them is told to their server of
/// ([`tell_events`]), as they are before a change of the list: only they are copied, however
/// many are kept on this computer alone.
fn categorized_events(s: &Session) -> Vec<mailcraft_model::Event> {
    let mb = &s.mailbox;
    let on_server = |e: &mailcraft_model::Event| !e.categories.is_empty() && e.origin.is_remote() && !e.origin.id.is_empty() && told(mb, e);
    mb.events.values().filter(|e| on_server(e)).map(|e| (**e).clone()).collect()
}

/// The events whose categories a rename or a deletion changed (`before`: as they were) are told
/// to their servers as an edit of each would be (#661). Mail and to-dos need nothing of the
/// kind: what differs from the server's copy goes by itself. An event of a calendar this account
/// may only read is not: it keeps the old name there. Nor is a shared mailbox's: only a list that
/// is its own could change its items on its server, and the list the rest share changes them
/// here only, as before.
fn tell_events(s: &mut Session, before: &[mailcraft_model::Event]) -> Result<(), CmdError> {
    let offset_at = super::remote::offset_of(s);
    for e in before {
        if !told(&s.mailbox, e) {
            continue;
        }
        if s.mailbox.event(e.id).is_some_and(|now| now.categories != e.categories)
            && let Err(full) = super::remote::note_changed(&mut s.mailbox, e, &offset_at)
        {
            return Err(more_than_can_wait(s, before).unwrap_or(full));
        }
    }
    Ok(())
}

/// Whether a change of `e`'s categories goes to its server ([`tell_events`]): its calendar may be
/// changed there, and isn't a shared mailbox's.
pub(crate) fn told(mb: &mailcraft_model::Mailbox, e: &mailcraft_model::Event) -> bool {
    let shared = mb.calendar(e.calendar).and_then(|c| mb.account(c.origin.account)).is_some_and(|a| a.is_shared());
    !shared && super::remote::writable(mb, e.calendar).is_ok()
}

/// The refusal of a rename or a deletion that changes more events of calendars kept on a server
/// than can wait to be sent at once (#661): Send/Receive getting the changes waiting now through
/// would never make room for them all. `None` when it would.
fn more_than_can_wait(s: &Session, before: &[mailcraft_model::Event]) -> Option<CmdError> {
    let mb = &s.mailbox;
    let on_server = |e: &&mailcraft_model::Event| e.origin.is_remote() && !e.origin.id.is_empty() && told(mb, e);
    let changed = before.iter().filter(on_server).filter(|e| mb.event(e.id).is_some_and(|now| now.categories != e.categories)).count();
    (changed > mailcraft_model::remote::MAX_EVENT_CHANGES)
        .then(|| failed!("This category is on more events of calendars kept on a server than can wait to be sent at once: nothing was changed."))
}

fn add_to_outlook(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::req_id(v, "account")?;
    s.mailbox.account(account).ok_or_else(|| bad("`account` isn't an account"))?;
    let name = p::req_str(v, "name")?;
    if s.mailbox.synced_list_owner(account).is_none() {
        return Err(super::fail("this account's categories aren't kept in step with a server"));
    }
    if !s.mailbox.send_category(account, name)? {
        return Err(failed!("\"{name}\" isn't a category kept on this computer only", name = name.trim()));
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
