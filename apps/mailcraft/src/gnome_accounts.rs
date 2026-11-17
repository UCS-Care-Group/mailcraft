// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Public GOA metadata for explicit import and credentials for user-selected linked accounts (#989).
//! Specs: https://gnome.pages.gitlab.gnome.org/gnome-online-accounts/
//! Credentials are resolved only for explicitly linked accounts, never copied to local storage.

use std::collections::HashMap;
use std::time::Duration;

use mailcraft_i18n::{Text, said};
use mailcraft_model::sync::{SyncRequest, SyncResult};
use mailcraft_model::{Account as MailAccount, AccountKind, GoaAccount, OAuth, OAuthProvider, Security, Server};
use mailcraft_ui_egui::gnome_accounts::{Account, Discovery, DiscoveryError, Provider};
use std::sync::atomic::{AtomicBool, Ordering};
use zbus::zvariant::OwnedValue;
const OAUTH: &str = "org.gnome.OnlineAccounts.OAuth2Based";
const PASSWORD: &str = "org.gnome.OnlineAccounts.PasswordBased";

const SERVICE: &str = "org.gnome.OnlineAccounts";
const PATH: &str = "/org/gnome/OnlineAccounts";
const ACCOUNT: &str = "org.gnome.OnlineAccounts.Account";
const MAIL: &str = "org.gnome.OnlineAccounts.Mail";
type Properties = HashMap<String, OwnedValue>;
mod metadata;

pub fn discover() -> Discovery {
    let bus = zbus::blocking::connection::Builder::session()
        .map_err(|_| DiscoveryError::Unavailable)?
        .method_timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| DiscoveryError::Unavailable)?;
    read(&bus)
}

fn read(bus: &zbus::blocking::Connection) -> Discovery {
    parse(metadata::snapshot(bus)?)
}

fn text<'a>(props: &'a Properties, key: &str) -> Option<&'a str> {
    let value = props.get(key)?;
    <&str>::try_from(value).ok()
}

fn clean(value: &str) -> bool {
    value.chars().count() <= 254 && !value.chars().any(char::is_control)
}

fn email_valid(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else { return false };
    value.len() <= 254
        && clean(value)
        && !local.is_empty()
        && !value.chars().any(|c| c.is_whitespace() || mailcraft_names::is_format(c))
        && mailcraft_model::provider::is_domain(domain)
}

fn parse(objects: zbus::fdo::ManagedObjects) -> Discovery {
    if objects.len() > 256 {
        return Err(DiscoveryError::TooManyAccounts);
    }
    let mut accounts = Vec::new();
    for (path, interfaces) in objects {
        if !path.as_str().starts_with("/org/gnome/OnlineAccounts/Accounts/") {
            continue;
        }
        let (Some(account), Some(mail)) = (interfaces.get(ACCOUNT), interfaces.get(MAIL)) else { continue };
        // The Mail interface may disappear asynchronously after the setting changes.
        if account.get("MailDisabled").and_then(|v| bool::try_from(v).ok()) != Some(false) {
            continue;
        }
        let Some(email) = text(mail, "EmailAddress").filter(|v| email_valid(v)) else { continue };
        let name = text(mail, "Name").and_then(|v| mailcraft_names::incoming(v, 254)).unwrap_or_default();
        let provider_name = text(account, "ProviderName").and_then(|v| mailcraft_names::incoming(v, 80)).unwrap_or_default();
        let provider = match text(account, "ProviderType") {
            Some("google") => Provider::Google,
            Some("ms_graph") => Provider::Microsoft365,
            Some("windows_live") => Provider::OutlookCom,
            _ => Provider::Unsupported,
        };
        let linked = configuration(account, mail, &interfaces, email, &name);
        accounts.push(Account { email: email.to_owned(), name, provider_name, provider, linked });
    }
    if accounts.len() > 128 {
        return Err(DiscoveryError::TooManyAccounts);
    }
    accounts.sort_by_key(|a| {
        (
            a.email.to_ascii_lowercase(),
            a.provider,
            a.linked.is_none(),
            a.linked.as_ref().and_then(|a| a.goa.as_ref()).map(|g| g.id.clone()),
            a.provider_name.clone(),
        )
    });
    accounts.dedup_by(|a, b| a.email.eq_ignore_ascii_case(&b.email) && a.provider == b.provider);
    Ok(accounts)
}

