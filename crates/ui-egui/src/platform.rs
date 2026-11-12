// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The desktop's [`Platform`]: the window's own [`Services`], asked by the names the host
//! crate gives them. Nothing here does anything the window didn't already do; an entry the
//! desktop doesn't have is left to the trait's default.

use std::sync::Arc;

use mailcraft_host::{Notification, NotificationKind, Platform, PlatformError, SecretStore};

use crate::Services;
use crate::alerts::{AlertKind, DesktopAlert, Opens};

impl Platform for Services {
    fn secrets(&self) -> Option<Arc<dyn SecretStore>> {
        self.secrets.clone()
    }

    fn open_sign_in_page(&self, url: &str) -> Result<(), PlatformError> {
        match &self.open_page {
            Some(open) if open(url) => Ok(()),
            Some(_) => Err(PlatformError::Failed("no browser could be started".to_string())),
            None => Err(PlatformError::Unsupported),
        }
    }

    /// Only a reminder: what a mail alert opens is the window's to say, so it raises those itself.
    fn notify(&self, notification: &Notification) -> Result<(), PlatformError> {
        let (Some(host), NotificationKind::Reminder) = (&self.alert, notification.kind) else {
            return Err(PlatformError::Unsupported);
        };
        let alert = DesktopAlert {
            key: notification.key,
            kind: AlertKind::Reminder,
            title: notification.title.clone(),
            body: notification.body.clone(),
            opens: Opens::Reminders,
        };
        host.show(&alert).map_err(PlatformError::Failed)
    }

    fn open_file(&self, name: &str, bytes: &[u8]) -> Result<(), PlatformError> {
        match &self.open_file {
            Some(open) => open(name, bytes).map_err(PlatformError::Failed),
            None => Err(PlatformError::Unsupported),
        }
    }
}
