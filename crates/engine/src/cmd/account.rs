// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Accounts, signatures and synchronising with a mail server.
//!
//! The engine never talks to a server: `sync.request` says what a synchronisation should do and
//! `sync.apply` takes in what it brought back (`mailcraft_model::sync`). A host with sockets
//! (`mailcraft-net`, in the desktop app and the CLI) does the part in between.

use std::collections::{HashMap, HashSet};

use mailcraft_model::sync::{
    Deleted, FlagsPushed, FolderDelta, FolderState, ImapPlace, MAX_PLACES, Moved, PLACE_BYTES, Placed, RemoteFlags, RemoteOp, SyncRequest,
    SyncResult, SyncStage, summary_line_in,
};
use mailcraft_model::{
    Account, AccountKind, Directory, EwsServer, FlagStatus, Folder, FolderKind, Id, JmapServer, MailIdentity, Mailbox, Message, OAuth, OAuthProvider,
    Pending, Queued, Security, Server, clean_name,
};
use serde_json::{Value, json};

use super::sync_progress::{self, Piece};
use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("account.add", "Add Account", "File › Info", add).params(ACCOUNT),
        CommandSpec::new("account.edit", "Account Settings", "File › Info", edit).params(r#"{"id": number, …the fields of account.add, "downloadInFull"?: "upTo10Mb"|"upTo25Mb"|"upTo50Mb"|"everything" (a Microsoft account: which messages are fetched whole in the background), "keepOffline"?: "3d"|"1w"|"2w"|"1m"|"3m"|"6m"|"12m"|"2y"|"5y"|"10y"|"all" (a Microsoft account's mail to keep offline: how far back, by received date, mail is kept in full on this computer; older mail keeps its headers and preview. A shared mailbox follows its owner's. Shortening it removes nothing by itself: account.thinOffline does), "askBeforeDeletingConversation"?: bool (Delete on a conversation's row that takes more than one message, in its folder or found by a search, asks first in the window; on unless turned off. A shared mailbox follows its owner's)}"#),
        CommandSpec::new("account.forgetEwsCertificate", "Forget Trusted Certificate", "File › Info", forget_ews_certificate)
            .params(r#"{"id": number} — an Exchange server account stops trusting the one certificate it was told to trust although the system doesn't: its fingerprint is forgotten, so the server's certificate is judged by the system alone and the owner is asked again if it isn't trusted. Answers {"id": id, "forgotten": bool}. Only while Exchange server accounts are being tested (MAILCRAFT_EWS=1)"#),
        CommandSpec::new("account.remove", "Remove Account", "File › Info", remove).params(r#"{"id": number} — removes the account, its mail and what is kept for it on this computer (bodies and attachments leave the content folder once a state without it is saved; no Undo step is kept), and what came from its server: its events, contacts, to-dos and calendars (what was made here and not sent yet stays, as this computer's own), Outlook's list of categories, and, where the window's settings are kept (the window, not mailcraft-cli), its mailbox's working hours; nothing on the server is touched. Mail moved into it from elsewhere that its server wasn't given yet goes back to where it came from, and so does mail moved into it whose original another server keeps until the copy is safe; once the copy is safe on its server, that original goes as it would have (mail moved onto this computer goes back to its original, as its copy goes with the account). The only account leaves an empty mailbox, as at a first start. Answers {"deleted": id, "leftToSweep": n (content files the sweep removes later)}"#),
        CommandSpec::new("account.list", "List Accounts", "File › Info", list).pure(),
        CommandSpec::new("account.preset", "Suggest Account Settings", "File › Info", preset)
            .pure()
            .params(r#"{"email": string, "provider"?: "microsoft", "service"?: "microsoft365"|"outlook.com"|"google"|"yahoo"|"icloud"|"fastmail"|"aol"|"zoho", "mx"?: [string] (the domain's mail exchangers, when the host looked them up)} — answers the servers and sign-in an address usually takes; changes nothing"#),
        CommandSpec::new("sync.request", "Send/Receive All Folders", "Send / Receive", request)
            .pure()
            .key("F9")
            .params(r#"{"account"?: number} — answers what a host should send and fetch"#)
            .when(has_server),
        CommandSpec::new("sync.apply", "Apply Send/Receive", "Send / Receive", apply).pure().params(r#"{"result": SyncResult}"#),
    ]
}

const ACCOUNT: &str = r#"{"email": string, "displayName"?: string, "name"?: string, "username"?: string, "incoming"?: {"host": string, "port"?: number, "security"?: "tls"|"starttls"|"none"} | null, "outgoing"?: same | null, "auth"?: "password"|"oauth2" (oauth2: sign in on the provider's page), "provider"?: "microsoft", "clientId"?: string, "tenant"?: string, "directory"?: "organisation"|"personal" (which kind of Microsoft account signs in, when the sign-in said), "kind"?: "imap"|"local"|"microsoft"|"jmap"|"exchange" (microsoft: kept in step through Microsoft Graph, no mail servers; jmap: kept in step over JMAP, no mail servers; exchange: an Exchange server account, kept in step through Exchange Web Services, no mail servers, only while Exchange server accounts are being tested: MAILCRAFT_EWS=1), "ews"?: {"url": string (the https:// address of the server's Exchange Web Services, `…/EWS/Exchange.asmx`), "username"?: string (DOMAIN\user or a UPN; default the e-mail address), "trusted"?: string (the SHA-256 fingerprint of the one certificate trusted for this account besides the system's)} (an Exchange server account's server; the password is never a setting), "jmap"?: {"session": string (the https:// address of the server's JMAP session resource), "bearer"?: bool (the secret is a bearer token, not the account's password)} (a JMAP account's server; the secret is never a setting)}"#;

fn has_server(s: &Session) -> Option<&'static str> {
    if s.mailbox.accounts.iter().any(|a| a.kind.has_server()) {
        None
    } else {
        Some("no account is connected to a mail server (File › Add Account)")
    }
}

fn server(v: &Value, key: &str) -> Result<Option<Option<Server>>, CmdError> {
    match v.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(x) => {
            let host: String = p::req_str(x, "host")?.trim().chars().take(253).collect();
            // A host name, not a URL: nothing that could smuggle a path, a user or a space.
            if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c)) {
                return Err(bad(format!("`{key}.host` isn't a host name")));
            }
            let security = match p::str(x, "security") {
                Some(sec) => Security::parse(sec).ok_or_else(|| bad("`security` is tls, starttls or none"))?,
                None => Security::Tls,
            };
            let port = p::u64(x, "port").unwrap_or(0);
            if port > 65_535 {
                return Err(bad("`port` is at most 65535"));
            }
            Ok(Some(Some(Server { host, port: port as u16, security })))
        }
    }
}

/// How the account's owner proves who they are: a password (asked for when it is needed, never
/// kept), or the provider's own sign-in page.
fn sign_in(a: &mut Account, v: &Value) -> Result<(), CmdError> {
    // An identifier, not a secret: what the provider knows this program by.
    let word = |k: &str, max: usize| -> Result<Option<String>, CmdError> {
        let Some(text) = p::str(v, k) else { return Ok(None) };
        let text = text.trim();
        if text.len() > max || !text.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c)) {
            return Err(bad(format!("`{k}` is letters, digits, dots and dashes")));
        }
        Ok(Some(text.to_string()))
    };
    match p::str(v, "auth").map(|x| x.trim().to_ascii_lowercase()) {
        None => {}
        Some(x) if x == "password" => a.oauth = None,
        Some(x) if x == "oauth2" || x == "oauth" => {
            if a.oauth.is_none() {
                a.oauth = Some(OAuth::default());
            }
        }
        Some(_) => return Err(bad("`auth` is password or oauth2")),
    }
    let provider = match p::str(v, "provider") {
        Some(name) => Some(OAuthProvider::parse(name).ok_or_else(|| bad("`provider` is microsoft"))?),
        None => None,
    };
    let (client_id, tenant) = (word("clientId", 200)?, word("tenant", 64)?);
    let directory = directory_of(v)?;
    if let Some(o) = a.oauth.as_mut() {
        if let Some(provider) = provider {
            o.provider = provider;
        }
        if let Some(client_id) = client_id {
            o.client_id = client_id;
        }
        if let Some(tenant) = tenant {
            o.tenant = tenant;
        }
        if let Some(directory) = directory {
            o.directory = directory;
        }
    }
    Ok(())
}

/// Which kind of Microsoft account a command says it is (`directory`), when it says.
fn directory_of(v: &Value) -> Result<Option<Directory>, CmdError> {
    match v.get("directory") {
        None | Some(Value::Null) => Ok(None),
        Some(_) => Ok(Some(p::str(v, "directory").and_then(Directory::parse).ok_or_else(|| bad("`directory` is organisation or personal"))?)),
    }
}

/// The servers and sign-in an address usually takes. Nothing is looked up here: the answer comes
/// from the address's domain, or from the domain's mail exchangers when a host that could ask the
/// DNS passes them in (`mx`), which is how a company's own domain is known to be, say, Microsoft's.
fn preset(_: &mut Session, v: &Value) -> CmdResult {
    use mailcraft_model::provider;
    let email: String = p::req_str(v, "email")?.trim().chars().take(254).collect();
    let domain = match email.rsplit_once('@') {
        Some((user, domain)) if !user.is_empty() && provider::is_domain(domain) => domain.to_ascii_lowercase(),
        _ => return Err(bad(format!("\"{email}\" isn't an e-mail address"))),
    };
    let asked = match p::str(v, "provider") {
        Some(name) => Some(OAuthProvider::parse(name).ok_or_else(|| bad("`provider` is microsoft"))?),
        None => None,
    };
    // The owner says whose the account is: that settles it.
    let named = match p::str(v, "service") {
        Some(name) => {
            Some(provider::by_name(name).ok_or_else(|| bad("`service` is microsoft365, outlook.com, google, yahoo, icloud, fastmail, aol or zoho"))?)
        }
        None => None,
    };
    let own = provider::by_domain(&domain);
    let exchangers = v.get("mx").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let by_mx = || exchangers.iter().take(20).filter_map(Value::as_str).find_map(provider::by_mx);
    let (found, via) = match (asked, own) {
        _ if named.is_some() => (named, "asked"),
        // Said to be Microsoft's: Outlook.com when the domain is, work or school mail otherwise.
        (Some(OAuthProvider::Microsoft), own) => {
            (Some(provider::microsoft(own.is_some_and(|p| p.oauth == Some(OAuthProvider::Microsoft) && p.service == "Outlook.com"))), "asked")
        }
        (None, Some(own)) => (Some(own), "domain"),
        (None, None) => (by_mx(), "mx"),
    };
    let known = found.is_some();
    let preset = found.unwrap_or_else(|| provider::guess(&domain));
    // In the words `account.add` takes.
    let server = |s: &Server| json!({"host": s.host, "port": s.port, "security": s.security.code()});
    Ok(json!({
        "known": known,
        "via": if known { via } else { "guess" },
        "service": preset.service,
        "auth": if preset.oauth.is_some() { "oauth2" } else { "password" },
        "provider": preset.oauth.map(OAuthProvider::code),
        "username": email,
        "incoming": server(&preset.incoming),
        "outgoing": server(&preset.outgoing),
        "note": preset.note,
    }))
}

