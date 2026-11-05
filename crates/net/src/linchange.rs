// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The browser approval a Linchange account goes through to obtain an application password.
//!
//! The account password and second factor are typed into Linchange's own page in the system
//! browser, never here. The client asks for a short approval, opens the page it is given, and
//! polls with its device code until the person has allowed it; the answer is an application
//! password, named for this computer, that is used as the password for IMAP, SMTP, CalDAV and
//! CardDAV. A compromised computer therefore never holds the account password.
//!
//! This blocks, so a host calls it on a background thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::NetError;
use crate::http::{self, Request as HttpRequest, Url};

/// How long the approval may take when the server doesn't say.
const DEFAULT_TTL: Duration = Duration::from_secs(10 * 60);
/// Longest the client waits between polls, whatever the server asks for.
const MAX_INTERVAL: Duration = Duration::from_secs(15);
/// Longest answer read.
const MAX_BODY: usize = 64 * 1024;
const PATIENCE: Duration = Duration::from_secs(20);

/// A short approval a mail client is waiting for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppTokenGrant {
    /// The secret only this client holds. It never goes to the browser.
    pub device_code: String,
    /// The short code the browser page names.
    pub user_code: String,
    /// The page in Linchange the person opens.
    pub verification_url: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Begin an approval for an application password. `server` is Linchange's web origin
/// (`https://mail.example.org`), and `description` is what the account page will show
/// (`MailCraft on the laptop`).
pub fn start(server: &str, description: &str) -> Result<AppTokenGrant, NetError> {
    let base = origin(server)?;
    let body = serde_json::json!({"description": description}).to_string();
    let reply = ask(&format!("{base}/api/v1/auth/app-token"), &body)?;
    if reply.0 != 200 {
        return Err(NetError::Proto(mailcraft_proto::ProtoError::Rejected(
            "the Linchange server wouldn't start an application-password approval".into(),
        )));
    }
    let json = serde_json::from_slice::<Value>(&reply.1).ok().filter(Value::is_object);
    let text = |name: &str| json.as_ref().and_then(|json| json.get(name)).and_then(Value::as_str).unwrap_or_default().to_string();
    let number = |name: &str| json.as_ref().and_then(|json| json.get(name)).and_then(Value::as_u64).unwrap_or(0);
    let grant = AppTokenGrant {
        device_code: text("device_code"),
        user_code: text("user_code"),
        verification_url: text("verification_url"),
        expires_in: number("expires_in"),
        interval: number("interval"),
    };
    if grant.device_code.is_empty() || grant.verification_url.is_empty() || grant.user_code.is_empty() {
        return Err(NetError::Proto(mailcraft_proto::ProtoError::Protocol("the Linchange server's approval answer is incomplete".into())));
    }
    // The page is the server's own: an answer may not send the owner's browser to another host.
    let base_url = Url::parse(&base)
        .map_err(|_| NetError::Proto(mailcraft_proto::ProtoError::Protocol("the Linchange server's address can't be used".into())))?;
    let page = Url::parse(grant.verification_url.trim()).ok().filter(|url| url.same_server(&base_url));
    if page.is_none() {
        return Err(NetError::Proto(mailcraft_proto::ProtoError::Protocol("the Linchange server's approval page is not on the server".into())));
    }
    Ok(grant)
}

/// Wait for the person to allow the approval, and return the application password.
///
/// `cancel` stops the wait; the caller opens `grant.verification_url` in the browser itself.
pub fn finish(server: &str, grant: &AppTokenGrant, cancel: &AtomicBool) -> Result<String, NetError> {
    let base = origin(server)?;
    let ttl = match grant.expires_in {
        0 => DEFAULT_TTL,
        seconds => Duration::from_secs(seconds.min(1_800)),
    };
    let interval = match grant.interval {
        0 => Duration::from_secs(5),
        seconds => Duration::from_secs(seconds.clamp(1, MAX_INTERVAL.as_secs())),
    };
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(NetError::SignInCancelled);
        }
        if started.elapsed() >= ttl {
            return Err(NetError::SignInTimedOut);
        }
        let body = serde_json::json!({"device_code": grant.device_code}).to_string();
        let (status, body) = ask(&format!("{base}/api/v1/auth/app-token/token"), &body)?;
        let json = serde_json::from_slice::<Value>(&body).ok();
        match status {
            200 => {
                let token = json.as_ref().and_then(|json| json.get("token")).and_then(Value::as_str).unwrap_or_default().to_string();
                if token.is_empty() {
                    return Err(NetError::Proto(mailcraft_proto::ProtoError::Protocol(
                        "the Linchange server's answer has no application password".into(),
                    )));
                }
                return Ok(token);
            }
            400 => {
                let why = json.as_ref().and_then(|json| json.get("error")).and_then(Value::as_str).unwrap_or_default();
                match why {
                    "authorization_pending" => {}
                    "access_denied" => {
                        return Err(NetError::Proto(mailcraft_proto::ProtoError::Rejected("the request was denied in the browser".into())));
                    }
                    "expired_token" => return Err(NetError::SignInTimedOut),
                    _ => {
                        return Err(NetError::Proto(mailcraft_proto::ProtoError::Rejected(
                            "the Linchange server wouldn't complete the application-password approval".into(),
                        )));
                    }
                }
            }
            _ => {
                return Err(NetError::Proto(mailcraft_proto::ProtoError::Rejected(format!(
                    "the Linchange server answered {status} to the approval"
                ))));
            }
        }
        // Sleep in short pieces, so Cancel is noticed promptly.
        let until = Instant::now() + interval;
        while Instant::now() < until {
            if cancel.load(Ordering::Relaxed) {
                return Err(NetError::SignInCancelled);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// The origin of `server`, without a trailing slash or path.
fn origin(server: &str) -> Result<String, NetError> {
    let url = Url::parse(server)?;
    if !url.path.trim_matches('/').is_empty() && url.path.trim_matches('/') != "api" {
        // A server address with a path is still allowed: the API paths are added to it.
    }
    let base = format!(
        "{}://{}{}",
        if url.secure { "https" } else { "http" },
        if url.host.contains(':') { format!("[{}]", url.host) } else { url.host.clone() },
        if url.port == 0 { String::new() } else { format!(":{}", url.port) }
    );
    let path = url.path.trim_end_matches('/');
    Ok(format!("{base}{path}"))
}

fn ask(url: &str, body: &str) -> Result<(u16, Vec<u8>), NetError> {
    let url = Url::parse(url)?;
    let headers = [("Content-Type", "application/json"), ("Accept", "application/json")];
    let request = HttpRequest { method: "POST", headers: &headers, body: Some(body.as_bytes()), patience: PATIENCE, max_body: MAX_BODY };
    let reply = http::request(&url, &request)?;
    Ok((reply.status, reply.body))
}

#[cfg(test)]
#[path = "linchange_tests.rs"]
mod tests;
