// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! GOA public metadata over D-Bus, queried only from the explicit import action (#989).
//! Specs: https://gnome.pages.gitlab.gnome.org/gnome-online-accounts/
//! No token/password methods, GOA files, libgoa, or automatic account synchronisation.

use std::collections::HashMap;
use std::time::Duration;

use mailcraft_ui_egui::gnome_accounts::{Account, Discovery, DiscoveryError, Provider};
use zbus::zvariant::OwnedValue;

const SERVICE: &str = "org.gnome.OnlineAccounts";
const PATH: &str = "/org/gnome/OnlineAccounts";
const ACCOUNT: &str = "org.gnome.OnlineAccounts.Account";
const MAIL: &str = "org.gnome.OnlineAccounts.Mail";
type Properties = HashMap<String, OwnedValue>;

pub fn discover() -> Discovery {
    let bus = zbus::blocking::connection::Builder::session()
        .map_err(|_| DiscoveryError::Unavailable)?
        .method_timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| DiscoveryError::Unavailable)?;
    read(&bus)
}

fn read(bus: &zbus::blocking::Connection) -> Discovery {
    let proxy = zbus::blocking::proxy::Builder::<zbus::blocking::Proxy<'_>>::new(bus)
        .destination(SERVICE)
        .and_then(|p| p.path(PATH))
        .and_then(|p| p.interface("org.freedesktop.DBus.ObjectManager"))
        .map_err(|_| DiscoveryError::Failed)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .map_err(|_| DiscoveryError::Failed)?;
    let objects: zbus::fdo::ManagedObjects = proxy.call("GetManagedObjects", &()).map_err(|_| DiscoveryError::Failed)?;
    parse(objects)
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
        accounts.push(Account { email: email.to_owned(), name, provider_name, provider });
    }
    if accounts.len() > 128 {
        return Err(DiscoveryError::TooManyAccounts);
    }
    accounts.sort_by_key(|a| (a.email.to_ascii_lowercase(), a.provider, a.provider_name.clone()));
    accounts.dedup_by(|a, b| a.email.eq_ignore_ascii_case(&b.email) && a.provider == b.provider);
    Ok(accounts)
}

#[cfg(test)]
mod tests;