fn apply_fields(a: &mut Account, v: &Value) -> Result<(), CmdError> {
    let line = |k: &str| p::str(v, k).map(|t| t.replace(['\r', '\n'], " ").trim().chars().take(254).collect::<String>());
    if let Some(e) = line("email") {
        a.email = e;
    }
    if let Some(n) = line("displayName") {
        a.display_name = n;
    }
    if let Some(n) = line("name") {
        a.name = n;
    }
    if let Some(u) = line("username") {
        a.username = u;
    }
    if let Some(i) = server(v, "incoming")? {
        a.incoming = i;
    }
    if let Some(o) = server(v, "outgoing")? {
        a.outgoing = o;
    }
    sign_in(a, v)?;
    // Which messages are fetched whole in the background (R3 spec S12).
    if let Some(limit) = p::str(v, "downloadInFull") {
        a.download_in_full =
            serde_json::from_value(json!(limit.trim())).map_err(|_| bad("`downloadInFull` is upTo10Mb, upTo25Mb, upTo50Mb or everything"))?;
    }
    // Mail to keep offline (#143).
    if let Some(keep) = p::str(v, "keepOffline") {
        a.keep_offline =
            mailcraft_model::KeepOffline::parse(keep).ok_or_else(|| bad("`keepOffline` is 3d, 1w, 2w, 1m, 3m, 6m, 12m, 2y, 5y, 10y or all"))?;
    }
    // Whether Delete on a whole conversation asks first (#503).
    match v.get("askBeforeDeletingConversation") {
        None => {}
        Some(Value::Bool(ask)) => a.skip_conversation_question = !ask,
        Some(_) => return Err(bad("`askBeforeDeletingConversation` is true or false")),
    }
    // A Microsoft account kept in step through Graph (R3), or an account kept in step over JMAP
    // (M6): asked for by name; one that is that already stays it. (One kept in step by IMAP becomes
    // one through Graph by switching: `account.graphSwitch`.) Otherwise the servers say: IMAP, or
    // local.
    let kind = match p::str(v, "kind").map(|k| k.trim().to_ascii_lowercase()) {
        None => a.kind,
        Some(k) if k == "microsoft" || k == "graph" => AccountKind::Microsoft,
        Some(k) if k == "jmap" => AccountKind::Jmap,
        Some(k) if k == "exchange" => AccountKind::Exchange,
        Some(k) if k == "imap" || k == "local" => AccountKind::Imap,
        Some(_) => return Err(bad("`kind` is imap, local, microsoft, jmap or exchange")),
    };
    match kind {
        AccountKind::Microsoft => {
            // Graph and nothing else: no mail servers, and a sign-in with Microsoft.
            a.kind = AccountKind::Microsoft;
            (a.incoming, a.outgoing, a.jmap, a.ews) = (None, None, None, None);
            let o = a.oauth.get_or_insert_with(OAuth::default);
            o.provider = OAuthProvider::Microsoft;
        }
        AccountKind::Jmap => {
            // JMAP and nothing else: no mail servers and no sign-in with a provider, but where the
            // server's session resource is (RFC 8620 §2) and how the secret goes.
            a.jmap = Some(jmap_server(v, a.jmap.as_ref())?);
            a.kind = AccountKind::Jmap;
            (a.incoming, a.outgoing, a.oauth, a.ews) = (None, None, None, None);
        }
        AccountKind::Exchange => {
            // Exchange Web Services and nothing else (#252): no mail servers and no sign-in with
            // a provider (a mailbox in Microsoft 365 is a Microsoft account), but where the
            // server's services are and who signs in.
            a.ews = Some(ews_server(v, a.ews.as_ref())?);
            a.kind = AccountKind::Exchange;
            (a.incoming, a.outgoing, a.oauth, a.jmap) = (None, None, None, None);
        }
        _ => {
            a.kind = if a.incoming.is_some() || a.outgoing.is_some() { AccountKind::Imap } else { AccountKind::Local };
            (a.jmap, a.ews) = (None, None);
        }
    }
    if a.name.is_empty() {
        a.name = a.email.clone();
    }
    Ok(())
}

/// A JMAP account's server from `jmap` in a command's params, over `before` (what it has, when it
/// has a server): `session`, the address of the session resource, and `bearer`, whether the secret
/// is a bearer token. An address that is given, or that a new server has, must be one the model
/// takes ([`JmapServer::check_session`]: `https://`, plain `http://` to this computer only, no
/// user name in it); nothing is sent to it. (One a mailbox file brought, and that is kept, is
/// refused when a run is asked for, so that the account can still be put right.)
fn jmap_server(v: &Value, before: Option<&JmapServer>) -> Result<JmapServer, CmdError> {
    let mut server = before.cloned().unwrap_or_default();
    let mut given_session = false;
    match v.get("jmap") {
        None => {}
        Some(given @ Value::Object(_)) => {
            match given.get("session") {
                None => {}
                Some(Value::String(session)) => (server.session, given_session) = (session.trim().to_string(), true),
                Some(_) => return Err(bad("`jmap.session` is a string")),
            }
            match given.get("bearer") {
                None => {}
                Some(Value::Bool(bearer)) => server.bearer = *bearer,
                Some(_) => return Err(bad("`jmap.bearer` is true or false")),
            }
        }
        Some(_) => return Err(bad("`jmap` is {\"session\": string, \"bearer\"?: bool}")),
    }
    if given_session || before.is_none() {
        JmapServer::check_session(&server.session).map_err(|why| bad(format!("`jmap.session` {why}")))?;
    }
    Ok(server)
}

/// An Exchange server account's server from `ews` in a command's params, over `before` (what it
/// has, when it has a server): `url`, the address of its Exchange Web Services, `username` and
/// `trusted`, the fingerprint of the one certificate trusted besides the system's. What is given,
/// or what a new server has, must be what the model takes ([`EwsServer::problem`]); nothing is
/// sent to it. (One a mailbox file brought, and that is kept, is refused when a run is asked for,
/// so that the account can still be put right.)
fn ews_server(v: &Value, before: Option<&EwsServer>) -> Result<EwsServer, CmdError> {
    let mut server = before.cloned().unwrap_or_default();
    let mut given = false;
    match v.get("ews") {
        None => {}
        Some(settings @ Value::Object(_)) => {
            for (key, field) in [("url", &mut server.url), ("username", &mut server.username), ("trusted", &mut server.trusted)] {
                match settings.get(key) {
                    None => {}
                    Some(Value::String(text)) => (*field, given) = (text.trim().to_string(), true),
                    Some(_) => return Err(bad(format!("`ews.{key}` is a string"))),
                }
            }
        }
        Some(_) => return Err(bad("`ews` is {\"url\": string, \"username\"?: string, \"trusted\"?: string}")),
    }
    if (given || before.is_none())
        && let Some(why) = server.problem()
    {
        let word = if EwsServer::check_url(&server.url).is_err() {
            "url"
        } else if EwsServer::check_username(&server.username).is_err() {
            "username"
        } else {
            "trusted"
        };
        return Err(bad(format!("`ews.{word}` {why}")));
    }
    Ok(server)
}

/// An account's address must be one, but for the account on this computer that has none and keeps
/// it so (`before`: what an edited account was), which holds imported mail, when nothing else of it
/// changes but its question before deleting a conversation (#503): it is no account to sign in with.
fn check_address(a: &Account, before: Option<&Account>) -> Result<(), CmdError> {
    let none_kept = before
        .is_some_and(|b| b.kind == AccountKind::Local && *a == Account { skip_conversation_question: a.skip_conversation_question, ..b.clone() });
    if !none_kept && !a.address().is_valid() {
        return Err(bad(format!("\"{}\" isn't an e-mail address", a.email)));
    }
    Ok(())
}

/// The account `account.add` would make from these params (for the question put to the owner
/// when an agent asks; `None` when they are not valid).
pub(crate) fn from_params(v: &Value) -> Option<Account> {
    let mut a = Account::default();
    apply_fields(&mut a, v).ok()?;
    check_address(&a, None).ok()?;
    Some(a)
}

/// The account `account.edit` would leave of `old` with these params (`None` when not valid).
pub(crate) fn edited(old: &Account, v: &Value) -> Option<Account> {
    let mut a = old.clone();
    apply_fields(&mut a, v).ok()?;
    check_address(&a, Some(old)).ok()?;
    Some(a)
}

/// Where an account's mail goes, in words: `imap host:port, smtp host:port`, or Microsoft.
pub fn servers_text(a: &Account) -> String {
    let one = |name: &str, s: &Option<Server>| match s {
        Some(s) if s.port == 0 => Some(format!("{name} {}", s.host)),
        Some(s) => Some(format!("{name} {}:{}", s.host, s.port)),
        None => None,
    };
    if a.kind == AccountKind::Microsoft {
        return "Microsoft".to_string();
    }
    if a.kind == AccountKind::Jmap {
        return match a.jmap.as_ref().filter(|server| server.problem().is_none()) {
            Some(server) => format!("jmap {}", jmap_origin(&server.session)),
            None => "jmap, no server address that can be used".to_string(),
        };
    }
    if a.kind == AccountKind::Exchange {
        return match a.ews.as_ref().filter(|server| server.problem().is_none()) {
            Some(server) => format!("exchange {}", jmap_origin(&server.url)),
            None => "exchange, no server address that can be used".to_string(),
        };
    }
    let parts: Vec<String> = [one("imap", &a.incoming), one("smtp", &a.outgoing)].into_iter().flatten().collect();
    if parts.is_empty() { "no server".to_string() } else { parts.join(", ") }
}

/// The scheme and the server, with its port, of a JMAP session address that may be used
/// (`https://mail.example.org:8443`): where the account connects, without the rest of the address,
/// which may hold the account's own (no user name can be in front of the server's name).
fn jmap_origin(session: &str) -> String {
    let Some((scheme, rest)) = session.split_once("://") else { return String::new() };
    let server = rest.split(['/', '?', '#']).next().unwrap_or_default();
    format!("{}://{}", scheme.to_ascii_lowercase(), server.to_ascii_lowercase())
}

/// What `account.list` says of a JMAP account's server: where it connects and whether the secret
/// is a bearer token, not the rest of the session's address (it may hold the account's own).
fn jmap_shown(a: &Account) -> Option<Value> {
    a.jmap.as_ref().map(|server| json!({"server": server.problem().is_none().then(|| jmap_origin(&server.session)), "bearer": server.bearer}))
}

/// What `account.list` says of an Exchange server account's server: where it connects and the
/// name it signs in with, not the rest of the address.
fn ews_shown(a: &Account) -> Option<Value> {
    a.ews.as_ref().map(|server| {
        json!({"server": server.problem().is_none().then(|| jmap_origin(&server.url)), "username": server.username, "trustsCertificate": !server.trusted.is_empty()})
    })
}

fn add(s: &mut Session, v: &Value) -> CmdResult {
    // Exchange server accounts are being tested (#252): not made while the switch is off.
    if !s.ews && p::str(v, "kind").is_some_and(|k| k.trim().eq_ignore_ascii_case("exchange")) {
        return Err(fail(super::ews_mail::SWITCHED_OFF));
    }
    // A new account keeps a year of mail in full (#143); one from before keeps all of it.
    let mut a = Account { keep_offline: mailcraft_model::KeepOffline::NEW_ACCOUNT, ..Default::default() };
    apply_fields(&mut a, v)?;
    check_address(&a, None)?;
    if s.mailbox.accounts.iter().any(|x| x.email.eq_ignore_ascii_case(&a.email)) {
        return Err(failed!("{address} is already set up", address = a.email));
    }
    if s.mailbox.accounts.len() >= 50 {
        return Err(fail("there are too many accounts"));
    }
    // The placeholder account of an empty mailbox makes way for the first real one.
    if let Some(placeholder) = s.mailbox.placeholder() {
        s.mailbox.remove_account(placeholder)?;
    }
    let id = s.mailbox.add_account(a);
    if s.mailbox.folder(s.view.folder).is_none() {
        s.view.folder = s.mailbox.folder_of_kind(id, FolderKind::Inbox).unwrap_or(0);
    }
    Ok(json!({"id": id}))
}

