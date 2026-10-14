//! Shared mailboxes (Microsoft 365): a mailbox such as `support@` that an administrator gave the
//! owner Full Access to, opened under the owner's own account as Outlook opens it.
//!
//! A shared mailbox is an account of its own here ([`Account::shared_of`]): its own folders, mail
//! and synchronisation state, its own heading in the folder pane, and a From address of its own
//! in new messages. It has no sign-in of its own. A synchronisation reads it over IMAP with the
//! owner's access token and the shared mailbox's address as the user (SASL XOAUTH2, which
//! Exchange Online takes from an owner with Full Access), and sends what waits in its Outbox
//! through SMTP signed in as the owner with the shared mailbox's address as the sender (which
//! takes the Send As right). No permission beyond what a sign-in already asks Microsoft for is
//! needed, and nothing goes through Microsoft Graph for it. Whether the server lets the owner in
//! is found at the first Send/Receive, which says so plainly when it doesn't.

use mailcraft_model::{Account, AccountKind, Address, Directory, FolderKind, Id, OAuthProvider};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// Most shared mailboxes one account opens.
pub const MAX_SHARED: usize = 20;
/// Most accounts in a mailbox, as `account.add` allows.
const MAX_ACCOUNTS: usize = 50;
/// Longest address of a shared mailbox (RFC 5321's longest path, less its brackets).
const MAX_ADDRESS: usize = 254;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("account.addShared", "Add Shared Mailbox", "File › Info", add)
            .params(r#"{"account"?: number (the account whose owner has Full Access to it; it signs in with Microsoft at work or school; default: the first such), "address": string (the shared mailbox's e-mail address), "displayName"?: string (the name mail sent from it shows; default: none)} — opens a shared mailbox of Microsoft 365 under that account, as Outlook does: its own folders and mail, listed under its own heading after the account, and a From address of its own in new messages. It signs in with the account's own sign-in (IMAP as the shared mailbox, with the account's token) and asks Microsoft for no new permission; whether the server lets the account in (Full Access) is found at the next Send/Receive, and sending from it takes the Send As right. No password or token is kept for it. Answers {"id": the shared mailbox's account id, "account", "address"}"#)
            .when(has_microsoft_account),
        CommandSpec::new("account.removeShared", "Remove Shared Mailbox", "File › Info", remove)
            .params(r#"{"id": number (the shared mailbox's account id)} | {"account"?: number, "address": string} — closes a shared mailbox opened with account.addShared, and removes what is kept of it on this device. Nothing on the server is touched. Answers {"deleted": id}"#)
            .when(has_shared),
    ]
}

fn has_microsoft_account(s: &Session) -> Option<&'static str> {
    if s.mailbox.accounts.iter().any(|a| can_open_shared(a).is_ok()) {
        None
    } else {
        Some("shared mailboxes open under an account that signs in with Microsoft at work or school (File › Add Account)")
    }
}

fn has_shared(s: &Session) -> Option<&'static str> {
    if s.mailbox.accounts.iter().any(Account::is_shared) { None } else { Some("no shared mailbox is open") }
}

/// Whether shared mailboxes can be opened under `account`, and if not, why.
fn can_open_shared(account: &Account) -> Result<(), &'static str> {
    if account.is_shared() {
        return Err("a shared mailbox can't open another one: name the account it is opened through");
    }
    let Some(oauth) = &account.oauth else { return Err("shared mailboxes open under an account that signs in with Microsoft") };
    if oauth.provider != OAuthProvider::Microsoft {
        return Err("shared mailboxes open under an account that signs in with Microsoft");
    }
    if oauth.directory == Directory::Personal {
        return Err("a personal Microsoft account (Outlook.com) has no shared mailboxes: they are Microsoft 365's, for work or school accounts");
    }
    if account.kind != AccountKind::Imap || account.incoming.is_none() {
        return Err("the account has no incoming mail server to open a shared mailbox on");
    }
    Ok(())
}

/// The address of a shared mailbox from a command: `local@domain`, in visible ASCII without
/// anything that could break out of a protocol line or a sign-in (it is the user name IMAP is
/// signed in to with, and the sender SMTP is told).
pub fn shared_address(text: &str) -> Result<String, CmdError> {
    let address = text.trim();
    if address.is_empty() {
        return Err(bad("`address` is the shared mailbox's e-mail address"));
    }
    let printable =
        address.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'<' | b'>' | b',' | b';' | b'"' | b'(' | b')' | b'[' | b']' | b'\\'));
    let one_at = address.matches('@').count() == 1;
    if address.len() > MAX_ADDRESS || !printable || !one_at || !Address::new("", address).is_valid() {
        let shown: String = address.chars().filter(|c| !c.is_control()).take(80).collect();
        return Err(bad(format!("\"{shown}\" isn't an e-mail address")));
    }
    Ok(address.to_string())
}

/// The account a command means: the one it names, else the first that can open shared mailboxes.
fn owner(s: &Session, v: &Value) -> Result<Id, CmdError> {
    match p::id(v, "account") {
        Some(id) => {
            let account = s.mailbox.account(id).ok_or_else(|| fail(format!("no account with id {id}")))?;
            can_open_shared(account).map_err(fail)?;
            Ok(id)
        }
        None => s
            .mailbox
            .accounts
            .iter()
            .find(|a| can_open_shared(a).is_ok())
            .map(|a| a.id)
            .ok_or_else(|| fail("shared mailboxes open under an account that signs in with Microsoft at work or school (File › Add Account)")),
    }
}

