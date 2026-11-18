// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A SOAP request in JSON and the reading of its answer.
//!
//! One request is a POST to `<server>/service/soap/<Name>Request` of
//! `{"Header": {"context": {"_jsns": "urn:zimbra", "format": {"type": "js"}, "authToken": …}},
//! "Body": {"<Name>Request": {"_jsns": "<namespace>", …}}}`; the answer is
//! `{"Body": {"<Name>Response": …}}`, or `{"Body": {"Fault": …}}`. The sign-in token goes in the
//! header's context, never in the address and never as a cookie (which would make the request one
//! a page in a browser could forge).

use serde_json::{Map, Value, json};

use crate::error::{Error, from_code};
use crate::http::{HttpRequest, HttpResponse, Method, SOAP_LIMIT, Secret};

/// The namespace of the account service (`AuthRequest`).
pub const ACCOUNT: &str = "urn:zimbraAccount";
/// The namespace of the mail service (folders, messages, sending).
pub const MAIL: &str = "urn:zimbraMail";

/// The SOAP document for `name` (`GetFolderRequest`) in namespace `ns`, with the elements of
/// `body` (an object; anything else counts as none), signed in with `token` when there is one.
pub fn envelope(name: &str, ns: &str, body: Value, token: Option<&Secret>) -> String {
    let mut request = match body {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    request.insert("_jsns".into(), json!(ns));
    let mut context = Map::new();
    context.insert("_jsns".into(), json!("urn:zimbra"));
    context.insert("format".into(), json!({"type": "js"}));
    if let Some(token) = token.filter(|t| !t.is_empty()) {
        context.insert("authToken".into(), json!({"_content": token.expose()}));
    }
    let mut operation = Map::new();
    operation.insert(name.to_string(), Value::Object(request));
    json!({"Header": {"context": Value::Object(context)}, "Body": Value::Object(operation)}).to_string()
}

/// The request for `name` in `ns` with `body`, to the server whose web client is at `base`.
pub fn request(base: &str, name: &str, ns: &str, body: Value, token: Option<&Secret>) -> Result<HttpRequest, Error> {
    let url = format!("{}/service/soap/{name}", base.trim_end_matches('/'));
    let fields = [("Content-Type", "application/json; charset=utf-8"), ("Accept", "application/json")];
    Ok(HttpRequest::new(Method::Post, &url, &fields, envelope(name, ns, body, token).into_bytes())?.with_response_limit(SOAP_LIMIT))
}

/// The `<Name>Response` element of the answer to `name`, or what went wrong. A fault is read for
/// its code alone; an answer that is not JSON, or has no body, or not the response asked for, is
/// [`Error::Unreadable`]; a busy server's answer (429, 503) is [`Error::Busy`].
pub fn answer(name: &str, response: &HttpResponse) -> Result<Value, Error> {
    if matches!(response.status(), 429 | 503) {
        return Err(Error::Busy(response.retry_after_seconds().unwrap_or(30).clamp(1, 3600)));
    }
    let parsed: Value = match serde_json::from_slice(response.body()) {
        Ok(parsed) => parsed,
        Err(_) if (200..300).contains(&response.status()) => return Err(Error::Unreadable),
        Err(_) => return Err(Error::Status(response.status())),
    };
    let body = parsed.get("Body").ok_or(Error::Unreadable)?;
    if let Some(fault) = body.get("Fault") {
        let code = fault.pointer("/Detail/Error/Code").and_then(Value::as_str).unwrap_or("");
        return Err(from_code(code));
    }
    if !(200..300).contains(&response.status()) {
        return Err(Error::Status(response.status()));
    }
    let wanted = format!("{}Response", name.strip_suffix("Request").unwrap_or(name));
    match body.get(&wanted) {
        Some(value @ Value::Object(_)) => Ok(value.clone()),
        _ => Err(Error::Unreadable),
    }
}