/// The answer to something only the newer MailCraft that added `a` does with it: an account of a
/// kind this version doesn't know ([`AccountKind::Unknown`]), kept as its file has it. `then` says
/// what happens instead, or what to do.
pub(crate) fn added_by_newer(a: &Account, then: &'static str) -> CmdError {
    failed!("{who} was added by a newer version of MailCraft, {then}", who = super::remote::who(a), then = mailcraft_i18n::Arg::words(then))
}

fn edit(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let mut a = s.mailbox.account(id).cloned().ok_or_else(|| failed!("no account with id {id}", id))?;
    // Its settings are written back as its file had them: a change here would be lost.
    if a.kind == AccountKind::Unknown {
        return Err(added_by_newer(&a, "which keeps its settings: change them there"));
    }
    let before = a.clone();
    // The settings of an Exchange server account (#252) are not changed while the switch is off.
    if !s.ews && (v.get("ews").is_some() || p::str(v, "kind").is_some_and(|k| k.trim().eq_ignore_ascii_case("exchange"))) {
        return Err(fail(super::ews_mail::SWITCHED_OFF));
    }
    apply_fields(&mut a, v)?;
    check_address(&a, Some(&before))?;
    // An Exchange server account is the same: its ids are the server's.
    if (before.kind == AccountKind::Exchange) != (a.kind == AccountKind::Exchange) {
        return Err(fail("an account becomes an Exchange server account, or stops being one, only by being removed and added again"));
    }
    // A JMAP account's folders and mail are known to its server by the ids it hands out, and any
    // other account's by paths and UIDs, or by nobody: an account doesn't turn from one to the other.
    if (before.kind == AccountKind::Jmap) != (a.kind == AccountKind::Jmap) {
        return Err(fail("an account becomes a JMAP account, or stops being one, only by being removed and added again"));
    }
    // And once its mail is here, those ids are one account's at one server and mailbox.
    let url = |x: &Account| x.ews.as_ref().filter(|server| server.problem().is_none()).map(|server| server.url.trim().to_string());
    // (The name that signs in, with the e-mail address when none is given: another name may
    // open another mailbox, and these ids are one mailbox's.)
    let login = |x: &Account| {
        let name = x.ews.as_ref().map(|server| server.username.trim()).filter(|name| !name.is_empty()).unwrap_or(x.email.trim());
        name.to_lowercase()
    };
    // Another server or another user name is another mailbox: where the folder list and the
    // folders went on from is the old one's, and is forgotten with it (below).
    let other_mailbox = a.kind == AccountKind::Exchange && (url(&before) != url(&a) || login(&before) != login(&a));
    if listed_ews(s, id) && a.kind == AccountKind::Exchange {
        let moved = url(&before).is_some_and(|was| Some(was) != url(&a));
        if moved || !before.email.trim().eq_ignore_ascii_case(a.email.trim()) || login(&before) != login(&a) {
            return Err(fail(
                "an Exchange server account's server, address and user name can't be changed once its mail is here: remove it and add the other one",
            ));
        }
    }
    // And once its mail is here, those ids are one account's at one session: another session
    // address, even on the same server, or another address of the owner's, may be another account
    // under the same ids. (A session address that can't be used, as a mailbox file may bring, may
    // be put right.)
    let listed = s.mailbox.folders.iter().any(|f| f.account == id && f.server.is_some());
    if listed && a.kind == AccountKind::Jmap {
        let session = |x: &Account| x.jmap.as_ref().filter(|server| server.problem().is_none()).map(|server| server.session.trim().to_string());
        let moved = session(&before).is_some_and(|was| Some(was) != session(&a));
        if moved || !before.email.trim().eq_ignore_ascii_case(a.email.trim()) {
            return Err(fail("a JMAP account's server and address can't be changed once its mail is here: remove it and add the other one"));
        }
    }
    // A shared mailbox is its address, read with its owner's sign-in (#70): another address
    // would be another mailbox under the same folders' ids.
    if before.is_shared() {
        if v.get("keepOffline").is_some() {
            return Err(fail("a shared mailbox keeps as much mail offline as the account it is opened through: change that account's"));
        }
        if v.get("askBeforeDeletingConversation").is_some() {
            return Err(fail("a shared mailbox asks before deleting a conversation as the account it is opened through does: change that account's"));
        }
        if !a.email.trim().eq_ignore_ascii_case(before.email.trim()) {
            return Err(fail("a shared mailbox's address can't be changed: remove it and add the other one"));
        }
        (a.email, a.kind, a.oauth, a.incoming, a.outgoing, a.username) = (before.email, before.kind, None, None, None, String::new());
        a.shared_of = before.shared_of;
    }
    if s.mailbox.accounts.iter().any(|x| x.id != id && x.email.eq_ignore_ascii_case(&a.email)) {
        return Err(failed!("{address} is already set up", address = a.email));
    }
    *s.mailbox.account_mut(id)? = a;
    if other_mailbox {
        forget_ews_states(&mut s.mailbox, id);
    }
    Ok(json!({"id": id}))
}

/// `account.forgetEwsCertificate`: the account no longer trusts the certificate it was told to trust (#252).
fn forget_ews_certificate(s: &mut Session, v: &Value) -> CmdResult {
    if !s.ews {
        return Err(fail(super::ews_mail::SWITCHED_OFF));
    }
    let id = p::req_id(v, "id")?;
    let a = s.mailbox.account_mut(id)?;
    if a.kind != AccountKind::Exchange {
        return Err(failed!("{name} isn't an Exchange server account", name = a.name));
    }
    let server = a.ews.as_mut().ok_or_else(|| fail("this Exchange server account has no server settings"))?;
    let forgotten = !server.trusted.is_empty();
    server.trusted.clear();
    Ok(json!({"id": id, "forgotten": forgotten}))
}

/// An Exchange server account's sync states belong to the mailbox they were given for: forgotten,
/// where the account's folder list went on from and where each of its folders did, so that the
/// next run reads from the start (#252).
fn forget_ews_states(mb: &mut mailcraft_model::Mailbox, account: Id) {
    if let Some(server) = mb.account_mut(account).ok().and_then(|a| a.ews.as_mut()) {
        server.hierarchy.clear();
    }
    for f in mb.folders.iter_mut().filter(|f| f.account == account) {
        if let Some(server) = f.server.as_mut() {
            server.sync_state.clear();
        }
    }
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    // The placeholder of an empty mailbox is all there is: there is no account to remove.
    if s.mailbox.placeholder() == Some(id) {
        return Err(fail("there is no account to remove"));
    }
    let left = remove_with_content(s, id)?;
    if s.mailbox.folder(s.view.folder).is_none() {
        s.view.folder = s.mailbox.default_account().and_then(|a| s.mailbox.folder_of_kind(a.id, FolderKind::Inbox)).unwrap_or(0);
    }
    Ok(json!({"deleted": id, "leftToSweep": left}))
}

/// Most bytes of content in memory one removal hashes to name the files it takes with it: what
/// is in memory is bounded by the session's budget already, and this keeps the window from
/// waiting on a large account. Messages past it leave their files to the sweep.
const HASH_BUDGET: u64 = 256 << 20;

/// What a message holds in memory (what hashing it costs).
fn in_memory(m: &Message) -> u64 {
    let body = m.body.text.len() as u64 + m.body.html.as_ref().map_or(0, |h| h.len() as u64) + m.hidden_text.len() as u64;
    body.saturating_add(m.attachments.iter().map(|a| a.data.len() as u64).sum::<u64>())
}

/// Remove account `id` (with the shared mailboxes opened through it) and what is kept on disk
/// for its mail (#177): the bodies and attachments that only its messages name go from the
/// mailbox's content folder once a state without the account is saved ([`crate::store::forget_when_saved`]),
/// not hours later with the sweep. Content another account's mail names too stays for it. What
/// came from their servers goes too (#325): their events, contacts and calendars
/// ([`super::remote::forget`]), Outlook's list of categories, and their mailbox's working hours
/// where the session holds the window's settings (`Session::run`).
/// No Undo step is kept (`Session::run`), as it would hold the mail on disk. The last account
/// makes way for a fresh placeholder, as at a first start (#313).
///
/// Answers how many content files were left to the sweep (hours later, the same result): those
/// of messages past [`HASH_BUDGET`] and names past the store's waiting list.
pub(crate) fn remove_with_content(s: &mut Session, id: Id) -> Result<usize, CmdError> {
    remove_within(s, id, HASH_BUDGET)
}

/// [`remove_with_content`] hashing at most `budget` bytes of content in memory.
pub(crate) fn remove_within(s: &mut Session, id: Id, budget: u64) -> Result<usize, CmdError> {
    let going: HashSet<Id> = s.mailbox.accounts.iter().filter(|a| a.id == id || a.shared_of == Some(id)).map(|a| a.id).collect();
    // Mail moved in that their servers haven't been given yet goes back to where it came from;
    // mail moved in whose original another server still has goes back to it, or lets it go.
    super::graph_place::put_back_waiting(&mut s.mailbox, |_, account, _| going.contains(&account));
    super::graph_place::settle_moves_into(s, &going);
    #[allow(unused_mut)]
    let mut left = 0usize;
    #[cfg(target_arch = "wasm32")]
    let _ = budget;
    #[cfg(not(target_arch = "wasm32"))]
    let refs: Vec<mailcraft_model::ContentRef> = if s.path.is_some() {
        let mut budget = budget;
        let mut refs = Vec::new();
        for (mid, m) in s.mailbox.messages.iter().filter(|(_, m)| going.contains(&m.account)) {
            let cost = in_memory(m);
            if cost > budget {
                left = left.saturating_add(1);
                continue;
            }
            budget -= cost;
            refs.extend(crate::store::content_names(*mid, m));
        }
        refs
    } else {
        Vec::new()
    };
    let gone: HashSet<Id> = s.mailbox.messages.iter().filter(|(_, m)| going.contains(&m.account)).map(|(mid, _)| *mid).collect();
    // What came from their servers goes with them, whoever removes them (#325): the window, the
    // CLI or an agent. Their events, contacts and calendars (a shared mailbox's too, #70: copies
    // of what its server keeps, which nothing would ever update again), in the mailbox's order:
    // which of their calendars is kept as the mailbox's own, when only theirs are left, doesn't
    // depend on a hash.
    let accounts: Vec<Id> = s.mailbox.accounts.iter().filter(|a| going.contains(&a.id)).map(|a| a.id).collect();
    // Outlook's list of categories, which is the mailbox's, from whichever Microsoft account read
    // it last: one that stays reads its own again with its contacts, which the window brings
    // every quarter of an hour.
    // (A shared mailbox reads no list of categories of its own: the owner's sign-in reads the
    // owner's.)
    let microsoft = s.mailbox.accounts.iter().any(|a| going.contains(&a.id) && a.kind == AccountKind::Microsoft && !a.is_shared());
    for account in accounts {
        super::remote::forget(s, account)?;
    }
    if microsoft {
        s.mailbox.outlook_categories.clear();
    }
    // (Their mailbox's working hours go once the removal has gone through: `Session::run`.)
    s.mailbox.remove_account(id)?;
    // A mailbox always has an account (#313). The placeholder goes in before `mailbox_changed`,
    // whose `normalize` moves the folder on screen to its Inbox and a message being written to
    // it: with no account yet, both would be left pointing at nothing.
    if s.mailbox.accounts.is_empty() {
        s.mailbox.add_account(Account::default());
    }
    s.let_go(&gone);
    s.mailbox_changed();
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = s.path.clone() {
        left = left.saturating_add(crate::store::forget_when_saved(&path, s.rev(), refs));
    }
    if left > 0 {
        log::info!("{left} content file(s) of a removed account are left to the sweep");
    }
    Ok(left)
}