fn add(s: &mut Session, v: &Value) -> CmdResult {
    let address = shared_address(p::req_str(v, "address")?)?;
    let parent_id = owner(s, v)?;
    let parent = s.mailbox.account(parent_id).cloned().ok_or_else(|| fail(format!("no account with id {parent_id}")))?;
    if parent.email.trim().eq_ignore_ascii_case(&address) {
        return Err(fail(format!("{address} is the account's own address: name the shared mailbox's")));
    }
    if s.mailbox.accounts.iter().any(|a| a.email.trim().eq_ignore_ascii_case(&address)) {
        return Err(fail(format!("{address} is already set up")));
    }
    if s.mailbox.shared_mailboxes(parent_id).len() >= MAX_SHARED {
        return Err(fail(format!("{} has as many shared mailboxes as it can open ({MAX_SHARED})", parent.email)));
    }
    if s.mailbox.accounts.len() >= MAX_ACCOUNTS {
        return Err(fail("there are too many accounts"));
    }
    let display_name: String =
        p::str(v, "displayName").map(|n| n.chars().filter(|c| !c.is_control()).take(254).collect::<String>()).unwrap_or_default().trim().to_string();
    // Its servers are its owner's (and are taken from there again for every run): written here
    // for the folder pane and Account Settings to show. No sign-in of its own, nothing secret.
    let shared = Account {
        name: address.clone(),
        email: address.clone(),
        display_name,
        kind: AccountKind::Imap,
        incoming: parent.incoming.clone(),
        outgoing: parent.outgoing.clone(),
        shared_of: Some(parent_id),
        ..Default::default()
    };
    let id = s.mailbox.add_account(shared);
    // Outlook doesn't put a shared mailbox's Inbox among the owner's Favorites by itself.
    for folder in s.mailbox.folders.iter_mut().filter(|f| f.account == id && f.kind == FolderKind::Inbox) {
        folder.favorite = false;
    }
    Ok(json!({"id": id, "account": parent_id, "address": address}))
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let id = match (p::id(v, "id"), p::str(v, "address")) {
        (Some(id), _) => id,
        (None, Some(address)) => {
            let address = shared_address(address)?;
            let parent = p::id(v, "account");
            s.mailbox
                .accounts
                .iter()
                .find(|a| a.is_shared() && a.email.trim().eq_ignore_ascii_case(&address) && parent.is_none_or(|parent| a.shared_of == Some(parent)))
                .map(|a| a.id)
                .ok_or_else(|| fail(format!("no shared mailbox {address} is open")))?
        }
        (None, None) => return Err(bad("name the shared mailbox: `id`, or `address`")),
    };
    let account = s.mailbox.account(id).ok_or_else(|| fail(format!("no account with id {id}")))?;
    if !account.is_shared() {
        return Err(fail(format!("{} isn't a shared mailbox: remove it with account.remove", account.email)));
    }
    s.mailbox.remove_account(id)?;
    if s.mailbox.folder(s.view.folder).is_none() {
        s.view.folder = s.mailbox.default_account().and_then(|a| s.mailbox.folder_of_kind(a.id, FolderKind::Inbox)).unwrap_or(0);
    }
    Ok(json!({"deleted": id}))
}

/// The account as a synchronisation signs in with it, and the name its outgoing server is
/// signed in to with (`SyncRequest::send_login`; empty for an account of its own).
///
/// A shared mailbox takes its owner's servers and Microsoft sign-in, with its own address as the
/// name the incoming server is signed in to with; the host brings the owner's access token. Any
/// other account is as it is.
pub fn signed_in_as(s: &Session, account: &Account) -> Result<(Account, String), CmdError> {
    let Some(parent_id) = account.shared_of else { return Ok((account.clone(), String::new())) };
    let parent = s
        .mailbox
        .account(parent_id)
        .filter(|parent| parent.id != account.id)
        .ok_or_else(|| fail(format!("the account {} was opened through is gone: remove it and add it again", account.email)))?;
    can_open_shared(parent).map_err(|why| fail(format!("{} can't be opened: {why}", account.email)))?;
    let login = parent.login().to_string();
    let signed = Account {
        kind: AccountKind::Imap,
        incoming: parent.incoming.clone(),
        outgoing: parent.outgoing.clone(),
        oauth: parent.oauth.clone(),
        username: account.email.trim().to_string(),
        ..account.clone()
    };
    Ok((signed, login))
}

/// Whether shared mailboxes can be opened under `account` (it signs in with Microsoft at work or
/// school, and is no shared mailbox itself).
pub fn opens_shared(s: &Session, account: Id) -> bool {
    s.mailbox.account(account).is_some_and(|a| can_open_shared(a).is_ok())
}

/// The account whose sign-in a synchronisation of `account` uses: its owner's for a shared
/// mailbox, its own for any other.
pub fn signer(s: &Session, account: Id) -> Id {
    s.mailbox.account(account).and_then(|a| a.shared_of).filter(|parent| s.mailbox.account(*parent).is_some()).unwrap_or(account)
}

#[cfg(test)]
#[path = "shared_tests.rs"]
mod tests;
