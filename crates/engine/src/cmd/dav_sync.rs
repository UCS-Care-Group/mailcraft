// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's side of keeping an account's calendars and contacts in step with a CalDAV and
//! CardDAV server (#209): what to ask (`sync.davRequest`), what came back (`sync.applyDav`), what
//! was changed here and goes up (`sync.davChanges`) and what the server made of it
//! (`sync.applyDavUploaded`). As for every server the engine does no I/O: a host with sockets does
//! the part in between.
//!
//! A skeleton for now: the four commands exist, so that the ribbon, the control channel and agents
//! know them, and refuse until what they do is built. They are the host's, never an agent's.

use serde_json::Value;

use super::fail;
use crate::{CmdResult, CommandSpec, Session};

/// What the four say while they have nothing to do yet.
const NOT_YET: &str = "Calendars and contacts on a server are not read or written yet in this version.";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.davRequest", "Send/Receive Calendars and Contacts Server", "Send / Receive", request)
            .pure()
            .params(r#"{"account"?: number} — answers what a host should read from an account's calendar and contacts server. Not built yet: it refuses."#)
            .when(available),
        CommandSpec::new("sync.applyDav", "Apply Calendars and Contacts Server", "Send / Receive", apply)
            .pure()
            .params(r#"{"result": DavResult} — takes in what a host read from a calendar and contacts server. Not built yet: it refuses."#)
            .when(super::dav_account::switched_on),
        CommandSpec::new("sync.davChanges", "Calendar and Contacts Changes to Send", "Send / Receive", changes)
            .pure()
            .params(r#"{"account"?: number} — answers what was changed here that a host should send to an account's calendar and contacts server. Not built yet: it refuses."#)
            .when(available),
        CommandSpec::new("sync.applyDavUploaded", "Apply Uploaded Calendar and Contacts Changes", "Send / Receive", uploaded)
            .pure()
            .params(r#"{"result": DavUploaded} — takes in what a calendar and contacts server made of changes sent to it. Not built yet: it refuses."#)
            .when(super::dav_account::switched_on),
    ]
}

/// Why the commands are out of reach, when they are: the feature is switched off, or no account
/// has a calendar and contacts server.
fn available(s: &Session) -> Option<&'static str> {
    super::dav_account::switched_on(s)
        .or_else(|| if s.mailbox.accounts.iter().any(|a| a.dav.is_some()) { None } else { Some("no account has a calendar and contacts server") })
}

fn request(_: &mut Session, _: &Value) -> CmdResult {
    Err(fail(NOT_YET))
}

fn apply(_: &mut Session, _: &Value) -> CmdResult {
    Err(fail(NOT_YET))
}

fn changes(_: &mut Session, _: &Value) -> CmdResult {
    Err(fail(NOT_YET))
}

fn uploaded(_: &mut Session, _: &Value) -> CmdResult {
    Err(fail(NOT_YET))
}