fn list(s: &mut Session, _: &Value) -> CmdResult {
    let accounts: Vec<Value> = s
        .mailbox
        .accounts
        .iter()
        .map(|a| {
            json!({"id": a.id, "name": a.name, "email": a.email, "displayName": a.display_name, "kind": a.kind, "username": a.username, "incoming": a.incoming, "outgoing": a.outgoing, "signature": a.signature, "replySignature": a.reply_signature(), "auth": if a.oauth.is_some() { "oauth2" } else { "password" }, "oauth": a.oauth, "downloadInFull": a.download_in_full, "keepOffline": s.mailbox.keep_offline_of(a.id), "askBeforeDeletingConversation": s.mailbox.asks_before_deleting_conversation(a.id), "sharedOf": a.shared_of, "jmap": jmap_shown(a), "ews": ews_shown(a)})
        })
        .collect();
    Ok(json!({"accounts": accounts, "signatures": s.mailbox.signatures}))
}

/// Most changes one synchronisation is asked to push; a longer backlog goes up over several runs.
const MAX_OPS: usize = 2_000;
/// Most folders followed upwards from one that only exists here, to where the server has one.
const MAX_LEVELS: usize = 64;

/// What a synchronisation of `account` should do: send the Outbox, push what was changed here
/// since the server last heard (flags, moves, deletions), download what is new. The host adds
/// the password.
pub fn sync_request(s: &Session, account: Id) -> Result<SyncRequest, CmdError> {
    let a = s.mailbox.account(account).ok_or_else(|| failed!("no account with id {id}", id = account))?;
    if a.kind == AccountKind::Unknown {
        return Err(added_by_newer(a, "which sends and receives its mail: this version leaves it as it is"));
    }
    // A JMAP account is kept in step over JMAP alone (`sync.jmapRequest`), which neither the window
    // nor the command line runs yet: nothing is asked of IMAP or SMTP servers it doesn't have.
    if a.kind == AccountKind::Jmap {
        return Err(failed!("{name} is a JMAP account, and this version doesn't send and receive with one yet", name = a.name));
    }
    // So is an Exchange server account (`sync.ewsRequest`), where the switch is on, and it is
    // not run at all where it is off.
    if a.kind == AccountKind::Exchange {
        return Err(fail(super::ews_mail::refusal_text(s)));
    }
    if a.kind != AccountKind::Imap {
        return Err(failed!("{name} isn't connected to a mail server", name = a.name));
    }
    let outbox_folder = s.mailbox.folder_of_kind(account, FolderKind::Outbox);
    let mut known: HashMap<Id, Vec<u32>> = HashMap::new();
    let mut outbox = Vec::new();
    for m in s.mailbox.messages.values() {
        // Only what was queued with Send goes; a message moved here is held until Send Again (#607).
        if Some(m.folder) == outbox_folder && m.held.is_empty() {
            outbox.push(m.as_ref().clone());
        }
        if m.uid != 0 {
            known.entry(m.folder).or_default().push(m.uid);
        }
    }
    // What waits to be moved or deleted on the server is there still: known, so that it doesn't
    // come down again.
    for p in s.mailbox.pending.iter().filter(|p| p.account() == account) {
        let (from, uid) = p.source();
        if let Some(f) = s.mailbox.folders.iter().find(|f| uid != 0 && f.account == account && f.remote == from) {
            known.entry(f.id).or_default().push(uid);
        }
    }
    let mut again = super::imap_again::wanted(&s.mailbox, account);
    let folders = s
        .mailbox
        .folders
        .iter()
        .filter(|f| f.account == account)
        .map(|f| FolderState {
            id: f.id,
            remote: f.remote.clone(),
            kind: f.kind,
            uid_validity: f.uid_validity,
            uid_seen: f.uid_seen,
            uid_from: f.uid_from,
            known_uids: known.remove(&f.id).unwrap_or_default(),
            again: again.remove(&f.id).unwrap_or_default(),
        })
        .collect();
    let (ops, _) = remote_ops(s, account, MAX_OPS);
    let (places, _) = place_ops(s, account, MAX_PLACES);
    // What is sent goes whole: the files a message keeps on disk are read for it.
    let outbox = s.loaded_outbox(outbox);
    Ok(SyncRequest { account: a.clone(), folders, outbox, max_per_folder: s.prefs.sync_limit, ops, places, ..Default::default() })
}

/// How many changes made in `account` its server hasn't heard of yet: what a synchronisation
/// would push now. A host asks to bring the server up to date soon after a change rather than at
/// the next timed run. One pass over the messages, and nothing is built.
///
/// A change the server refused stays counted until it goes through, so a host must not answer
/// a count that doesn't fall by trying again at once.
pub fn unpushed(s: &Session, account: Id) -> usize {
    if s.mailbox.account(account).is_some_and(|a| a.kind == AccountKind::Microsoft) {
        return super::graph_mail::unpushed(s, account);
    }
    // An Exchange server account's changes are counted as a Microsoft account's (#252): folders,
    // messages, drafts. (Nothing is counted while the switch is off: nothing would go.)
    if s.mailbox.account(account).is_some_and(|a| a.kind == AccountKind::Exchange) {
        return if s.ews { super::graph_mail::unpushed(s, account) } else { 0 };
    }
    remote_ops(s, account, 0).1 + place_ops(s, account, 0).1
}

/// How many originals of messages moved out of `account` wait on its server for their message to
/// be safe where it is now (#451). [`unpushed`] counts one only from then on, as a deletion; one
/// that would never go (its message has left for an account that keeps no copy) isn't counted.
pub fn originals_waiting(s: &Session, account: Id) -> usize {
    let mb = &s.mailbox;
    let saved = s.file_is_current();
    let mailboxes: HashSet<&str> =
        mb.folders.iter().filter(|f| f.account == account && !f.remote.is_empty() && f.uid_validity != 0).map(|f| f.remote.as_str()).collect();
    let waits = |p: &Pending| {
        let Pending::Move { message, from, uid, .. } = p else { return false };
        // (Not one the server can't be told of: a run would never delete it.)
        let Some(m) = mb.message(*message).filter(|_| *uid != 0 && mailboxes.contains(from.as_str())) else { return false };
        mb.takes_over(account, m.account) && !super::graph_place::safe_here(s, m, saved, false)
    };
    mb.pending.iter().filter(|p| p.account() == account && waits(p)).count()
}

/// The flags of `m` that are not as the server has them (`base`), with what they are here; `None`
/// when all are.
fn flag_changes(m: &Message, base: RemoteFlags) -> Option<[Option<bool>; 3]> {
    let here = RemoteFlags::of(m, base.uid);
    let changed = |here: bool, there: bool| (here != there).then_some(here);
    let changes = [changed(here.read, base.read), changed(here.flagged, base.flagged), changed(here.answered, base.answered)];
    changes.iter().any(Option::is_some).then_some(changes)
}

/// Where a folder's messages go on the server: the folder's own path there, or, for a folder
/// that only exists here, the path of the nearest folder above it that the server has (empty:
/// the top level) and the names of the folders to make below that.
fn destination(mb: &Mailbox, folder: &Folder) -> (String, Vec<String>) {
    let mut create = Vec::new();
    let mut at = Some(folder);
    // A tree that loops must not be followed for ever.
    for _ in 0..MAX_LEVELS {
        let Some(f) = at else { break };
        if !f.remote.is_empty() {
            create.reverse();
            return (f.remote.clone(), create);
        }
        create.push(f.name.clone());
        at = f.parent.and_then(|p| mb.folder(p)).filter(|p| p.account == folder.account);
    }
    create.reverse();
    (String::new(), create)
}

/// The changes made in `account` that its server hasn't heard of: the moves and deletions in
/// the order they were made (so that a long run of flag changes doesn't hold them up), then the
/// flags that are not as the server last had them. At most `limit` of them are spelled out; the
/// number is of all there are.
///
/// A message moved out of the account into one that keeps it ([`Mailbox::takes_over`], #451)
/// is deleted here once it is safe there ([`super::graph_place::safe_here`]): made on that
/// account's server, or whole in the mailbox file on this computer. Until then nothing is asked
/// of the server for it.
///
/// A change is only asked for in a mailbox one of the account's folders follows with a
/// `UIDVALIDITY` (the UIDs kept of it are good for that life of the mailbox only).
fn remote_ops(s: &Session, account: Id, limit: usize) -> (Vec<RemoteOp>, usize) {
    let mb = &s.mailbox;
    let saved = s.file_is_current();
    let (mut ops, mut count) = (Vec::new(), 0usize);
    // (`None`: it can't go after all, which is looked at only for one that is spelled out.)
    let mut note = |op: &dyn Fn() -> Option<RemoteOp>| {
        if ops.len() < limit {
            let Some(op) = op() else { return };
            ops.push(op);
        }
        count += 1;
    };
    let folders: HashMap<Id, &Folder> = mb.folders.iter().filter(|f| f.account == account).map(|f| (f.id, f)).collect();
    let followed = |f: &&Folder| !f.remote.is_empty() && f.uid_validity != 0;
    let mailboxes: HashSet<&str> = folders.values().copied().filter(followed).map(|f| f.remote.as_str()).collect();
    for pending in mb.pending.iter().filter(|p| p.account() == account) {
        let (from, uid) = pending.source();
        if uid == 0 || !mailboxes.contains(from) {
            continue;
        }
        match pending {
            Pending::Delete { .. } => note(&|| Some(RemoteOp::Delete { from: from.to_string(), uid })),
            Pending::Move { message, synced, to, .. } => {
                let Some(m) = mb.message(*message) else { continue };
                if m.account != account {
                    // Moved out of the account: the original goes once the message is safe where it is.
                    // Every part of one on this computer is read back only when it goes into the
                    // request: not for a count, nor for what waits past the limit.
                    if mb.takes_over(account, m.account) && super::graph_place::safe_here(s, m, saved, false) {
                        note(&|| super::graph_place::safe_here(s, m, saved, true).then(|| RemoteOp::Delete { from: from.to_string(), uid }));
                    }
                    continue;
                }
                let Some(m) = Some(m).filter(|m| m.folder == *to && m.uid == 0) else { continue };
                // The Outbox is no place on a server: the move waits until the message is elsewhere.
                let Some(folder) = folders.get(to).copied().filter(|f| f.kind != FolderKind::Outbox) else { continue };
                // What was changed about the message before or after it was moved goes first, so
                // that it arrives in its new mailbox as it is here.
                if let Some([read, flagged, answered]) = synced.filter(|base| base.uid == uid).and_then(|base| flag_changes(m, base)) {
                    note(&|| Some(RemoteOp::Flags { folder: from.to_string(), uid, read, flagged, answered }));
                }
                note(&|| {
                    let (to, create) = destination(mb, folder);
                    Some(RemoteOp::Move { message: *message, from: from.to_string(), uid, to, to_folder: folder.id, create })
                });
            }
        }
    }
    for m in mb.messages.values() {
        let Some(base) = m.synced.filter(|base| m.uid != 0 && base.uid == m.uid && m.account == account) else { continue };
        let Some([read, flagged, answered]) = flag_changes(m, base) else { continue };
        let Some(folder) = folders.get(&m.folder).copied().filter(followed) else { continue };
        note(&|| Some(RemoteOp::Flags { folder: folder.remote.clone(), uid: m.uid, read, flagged, answered }));
    }
    (ops, count)
}

