// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! **Device limits** (MB3.16): what a device that can't hold a whole mailbox (a phone) keeps of
//! it ([`mailcraft_model::DeviceLimits`]). A computer has none and nothing here changes it.
//!
//! - `account.deviceLimits` reads them, sets them (a device that had none starts with a phone's
//!   defaults), or takes them away.

use mailcraft_model::device_limits::{MAX_MESSAGES, MAX_STORAGE, MIN_MESSAGES, MIN_STORAGE};
use serde_json::{Value, json};

use super::bad;
use crate::{CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("account.deviceLimits", "Mail Kept on This Device", "File › Info", device_limits)
            .pure()
            .params(r#"{"storage"?: number (bytes of mail kept on this device, 50 MB to 64 GB), "messages"?: number (messages kept on this device, 2000 to 500000), "defaults"?: number (the version of the phone's defaults the mailbox was moved to), "off"?: true (no limits: a computer's way)} — a device that can't hold a whole mailbox (a phone): mail older than an account's mail to keep offline isn't listed from the server at all, and past either cap the oldest mail goes back to its headers, then leaves this device (never the server, never mail not sent or uploaded yet). With nothing given, changes nothing. Answers {"on": bool, "storage": number, "messages": number, "defaults": number}"#),
    ]
}

/// What `account.deviceLimits` answers.
pub fn shown(s: &Session) -> Value {
    match s.mailbox.device_limits {
        Some(l) => json!({"on": true, "storage": l.storage, "messages": l.messages, "defaults": l.defaults}),
        None => json!({"on": false, "storage": 0, "messages": 0, "defaults": 0}),
    }
}

fn device_limits(s: &mut Session, v: &Value) -> CmdResult {
    if !v.is_object() && !v.is_null() {
        return Err(bad("the parameters are an object"));
    }
    if p::opt_bool(v, "off")? == Some(true) {
        if s.mailbox.device_limits.take().is_some() {
            s.mailbox_changed();
        }
        return Ok(shown(s));
    }
    let number = |k: &str, least: u64, most: u64| -> Result<Option<u64>, crate::CmdError> {
        match v.get(k) {
            None => Ok(None),
            Some(_) => p::u64(v, k)
                .filter(|n| (least..=most).contains(n))
                .map(Some)
                .ok_or_else(|| crate::CmdError::Params(format!("`{k}` is a whole number from {least} to {most}"))),
        }
    };
    let storage = number("storage", MIN_STORAGE, MAX_STORAGE)?;
    let messages = number("messages", MIN_MESSAGES, MAX_MESSAGES)?;
    let defaults = number("defaults", 0, u64::from(u32::MAX))?.and_then(|d| u32::try_from(d).ok());
    if storage.is_none() && messages.is_none() && defaults.is_none() {
        return Ok(shown(s));
    }
    let before = s.mailbox.device_limits;
    let mut l = before.unwrap_or_default();
    if let Some(n) = storage {
        l.storage = n;
    }
    if let Some(n) = messages {
        l.messages = n;
    }
    if let Some(n) = defaults {
        l.defaults = n;
    }
    let l = l.clamped();
    if before != Some(l) {
        s.mailbox.device_limits = Some(l);
        s.mailbox_changed();
    }
    Ok(shown(s))
}