fn flag(props: &Properties, key: &str) -> bool {
    props.get(key).and_then(|v| bool::try_from(v).ok()) == Some(true)
}

fn server(props: &Properties, prefix: &str) -> Option<Server> {
    if !flag(props, &format!("{prefix}Supported")) || flag(props, &format!("{prefix}AcceptSslErrors")) {
        return None;
    }
    let security = if flag(props, &format!("{prefix}UseSsl")) {
        Security::Tls
    } else if flag(props, &format!("{prefix}UseTls")) {
        Security::StartTls
    } else {
        return None;
    };
    let default_port = match (prefix, security) {
        ("Imap", Security::Tls) => 993,
        ("Imap", _) => 143,
        (_, Security::Tls) => 465,
        _ => 587,
    };
    let address = text(props, &format!("{prefix}Host"))?;
    if address.len() > 260 || !clean(address) {
        return None;
    }
    // GOA permits an explicit numeric port, including bracketed IPv6. No service-name guessing.
    let (host, port) = if let Some(rest) = address.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        host.parse::<std::net::Ipv6Addr>().ok()?;
        let port = if tail.is_empty() { default_port } else { tail.strip_prefix(':')?.parse::<u16>().ok()? };
        (host, port)
    } else if let Some((host, port)) = address.rsplit_once(':') {
        (host, port.parse::<u16>().ok()?)
    } else {
        (address, default_port)
    };
    if port == 0 || !(mailcraft_model::provider::is_domain(host) || host.parse::<std::net::IpAddr>().is_ok()) {
        return None;
    }
    Some(Server { host: host.to_owned(), port, security })
}

fn configuration(
    account: &Properties,
    mail: &Properties,
    interfaces: &HashMap<zbus::names::OwnedInterfaceName, Properties>,
    email: &str,
    name: &str,
) -> Option<MailAccount> {
    let id = text(account, "Id")?;
    let provider = text(account, "ProviderType")?;
    let valid = |s: &str| !s.is_empty() && s.len() <= 128 && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
    if !valid(id) || !valid(provider) {
        return None;
    }
    if provider == "ms_graph" && interfaces.contains_key(OAUTH) {
        return Some(MailAccount {
            email: email.to_owned(),
            display_name: name.to_owned(),
            kind: AccountKind::Microsoft,
            goa: Some(GoaAccount { id: id.to_owned(), provider: provider.to_owned() }),
            ..Default::default()
        });
    }
    if !flag(mail, "SmtpUseAuth") {
        return None;
    }
    let incoming = server(mail, "Imap")?;
    let outgoing = server(mail, "Smtp")?;
    let oauth = interfaces.contains_key(OAUTH);
    if oauth {
        // Only public mail protocol capabilities. In particular an ms_graph token is not an
        // Outlook IMAP token, and is never handed to MailCraft's Graph/calendar backends.
        let allowed = match provider {
            "google" => incoming.host.eq_ignore_ascii_case("imap.gmail.com") && outgoing.host.eq_ignore_ascii_case("smtp.gmail.com"),
            "windows_live" => {
                incoming.host.eq_ignore_ascii_case("outlook.office365.com") && outgoing.host.eq_ignore_ascii_case("smtp-mail.outlook.com")
            }
            _ => false,
        };
        if !allowed || !flag(mail, "SmtpAuthXoauth2") {
            return None;
        }
    } else if !interfaces.contains_key(PASSWORD) || !(flag(mail, "SmtpAuthPlain") || flag(mail, "SmtpAuthLogin")) {
        return None;
    }
    let username = text(mail, "ImapUserName").filter(|s| !s.is_empty() && clean(s))?;
    text(mail, "SmtpUserName").filter(|s| !s.is_empty() && clean(s))?;
    Some(MailAccount {
        email: email.to_owned(),
        display_name: name.to_owned(),
        username: username.to_owned(),
        kind: AccountKind::Imap,
        incoming: Some(incoming),
        outgoing: Some(outgoing),
        goa: Some(GoaAccount { id: id.to_owned(), provider: provider.to_owned() }),
        ..Default::default()
    })
}