/// The messages waiting to be put on the account's server, moved or copied into its folders from
/// elsewhere ([`Queued::PlaceMessage`], #451): at most `limit` spelled out, each whole (its body
/// and files read in when they are kept on disk), and how many there are. One in the Outbox
/// waits until it is elsewhere.
fn place_ops(s: &Session, account: Id, limit: usize) -> (Vec<ImapPlace>, usize) {
    let mb = &s.mailbox;
    let (mut out, mut count, mut spent) = (Vec::new(), 0usize, 0u64);
    for queued in &mb.queued {
        let Queued::PlaceMessage { account: a, message, marker, .. } = queued else { continue };
        if *a != account {
            continue;
        }
        // (Not one whose text couldn't be read here: the server's original is all there is of it.)
        let whole = |m: &Message| !m.unfetched && !m.incomplete && m.not_downloaded == 0 && !m.kept.as_deref().is_some_and(|k| k.failed);
        let Some(m) = mb.message(*message).filter(|m| m.account == account && m.uid == 0 && whole(m)) else { continue };
        let Some(folder) = mb.folder(m.folder).filter(|f| f.account == account && f.kind != FolderKind::Outbox) else { continue };
        count += 1;
        if out.len() >= limit || (!out.is_empty() && spent.saturating_add(m.size()) > PLACE_BYTES) {
            continue;
        }
        spent = spent.saturating_add(m.size());
        match s.loaded_message(m.id) {
            Ok(whole) => {
                let (to, create) = destination(mb, folder);
                out.push(ImapPlace { message: whole.as_ref().clone(), to, to_folder: folder.id, create, marker: marker.clone() });
            }
            Err(e) => log::warn!("message {} waits to go to the server: {e}", m.id),
        }
    }
    (out, count)
}

fn request(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::id(v, "account")
        .or_else(|| s.mailbox.accounts.iter().find(|a| a.kind.has_server()).map(|a| a.id))
        .ok_or_else(|| fail("no account is connected to a mail server"))?;
    // One kept in step through Microsoft Graph has a request of its own (`sync.graphRequest`).
    if s.mailbox.account(account).is_some_and(|a| a.kind == AccountKind::Microsoft) {
        let mut req = serde_json::to_value(super::graph_mail::graph_mail_request(s, account)?).map_err(|e| fail(e.to_string()))?;
        req["graph"] = json!(true);
        return Ok(req);
    }
    // So has one kept in step over JMAP (`sync.jmapRequest`): every mailbox is read.
    if s.mailbox.account(account).is_some_and(|a| a.kind == AccountKind::Jmap) {
        s.jmap_read.remove(&account);
        let mut req = serde_json::to_value(super::jmap_mail::jmap_mail_request(s, account, false)?).map_err(|e| fail(e.to_string()))?;
        req["jmap"] = json!(true);
        return Ok(req);
    }
    // So has one of an Exchange server account (`sync.ewsRequest`).
    if s.mailbox.account(account).is_some_and(|a| a.kind == AccountKind::Exchange) {
        return Err(fail(super::ews_mail::refusal_text(s)));
    }
    let req = sync_request(s, account)?;
    serde_json::to_value(&req).map_err(|e| fail(e.to_string()))
}

/// A folder name from a server path when the server's list didn't give one: the last part of
/// `INBOX/Projects/Apollo`, and of `Archive.2025` on a server that parts names with dots. A name
/// with no slash that reads like an address or a host (`ada@example.org`) is one name, not a path.
fn leaf_name(remote: &str) -> String {
    let leaf = match remote.rsplit_once('/') {
        Some((_, leaf)) if !leaf.trim().is_empty() => leaf,
        _ if remote.contains('@') => remote,
        _ => remote.rsplit('.').find(|p| !p.trim().is_empty()).unwrap_or(remote),
    };
    clean_name(leaf).unwrap_or_else(|| "Folder".into())
}

/// What a folder was called by the versions that took the last part after any dot or slash,
/// which cut `ada@example.org` down to `org`: a folder still called that was never renamed by
/// its owner, and takes the name the server's list gives it.
fn cut_name(remote: &str) -> String {
    let leaf = remote.rsplit(['/', '.']).find(|p| !p.trim().is_empty()).unwrap_or(remote);
    clean_name(leaf).unwrap_or_else(|| "Folder".into())
}

/// Take in what a synchronisation brought back. Not an undo step: the server is the source of
/// truth, except for what was changed here and hasn't reached it yet.
///
/// - Changes the server took on leave [`Mailbox::pending`]; a message it moved has its UID
///   again when the server said it.
/// - Flags merge three ways ([`merged_flags`]): a flag changed here since the server last heard,
///   also one changed while the synchronisation ran, stays and goes up next time.
/// - The server's copy of a message that is here already (one that was sent, or moved by a
///   server that didn't say where to) is matched to it instead of being added.
/// - Nothing taken out here because the server no longer has it is ever told back to the server
///   as a deletion, and a change that didn't go through keeps waiting.
/// - When the server was told to move or delete something, or mail was sent, the undo history is
///   forgotten: an undo would bring back a mailbox the server no longer has, or send again. A
///   read mark or a flag going up doesn't cost the history (opening a message would otherwise
///   wipe it every few seconds); undoing one after it went up is overruled by the server's copy
///   at the next run.
///
/// An answer may come in pieces ([`SyncResult::partial`]), each taken in as it comes, in order:
/// see [`apply_sync_part`], which also keeps count over all of them. Taking in a piece on its own
/// here is as good, but for the status line at the end, which then speaks of the last piece only.
pub fn apply_sync(s: &mut Session, result: SyncResult) -> CmdResult {
    apply_sync_part(s, &mut SyncTotals::default(), result)
}

/// What the pieces of one synchronisation came to, for a host that takes its answer in piece by
/// piece ([`apply_sync_part`]): start with an empty one for every run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncTotals {
    /// Messages that are really new: not the server's copy of one that was here.
    pub new: usize,
    pub sent: usize,
    pub removed: usize,
    pub updated: usize,
    pub matched: usize,
    pub pushed: usize,
    /// Every problem the run told of, in the order they came.
    pub errors: Vec<String>,
    /// The run was called off before it was through.
    pub stopped: bool,
    /// The last piece is in: the run is over.
    pub finished: bool,
}

impl SyncTotals {
    /// How the run went, in one line: its first problem, else what it brought (`3 new messages,
    /// 1 sent`, `Up to date`), or that it was stopped and what it had brought by then.
    pub fn summary(&self) -> String {
        self.summary_in(mailcraft_i18n::Lang::English)
    }

    /// [`summary`](Self::summary) in `lang`.
    pub fn summary_in(&self, lang: mailcraft_i18n::Lang) -> String {
        let line = summary_line_in(lang, self.errors.first().map(String::as_str), self.new, self.sent);
        match (self.stopped && self.errors.is_empty(), self.new + self.sent) {
            (false, _) => line,
            (true, 0) => lang.t("Stopped").to_string(),
            (true, _) => mailcraft_i18n::fmt(lang.t("Stopped ({line})"), &[("line", &line)]),
        }
    }
}

/// [`apply_sync`] for an answer that comes in pieces, as `mailcraft_net::sync_with` hands it
/// over: call it with each piece, in the order they come, and the same `run`, which adds up what
/// they changed.
///
/// While pieces are still to come, the status line is the piece's own line on how far the run
/// has got ([`SyncResult::progress`]); with the last piece it says how the whole run went
/// ([`SyncTotals::summary`]) and when. A piece with nothing to take in (a line of progress, a
/// problem to tell) leaves the mailbox as it is, and unsaved changes are not claimed for it.
///
/// Every piece is whole in itself: when the pieces stop coming (the run was called off, the
/// program is closed), what was taken in is as good as a run that ended there.
///
/// Answers what this piece changed, with the run's line (`summary`) and its sums so far (`total`).
pub fn apply_sync_part(s: &mut Session, run: &mut SyncTotals, mut result: SyncResult) -> CmdResult {
    let account = result.account;
    let Some(a) = s.mailbox.account(account) else { return Err(failed!("no account with id {id}", id = account)) };
    // No run here asks a server anything of it, so nothing that comes back is for it.
    if a.kind == AccountKind::Unknown {
        return Err(added_by_newer(a, "which sends and receives its mail: this version leaves it as it is"));
    }
    // An answer for another account (the mailbox was replaced while the run was under way) would
    // tie this one's messages to a stranger's server.
    if !result.email.is_empty() && !a.email.trim().eq_ignore_ascii_case(result.email.trim()) {
        return Err(failed!("this Send/Receive was for another account, not for {name}", name = a.name));
    }
    // A host that says only the numbers has its line made of them.
    let progress = match (&result.step, result.progress.is_empty()) {
        (Some(step), true) => step.line_in(s.interface_language),
        _ => result.progress.clone(),
    };
    let (partial, stopped) = (result.partial, result.stopped);
    let pushed = result.pushed_count();
    let (counts, sent) = if result.has_news() { take_in_news(s, account, &mut result) } else { (Counts::default(), 0) };
    // Looking at a folder says which of how many it is.
    let folders = result.step.as_ref().filter(|step| step.stage == SyncStage::Checking).map(|step| (step.done, step.total));
    let piece = Piece {
        step: result.step.as_ref().map(sync_progress::imap_step),
        folders,
        new: counts.new,
        sent,
        problems: &result.errors,
        last: !partial,
        stopped,
        timings: Some(&result.timings),
    };
    s.sync_progress.take(account, piece);
    run.new += counts.new;
    run.sent += sent;
    run.removed += counts.removed;
    run.updated += counts.updated;
    run.matched += counts.matched;
    run.pushed += pushed;
    run.errors.extend(result.errors.iter().cloned());
    run.stopped |= stopped;
    run.finished = !partial;
    if !partial {
        for said in super::graph_place::unreadable_waiting(s, account) {
            if !run.errors.contains(&said) {
                run.errors.push(said);
            }
        }
    }
    let summary = run.summary();
    if !partial {
        let t = s.local_now();
        s.sync_status = format!("{} — {}", run.summary_in(s.interface_language), s.time_said(t));
    } else if !progress.is_empty() {
        s.sync_status = progress.clone();
    }
    Ok(json!({
        "new": counts.new,
        "sent": sent,
        "removed": counts.removed,
        "updated": counts.updated,
        "matched": counts.matched,
        "pushed": pushed,
        "errors": result.errors,
        "summary": summary,
        "partial": partial,
        "progress": progress,
        "stopped": stopped,
        "total": {
            "new": run.new,
            "sent": run.sent,
            "removed": run.removed,
            "updated": run.updated,
            "matched": run.matched,
            "pushed": run.pushed,
            "errors": run.errors,
        },
    }))
}

/// Take in what a result has for the mailbox. Returns what that changed, and how many messages
/// were sent.
fn take_in_news(s: &mut Session, account: Id, result: &mut SyncResult) -> (Counts, usize) {
    let told =
        !result.moved.is_empty() || !result.deleted.is_empty() || !result.sent.is_empty() || !result.placed.is_empty() || !result.unplaced.is_empty();
    let mb = &mut s.mailbox;
    let sent = file_sent(mb, account, result);
    for made in &result.created {
        // A folder that only existed here has its mailbox on the server now.
        let free = !made.remote.is_empty() && !mb.folders.iter().any(|f| f.account == account && f.remote == made.remote);
        if let (true, Ok(f)) = (free, mb.folder_mut(made.folder))
            && f.account == account
            && f.remote.is_empty()
        {
            f.remote = made.remote.clone();
        }
    }
    let mut on_server = OnServer::of(mb, account);
    flags_pushed(mb, account, &on_server, &result.flagged);
    for moved in &result.moved {
        move_finished(mb, account, &mut on_server, moved);
    }
    deletes_finished(mb, account, &mut on_server, &result.deleted);
    places_finished(mb, account, &mut on_server, &result.placed, &result.unplaced);
    on_server.expect(mb, account);
    adopt_listed(mb, account, &mut on_server, &result.listed);
    // Only what is really new counts as new: not the server's copy of a message that was here.
    let mut counts = Counts::default();
    for delta in std::mem::take(&mut result.folders) {
        take_in(mb, account, &mut on_server, delta, &mut counts);
    }
    moved_in_for_good(mb, account);
    result.errors.append(&mut counts.notes);
    if told {
        s.forget_history();
    }
    // Not through `run` when the host calls this, so say it here: the lists are out of date.
    s.mailbox_changed();
    (counts, sent)
}

