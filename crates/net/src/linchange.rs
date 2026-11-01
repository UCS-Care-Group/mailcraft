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
mod tests {
    use super::*;
    use crate::fake::{FakeHttp, json_answer};

    #[test]
    fn an_approval_is_started_and_read() {
        let server = FakeHttp::answering(json_answer(
            "200 OK",
            r#"{"device_code":"device-secret","user_code":"ABCD-EFGH","verification_url":"https://mail.example.org/app-auth?code=ABCD-EFGH","expires_in":600,"interval":5}"#,
        ));
        let grant = start(&server.url("/"), "MailCraft on a laptop").unwrap();
        assert_eq!(grant.device_code, "device-secret");
        assert_eq!(grant.user_code, "ABCD-EFGH");
        assert!(grant.verification_url.starts_with("https://mail.example.org/app-auth"));
        let request = server.request(0);
        assert!(request.starts_with("POST /api/v1/auth/app-token HTTP/1.1\r\n"), "{request}");
        assert!(request.contains("MailCraft on a laptop"));
    }

    #[test]
    fn a_pending_approval_is_polled_until_the_password_comes() {
        let server = FakeHttp::start(|n, _| {
            if n == 0 {
                json_answer("400 Bad Request", r#"{"error":"authorization_pending"}"#)
            } else {
                json_answer("200 OK", r#"{"token":"lcapp_prefix_secret","username":"ada@example.org"}"#)
            }
        });
        let grant = AppTokenGrant {
            device_code: "device-secret".into(),
            user_code: "ABCD-EFGH".into(),
            verification_url: "https://mail.example.org/app-auth?code=ABCD-EFGH".into(),
            expires_in: 60,
            interval: 1,
        };
        let cancel = AtomicBool::new(false);
        let password = finish(&server.url("/"), &grant, &cancel).unwrap();
        assert_eq!(password, "lcapp_prefix_secret");
        assert_eq!(server.asked(), 2);
        assert!(server.request(0).contains("\"device_code\":\"device-secret\""));
    }

    #[test]
    fn cancelling_the_wait_stops_the_polling() {
        let server = FakeHttp::answering(json_answer("400 Bad Request", r#"{"error":"authorization_pending"}"#));
        let grant = AppTokenGrant {
            device_code: "device-secret".into(),
            user_code: "ABCD-EFGH".into(),
            verification_url: "https://mail.example.org/app-auth?code=ABCD-EFGH".into(),
            expires_in: 60,
            interval: 1,
        };
        let cancel = AtomicBool::new(true);
        assert!(matches!(finish(&server.url("/"), &grant, &cancel), Err(NetError::SignInCancelled)));
        assert_eq!(server.asked(), 0);
    }
}