// No property cache: credential calls must not implicitly fetch OAuth client properties.
fn uncached<'a>(bus: &'a zbus::blocking::Connection, path: &'a str, interface: &'a str) -> zbus::Result<zbus::blocking::Proxy<'a>> {
    zbus::blocking::proxy::Builder::<zbus::blocking::Proxy<'a>>::new(bus)
        .destination(SERVICE)?
        .path(path)?
        .interface(interface)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
}

/// Each run validates fresh public metadata, then asks GOA to ensure/refresh its credentials.
/// Only the returned in-memory request reaches the mail transport. No refresh tokens or
/// application ids are requested. Errors never expose D-Bus/provider text.
struct Checked {
    account: MailAccount,
    path: zbus::zvariant::OwnedObjectPath,
    oauth: bool,
    smtp_username: Option<String>,
}

fn checked(bus: &zbus::blocking::Connection, account: &MailAccount, cancel: &AtomicBool) -> Result<Checked, Text> {
    let linked = account.goa.as_ref().ok_or_else(|| said!("This account is not linked to GNOME."))?;
    let accounts = read(bus).map_err(|_| said!("GNOME Online Accounts is unavailable. Open GNOME Online Accounts and try Send/Receive again."))?;
    let current =
        accounts.iter().filter_map(|a| a.linked.as_ref()).find(|a| a.goa.as_ref() == Some(linked)).ok_or_else(|| {
            said!("This GNOME account is missing, disabled, or no longer exposes supported mail access. Check GNOME Online Accounts.")
        })?;
    if !current.email.eq_ignore_ascii_case(&account.email)
        || current.kind != account.kind
        || current.username != account.username
        || current.incoming != account.incoming
        || current.outgoing != account.outgoing
    {
        return Err(said!(
            "The GNOME account connection changed. Remove it from MailCraft and link it again to keep mail from different servers separate."
        ));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(said!("GNOME account request cancelled."));
    }
    // Resolve the path from public Id rather than constructing it from untrusted file contents.
    let objects = metadata::snapshot(bus).map_err(|_| said!("Could not read GNOME accounts."))?;
    if objects.len() > 256 {
        return Err(said!("GNOME returned too many accounts. Use Add Account instead."));
    }
    let (path, interfaces) = objects
        .iter()
        .find(|(path, interfaces)| {
            if !path.as_str().starts_with("/org/gnome/OnlineAccounts/Accounts/") {
                return false;
            }
            interfaces
                .get(ACCOUNT)
                .is_some_and(|p| text(p, "Id") == Some(linked.id.as_str()) && text(p, "ProviderType") == Some(linked.provider.as_str()))
        })
        .ok_or_else(|| said!("The linked GNOME account is no longer available."))?;
    // Revalidate this snapshot too: credentials and configuration must refer to the same object.
    let props = interfaces.get(ACCOUNT).ok_or_else(|| said!("The GNOME account is unavailable."))?;
    let mail = interfaces.get(MAIL).ok_or_else(|| said!("Mail is disabled in GNOME Online Accounts."))?;
    if props.get("MailDisabled").and_then(|v| bool::try_from(v).ok()) != Some(false)
        || configuration(props, mail, interfaces, &current.email, &current.display_name).as_ref() != Some(current)
        || text(mail, "EmailAddress") != Some(current.email.as_str())
    {
        return Err(said!("The GNOME account changed. Try again."));
    }
    let proxy = uncached(bus, path.as_str(), ACCOUNT).map_err(|_| said!("The GNOME account is unavailable."))?;
    let _: i32 = proxy
        .call("EnsureCredentials", &())
        .map_err(|_| said!("The account needs attention. Repair its sign-in in GNOME Online Accounts and try again."))?;
    if cancel.load(Ordering::Relaxed) {
        return Err(said!("GNOME account request cancelled."));
    }
    Ok(Checked {
        account: current.clone(),
        path: path.clone(),
        oauth: interfaces.contains_key(OAUTH),
        smtp_username: text(mail, "SmtpUserName").map(str::to_owned),
    })
}

