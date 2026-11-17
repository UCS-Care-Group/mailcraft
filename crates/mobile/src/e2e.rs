// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The end-to-end run's one seam (MB3.15), in the `e2e` build alone: every call of the process
//! to Microsoft Graph and to the sign-in's token endpoint goes to the Graph stand-in
//! (`cargo run -p mailcraft-net --features stand-in --example standin -- --port 8780 --e2e`) on
//! the device's own loopback address, which `adb reverse` joins to the computer's. The sign-in
//! page is named at an address that is never found, for the instrumented test to play. Nothing
//! reaches Microsoft or any real account; a release build never has this module (the Android
//! build refuses to make one with `mailcraft.e2e` set).

// The token the stand-in takes: a made-up word, no service's.
use mailcraft_net::graph::standin::TOKEN;

use crate::CoreError;

/// Where the stand-in listens, on the device as on the computer (the port `adb reverse` joins).
pub(crate) const STAND_IN: &str = "http://127.0.0.1:8780";

/// Send the process's sign-ins and Graph to [`STAND_IN`]; before the core opens.
pub(crate) fn route() -> Result<(), CoreError> {
    if mailcraft_net::graph::standin::route_this_process(STAND_IN) {
        Ok(())
    } else {
        Err(CoreError::Init { reason: "the end-to-end build could not be pointed at the stand-in".into() })
    }
}

/// The most of the stand-in's answer that is read: Sent Items of a run holds a few messages.
const MOST_ANSWER: u64 = 1024 * 1024;
/// How long the question may take.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

/// [`crate::stand_in_sent_to`] in the end-to-end build: asks the stand-in, bounded.
pub(crate) fn sent_to(subject: &str) -> Result<Vec<String>, CoreError> {
    let failed = |what: &str| CoreError::Failed { reason: format!("the stand-in couldn't be asked: {what}") };
    let body = get("/v1.0/me/mailFolders/sentitems/messages?$select=subject,toRecipients&$top=50").map_err(|e| failed(&e))?;
    let json: serde_json::Value = serde_json::from_str(&body).map_err(|_| failed("its answer isn't JSON"))?;
    let list = json.get("value").and_then(serde_json::Value::as_array).ok_or_else(|| failed("its answer has no list"))?;
    let address = |r: &serde_json::Value| r.pointer("/emailAddress/address").and_then(serde_json::Value::as_str).map(str::to_string);
    Ok(list
        .iter()
        .filter(|m| m.get("subject").and_then(serde_json::Value::as_str).is_some_and(|s| s.contains(subject)))
        .flat_map(|m| m.get("toRecipients").and_then(serde_json::Value::as_array).into_iter().flatten().filter_map(address))
        .collect())
}

/// A GET to the stand-in: the body of a 200, else why not.
fn get(target: &str) -> Result<String, String> {
    use std::io::{Read, Write};
    let host = STAND_IN.trim_start_matches("http://");
    let address: std::net::SocketAddr = host.parse().map_err(|_| "no address".to_string())?;
    let mut stream = std::net::TcpStream::connect_timeout(&address, PATIENCE).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(PATIENCE)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(PATIENCE)).map_err(|e| e.to_string())?;
    let request = format!("GET {target} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    let mut whole = Vec::new();
    stream.take(MOST_ANSWER).read_to_end(&mut whole).map_err(|e| e.to_string())?;
    let whole = String::from_utf8_lossy(&whole);
    let (head, body) = whole.split_once("\r\n\r\n").ok_or_else(|| "an answer cut short".to_string())?;
    match head.split(' ').nth(1) {
        Some("200") => Ok(body.to_string()),
        other => Err(format!("status {}", other.unwrap_or("none"))),
    }
}
