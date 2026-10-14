//! Accounts, signatures and synchronising with a mail server.
//!
//! The engine never talks to a server: `sync.request` says what a synchronisation should do and
//! `sync.apply` takes in what it brought back (`mailcraft_model::sync`). A host with sockets
//! (`mailcraft-net`, in the desktop app and the CLI) does the part in between.

use std::collections::{HashMap, HashSet};

use mailcraft_model::sync::{
    CategoriesPushed, Deleted, FlagsPushed, FolderDelta, FolderState, Moved, RemoteCategories, RemoteFlags, RemoteOp, SyncRequest, SyncResult,
    Upload, Uploaded, category_keyword, summary_line,
};
use mailcraft_model::{
    Account, AccountKind, Directory, FavoritesSource, FlagStatus, Folder, FolderKind, Id, Mailbox, Message, OAuth, OAuthProvider, Pending, Security,
    Server, Signature, clean_name,
};
use serde_json::{Value, json};

use super::{bad, done, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("account.add", "Add Account", "File › Info", add).params(ACCOUNT),
        CommandSpec::new("account.edit", "Account Settings", "File › Info", edit).params(r#"{"id": number, …the fields of account.add}"#),
        CommandSpec::new("account.remove", "Remove Account", "File › Info", remove).params(r#"{"id": number}"#),
        CommandSpec::new("account.list", "List Accounts", "File › Info", list).pure(),
        CommandSpec::new("account.favoritesSource", "Where Outlook's Favorites Come From", "File › Options", favorites_source_command)
            .params(r#"{"account"?: number (default: the first account that signs in with Microsoft), "source"?: "graph" (Microsoft Graph: lasting; in an organisation an administrator may have to approve it once) | "ews" (Exchange Web Services: needs no administrator where the organisation still allows it; Microsoft switches it off on 1 April 2027, and it is refused from that day on by the session's clock, and for a personal Microsoft account) | "none" (not now: the Favorites marked here are the account's, and what was kept of the server's list goes), "directory"?: "organisation"|"personal" (which kind of Microsoft account it is, when the caller knows from the sign-in)} — how the folders the owner keeps under Favorites in Outlook are read for that account; only ever read, whichever it is. It sets the choice and nothing else: a host with sockets does the reading (`ui.favoritesSource` in the app). Without "source" nothing changes. Answers {"account", "source": "graph"|"ews"|"none"|null (null: never chosen, which is as it was before there was a choice: Microsoft Graph once the owner has agreed to it), "directory": "organisation"|"personal"|null, "offered": [string] (what may be chosen now), "ewsEnds": "2027-04-01", "ewsEnded": bool, "from": "server"|"mailbox"}"#),
        CommandSpec::new("account.preset", "Suggest Account Settings", "File › Info", preset)
            .pure()
            .params(r#"{"email": string, "provider"?: "microsoft", "service"?: "microsoft365"|"outlook.com"|"google"|"yahoo"|"icloud"|"fastmail"|"aol"|"zoho", "mx"?: [string] (the domain's mail exchangers, when the host looked them up)} — answers the servers and sign-in an address usually takes; changes nothing"#),
        CommandSpec::new("signature.set", "Signatures", "File › Options", signature_set)
            .params(r#"{"name": string, "text": string, "default"?: bool (use it for new messages), "account"?: number}"#),
        CommandSpec::new("signature.remove", "Delete Signature", "File › Options", signature_remove).params(r#"{"name": string}"#),
        CommandSpec::new("sync.request", "Send/Receive All Folders", "Send / Receive", request)
            .pure()
            .key("F9")
            .params(r#"{"account"?: number} — answers what a host should send and fetch"#)
            .when(has_server),
        CommandSpec::new("sync.apply", "Apply Send/Receive", "Send / Receive", apply).pure().params(r#"{"result": SyncResult}"#),
    ]
}

const ACCOUNT: &str = r#"{"email": string, "displayName"?: string, "name"?: string, "username"?: string, "incoming"?: {"host": string, "port"?: number, "security"?: "tls"|"starttls"|"none"} | null, "outgoing"?: same | null, "auth"?: "password"|"oauth2" (oauth2: sign in on the provider's page), "provider"?: "microsoft", "clientId"?: string, "tenant"?: string, "directory"?: "organisation"|"personal" (which kind of Microsoft account signs in, when the sign-in said)}"#;

fn has_server(s: &Session) -> Option<&'static str> {
    if s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Imap) {
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

/// Whether Exchange Web Services is no longer there by the session's clock: Microsoft switches
/// it off for everybody on 1 April 2027 ([`FavoritesSource::EWS_ENDS`]). From that day on it is
/// not offered, and an account set to it reads nothing through it.
pub fn ews_ended(s: &Session) -> bool {
    let (year, month, day) = FavoritesSource::EWS_ENDS;
    mailcraft_time::Date::new(year, month, day).is_some_and(|ends| s.today() >= ends)
}

/// Whether `account` signs in with Microsoft.
fn signs_in_with_microsoft(account: &Account) -> bool {
    account.oauth.as_ref().is_some_and(|oauth| oauth.provider == OAuthProvider::Microsoft)
}

/// How the Favorites of Outlook may be read for an account of this kind today, by what a
/// command calls each way: Exchange Web Services only for an account that is not known to be a
/// personal one, and only while it exists.
pub fn favorites_sources_offered(s: &Session, directory: Directory) -> Vec<&'static str> {
    let mut offered = vec!["graph"];
    if directory != Directory::Personal && !ews_ended(s) {
        offered.push("ews");
    }
    offered.push("none");
    offered
}

/// `account.favoritesSource`: the owner's choice of how an account's Favorites in Outlook are
/// read, and nothing else: a setting of the account, undone and redone like its others.
fn favorites_source_command(s: &mut Session, v: &Value) -> CmdResult {
    let named = match v.get("account") {
        None | Some(Value::Null) => None,
        Some(_) => Some(p::req_id(v, "account")?),
    };
    let account = match named {
        Some(id) => s.mailbox.account(id).ok_or_else(|| fail(format!("no account with id {id}")))?,
        None => s.mailbox.accounts.iter().find(|a| signs_in_with_microsoft(a)).ok_or_else(|| fail("No account signs in with Microsoft"))?,
    };
    if !signs_in_with_microsoft(account) {
        return Err(fail("This account doesn't sign in with Microsoft"));
    }
    let id = account.id;
    let source = match v.get("source") {
        None | Some(Value::Null) => None,
        Some(_) => Some(p::str(v, "source").and_then(FavoritesSource::parse).ok_or_else(|| bad("`source` is graph, ews or none"))?),
    };
    let said = directory_of(v)?;
    let directory = said.or(account.oauth.as_ref().map(|oauth| oauth.directory)).unwrap_or_default();
    if source == Some(FavoritesSource::Ews) {
        if ews_ended(s) {
            return Err(fail(
                "Microsoft switched Exchange Web Services off on 1 April 2027. Outlook's Favorites can be read through Microsoft Graph.",
            ));
        }
        if directory == Directory::Personal {
            return Err(fail("Exchange Web Services is not there for a personal Microsoft account. Its Favorites are read through Microsoft Graph."));
        }
    }
    if let Some(source) = source {
        let account = s.mailbox.account_mut(id)?;
        account.favorites_source = source;
        // Which kind of account it is goes into the file with the choice, and not before.
        if let Some(oauth) = account.oauth.as_mut().filter(|oauth| said.is_some_and(|said| said != oauth.directory)) {
            oauth.directory = directory;
        }
        // "Not now": what was kept of the server's list goes, and the marks made here count.
        if source == FavoritesSource::None {
            s.mailbox.set_server_favorites(id, None);
        }
    }
    let (year, month, day) = FavoritesSource::EWS_ENDS;
    Ok(json!({
        "account": id,
        "source": s.mailbox.account(id).and_then(|a| a.favorites_source.code()),
        "directory": directory.code(),
        "offered": favorites_sources_offered(s, directory),
        "ewsEnds": format!("{year:04}-{month:02}-{day:02}"),
        "ewsEnded": ews_ended(s),
        "from": if super::folder::favorites_from_server(s, id) { "server" } else { "mailbox" },
    }))
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
    a.kind = if a.incoming.is_some() || a.outgoing.is_some() { AccountKind::Imap } else { AccountKind::Local };
    if !a.address().is_valid() {
        return Err(bad(format!("\"{}\" isn't an e-mail address", a.email)));
    }
    if a.name.is_empty() {
        a.name = a.email.clone();
    }
    Ok(())
}

fn add(s: &mut Session, v: &Value) -> CmdResult {
    let mut a = Account::default();
    apply_fields(&mut a, v)?;
    if s.mailbox.accounts.iter().any(|x| x.email.eq_ignore_ascii_case(&a.email)) {
        return Err(fail(format!("{} is already set up", a.email)));
    }
    if s.mailbox.accounts.len() >= 50 {
        return Err(fail("there are too many accounts"));
    }
    // The placeholder account of an empty mailbox makes way for the first real one.
    if let [only] = s.mailbox.accounts.as_slice()
        && only.email.is_empty()
        && s.mailbox.messages.is_empty()
    {
        let placeholder = only.id;
        s.mailbox.remove_account(placeholder)?;
    }
    let id = s.mailbox.add_account(a);
    if s.mailbox.folder(s.view.folder).is_none() {
        s.view.folder = s.mailbox.folder_of_kind(id, FolderKind::Inbox).unwrap_or(0);
    }
    Ok(json!({"id": id}))
}

fn edit(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let mut a = s.mailbox.account(id).cloned().ok_or_else(|| fail(format!("no account with id {id}")))?;
    if a.is_shared() {
        // Its address, servers and sign-in are what it is opened by: only its names change.
        if ["email", "username", "incoming", "outgoing", "auth", "provider", "clientId", "tenant", "directory"].iter().any(|k| v.get(*k).is_some()) {
            return Err(fail(format!("{} is a shared mailbox: only its names change here (remove it and add it again for another)", a.email)));
        }
        let line = |k: &str| p::str(v, k).map(|t| t.replace(['\r', '\n'], " ").trim().chars().take(254).collect::<String>());
        if let Some(n) = line("displayName") {
            a.display_name = n;
        }
        if let Some(n) = line("name") {
            a.name = if n.is_empty() { a.email.clone() } else { n };
        }
        *s.mailbox.account_mut(id)? = a;
        return Ok(json!({"id": id}));
    }
    apply_fields(&mut a, v)?;
    if s.mailbox.accounts.iter().any(|x| x.id != id && x.email.eq_ignore_ascii_case(&a.email)) {
        return Err(fail(format!("{} is already set up", a.email)));
    }
    *s.mailbox.account_mut(id)? = a;
    Ok(json!({"id": id}))
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    // The shared mailboxes opened through it go with it.
    if s.mailbox.accounts.iter().all(|a| a.id == id || a.shared_of == Some(id)) {
        return Err(fail("the last account can't be removed"));
    }
    s.mailbox.remove_account(id)?;
    Ok(json!({"deleted": id}))
}

fn list(s: &mut Session, _: &Value) -> CmdResult {
    let accounts: Vec<Value> = s
        .mailbox
        .accounts
        .iter()
        .map(|a| {
            json!({"id": a.id, "name": a.name, "email": a.email, "displayName": a.display_name, "kind": a.kind, "username": a.username, "incoming": a.incoming, "outgoing": a.outgoing, "signature": a.signature, "auth": if a.oauth.is_some() { "oauth2" } else { "password" }, "oauth": a.oauth, "favoritesSource": a.favorites_source.code(), "sharedOf": a.shared_of})
        })
        .collect();
    Ok(json!({"accounts": accounts, "signatures": s.mailbox.signatures}))
}

fn signature_set(s: &mut Session, v: &Value) -> CmdResult {
    let name = clean_name(p::req_str(v, "name")?).ok_or_else(|| bad("`name` is empty"))?;
    let text: String = p::req_str(v, "text")?.chars().take(10_000).collect();
    match s.mailbox.signatures.iter().position(|x| x.name.eq_ignore_ascii_case(&name)) {
        Some(i) => {
            if let Some(sig) = s.mailbox.signatures.get_mut(i) {
                sig.text = text;
            }
        }
        None if s.mailbox.signatures.len() >= 100 => return Err(fail("there are too many signatures")),
        None => s.mailbox.signatures.push(Signature { name: name.clone(), text }),
    }
    if p::bool(v, "default") == Some(true) {
        let account = p::id(v, "account").or(s.mailbox.default_account().map(|a| a.id)).ok_or_else(|| fail("add an account first"))?;
        s.mailbox.account_mut(account)?.signature = name;
    }
    done()
}

fn signature_remove(s: &mut Session, v: &Value) -> CmdResult {
    let name = p::req_str(v, "name")?.trim().to_string();
    let before = s.mailbox.signatures.len();
    s.mailbox.signatures.retain(|x| !x.name.eq_ignore_ascii_case(&name));
    if s.mailbox.signatures.len() == before {
        return Err(fail(format!("no signature named \"{name}\"")));
    }
    for a in s.mailbox.accounts.iter_mut().filter(|a| a.signature.eq_ignore_ascii_case(&name)) {
        a.signature.clear();
    }
    done()
}

/// Most changes one synchronisation is asked to push; a longer backlog goes up over several runs.
const MAX_OPS: usize = 2_000;
/// Most folders followed upwards from one that only exists here, to where the server has one.
const MAX_LEVELS: usize = 64;
/// Most messages one synchronisation is asked to put on the server; the rest goes up later.
const MAX_UPLOADS: usize = 200;
/// The largest message put on the server from here, by its size here.
const MAX_UPLOAD: u64 = 25 * 1024 * 1024;
/// The header a message kept without its text has (`mailcraft-net` writes it): what is here of
/// it is not all of it, so it is never put on a server from here, only copied there.
const NOT_DOWNLOADED: &str = "X-MailCraft-Not-Downloaded";
/// Most categories a mailbox gets from what servers keep as keywords; past that, a keyword that
/// is no category here is left out.
const MAX_CATEGORIES: usize = 200;
/// What Undo says once a change it would go back past can't be taken back on the mail server.
pub const SEALED_BY_DELETE: &str = "a message was deleted for good on the mail server, which can't be undone";
pub const SEALED_BY_SENDING: &str = "mail that was sent can't be called back";

/// What a synchronisation of `account` should do: send the Outbox, push what was changed here
/// since the server last heard (flags, moves, deletions), download what is new. The host adds
/// the password.
pub fn sync_request(s: &Session, account: Id) -> Result<SyncRequest, CmdError> {
    let a = s.mailbox.account(account).ok_or_else(|| fail(format!("no account with id {account}")))?;
    if a.kind != AccountKind::Imap {
        return Err(fail(format!("{} isn't connected to a mail server", a.name)));
    }
    let outbox_folder = s.mailbox.folder_of_kind(account, FolderKind::Outbox);
    let mut known: HashMap<Id, Vec<u32>> = HashMap::new();
    let mut outbox = Vec::new();
    let now = s.now();
    for m in s.mailbox.messages.values() {
        // A message whose delivery is delayed stays in the Outbox until its time; it then goes
        // dated when it went.
        if Some(m.folder) == outbox_folder && !m.is_deferred(now) {
            let mut going = m.as_ref().clone();
            if let Some(at) = going.deliver_after.take() {
                going.date = going.date.max(at);
            }
            outbox.push(going);
        }
        if m.uid != 0 {
            known.entry(m.folder).or_default().push(m.uid);
        }
    }
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
            known_uids: known.remove(&f.id).unwrap_or_default(),
            mark: f.mark,
        })
        .collect();
    let (ops, _) = remote_ops(&s.mailbox, account, MAX_OPS);
    let (uploads, _) = uploads(&s.mailbox, account, MAX_UPLOADS);
    let folder_ops = super::folder_sync::folder_ops(&s.mailbox, account);
    // A shared mailbox signs in with its owner's account ([`super::shared`]).
    let (a, send_login) = super::shared::signed_in_as(s, a)?;
    Ok(SyncRequest { account: a, folders, outbox, max_per_folder: s.prefs.sync_limit, ops, folder_ops, send_login, uploads, ..Default::default() })
}

/// How many messages in the Outbox of `account` are to go now: all of them but those whose
/// delivery is delayed to a time still to come.
pub fn outbox_due(s: &Session, account: Id) -> usize {
    let Some(outbox) = s.mailbox.folder_of_kind(account, FolderKind::Outbox) else { return 0 };
    let now = s.now();
    s.mailbox.messages_in(outbox).filter(|m| !m.is_deferred(now)).count()
}

/// How many changes made in `account` its server hasn't heard of yet: what a synchronisation
/// would push now. A host asks to bring the server up to date soon after a change rather than at
/// the next timed run. One pass over the messages, and nothing is built.
///
/// A change the server refused stays counted until it goes through, so a host must not answer
/// a count that doesn't fall by trying again at once.
pub fn unpushed(s: &Session, account: Id) -> usize {
    remote_ops(&s.mailbox, account, 0).1 + uploads(&s.mailbox, account, 0).1 + super::folder_sync::folder_ops(&s.mailbox, account).len()
}

/// Whether `m` is kept here without all of its text (it was too large to download).
fn partly_here(m: &Message) -> bool {
    m.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case(NOT_DOWNLOADED))
}

/// The messages of `account` that are in a folder following a server mailbox and that the
/// server doesn't have there yet ([`Upload`]): a draft written here, a copy, mail brought in
/// from a file, a message moved here from another account. At most `limit` of them are spelled
/// out; the number is of all there are.
///
/// Such a message has no UID and nothing the server was ever said to have of it
/// ([`Message::synced`]), and no move waiting for it (it is on the server elsewhere and goes
/// there by the move). One that the server has with the same content (the same Message-ID) in
/// another of its mailboxes is copied from there. One that is only partly here is never put
/// there from here, and one too large to be is left out.
fn uploads(mb: &Mailbox, account: Id, limit: usize) -> (Vec<Upload>, usize) {
    let folders: HashMap<Id, &Folder> =
        mb.folders.iter().filter(|f| f.account == account && !f.remote.is_empty() && f.kind != FolderKind::Outbox).map(|f| (f.id, f)).collect();
    let moving: HashSet<Id> = mb
        .pending
        .iter()
        .filter_map(|p| match p {
            Pending::Move { message, .. } => Some(*message),
            Pending::Delete { .. } => None,
        })
        .collect();
    let waiting: Vec<(&Message, &Folder)> = mb
        .messages
        .values()
        .filter(|m| m.account == account && m.uid == 0 && m.synced.is_none() && !moving.contains(&m.id))
        .filter_map(|m| folders.get(&m.folder).map(|f| (m.as_ref(), *f)))
        .collect();
    if waiting.is_empty() {
        return (Vec::new(), 0);
    }
    // Where the server has a message by its Message-ID: the mailbox and the UID.
    let mut on_server: HashMap<&str, (&str, u32)> = HashMap::new();
    for m in mb.messages.values().filter(|m| m.account == account && m.uid != 0 && !m.message_id.is_empty()) {
        if let Some(f) = folders.get(&m.folder).filter(|f| f.uid_validity != 0 && m.synced.is_some_and(|s| s.uid == m.uid)) {
            on_server.entry(m.message_id.as_str()).or_insert((f.remote.as_str(), m.uid));
        }
    }
    let (mut out, mut count) = (Vec::new(), 0usize);
    for (m, folder) in waiting {
        let source = on_server.get(m.message_id.as_str()).filter(|_| !m.message_id.is_empty()).filter(|(remote, _)| *remote != folder.remote);
        let partial = partly_here(m);
        if (partial && source.is_none()) || (!partial && m.size() > MAX_UPLOAD) {
            continue;
        }
        count += 1;
        if out.len() < limit {
            let (copy_from, copy_uid) = source.map_or((String::new(), 0), |(remote, uid)| (remote.to_string(), *uid));
            out.push(Upload { message: m.clone(), to: folder.remote.clone(), copy_from, copy_uid, copy_only: partial });
        }
    }
    (out, count)
}

/// The categories of `names` that can go to a server as keywords, each once (by its keyword).
fn keyword_categories(names: &[String]) -> Vec<&String> {
    let mut seen: HashSet<String> = HashSet::new();
    names.iter().filter(|name| category_keyword(name).is_some_and(|k| seen.insert(k.to_ascii_lowercase()))).collect()
}

/// The categories given to `m` here, and those taken away, since the server last had them
/// ([`Message::synced_categories`]); `None` when it has what the server has.
fn category_changes(m: &Message) -> Option<(Vec<String>, Vec<String>)> {
    let key = |name: &String| category_keyword(name).map(|k| k.to_ascii_lowercase());
    let here = keyword_categories(&m.categories);
    let there = keyword_categories(&m.synced_categories);
    let add: Vec<String> = here.iter().filter(|h| !there.iter().any(|t| key(t) == key(h))).map(|h| (*h).clone()).collect();
    let remove: Vec<String> = there.iter().filter(|t| !here.iter().any(|h| key(h) == key(t))).map(|t| (*t).clone()).collect();
    (!add.is_empty() || !remove.is_empty()).then_some((add, remove))
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
/// A change is only asked for in a mailbox one of the account's folders follows with a
/// `UIDVALIDITY` (the UIDs kept of it are good for that life of the mailbox only).
fn remote_ops(mb: &Mailbox, account: Id, limit: usize) -> (Vec<RemoteOp>, usize) {
    let (mut ops, mut count) = (Vec::new(), 0usize);
    let mut note = |op: &dyn Fn() -> RemoteOp| {
        if ops.len() < limit {
            ops.push(op());
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
            Pending::Delete { .. } => note(&|| RemoteOp::Delete { from: from.to_string(), uid }),
            Pending::Move { message, synced, to, .. } => {
                let Some(m) = mb.message(*message).filter(|m| m.account == account && m.folder == *to && m.uid == 0) else { continue };
                // The Outbox is no place on a server: the move waits until the message is elsewhere.
                let Some(folder) = folders.get(to).copied().filter(|f| f.kind != FolderKind::Outbox) else { continue };
                // What was changed about the message before or after it was moved goes first, so
                // that it arrives in its new mailbox as it is here.
                if let Some([read, flagged, answered]) = synced.filter(|base| base.uid == uid).and_then(|base| flag_changes(m, base)) {
                    note(&|| RemoteOp::Flags { folder: from.to_string(), uid, read, flagged, answered });
                }
                note(&|| {
                    let (to, create) = destination(mb, folder);
                    RemoteOp::Move { message: *message, from: from.to_string(), uid, to, to_folder: folder.id, create }
                });
            }
        }
    }
    for m in mb.messages.values() {
        let Some(base) = m.synced.filter(|base| m.uid != 0 && base.uid == m.uid && m.account == account) else { continue };
        let Some(folder) = folders.get(&m.folder).copied().filter(followed) else { continue };
        if let Some([read, flagged, answered]) = flag_changes(m, base) {
            note(&|| RemoteOp::Flags { folder: folder.remote.clone(), uid: m.uid, read, flagged, answered });
        }
        // A mailbox that keeps no keywords of the owner's keeps the categories here.
        if !folder.no_keywords
            && let Some((add, remove)) = category_changes(m)
        {
            note(&|| RemoteOp::Categories { folder: folder.remote.clone(), uid: m.uid, add: add.clone(), remove: remove.clone() });
        }
    }
    (ops, count)
}

fn request(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::id(v, "account")
        .or_else(|| s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Imap).map(|a| a.id))
        .ok_or_else(|| fail("no account is connected to a mail server"))?;
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
        let line = summary_line(self.errors.first().map(String::as_str), self.new, self.sent);
        match (self.stopped && self.errors.is_empty(), self.new + self.sent) {
            (false, _) => line,
            (true, 0) => "Stopped".into(),
            (true, _) => format!("Stopped ({line})"),
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
    let Some(a) = s.mailbox.account(account) else { return Err(fail(format!("no account with id {account}"))) };
    // An answer for another account (the mailbox was replaced while the run was under way) would
    // tie this one's messages to a stranger's server.
    if !result.email.is_empty() && !a.email.trim().eq_ignore_ascii_case(result.email.trim()) {
        return Err(fail(format!("this Send/Receive was for another account, not for {}", a.name)));
    }
    let (partial, stopped, progress) = (result.partial, result.stopped, result.progress.clone());
    let pushed = result.pushed_count();
    let (counts, sent) = if result.has_news() { take_in_news(s, account, &mut result) } else { (Counts::default(), 0) };
    run.new += counts.new;
    run.sent += sent;
    run.removed += counts.removed;
    run.updated += counts.updated;
    run.matched += counts.matched;
    run.pushed += pushed;
    run.errors.extend(result.errors.iter().cloned());
    run.stopped |= stopped;
    run.finished = !partial;
    let summary = run.summary();
    if !partial {
        let t = s.local_now();
        s.sync_status = format!("{summary} — {}", t.time_short(s.prefs.twelve_hour));
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
    // Whether the undo history can be kept: see [`keep_history`]. Worked out before the mailbox
    // takes the answer in, while what it had is still there to look at.
    let history = keep_history(s, account, result);
    let mb = &mut s.mailbox;
    // Folders first: what follows names the mailboxes as they are called now.
    let folders_told = super::folder_sync::folders_pushed(mb, account, &result.folders_pushed);
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
    // A folder that follows a mailbox the server's whole list doesn't have is gone from there.
    if result.listed_whole && !result.listed.is_empty() {
        super::folder_sync::forget_unlisted(mb, account, &result.listed);
    }
    let mut on_server = OnServer::of(mb, account);
    flags_pushed(mb, account, &on_server, &result.flagged);
    for moved in &result.moved {
        move_finished(mb, account, &mut on_server, moved);
    }
    deletes_finished(mb, account, &mut on_server, &result.deleted);
    for up in &result.uploaded {
        upload_finished(mb, account, &mut on_server, up);
    }
    categories_pushed(mb, &on_server, &result.categorized);
    for remote in &result.keywords_refused {
        for f in mb.folders.iter_mut().filter(|f| f.account == account && f.remote == *remote) {
            f.no_keywords = true;
        }
    }
    on_server.expect(mb, account);
    adopt_listed(mb, account, &mut on_server, &result.listed);
    // Only what is really new counts as new: not the server's copy of a message that was here.
    let mut counts = Counts::default();
    for delta in std::mem::take(&mut result.folders) {
        take_in(mb, account, &mut on_server, delta, &mut counts);
    }
    mb.settle_folder_changes();
    match history {
        _ if folders_told => s.forget_history(),
        History::Keep => {}
        History::Rebase => {
            let (flagged, moved, uploaded, deleted) = (result.flagged.clone(), result.moved.clone(), result.uploaded.clone(), result.deleted.clone());
            s.rebase_history(|mb| rebase(mb, account, &flagged, &moved, &uploaded, &deleted));
        }
        History::Forget => s.forget_history(),
        History::Seal(why) => s.seal_history(why),
    }
    // Not through `run` when the host calls this, so say it here: the lists are out of date.
    s.mailbox_changed();
    (counts, sent)
}

/// What becomes of the undo history when an answer is taken in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum History {
    /// Nothing the server took on stands in its way (a read mark, a flag: undoing one is
    /// overruled by the server's copy at the next run, or goes up with it).
    Keep,
    /// Every mailbox Undo would go back to is brought up to what the server did ([`rebase`]):
    /// going back to one asks the server for the opposite.
    Rebase,
    /// Let go of: what the server did can't be followed in the mailboxes Undo would go back to.
    Forget,
    /// Let go of, and Undo says why: what the server did can't be undone there.
    Seal(&'static str),
}

/// What becomes of the undo history once `result` is taken in ([`History`]).
///
/// - Mail that was sent can't be called back, and a message deleted for good is gone from the
///   server: Undo says so. (The copy a draft saved again replaced is no such message: the draft
///   lives on here, and going back to the one before puts that on the server again.)
/// - A move the server said the new place of, and a message it has now, are followed in each of
///   those mailboxes: an undo moves the message back on the server too, or takes away what was
///   put there.
/// - A move whose new place the server didn't say can't be followed: the history goes.
fn keep_history(s: &Session, account: Id, result: &SyncResult) -> History {
    if !result.sent.is_empty() {
        return History::Seal(SEALED_BY_SENDING);
    }
    if !result.deleted.is_empty() {
        // A message the server deleted that a mailbox Undo would go back to has, and that isn't
        // here any more: deleted for good.
        let by_remote: HashMap<&str, Id> =
            s.mailbox.folders.iter().filter(|f| f.account == account && !f.remote.is_empty()).map(|f| (f.remote.as_str(), f.id)).collect();
        let gone = s.snapshots().any(|mb| {
            mb.messages.values().any(|m| {
                m.account == account
                    && m.uid != 0
                    && result.deleted.iter().any(|d| d.uid == m.uid && by_remote.get(d.from.as_str()) == Some(&m.folder))
                    && s.mailbox.message(m.id).is_none()
            })
        });
        if gone {
            return History::Seal(SEALED_BY_DELETE);
        }
    }
    if result.moved.iter().any(|m| m.new_uid == 0 || m.uid_validity == 0) {
        return History::Forget;
    }
    if result.moved.is_empty() && result.uploaded.is_empty() && result.deleted.is_empty() {
        return History::Keep;
    }
    History::Rebase
}

/// Bring a mailbox Undo would go back to up to what the server did ([`History::Rebase`]): the
/// moves it made, as if they had been made while it was the mailbox ([`move_finished`]), which
/// asks the server to move back what is not where the server has it; the messages it has now
/// ([`upload_finished`]), which takes away there what this mailbox doesn't have; and copies it
/// deleted of messages this mailbox still has (a draft before it was saved again), which go up
/// again when it is gone back to.
fn rebase(mb: &mut Mailbox, account: Id, flagged: &[FlagsPushed], moved: &[Moved], uploaded: &[Uploaded], deleted: &[Deleted]) {
    let mut on_server = OnServer::of(mb, account);
    flags_pushed(mb, account, &on_server, flagged);
    for m in moved {
        move_finished(mb, account, &mut on_server, m);
    }
    for gone in deleted {
        mb.pending.retain(|p| !(matches!(p, Pending::Delete { .. }) && p.account() == account && p.source() == (gone.from.as_str(), gone.uid)));
        let here = on_server.by_remote.get(&gone.from).and_then(|folder| on_server.by_uid.remove(&(*folder, gone.uid)));
        if let Some(Ok(m)) = here.map(|id| mb.message_mut(id)) {
            (m.uid, m.synced, m.synced_categories) = (0, None, Vec::new());
        }
    }
    for up in uploaded {
        upload_finished(mb, account, &mut on_server, up);
    }
}

/// A message the server has now ([`Uploaded`]). The message here gets its UID there, or, when
/// the server didn't say, is marked as expected there, to be matched when its copy comes down.
/// When the message here is another by now (a draft saved again meanwhile: another Message-ID),
/// or is gone, or is elsewhere, the server's copy is not the message's: it is deleted there
/// ([`Pending::Delete`]), and the message here goes up as it is now.
fn upload_finished(mb: &mut Mailbox, account: Id, on_server: &mut OnServer, up: &Uploaded) {
    let Some((validity, remote)) = mb.folder(up.folder).filter(|f| f.account == account).map(|f| (f.uid_validity, f.remote.clone())) else { return };
    // A UID that can be trusted is one of the mailbox's present life that no message here has.
    let usable = up.uid != 0 && up.uid_validity != 0 && (validity == 0 || validity == up.uid_validity) && remote == up.to;
    let free = usable && !on_server.by_uid.contains_key(&(up.folder, up.uid));
    let same = mb.message(up.message).is_some_and(|m| {
        m.account == account
            && m.folder == up.folder
            && m.uid == 0
            && m.synced.is_none()
            && remote == up.to
            && (m.message_id == up.message_id || m.message_id.is_empty())
    });
    if !same {
        if free {
            mb.pending.push(Pending::Delete { account, from: up.to.clone(), uid: up.uid });
        }
        return;
    }
    let Ok(m) = mb.message_mut(up.message) else { return };
    if m.message_id.is_empty() {
        m.message_id = up.message_id.clone();
    }
    // A copy made on the server has the keywords of what it was copied from; one put there has none.
    if !up.copied {
        m.synced_categories.clear();
    }
    if free {
        m.uid = up.uid;
        m.synced = Some(up.flags.at(up.uid));
        on_server.by_uid.insert((up.folder, up.uid), up.message);
        if let Ok(f) = mb.folder_mut(up.folder)
            && f.uid_validity == 0
        {
            f.uid_validity = up.uid_validity;
        }
    } else {
        m.synced = Some(up.flags.at(0));
    }
}

/// Changes of categories the server took on: they are what the server has now, whatever was
/// changed here meanwhile (which then differs from it, and goes up next time).
fn categories_pushed(mb: &mut Mailbox, on_server: &OnServer, pushed: &[CategoriesPushed]) {
    for change in pushed {
        let Some(id) = on_server.by_remote.get(&change.folder).and_then(|folder| on_server.by_uid.get(&(*folder, change.uid))).copied() else {
            continue;
        };
        let Some(mut there) = mb.message(id).map(|m| m.synced_categories.clone()) else { continue };
        there.retain(|name| !change.remove.iter().chain(&change.add).any(|other| other.eq_ignore_ascii_case(name)));
        there.extend(change.add.iter().cloned());
        if let Ok(m) = mb.message_mut(id) {
            m.synced_categories = there;
        }
    }
}

/// The name of the mailbox's category `name` is, by its letters whatever their case; a name it
/// has no category of becomes one (in a colour of the usual six), unless the mailbox has as
/// many as it takes from servers. `None`: left out.
fn category_named(mb: &mut Mailbox, name: &str) -> Option<String> {
    if let Some(known) = mb.categories.iter().find(|c| c.name.eq_ignore_ascii_case(name)) {
        return Some(known.name.clone());
    }
    if mb.categories.len() >= MAX_CATEGORIES {
        return None;
    }
    let palette = [(0xD1, 0x3B, 0x3B), (0xE0, 0x7B, 0x1F), (0xD9, 0xA4, 0x0C), (0x2E, 0x8B, 0x57), (0x2F, 0x6F, 0xD0), (0x7E, 0x57, 0xC2)];
    let (r, g, b) = palette.get(mb.categories.len() % palette.len()).copied().unwrap_or((0x80, 0x80, 0x80));
    mb.categories.push(mailcraft_model::Category { name: name.to_string(), color: mailcraft_model::Rgb(r, g, b) });
    Some(name.to_string())
}

/// The server's categories for a message (`theirs`), as the mailbox names them: each one it
/// has a category of, and new ones made, as far as [`category_named`] goes.
fn their_categories(mb: &mut Mailbox, theirs: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in theirs.iter().take(mailcraft_model::sync::MAX_MESSAGE_KEYWORDS) {
        if let Some(name) = category_named(mb, name)
            && !out.iter().any(|known| known.eq_ignore_ascii_case(&name))
        {
            out.push(name);
        }
    }
    out
}

/// Give message `id` the categories the server keeps as keywords on it (`theirs`), merged with
/// what was changed here: when its categories that can be keywords are as the server last had
/// them, they follow the server; otherwise they stay as they are here, to go up with the next
/// run. Either way the server's are what it has from now on. Returns whether the owner sees a
/// change.
fn merge_categories(mb: &mut Mailbox, id: Id, theirs: &[String]) -> bool {
    let theirs = their_categories(mb, theirs);
    let Some(m) = mb.message(id) else { return false };
    let unchanged_here = category_changes(m).is_none();
    let mut categories = m.categories.clone();
    if unchanged_here {
        // The ones that can't be keywords are the mailbox's own, and stay.
        categories.retain(|name| category_keyword(name).is_none());
        categories.extend(theirs.iter().cloned());
    }
    let seen = categories != m.categories;
    if (seen || m.synced_categories != theirs)
        && let Ok(m) = mb.message_mut(id)
    {
        (m.categories, m.synced_categories) = (categories, theirs);
    }
    seen
}

/// What taking in a result changed.
#[derive(Default)]
struct Counts {
    new: usize,
    removed: usize,
    updated: usize,
    /// Messages that came down and were the server's copy of one already here.
    matched: usize,
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
}

impl OnServer {
    fn of(mb: &Mailbox, account: Id) -> OnServer {
        let by_uid = mb.messages.values().filter(|m| m.account == account && m.uid != 0).map(|m| ((m.folder, m.uid), m.id)).collect();
        let by_remote = mb.folders.iter().filter(|f| f.account == account && !f.remote.is_empty()).map(|f| (f.remote.clone(), f.id)).collect();
        OnServer { by_uid, by_remote, expected: HashMap::new(), leaving: HashSet::new() }
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
        // Moved on meanwhile, to where the server didn't say: its copy there comes down as a
        // message of its own, and this one is not put on the server a second time.
        (Some(Pending::Move { message, synced, .. }), None) => {
            if let Ok(m) = mb.message_mut(message)
                && m.uid == 0
                && m.synced.is_none()
            {
                m.synced = Some(synced.unwrap_or_else(|| RemoteFlags::of(m, 0)).at(0));
            }
        }
        // Deleted for good meanwhile: the next run deletes it where the server has it now.
        (Some(Pending::Delete { .. }), Some((from, uid))) => mb.pending.push(Pending::Delete { account, from, uid }),
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
    mb.pending.retain(|p| !(matches!(p, Pending::Delete { .. }) && p.account() == account && finished.contains(&p.source())));
    for gone in deleted {
        let here = on_server.by_remote.get(&gone.from).and_then(|folder| on_server.by_uid.remove(&(*folder, gone.uid)));
        if let Some(Ok(m)) = here.map(|id| mb.message_mut(id)) {
            m.uid = 0;
            m.synced = None;
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
    let words = |m: &Message| m.subject.split_whitespace().collect::<Vec<_>>().join(" ");
    let people = |m: &Message| m.to.iter().chain(&m.cc).map(|address| address.email.to_ascii_lowercase()).collect::<Vec<_>>();
    a.date != 0 && a.date == b.date && a.from.email.eq_ignore_ascii_case(&b.from.email) && words(a) == words(b) && people(a) == people(b)
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
    let by_id = expected.iter().position(|id| waiting(id).is_some_and(same_id));
    let at = by_id.or_else(|| expected.iter().position(|id| waiting(id).is_some_and(|m| same_mail(m, theirs))))?;
    let id = expected.swap_remove(at);
    let m = mb.message_mut(id).ok()?;
    m.uid = theirs.uid;
    if !theirs.message_id.is_empty() {
        // The name the server knows it by, which a reply to it will carry.
        m.message_id = theirs.message_id.clone();
    }
    merge_flags(mb, id, RemoteFlags::of(theirs, theirs.uid));
    merge_categories(mb, id, &theirs.synced_categories);
    on_server.by_uid.insert((folder, theirs.uid), id);
    Some(id)
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
/// folder under the one its parent has here. A folder that is here already keeps its name; it
/// moves to where the server has it, unless a change of the owner's to it is still to reach the
/// server (folders moved here are moved there first, so the server's tree is the one that holds).
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
                let waiting = mb.folder_changes.iter().any(|c| c.folder() == Some(id));
                let elsewhere = mb.folder(id).is_some_and(|here| here.parent != parent && here.kind == FolderKind::Custom);
                if let Some(p) = parent
                    && elsewhere
                    && !waiting
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
fn take_in(mb: &mut Mailbox, account: Id, on_server: &mut OnServer, delta: FolderDelta, counts: &mut Counts) {
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
    let (old_validity, old_seen) = mb.folder(folder).map_or((0, 0), |f| (f.uid_validity, f.uid_seen));
    let reset = old_validity != 0 && delta.uid_validity != 0 && old_validity != delta.uid_validity;
    // The folder follows the mailbox from here on: what a rule does with a new message below is
    // a change to a message the server has.
    on_server.by_remote.insert(delta.remote.clone(), folder);
    if let Ok(f) = mb.folder_mut(folder) {
        f.remote = delta.remote.clone();
        // A server that names no UIDVALIDITY doesn't take away the one that is known.
        if delta.uid_validity != 0 {
            f.uid_validity = delta.uid_validity;
        }
    }
    let gone: Vec<Id> = if reset {
        // UIDs mean nothing after a validity change: what came from the server goes, and comes
        // back; what was to be pushed about its messages can't be said any more.
        mb.pending.retain(|p| !(p.account() == account && p.source().0 == delta.remote));
        on_server.leaving.retain(|(f, _)| *f != folder);
        on_server.by_uid.iter().filter(|((f, _), _)| *f == folder).map(|(_, id)| *id).collect()
    } else {
        delta.removed.iter().filter_map(|uid| on_server.by_uid.get(&(folder, *uid)).copied()).collect()
    };
    for id in gone {
        // The server no longer has it: taken out without a word back to the server.
        if let Ok(m) = mb.forget_message(id) {
            on_server.by_uid.remove(&(folder, m.uid));
            counts.removed += 1;
        }
    }
    let mut categories: HashMap<u32, &RemoteCategories> = delta.categories.iter().map(|c| (c.uid, c)).collect();
    for theirs in &delta.flags {
        if let Some(id) = on_server.by_uid.get(&(folder, theirs.uid)).copied() {
            let changed = merge_flags(mb, id, *theirs);
            // A message the server keeps no keywords on has none of its categories there.
            let keywords = categories.remove(&theirs.uid).map(|c| c.names.as_slice()).unwrap_or_default();
            let recategorized = merge_categories(mb, id, keywords);
            counts.updated += usize::from(changed || recategorized);
        }
    }
    let inbox = delta.kind == FolderKind::Inbox;
    for mut m in delta.new {
        let uid = m.uid;
        // Here already, or on its way out of that mailbox by a change still to be pushed.
        if uid != 0 && (on_server.by_uid.contains_key(&(folder, uid)) || on_server.leaving.contains(&(folder, uid))) {
            continue;
        }
        if matched(mb, on_server, folder, &m).is_some() {
            counts.matched += 1;
            continue;
        }
        m.id = 0;
        m.folder = folder;
        m.synced = (uid != 0).then(|| RemoteFlags::of(&m, uid));
        // Its categories from the server are the mailbox's, under the names it knows them by.
        // (Those it took no category for are left out; what its header says stays.)
        let keywords = std::mem::take(&mut m.synced_categories);
        let theirs = their_categories(mb, &keywords);
        m.categories.retain(|name| !keywords.iter().chain(&theirs).any(|k| k.eq_ignore_ascii_case(name)));
        m.categories.extend(theirs.iter().cloned());
        m.synced_categories = if uid != 0 { theirs } else { Vec::new() };
        let Ok(id) = mb.add_message(m) else { continue };
        if uid != 0 {
            on_server.by_uid.insert((folder, uid), id);
        }
        if inbox {
            mb.apply_rules(id);
            // A rule that moved it away has noted the move.
            if uid != 0 && !mb.message(id).is_some_and(|m| m.folder == folder && m.uid == uid) {
                on_server.by_uid.remove(&(folder, uid));
                on_server.leaving.insert((folder, uid));
            }
        }
        counts.new += 1;
    }
    if let Ok(f) = mb.folder_mut(folder) {
        f.uid_seen = if reset { delta.uid_seen } else { delta.uid_seen.max(old_seen) };
        // How the mailbox stands is what the latest piece about it says.
        f.mark = delta.mark;
    }
}

fn apply(s: &mut Session, v: &Value) -> CmdResult {
    let result: SyncResult =
        serde_json::from_value(v.get("result").cloned().ok_or_else(|| bad("`result` is required"))?).map_err(|e| bad(format!("`result`: {e}")))?;
    apply_sync(s, result)
}