fn resolve(bus: &zbus::blocking::Connection, request: &SyncRequest, cancel: &AtomicBool) -> Result<(SyncRequest, Option<Text>), Text> {
    let checked = checked(bus, &request.account, cancel)?;
    let current = &checked.account;
    let path = &checked.path;
    let linked = current.goa.as_ref().ok_or_else(|| said!("This account is not linked to GNOME."))?;
    if current.kind != AccountKind::Imap {
        return Err(said!("This GNOME account uses Microsoft Graph, not IMAP or SMTP."));
    }
    let mut resolved = request.clone();
    let mut outgoing_error = None;
    resolved.account.goa = None;
    resolved.account.oauth = None;
    resolved.account.outgoing_without_sign_in = false;
    resolved.account.display_name = current.display_name.clone();
    resolved.password.clear();
    resolved.access_token.clear();
    resolved.smtp_password = None;
    resolved.smtp_username = checked.smtp_username;
    resolved.gmail_moves.clear(); // No provider API access with GNOME's mail token.
    if checked.oauth {
        let proxy = uncached(bus, path.as_str(), OAUTH).map_err(|_| said!("GNOME sign-in is unavailable."))?;
        let (token, _expires): (String, i32) =
            proxy.call("GetAccessToken", &()).map_err(|_| said!("Repair this account's sign-in in GNOME Online Accounts."))?;
        if token.is_empty() || token.len() > 65536 {
            return Err(said!("GNOME returned an invalid mail credential."));
        }
        resolved.access_token = token;
        resolved.account.oauth = Some(OAuth {
            provider: if linked.provider == "google" { OAuthProvider::Google } else { OAuthProvider::Microsoft },
            ..Default::default()
        });
    } else {
        let proxy = uncached(bus, path.as_str(), PASSWORD).map_err(|_| said!("GNOME sign-in is unavailable."))?;
        let password: String =
            proxy.call("GetPassword", &("imap-password",)).map_err(|_| said!("Repair this account's sign-in in GNOME Online Accounts."))?;
        if password.is_empty() || password.len() > 65536 {
            return Err(said!("GNOME returned an invalid mail credential."));
        }
        resolved.password = password;
        if cancel.load(Ordering::Relaxed) {
            return Err(said!("GNOME account request cancelled."));
        }
        if !request.outbox.is_empty() {
            let password: Result<String, _> = proxy.call("GetPassword", &("smtp-password",));
            match password {
                Ok(password) if !password.is_empty() && password.len() <= 65536 => resolved.smtp_password = Some(password),
                other => {
                    outgoing_error = Some(if other.is_err() {
                        said!("Repair this account's outgoing sign-in in GNOME Online Accounts.")
                    } else {
                        said!("GNOME returned an invalid outgoing credential.")
                    });
                    // No send is attempted, and the original queue is retained. Receiving and
                    // IMAP changes may proceed with the independently resolved incoming secret.
                    resolved.outbox.clear();
                }
            }
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(said!("GNOME account request cancelled."));
    }
    Ok((resolved, outgoing_error))
}

fn bus() -> Result<zbus::blocking::Connection, Text> {
    zbus::blocking::connection::Builder::session()
        .ok()
        .map(|builder| builder.method_timeout(Duration::from_secs(30)))
        .and_then(|builder| builder.build().ok())
        .ok_or_else(|| said!("GNOME Online Accounts is unavailable. Open GNOME Online Accounts and try again."))
}

fn graph_token(account: &MailAccount, cancel: &AtomicBool) -> Result<mailcraft_net::oauth::GraphTokens, mailcraft_net::NetError> {
    let get = || -> Result<mailcraft_net::oauth::GraphTokens, Text> {
        let bus = bus()?;
        let checked = checked(&bus, account, cancel)?;
        if checked.account.kind != AccountKind::Microsoft || !checked.account.goa.as_ref().is_some_and(|g| g.provider == "ms_graph") || !checked.oauth
        {
            return Err(said!("This GNOME account does not expose Microsoft Graph mail access."));
        }
        let proxy = uncached(&bus, checked.path.as_str(), OAUTH).map_err(|_| said!("GNOME sign-in is unavailable."))?;
        let (access, expires): (String, i32) =
            proxy.call("GetAccessToken", &()).map_err(|_| said!("Repair this account's sign-in in GNOME Online Accounts."))?;
        if access.is_empty() || access.len() > 65536 {
            return Err(said!("GNOME returned an invalid mail credential."));
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(said!("GNOME account request cancelled."));
        }
        Ok(mailcraft_net::oauth::GraphTokens {
            tokens: mailcraft_model::sync::Tokens {
                access,
                email: checked.account.email,
                expires_at: mailcraft_time::now().saturating_add(i64::from(expires.clamp(0, 86400))),
                ..Default::default()
            },
            extras_missing: false,
            covers_favorites: false,
        })
    };
    get().map_err(mailcraft_net::NetError::Config)
}

/// Only the selected account's mail is run with GOA's Graph profile. The ordinary Graph host
/// retains its own OAuth registration; this path never supplies it a refresh token.
pub fn graph(request: mailcraft_host::graph::GraphRequest) -> mailcraft_host::graph::GraphResult {
    let account = request.account.clone();
    let cancel = request.cancel.clone();
    let mut result = mailcraft_host::graph::managed_mail(request, &|_, _, _| graph_token(&account, &cancel));
    if result.consent_needed || result.sign_in_refused || result.token_refused {
        result
            .errors
            .push(said!("Repair this account's sign-in or mail permissions in GNOME Online Accounts, then try Send/Receive again.").to_string());
        result.consent_needed = false;
        result.sign_in_refused = false;
    }
    result
}

pub fn fetch(mut job: mailcraft_host::fetch::GraphFetchJob) -> mailcraft_model::sync::GraphFetchResult {
    let Some(account) = job.goa.take() else { return mailcraft_host::fetch::run(job) };
    let valid = !job.request.shared && job.request.account == account.id && job.request.email.eq_ignore_ascii_case(&account.email);
    let token = if valid {
        graph_token(&account, &job.cancel)
    } else {
        Err(mailcraft_net::NetError::Config(said!("This GNOME connection supports only the selected account's mail.")))
    };
    match token {
        Ok(got) => mailcraft_net::graph::fetch_messages(&got.tokens.access, &job.request, &job.cancel),
        Err(error) => mailcraft_model::sync::GraphFetchResult {
            account: job.request.account,
            email: job.request.email,
            errors: vec![error.to_string()],
            ..Default::default()
        },
    }
}

pub fn search(job: mailcraft_ui_egui::server_search::GraphSearchJob) -> mailcraft_model::sync::GraphSearchResult {
    let Some(account) = job.goa else { return mailcraft_net::graph::search_messages(&job.access, &job.request) };
    let valid = !job.request.shared && job.request.account == account.id && job.request.email.eq_ignore_ascii_case(&account.email);
    let token = if valid {
        graph_token(&account, &AtomicBool::new(false))
    } else {
        Err(mailcraft_net::NetError::Config(said!("This GNOME connection supports only the selected account's mail.")))
    };
    match token {
        Ok(got) => mailcraft_net::graph::search_messages(&got.tokens.access, &job.request),
        Err(error) => mailcraft_model::sync::GraphSearchResult {
            account: job.request.account,
            email: job.request.email,
            ticket: job.request.ticket,
            errors: vec![error.to_string()],
            ..Default::default()
        },
    }
}

pub fn sync(request: &SyncRequest, cancel: &AtomicBool, emit: &mut dyn FnMut(SyncResult)) {
    let result = zbus::blocking::connection::Builder::session()
        .ok()
        .map(|builder| builder.method_timeout(Duration::from_secs(30)))
        .and_then(|builder| builder.build().ok())
        .ok_or_else(|| said!("GNOME Online Accounts is unavailable. Open GNOME Online Accounts and try again."))
        .and_then(|bus| resolve(&bus, request, cancel));
    match result {
        Ok((resolved, outgoing_error)) => {
            if let Some(error) = outgoing_error {
                let mut result = SyncResult { account: request.account.id, email: request.account.email.clone(), ..Default::default() };
                result.problem(error);
                emit(result);
            }
            mailcraft_net::sync_with(&resolved, cancel, emit);
        }
        Err(error) => {
            let mut result = SyncResult {
                account: request.account.id,
                email: request.account.email.clone(),
                stopped: cancel.load(Ordering::Relaxed),
                ..Default::default()
            };
            result.problem(error);
            emit(result);
        }
    }
}

#[cfg(test)]
mod tests;