/// What taking in a result changed.
#[derive(Default)]
struct Counts {
    new: usize,
    removed: usize,
    updated: usize,
    /// Messages that came down and were the server's copy of one already here.
    matched: usize,
    /// What the owner is to be told of (a mailbox the server numbered anew).
    notes: Vec<String>,
}

/// What is known, while a result is taken in, about where the server has the account's messages.
struct OnServer {
    /// The message with a UID in a folder.
    by_uid: HashMap<(Id, u32), Id>,
    /// Folder by the server path it follows.
    by_remote: HashMap<String, Id>,
    /// Messages known to be in their folder's mailbox whose UID there isn't known yet
    /// ([`Message::synced`] with `uid` 0), by folder.
    expected: HashMap<Id, Vec<Id>>,
    /// Server copies a move or deletion made here is still waiting to be pushed for: they are
    /// not new mail, whatever a download says.
    leaving: HashSet<(Id, u32)>,
    /// Messages put back as the server's after a refused copy ([`Message::returned`]).
    returned: Vec<Id>,
}

impl OnServer {
    fn of(mb: &Mailbox, account: Id) -> OnServer {
        let by_uid = mb.messages.values().filter(|m| m.account == account && m.uid != 0).map(|m| ((m.folder, m.uid), m.id)).collect();
        let by_remote = mb.folders.iter().filter(|f| f.account == account && !f.remote.is_empty()).map(|f| (f.remote.clone(), f.id)).collect();
        let returned = mb.messages.values().filter(|m| m.account == account && m.returned > 0).map(|m| m.id).collect();
        OnServer { by_uid, by_remote, expected: HashMap::new(), leaving: HashSet::new(), returned }
    }

    /// Work out `expected` and `leaving`, once the finished changes are taken in.
    fn expect(&mut self, mb: &Mailbox, account: Id) {
        for m in mb.messages.values().filter(|m| m.account == account && m.uid == 0 && m.synced.is_some()) {
            self.expected.entry(m.folder).or_default().push(m.id);
        }
        for pending in mb.pending.iter().filter(|p| p.account() == account) {
            let (from, uid) = pending.source();
            if let Some(folder) = self.by_remote.get(from) {
                self.leaving.insert((*folder, uid));
            }
        }
    }
}

/// Sent messages leave the Outbox for Sent Items. One with a copy in the server's mailbox for
/// sent mail (`filed`) is marked as expected there, so that the copy is matched to it when it
/// comes down. That it was filed may be said later than that it was sent (a piece of the answer
/// each): a message of the account's that is in Sent Items and not on the server yet is marked
/// all the same. One whose copy didn't get to the server after all (`unfiled`) is no longer
/// expected there. Returns how many were sent.
fn file_sent(mb: &mut Mailbox, account: Id, result: &SyncResult) -> usize {
    let (Some(outbox), Some(sent_items)) = (mb.folder_of_kind(account, FolderKind::Outbox), mb.folder_of_kind(account, FolderKind::Sent)) else {
        return 0;
    };
    let mut moved = 0;
    for id in &result.sent {
        // The server took it: what it answers or forwards is marked so now (#154).
        if mb.message(*id).is_some_and(|m| m.account == account) {
            mb.mark_answered(*id);
        }
        if mb.message(*id).is_some_and(|m| m.account == account && m.folder == outbox) && mb.move_message(*id, sent_items).is_ok() {
            moved += 1;
        }
    }
    // The server's copy is marked as read.
    let (there, not_there) = (Some(RemoteFlags { read: true, ..Default::default() }), None);
    for (ids, was, is) in [(&result.filed, not_there, there), (&result.unfiled, there, not_there)] {
        for id in ids {
            let waits = |m: &Message| m.account == account && m.folder == sent_items && m.uid == 0 && m.synced == was;
            if mb.message(*id).is_some_and(waits)
                && let Ok(m) = mb.message_mut(*id)
            {
                m.synced = is;
            }
        }
    }
    moved
}

/// Flag changes the server took on: they are what the server has now, whatever was changed here
/// meanwhile (which then differs from it, and goes up next time).
fn flags_pushed(mb: &mut Mailbox, account: Id, on_server: &OnServer, pushed: &[FlagsPushed]) {
    for change in pushed {
        let took = |flags: &mut RemoteFlags| {
            flags.read = change.read.unwrap_or(flags.read);
            flags.flagged = change.flagged.unwrap_or(flags.flagged);
            flags.answered = change.answered.unwrap_or(flags.answered);
        };
        let message = on_server.by_remote.get(&change.folder).and_then(|folder| on_server.by_uid.get(&(*folder, change.uid)));
        if let Some(id) = message {
            let Some(mut synced) = mb.message(*id).and_then(|m| m.synced) else { continue };
            took(&mut synced);
            if mb.message(*id).is_some_and(|m| m.synced != Some(synced))
                && let Ok(m) = mb.message_mut(*id)
            {
                m.synced = Some(synced);
            }
        } else {
            // A message that was moved away since: its move remembers what the server has of it.
            let waiting = mb.pending.iter_mut().find(|p| p.account() == account && p.source() == (change.folder.as_str(), change.uid));
            if let Some(Pending::Move { synced: Some(synced), .. }) = waiting {
                took(synced);
            }
        }
    }
}

/// A move the server finished. Usually the message is where it was moved to, waiting: it gets
/// its UID there when the server said it, and is marked as expected there otherwise. When the
/// owner did something else with it while the synchronisation ran (moved it on, deleted it for
/// good, put it back with an undo), that is turned into a change from where the server has the
/// message now, when the server said where that is; when it didn't, the server's copy comes
/// down as a message of its own.
fn move_finished(mb: &mut Mailbox, account: Id, on_server: &mut OnServer, moved: &Moved) {
    // Where the server has it now: a UID that can be trusted is one of the mailbox the folder
    // follows today, which no message here has.
    let there = mb
        .folder(moved.to_folder)
        .filter(|f| f.account == account && !f.remote.is_empty() && moved.new_uid != 0 && moved.uid_validity != 0)
        .filter(|f| (f.uid_validity == 0 || f.uid_validity == moved.uid_validity) && !on_server.by_uid.contains_key(&(f.id, moved.new_uid)))
        .map(|f| (f.remote.clone(), moved.new_uid));
    let waiting = mb.pending.iter().position(|p| p.account() == account && p.source() == (moved.from.as_str(), moved.uid));
    let waiting = waiting.filter(|at| *at < mb.pending.len()).map(|at| mb.pending.remove(at));
    match (waiting, there) {
        (Some(Pending::Move { message, synced, to, .. }), there) if to == moved.to_folder => {
            let Ok(m) = mb.message_mut(message) else { return };
            if m.folder != to || m.uid != 0 {
                return;
            }
            match there {
                Some((_, uid)) => {
                    m.uid = uid;
                    m.synced = synced.map(|flags| flags.at(uid));
                    on_server.by_uid.insert((to, uid), message);
                    // The folder's mailbox may be new: its UIDs are good for this life of it.
                    if let Ok(f) = mb.folder_mut(to)
                        && f.uid_validity == 0
                    {
                        f.uid_validity = moved.uid_validity;
                    }
                }
                None => m.synced = Some(synced.unwrap_or_else(|| RemoteFlags::of(m, 0)).at(0)),
            }
        }
        // Moved on meanwhile: the next run takes it from where the server has it now.
        (Some(Pending::Move { message, synced, to, .. }), Some((from, uid))) => {
            mb.pending.push(Pending::Move { account, message, from, uid, synced: synced.map(|flags| flags.at(uid)), to });
        }
        // Deleted for good meanwhile: the next run deletes it where the server has it now.
        (Some(Pending::Delete { message_id, seen, .. }), Some((from, uid))) => {
            mb.pending.push(Pending::Delete { account, from, uid, message_id, seen })
        }
        // Put back meanwhile: the next run moves it back on the server too.
        (None, Some((from, uid))) => {
            let back = on_server.by_remote.get(&moved.from).copied().filter(|folder| *folder != moved.to_folder);
            let Some((folder, id)) = back.and_then(|folder| on_server.by_uid.get(&(folder, moved.uid)).map(|id| (folder, *id))) else { return };
            let Ok(m) = mb.message_mut(id) else { return };
            let synced = m.synced.take().map(|flags| flags.at(uid));
            m.uid = 0;
            on_server.by_uid.remove(&(folder, moved.uid));
            mb.pending.push(Pending::Move { account, message: id, from, uid, synced, to: folder });
        }
        _ => {}
    }
}

/// Deletions the server finished leave the list. A message that is here again all the same (an
/// undo while the synchronisation ran) is the only copy there is now: it stays, as a message
/// the server doesn't have.
fn deletes_finished(mb: &mut Mailbox, account: Id, on_server: &mut OnServer, deleted: &[Deleted]) {
    if deleted.is_empty() {
        return;
    }
    let finished: HashSet<(&str, u32)> = deleted.iter().map(|d| (d.from.as_str(), d.uid)).collect();
    // A deletion asked for, or the original of a message moved out of the account (#451); a
    // run deletes nothing else. One moved back into the account meanwhile, into another folder,
    // has no copy on the server now: it goes there again.
    let back: Vec<Id> = mb
        .pending
        .iter()
        .filter(|p| p.account() == account && finished.contains(&p.source()))
        .filter_map(|p| match p {
            Pending::Move { message, .. } => mb.message(*message).filter(|m| m.account == account).map(|m| m.id),
            Pending::Delete { .. } => None,
        })
        .collect();
    mb.pending.retain(|p| !(p.account() == account && finished.contains(&p.source())));
    for id in back {
        mb.place_again(id);
    }
    for gone in deleted {
        let here = on_server.by_remote.get(&gone.from).and_then(|folder| on_server.by_uid.remove(&(*folder, gone.uid)));
        let Some(id) = here else { continue };
        if let Ok(m) = mb.message_mut(id) {
            m.uid = 0;
            m.synced = None;
        }
        // Here all the same (moved back, or undone, while it went): it goes on the server again.
        mb.place_again(id);
    }
}

