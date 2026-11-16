// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account's calendar and contacts servers (CalDAV and CardDAV, #209, spec C4): the settings
//! ([`DavServer`], `Account::dav`) and the commands that set and clear them, and the host's side of
//! a synchronisation, which for now only refuses ([`super::dav_sync`]).
//!
//! The engine asks nothing of any server. `account.davSet` checks the settings as text, as
//! `account.edit` does an Exchange server's, and keeps them; nothing is sent until a host with
//! sockets is asked to look, which it does only for what these settings name. The password is
//! never a setting: the window holds it, per origin ([`DavServer::origins`]), and never lends one
//! origin's to another.
//!
//! Everything here ships switched off ([`Session::dav_enabled`], read by the host from
//! `MAILCRAFT_DAV=1`) until the feature is ready.

use mailcraft_model::{Account, AccountKind, DavServer};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("account.davSet", "Calendar and Contacts Server", "File › Info", set)
            .params(r#"{"id": number (an account), "calendarHomes"?: [string] (the https:// addresses of its CalDAV homes, at most 8), "contactHomes"?: [string] (its CardDAV homes), "username"?: string (the name that signs in; empty: the account's address), "trusted"?: string (the SHA-256 fingerprint of the one certificate trusted for these servers besides the system's), "separatePassword"?: bool (the password for them is not the account's own)} — keeps where the account's calendars and contacts are. Checked as text only: https, no user name in an address, not Microsoft's or Google's; nothing is sent. The password is never a setting: it is held per origin, and never borrowed from another. Another home, user name or certificate is another server: what was kept of the old one (each collection's sync state) is forgotten. Answers {"id", "origins": number, "changedServer": bool}"#)
            .when(switched_on),
        CommandSpec::new("account.davOff", "Stop Using Calendar and Contacts Server", "File › Info", off)
            .params(r#"{"id": number (an account)} — forgets where the account's calendars and contacts server is and what was kept of its collections; nothing is sent and nothing is removed from the server. Answers {"id", "off": bool (the account had one)}"#),
    ]
}

/// Why the feature is out of reach, when it is: it ships switched off.
pub(super) fn switched_on(s: &Session) -> Option<&'static str> {
    if s.dav_enabled { None } else { Some("isn't switched on yet") }
}

/// Whether an account of this kind may have calendar and contacts servers of its own: one that
/// has mail servers or a server of its own (IMAP, JMAP, Exchange server), not a Microsoft account
/// (Microsoft Graph keeps its calendars) nor one that only lives here, nor a shared mailbox
/// (which follows its owner's).
fn may_have(a: &Account) -> bool {
    !a.is_shared() && matches!(a.kind, AccountKind::Imap | AccountKind::Jmap | AccountKind::Exchange)
}

/// The list of addresses a command's `key` holds, over `before`, or an error naming it.
fn homes(v: &Value, key: &str, before: &[String]) -> Result<Vec<String>, CmdError> {
    match v.get(key) {
        None => Ok(before.to_vec()),
        Some(Value::Array(list)) => {
            if list.len() > DavServer::MAX_HOMES {
                return Err(bad(format!("`{key}` has at most {} addresses", DavServer::MAX_HOMES)));
            }
            let mut homes: Vec<String> = Vec::new();
            for entry in list {
                let Value::String(address) = entry else { return Err(bad(format!("`{key}` is a list of strings"))) };
                let address = address.trim().to_string();
                if !homes.contains(&address) {
                    homes.push(address);
                }
            }
            Ok(homes)
        }
        Some(_) => Err(bad(format!("`{key}` is a list of strings"))),
    }
}

fn set(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let account = s.mailbox.account(id).ok_or_else(|| failed!("no account with id {id}", id))?;
    if !may_have(account) {
        return Err(fail(
            "this account has no calendar and contacts server of its own: a Microsoft account keeps its calendars through Microsoft, and a shared mailbox follows its owner's",
        ));
    }
    let before = account.dav.clone();
    let mut server = before.clone().unwrap_or_default();
    server.calendar_homes = homes(v, "calendarHomes", &server.calendar_homes)?;
    server.contact_homes = homes(v, "contactHomes", &server.contact_homes)?;
    for (key, field) in [("username", &mut server.username), ("trusted", &mut server.trusted)] {
        match v.get(key) {
            None => {}
            Some(Value::String(text)) => *field = text.trim().to_string(),
            Some(_) => return Err(bad(format!("`{key}` is a string"))),
        }
    }
    match v.get("separatePassword") {
        None => {}
        Some(Value::Bool(separate)) => server.separate_password = *separate,
        Some(_) => return Err(bad("`separatePassword` is true or false")),
    }
    if let Some((setting, why)) = server.problem() {
        let name = match setting {
            "homes" => "`calendarHomes` and `contactHomes`",
            "username" => "`username`",
            _ => "`trusted`",
        };
        return Err(bad(format!("{name} {why}")));
    }
    // Another server is another place: what the old one said of its collections is not the new
    // one's. (A change of the password flag alone is no other server.)
    let changed = before.as_ref().is_some_and(|was| was.is_another_server(&server));
    if changed {
        server.collections.clear();
    }
    let origins = server.origins().len();
    s.mailbox.account_mut(id)?.dav = Some(server);
    if changed {
        super::dav_sync::forget_account(s, id);
    }
    s.dirty = true;
    Ok(json!({"id": id, "origins": origins, "changedServer": changed}))
}

fn off(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let account = s.mailbox.account_mut(id).map_err(|_| failed!("no account with id {id}", id))?;
    let had = account.dav.take().is_some();
    if had {
        super::dav_sync::forget_account(s, id);
        s.dirty = true;
    }
    Ok(json!({"id": id, "off": had}))
}
