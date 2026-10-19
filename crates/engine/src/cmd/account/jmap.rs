//! The JMAP part of `account.add` and `account.edit`: what makes an account a JMAP account, and
//! the server it names.
//!
//! The address of the server is checked as text and nothing is looked up (`JmapServer::check_session`):
//! a mailbox file can say anything, so whatever connects checks it again.

use mailcraft_model::{Account, AccountKind, JmapServer};
use serde_json::Value;

use super::bad;
use crate::{CmdError, p};

/// Make `a` a JMAP account as the parameters `v` of the command say, or keep it one.
///
/// A JMAP account has no mail servers and no OAuth sign-in: its server is `jmap`, and the secret
/// it is opened with is given apart. A command that names the other things for it is refused
/// rather than quietly ignored. Only what the command says of the server is checked here: the
/// rest is the account's own.
pub(super) fn make(a: &mut Account, v: &Value) -> Result<(), CmdError> {
    for key in ["incoming", "outgoing"] {
        if v.get(key).is_some_and(|given| !given.is_null()) {
            return Err(bad(format!("a JMAP account has no `{key}` mail server: its server is `jmap`")));
        }
    }
    if p::str(v, "auth").is_some_and(|auth| matches!(auth.trim().to_ascii_lowercase().as_str(), "oauth2" | "oauth")) {
        return Err(bad("a JMAP account signs in with a password or a bearer token (`jmap.bearer`), not with OAuth 2.0"));
    }
    let mut server = a.jmap.clone().unwrap_or_default();
    match v.get("jmap") {
        // (Said to be nothing, it is not said: an account that has a server keeps it.)
        None | Some(Value::Null) => {}
        Some(Value::Object(given)) => {
            match given.get("session") {
                None => {}
                Some(Value::String(session)) => {
                    JmapServer::check_session(session).map_err(|why| bad(format!("`jmap.session` {why}")))?;
                    server.session = session.clone();
                }
                Some(_) => return Err(bad("`jmap.session` (string) is the address of the server's session resource")),
            }
            match given.get("bearer") {
                None => {}
                Some(Value::Bool(bearer)) => server.bearer = *bearer,
                Some(_) => return Err(bad("`jmap.bearer` is true (the secret is a bearer token) or false (a password)")),
            }
        }
        Some(_) => return Err(bad(r#"`jmap` is {"session": string, "bearer"?: boolean}"#)),
    }
    if server.session.is_empty() {
        return Err(bad("`jmap.session` (string) is required for a JMAP account: the address of the server's session resource"));
    }
    a.kind = AccountKind::Jmap;
    (a.incoming, a.outgoing, a.oauth) = (None, None, None);
    a.jmap = Some(server);
    Ok(())
}

/// After the kind is settled: only a JMAP account has the setting, and an account of any other
/// kind isn't given one.
pub(super) fn settle(a: &mut Account, v: &Value) -> Result<(), CmdError> {
    if a.kind == AccountKind::Jmap {
        return Ok(());
    }
    if v.get("jmap").is_some_and(|given| !given.is_null()) {
        return Err(bad(r#"`jmap` is only for an account whose `kind` is "jmap""#));
    }
    a.jmap = None;
    Ok(())
}