/// Messages put on the server ([`SyncRequest::places`], #451). One the server took is in its
/// folder's mailbox now: under the UID the server said, when that can be trusted (as
/// [`move_finished`] trusts one); expected there otherwise, so that its copy is matched to it
/// when it comes down. One moved on in the account meanwhile is moved there from where the
/// server has it; one moved away, deleted or undone meanwhile has its copy deleted there again.
/// One the server wouldn't take goes back to where it came from ([`super::graph_place::put_back`]).
fn places_finished(mb: &mut Mailbox, account: Id, on_server: &mut OnServer, placed: &[Placed], unplaced: &[Id]) {
    let waiting = |mb: &Mailbox, id: Id| {
        mb.queued.iter().position(|q| matches!(q, Queued::PlaceMessage { account: a, message, .. } if *a == account && *message == id))
    };
    for done in placed {
        // Where the server has it now: a UID that can be trusted is one of the mailbox the folder
        // follows today, which no message here has.
        let there = mb
            .folder(done.to_folder)
            .filter(|f| f.account == account && !f.remote.is_empty() && done.uid != 0 && done.uid_validity != 0)
            .filter(|f| (f.uid_validity == 0 || f.uid_validity == done.uid_validity) && !on_server.by_uid.contains_key(&(f.id, done.uid)))
            .map(|f| (f.remote.clone(), done.uid));
        let entry = waiting(mb, done.message);
        let here = mb.message(done.message).filter(|m| entry.is_some() && m.account == account && m.uid == 0).map(|m| m.folder);
        match here {
            Some(folder) if folder == done.to_folder => {
                let marker = match entry.filter(|at| *at < mb.queued.len()).map(|at| mb.queued.remove(at)) {
                    Some(Queued::PlaceMessage { marker, .. }) => marker,
                    _ => String::new(),
                };
                let Ok(m) = mb.message_mut(done.message) else { continue };
                let uid = there.map_or(0, |(_, uid)| uid);
                // As the server was given it: what changed here since goes up next.
                m.synced = Some(done.flags.at(uid));
                if uid == 0 {
                    // The server didn't say where: expected there, as the message carrying the
                    // copy's marker.
                    m.placed = marker;
                    continue;
                }
                m.uid = uid;
                m.placed.clear();
                on_server.by_uid.insert((folder, uid), done.message);
                // The folder's mailbox may be new: its UIDs are good for this life of it.
                if let Ok(f) = mb.folder_mut(folder)
                    && f.uid_validity == 0
                {
                    f.uid_validity = done.uid_validity;
                }
            }
            // Moved on in the account meanwhile: the next run takes it from where the server has it.
            Some(to) => {
                let Some((from, uid)) = there else { continue };
                if let Some(at) = entry.filter(|at| *at < mb.queued.len()) {
                    mb.queued.remove(at);
                }
                original_goes(mb, done.message);
                let synced = Some(done.flags.at(uid));
                mb.pending.push(Pending::Move { account, message: done.message, from, uid, synced, to });
            }
            // Called off meanwhile: the copy goes again.
            None => {
                if let Some((from, uid)) = there {
                    mb.pending.push(Pending::Delete { account, from, uid, message_id: String::new(), seen: None });
                }
            }
        }
    }
    for id in unplaced {
        let Some(at) = waiting(mb, *id).filter(|at| *at < mb.queued.len()) else { continue };
        let (copy, from) = match mb.queued.remove(at) {
            Queued::PlaceMessage { copy, from, .. } => (copy, from),
            _ => (false, None),
        };
        super::graph_place::put_back(mb, *id, copy, from);
    }
}

/// Messages moved into the account out of another IMAP account that are this server's messages
/// now (they have their UID here, #451): the original goes from the other server, by a deletion
/// there, whatever is done with them here from now on.
fn moved_in_for_good(mb: &mut Mailbox, account: Id) {
    let ids: Vec<Id> = mb
        .pending
        .iter()
        .filter_map(|p| match p {
            Pending::Move { account: owner, message, .. } if *owner != account => Some(*message),
            _ => None,
        })
        .filter(|id| mb.message(*id).is_some_and(|m| m.account == account && m.uid != 0))
        .collect();
    for id in ids {
        original_goes(mb, id);
    }
}

/// The move out of another account that waits for `id`, by which its original goes once the
/// message is safe where it is now, becomes a deletion of that original: it is.
pub(crate) fn original_goes(mb: &mut Mailbox, id: Id) {
    let Some(m) = mb.message(id) else { return };
    let (account, message_id, seen) = (m.account, m.message_id.clone(), (!m.message_id.is_empty()).then(|| MailIdentity::of(m)));
    for p in mb.pending.iter_mut() {
        if let Pending::Move { account: owner, message, from, uid, .. } = p
            && *message == id
            && *owner != account
        {
            let delete =
                Pending::Delete { account: *owner, from: std::mem::take(from), uid: *uid, message_id: message_id.clone(), seen: seen.clone() };
            *p = delete;
        }
    }
}

/// What a message's read state, follow-up flag and answered mark become when the server says
/// `theirs`. Each is taken on its own: one that was changed here since the server last heard
/// (it differs from [`Message::synced`]) stays as it is here, to go up with the next run; any
/// other follows the server. A message without `synced` has nothing of its own yet.
fn merged_flags(m: &Message, theirs: RemoteFlags) -> (bool, FlagStatus, bool) {
    let here = RemoteFlags::of(m, m.uid);
    let base = m.synced.unwrap_or(here);
    let read = if here.read == base.read { theirs.read } else { m.read };
    let flag = match (theirs.flagged, m.flag) {
        _ if here.flagged != base.flagged => m.flag,
        (true, FlagStatus::None) => FlagStatus::Flagged,
        (false, FlagStatus::Flagged) => FlagStatus::None,
        // A flag that is done with stays done, whatever the server shows.
        (_, keep) => keep,
    };
    let answered = if here.answered == base.answered { theirs.answered } else { m.answered };
    (read, flag, answered)
}

/// Give message `id` the flags the server has for it (`theirs`), merged with what was changed
/// here. Returns whether anything the owner sees changed.
fn merge_flags(mb: &mut Mailbox, id: Id, theirs: RemoteFlags) -> bool {
    let Some(m) = mb.message(id) else { return false };
    let theirs = theirs.at(m.uid);
    let (read, flag, answered) = merged_flags(m, theirs);
    let seen = (m.read, m.flag, m.answered, m.draft) != (read, flag, answered, theirs.draft);
    if (seen || m.synced != Some(theirs))
        && let Ok(m) = mb.message_mut(id)
    {
        (m.read, m.flag, m.answered, m.draft, m.synced) = (read, flag, answered, theirs.draft, Some(theirs));
    }
    seen
}

/// The Message-ID a message was sent with, when a server gave it another on the way and kept the
/// first in a header of its own (Microsoft's do that to mail sent through them).
fn original_id(m: &Message) -> Option<&str> {
    let value = m.headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("X-Microsoft-Original-Message-ID")).map(|(_, value)| value.as_str())?;
    Some(value.trim().trim_start_matches('<').trim_end_matches('>').trim()).filter(|id| !id.is_empty())
}

/// Whether two messages are one by what they say of themselves: sent at the same second, about
/// the same thing, from and to the same people. For a copy that came back without the
/// Message-ID it left with.
fn same_mail(a: &Message, b: &Message) -> bool {
    MailIdentity::of(a).fits(b)
}

/// The message here that `theirs`, just come down into `folder`, is the server's copy of: one
/// expected in that mailbox ([`OnServer::expected`]) with the same Message-ID, or, failing that
/// (a server may give what it files an id of its own), one that is the same mail by its date,
/// subject and people. It takes the UID, and its flags merge with the server's. Mail that merely
/// looks like a message that was never on the server matches nothing.
fn matched(mb: &mut Mailbox, on_server: &mut OnServer, folder: Id, theirs: &Message) -> Option<Id> {
    if theirs.uid == 0 {
        return None;
    }
    let expected = on_server.expected.get_mut(&folder)?;
    let original = original_id(theirs);
    let same_id = |m: &Message| !m.message_id.is_empty() && (m.message_id == theirs.message_id || Some(m.message_id.as_str()) == original);
    let waiting = |id: &Id| mb.message(*id).filter(|m| m.folder == folder && m.uid == 0 && m.synced.is_some());
    // A copy put there from elsewhere is known by its marker alone: mail that merely looks like
    // it may be the server's own (#451).
    let carries =
        |m: &Message| theirs.headers.iter().any(|(name, value)| name.eq_ignore_ascii_case(mailcraft_mime::PLACE_HEADER) && *value == m.placed);
    let by_id = expected.iter().position(|id| waiting(id).is_some_and(|m| if m.placed.is_empty() { same_id(m) } else { carries(m) }));
    let at = by_id.or_else(|| expected.iter().position(|id| waiting(id).is_some_and(|m| m.placed.is_empty() && same_mail(m, theirs))))?;
    let id = expected.swap_remove(at);
    let m = mb.message_mut(id).ok()?;
    m.uid = theirs.uid;
    m.placed.clear();
    if !theirs.message_id.is_empty() {
        // The name the server knows it by, which a reply to it will carry.
        m.message_id = theirs.message_id.clone();
    }
    merge_flags(mb, id, RemoteFlags::of(theirs, theirs.uid));
    on_server.by_uid.insert((folder, theirs.uid), id);
    Some(id)
}

/// A server mailbox numbered anew (its UIDVALIDITY changed): the UIDs kept of it mean nothing
/// any more, and what came from it goes, to come back as the server has it. What was changed
/// here and not pushed yet is kept, to be found again by Message-ID as the mailbox comes down
/// afresh: a message whose flags were changed stays, without a UID, until its new copy is
/// matched to it ([`matched`]); a move or a deletion waits with UID 0 for the copy it is about
/// ([`waited_for`]). What has no Message-ID to be found by can't be kept, and the owner is told.
/// Returns the messages that go.
fn renumbered(mb: &mut Mailbox, account: Id, on_server: &mut OnServer, folder: Id, remote: &str, counts: &mut Counts) -> Vec<Id> {
    let (mut kept, mut lost) = (0usize, 0usize);
    let mut pending = std::mem::take(&mut mb.pending);
    pending.retain_mut(|p| {
        if p.account() != account || p.source().0 != remote {
            return true;
        }
        let findable = match &*p {
            Pending::Move { message, .. } => mb.message(*message).is_some_and(|m| !m.message_id.is_empty()),
            Pending::Delete { message_id, seen, .. } => !message_id.is_empty() && seen.is_some(),
        };
        match p {
            Pending::Move { uid, synced, .. } if findable => (*uid, *synced) = (0, synced.map(|flags| flags.at(0))),
            Pending::Delete { uid, .. } if findable => *uid = 0,
            _ => lost += 1,
        }
        kept += usize::from(findable);
        findable
    });
    mb.pending = pending;
    on_server.leaving.retain(|(f, _)| *f != folder);
    let here: Vec<(u32, Id)> = on_server.by_uid.iter().filter(|((f, _), _)| *f == folder).map(|((_, uid), id)| (*uid, *id)).collect();
    let mut gone = Vec::new();
    for (uid, id) in here {
        on_server.by_uid.remove(&(folder, uid));
        let changed = mb.message(id).and_then(|m| Some((m.synced.and_then(|base| flag_changes(m, base))?, !m.message_id.is_empty())));
        match changed {
            Some((_, true)) => {
                if let Ok(m) = mb.message_mut(id) {
                    m.uid = 0;
                    m.synced = m.synced.map(|flags| flags.at(0));
                }
                on_server.expected.entry(folder).or_default().push(id);
                kept += 1;
            }
            Some((_, false)) => {
                lost += 1;
                gone.push(id);
            }
            None => gone.push(id),
        }
    }
    if kept + lost > 0 {
        let name = mb.folder(folder).map_or(remote, |f| f.name.as_str());
        let mut note = format!("The server numbered the messages in “{name}” anew.");
        if kept > 0 {
            note += &format!(" {kept} change(s) made here will be made again as its messages come back.");
        }
        if lost > 0 {
            note += &format!(" {lost} change(s) made here could not be kept: the message could not be found again for certain.");
        }
        counts.notes.push(note);
    }
    gone
}

/// Whether `theirs`, come down from server mailbox `remote` after it was numbered anew, is the
/// copy a move or a deletion made here waits for ([`renumbered`]): it then gets its UID, and the
/// next run pushes it. A Message-ID is the sender's to choose, so the change goes only to the
/// one arrival with that id that is also the same mail (date, sender, subject and recipients).
/// When several arrivals share the id, or the one that does is not that mail, the change is
/// dropped, and the owner told: it must never reach another message. `shared` is how many
/// arrivals of this run have `theirs`'s Message-ID.
fn waited_for(mb: &mut Mailbox, account: Id, remote: &str, theirs: &Message, shared: usize, counts: &mut Counts) -> bool {
    if theirs.message_id.is_empty() {
        return false;
    }
    let messages = &mb.messages;
    let at = mb.pending.iter().position(|p| {
        p.account() == account
            && p.source() == (remote, 0)
            && match p {
                Pending::Move { message, .. } => messages.get(message).is_some_and(|m| m.message_id == theirs.message_id),
                Pending::Delete { message_id, .. } => *message_id == theirs.message_id,
            }
    });
    let Some(at) = at else { return false };
    let is_it = shared == 1
        && match mb.pending.get(at) {
            Some(Pending::Move { message, .. }) => messages.get(message).is_some_and(|m| MailIdentity::of(m).fits(theirs)),
            Some(Pending::Delete { seen, .. }) => seen.as_ref().is_some_and(|seen| seen.fits(theirs)),
            None => false,
        };
    if !is_it {
        mb.pending.remove(at);
        counts.notes.push(
            "A change made here to a message could not be matched to its new copy for certain (more than one message has its Message-ID, or the copy differs), so it was not made on the server."
                .to_string(),
        );
        return false;
    }
    match mb.pending.get_mut(at) {
        Some(Pending::Move { uid, synced, .. }) => (*uid, *synced) = (theirs.uid, synced.map(|flags| flags.at(theirs.uid))),
        Some(Pending::Delete { uid, .. }) => *uid = theirs.uid,
        None => return false,
    }
    true
}

