// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Introspection names only, then Account/Mail properties. Never GetManagedObjects or OAuth
//! GetAll: their replies include registration properties that discovery has no reason to read.

use std::io::ErrorKind;
use std::time::{Duration, Instant};

use quick_xml::{Reader, events::Event};

use super::{ACCOUNT, DiscoveryError, MAIL, OAUTH, PASSWORD, PATH, Properties, uncached};

/// Direct children/interfaces of one introspection document; nested XML is never interpreted
/// as another object. Bounded even when the service sends a hostile document.
pub(super) fn names(xml: &str, tag: &str, limit: usize) -> Result<Vec<String>, DiscoveryError> {
    if xml.len() > 262_144 {
        return Err(DiscoveryError::TooManyAccounts);
    }
    let mut reader = Reader::from_str(xml);
    let mut depth = 0_usize;
    let mut names = Vec::new();
    let mut root_seen = false;
    for _ in 0..=xml.len() {
        let event = reader.read_event().map_err(|_| DiscoveryError::Failed)?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                if depth == 0 {
                    if root_seen || element.name().as_ref() != "node" {
                        return Err(DiscoveryError::Failed);
                    }
                    root_seen = true;
                }
                if depth == 1 && element.name().as_ref() == tag {
                    for attr in element.attributes() {
                        let attr = attr.map_err(|_| DiscoveryError::Failed)?;
                        if attr.key.as_ref() == "name" {
                            if names.len() >= limit {
                                return Err(DiscoveryError::TooManyAccounts);
                            }
                            let value = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).map_err(|_| DiscoveryError::Failed)?;
                            if value.len() > 256 {
                                return Err(DiscoveryError::Failed);
                            }
                            names.push(value.into_owned());
                        }
                    }
                }
                if !empty {
                    depth = depth.saturating_add(1);
                    if depth > 32 {
                        return Err(DiscoveryError::Failed);
                    }
                }
            }
            Event::End(_) => depth = depth.checked_sub(1).ok_or(DiscoveryError::Failed)?,
            Event::Eof => return if depth == 0 && root_seen { Ok(names) } else { Err(DiscoveryError::Failed) },
            _ => {}
        }
    }
    Err(DiscoveryError::Failed)
}

const BUDGET: Duration = Duration::from_secs(30);
const RETRY_DELAY: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BusFailure {
    Temporary,
    Unavailable,
    Denied,
    Invalid,
}

fn method_failure(name: &str) -> BusFailure {
    match name {
        "org.freedesktop.DBus.Error.NoReply"
        | "org.freedesktop.DBus.Error.Timeout"
        | "org.freedesktop.DBus.Error.TimedOut"
        | "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner" => BusFailure::Temporary,
        "org.freedesktop.DBus.Error.Disconnected" => BusFailure::Unavailable,
        "org.freedesktop.DBus.Error.AccessDenied" | "org.freedesktop.DBus.Error.AuthFailed" => BusFailure::Denied,
        _ => BusFailure::Invalid,
    }
}

fn bus_failure(error: &zbus::Error) -> BusFailure {
    match error {
        zbus::Error::MethodError(name, ..) => method_failure(name.as_str()),
        zbus::Error::FDO(error) => method_failure(zbus::DBusError::name(error.as_ref()).as_str()),
        zbus::Error::InputOutput(error) => match error.kind() {
            ErrorKind::TimedOut | ErrorKind::Interrupted | ErrorKind::WouldBlock => BusFailure::Temporary,
            _ => BusFailure::Unavailable,
        },
        _ => BusFailure::Invalid,
    }
}

struct MetadataRead {
    started: Instant,
    retried: bool,
}

impl MetadataRead {
    /// Only public, read-only calls share this retry. A failed credential call is never replayed.
    fn call<T>(&mut self, mut call: impl FnMut() -> zbus::Result<T>) -> Result<T, DiscoveryError> {
        for _ in 0..2 {
            if self.started.elapsed() >= BUDGET {
                return Err(DiscoveryError::Unavailable);
            }
            let failure = match call() {
                Ok(value) => return Ok(value),
                Err(error) => bus_failure(&error),
            };
            match failure {
                BusFailure::Temporary if !self.retried && self.started.elapsed().saturating_add(RETRY_DELAY) < BUDGET => {
                    self.retried = true;
                    std::thread::sleep(RETRY_DELAY);
                }
                BusFailure::Temporary | BusFailure::Unavailable => return Err(DiscoveryError::Unavailable),
                BusFailure::Denied => {
                    log::debug!("D-Bus denied access to GNOME account metadata");
                    return Err(DiscoveryError::Failed);
                }
                BusFailure::Invalid => return Err(DiscoveryError::Failed),
            }
        }
        Err(DiscoveryError::Unavailable)
    }

    fn introspect(&mut self, bus: &zbus::blocking::Connection, path: &str) -> Result<String, DiscoveryError> {
        self.call(|| uncached(bus, path, "org.freedesktop.DBus.Introspectable")?.call("Introspect", &()))
    }
}

pub(super) fn snapshot(bus: &zbus::blocking::Connection) -> Result<zbus::fdo::ManagedObjects, DiscoveryError> {
    let mut read = MetadataRead { started: Instant::now(), retried: false };
    let mut objects = zbus::fdo::ManagedObjects::new();
    if !names(&read.introspect(bus, PATH)?, "node", 256)?.iter().any(|child| child == "Accounts") {
        return Ok(objects);
    }
    let root = format!("{PATH}/Accounts");
    let children = names(&read.introspect(bus, &root)?, "node", 256)?;
    for child in children {
        if child.is_empty() || !child.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            return Err(DiscoveryError::Failed);
        }
        let path = format!("{root}/{child}");
        let interfaces = names(&read.introspect(bus, &path)?, "interface", 64)?;
        if ![ACCOUNT, MAIL].iter().all(|wanted| interfaces.iter().any(|name| name == wanted)) {
            continue;
        }
        let proxy = read.call(|| uncached(bus, &path, "org.freedesktop.DBus.Properties"))?;
        let mut metadata = std::collections::HashMap::new();
        for interface in [ACCOUNT, MAIL] {
            let props: Properties = read.call(|| proxy.call("GetAll", &(interface,)))?;
            if props.len() > 64 {
                return Err(DiscoveryError::TooManyAccounts);
            }
            metadata.insert(interface.try_into().map_err(|_| DiscoveryError::Failed)?, props);
        }
        for capability in [OAUTH, PASSWORD] {
            if interfaces.iter().any(|name| name == capability) {
                metadata.insert(capability.try_into().map_err(|_| DiscoveryError::Failed)?, Properties::new());
            }
        }
        drop(proxy);
        objects.insert(path.try_into().map_err(|_| DiscoveryError::Failed)?, metadata);
    }
    Ok(objects)
}

#[cfg(test)]
mod retry_tests;