/// The local folder of a server mailbox that has none yet: the standard folder of its kind when
/// that follows no mailbox, else a new one named after the last part of the path.
fn folder_for(mb: &mut Mailbox, account: Id, remote: &str, kind: FolderKind) -> Option<Id> {
    let standard = if kind == FolderKind::Custom { None } else { mb.folder_of_kind(account, kind) };
    if let Some(id) = standard.filter(|id| mb.folder(*id).is_some_and(|f| f.remote.is_empty())) {
        return Some(id);
    }
    let base = leaf_name(remote);
    // A name already taken (two server folders with the same last part) gets a number.
    (1..=50).find_map(|n| mb.add_folder(account, None, &if n == 1 { base.clone() } else { format!("{base} ({n})") }).ok())
}

/// Give every folder of mail the server lists a folder here, in the same tree, before any of its
/// mail has come: the standard folder of its kind when that follows no mailbox yet, else a new
/// folder under the one its parent has here. A folder that is here already stays as it is,
/// except one at the top whose place in the tree is now known (folders moved here are not moved
/// on the server yet, so the server's tree is the one that holds).
fn adopt_listed(mb: &mut Mailbox, account: Id, on_server: &mut OnServer, listed: &[mailcraft_model::sync::ListedFolder]) {
    // Parents before their children, however the server ordered them.
    let depth = |f: &mailcraft_model::sync::ListedFolder| {
        let (mut above, mut n) = (f.parent.as_str(), 0);
        while !above.is_empty() && n < 32 {
            above = listed.iter().find(|p| p.remote == above).map_or("", |p| p.parent.as_str());
            n += 1;
        }
        n
    };
    let mut order: Vec<&mailcraft_model::sync::ListedFolder> = listed.iter().take(2000).filter(|f| !f.remote.is_empty()).collect();
    order.sort_by_cached_key(|f| depth(f));
    for f in order {
        let parent = on_server.by_remote.get(&f.parent).copied().filter(|_| !f.parent.is_empty());
        match on_server.by_remote.get(&f.remote).copied() {
            Some(id) => {
                // A name an earlier version cut short at a dot is put right.
                let listed_as = clean_name(&f.name).filter(|name| !name.contains('/'));
                if let (Some(name), Some(here)) = (listed_as, mb.folder(id))
                    && here.kind == FolderKind::Custom
                    && here.name != name
                    && here.name == cut_name(&f.remote)
                {
                    let _ = mb.rename_folder(id, &name);
                }
                let at_top = mb.folder(id).is_some_and(|here| here.parent.is_none() && here.kind == FolderKind::Custom);
                if let Some(p) = parent
                    && at_top
                    && !mb.subtree(id).contains(&p)
                {
                    let _ = mb.move_folder(id, Some(p));
                }
            }
            None => {
                let standard = if f.kind == FolderKind::Custom { None } else { mb.folder_of_kind(account, f.kind) };
                let standard = standard.filter(|id| mb.folder(*id).is_some_and(|here| here.remote.is_empty()));
                let base = clean_name(&f.name).unwrap_or_else(|| leaf_name(&f.remote));
                // A name already taken beside it gets a number.
                let made = standard.or_else(|| {
                    (1..=50).find_map(|n| mb.add_folder(account, parent, &if n == 1 { base.clone() } else { format!("{base} ({n})") }).ok())
                });
                if let Some(id) = made {
                    if let Ok(here) = mb.folder_mut(id) {
                        here.remote = f.remote.clone();
                    }
                    on_server.by_remote.insert(f.remote.clone(), id);
                }
            }
        }
    }
}

/// Take in what changed in one server mailbox.
fn take_in(mb: &mut Mailbox, account: Id, on_server: &mut OnServer, mut delta: FolderDelta, counts: &mut Counts) {
    if delta.remote.is_empty() {
        return;
    }
    let follower = on_server.by_remote.get(&delta.remote).copied();
    // The folder the news was worked out against, when it is the one to follow this mailbox.
    let fits = |f: &&Folder| f.account == account && follower.unwrap_or(f.id) == f.id && (f.remote.is_empty() || f.remote == delta.remote);
    let folder = match delta.id {
        // Worked out against a folder that is gone by now, or follows another mailbox: its news
        // would only be part of the story. The next run reads the mailbox from the start.
        Some(id) => match mb.folder(id).filter(fits) {
            Some(f) => f.id,
            None => return,
        },
        None => match follower.or_else(|| folder_for(mb, account, &delta.remote, delta.kind)) {
            Some(id) => id,
            None => return,
        },
    };
    let (old_validity, old_seen, was_whole) = mb.folder(folder).map_or((0, 0, false), |f| (f.uid_validity, f.uid_seen, f.read_whole()));
    let reset = old_validity != 0 && delta.uid_validity != 0 && old_validity != delta.uid_validity;
    // The folder follows the mailbox from here on: what a rule does with a new message below is
    // a change to a message the server has.
    on_server.by_remote.insert(delta.remote.clone(), folder);
    if let Ok(f) = mb.folder_mut(folder) {
        f.remote = delta.remote.clone();
        // Whether it was read whole is said from now on, not worked out from how far it was read
        // (#504): numbered anew, it is to be read whole again.
        f.whole = Some(was_whole && !reset);
        // A server that names no UIDVALIDITY doesn't take away the one that is known.
        if delta.uid_validity != 0 {
            f.uid_validity = delta.uid_validity;
        }
    }
    if !reset && !delta.removed.is_empty() {
        // Moved out of the account, its original gone from the server meanwhile (deleted on
        // another device): there is nothing left there to delete, or to go back to (#451).
        let removed: HashSet<u32> = delta.removed.iter().copied().collect();
        let messages = &mb.messages;
        mb.pending.retain(|p| {
            !matches!(p, Pending::Move { account: owner, message, from, uid, .. }
                if *owner == account && *from == delta.remote && removed.contains(uid) && messages.get(message).is_some_and(|m| m.account != account))
        });
    }
    let gone: Vec<Id> = if reset {
        renumbered(mb, account, on_server, folder, &delta.remote, counts)
    } else {
        delta.removed.iter().filter_map(|uid| on_server.by_uid.get(&(folder, *uid)).copied()).collect()
    };
    for id in gone {
        let uid = mb.message(id).map_or(0, |m| m.uid);
        // Put back after a refused copy: kept here instead (#451).
        if super::graph_place::kept_instead(mb, id) {
            on_server.by_uid.remove(&(folder, uid));
            continue;
        }
        // The server no longer has it: taken out without a word back to the server.
        if let Ok(m) = mb.forget_message(id) {
            on_server.by_uid.remove(&(folder, m.uid));
            counts.removed += 1;
        }
    }
    // What was put back here after a refused copy and is listed as still there (the first piece
    // of a listing names every UID known that the server has): one listing nearer to being the
    // server's message as any other.
    for id in &on_server.returned {
        if let Ok(m) = mb.message_mut(*id)
            && m.folder == folder
            && m.returned > 0
            && delta.flags.iter().any(|f| f.uid == m.uid)
        {
            m.returned -= 1;
        }
    }
    for theirs in &delta.flags {
        if let Some(id) = on_server.by_uid.get(&(folder, theirs.uid)) {
            counts.updated += usize::from(merge_flags(mb, *id, *theirs));
        }
    }
    // Mail downloaded again to be read anew (#650); a mailbox numbered anew has no UID to find it by.
    if reset {
        delta.refetched.clear();
        delta.refetch_done.clear();
    } else {
        super::imap_again::take_in(mb, &mut delta, |uid| on_server.by_uid.get(&(folder, uid)).copied());
    }
    let inbox = delta.kind == FolderKind::Inbox;
    let junk = delta.kind == FolderKind::Junk;
    // The lists sort what arrives once the folder has been read whole, above how far it was
    // read: never a first download (in however many pieces, over however many runs), a mailbox
    // numbered anew, or older mail a raised limit brings down. What was there stays where the
    // server has it.
    let sorting = was_whole && !reset;
    // Only mail that came to the owner counts as new: in the Inbox, or a folder of theirs a
    // server's rule files it in, and everything of a folder's first download. What later turns
    // up in Deleted Items, Junk, Sent and the rest was put there (a message deleted on a phone
    // was "1 new message").
    let arriving = matches!(delta.kind, FolderKind::Inbox | FolderKind::Custom) || old_seen == 0;
    // How many of these arrivals have each Message-ID: only a lone one can be the copy a change waits for.
    let mut shared: HashMap<String, usize> = HashMap::new();
    if reset {
        for m in &delta.new {
            *shared.entry(m.message_id.clone()).or_default() += 1;
        }
    }
    for mut m in delta.new {
        let uid = m.uid;
        // Here already, or on its way out of that mailbox by a change still to be pushed.
        if uid != 0 && (on_server.by_uid.contains_key(&(folder, uid)) || on_server.leaving.contains(&(folder, uid))) {
            continue;
        }
        // The copy a move or deletion made here was waiting for, the mailbox numbered anew.
        if uid != 0 && waited_for(mb, account, &delta.remote, &m, shared.get(&m.message_id).copied().unwrap_or(1), counts) {
            on_server.leaving.insert((folder, uid));
            counts.matched += 1;
            continue;
        }
        if matched(mb, on_server, folder, &m).is_some() {
            counts.matched += 1;
            continue;
        }
        m.id = 0;
        m.folder = folder;
        m.synced = (uid != 0).then(|| RemoteFlags::of(&m, uid));
        let Ok(id) = mb.add_message(m) else { continue };
        if uid != 0 {
            on_server.by_uid.insert((folder, uid), id);
        }
        if inbox || junk {
            // The Safe and Blocked Senders lists first (#504): mail a list sends to Junk E-mail
            // isn't the rules' to file, and a safe sender's taken out of Junk E-mail goes through
            // them as any mail arriving in the Inbox does.
            let sorted = sorting && uid > old_seen && mb.sort_junk(id);
            if inbox != sorted {
                mb.apply_rules(id);
            }
            // A list or a rule that moved it away has noted the move.
            if uid != 0 && !mb.message(id).is_some_and(|m| m.folder == folder && m.uid == uid) {
                on_server.by_uid.remove(&(folder, uid));
                on_server.leaving.insert((folder, uid));
            }
        }
        counts.new += usize::from(arriving);
    }
    if let Ok(f) = mb.folder_mut(folder) {
        if delta.complete {
            f.whole = Some(true);
        }
        f.uid_seen = if reset { delta.uid_seen } else { delta.uid_seen.max(old_seen) };
        if reset || delta.uid_from != 0 {
            f.uid_from = delta.uid_from;
        }
    }
}

fn apply(s: &mut Session, v: &Value) -> CmdResult {
    let result: SyncResult =
        serde_json::from_value(v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?).map_err(|e| bad(format!("`result`: {e}")))?;
    apply_sync(s, result)
}

/// Whether any folder of `account` is the server's already (its mail is here).
fn listed_ews(s: &Session, account: Id) -> bool {
    s.mailbox.folders.iter().any(|f| f.account == account && f.server.is_some())
}
