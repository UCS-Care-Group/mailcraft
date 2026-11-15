// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the phone keeps of a mailbox (MB3.16): a phone can't hold years of a work mailbox, so it
//! keeps less than a computer does, and its owner can change how much.
//!
//! - **The phone's defaults.** A Microsoft account added on the phone keeps one month of mail in
//!   full ([`PHONE_KEEP`]), fetches a message whole in the background only up to 1 MB
//!   ([`PHONE_DOWNLOAD`]; attachments come when opened), and its mail older than the month isn't
//!   listed at all, not even its headers ([`mailcraft_model::Mailbox::listed_since`]).
//! - **Caps.** At most 500 MB of mail on the phone's disk, and a number of messages worked out
//!   from what the platform says the app may hold in memory ([`DeviceLimits::phone`]).
//! - **Once.** A mailbox kept on a phone before these defaults (its accounts at a computer's 12
//!   months and 10 MB, every header of their history listed) is moved to them the first time the
//!   core opens it, and thinned to them; never again after that, so that what the owner chooses
//!   later stands.

use mailcraft_engine::Session;
use mailcraft_model::{AccountKind, DeviceLimits, KeepOffline, PHONE_DEFAULTS, PHONE_DOWNLOAD, PHONE_KEEP, PHONE_STORAGE};

/// Move the mailbox to the phone's defaults if it hasn't been yet: its limits, and each Microsoft
/// account's window and download size. True when something changed (thinning is then due).
pub(crate) fn settle(session: &mut Session, memory_budget: Option<u64>) -> bool {
    let before = session.mailbox.device_limits;
    if before.is_some_and(|l| l.defaults >= PHONE_DEFAULTS) {
        return false;
    }
    // (A cap of storage the mailbox already had is kept: the owner chose it.)
    let storage = before.map_or(PHONE_STORAGE, |l| l.storage);
    session.mailbox.device_limits = Some(DeviceLimits { storage, ..DeviceLimits::phone(memory_budget) }.clamped());
    // Not an undo step: the mailbox was set up before the phone had defaults of its own.
    let accounts: Vec<u64> = session.mailbox.accounts.iter().filter(|a| a.kind == AccountKind::Microsoft && !a.is_shared()).map(|a| a.id).collect();
    for id in accounts {
        if let Ok(a) = session.mailbox.account_mut(id) {
            a.keep_offline = PHONE_KEEP;
            a.download_in_full = PHONE_DOWNLOAD;
        }
    }
    session.mailbox_changed();
    log::info!("phone: the mailbox was moved to the phone's limits");
    true
}

/// How far back mail is kept on the phone, in the steps the app offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum KeepWindow {
    Days3,
    Week1,
    Weeks2,
    Month1,
    Months3,
    Months6,
    Months12,
    Years2,
    Years5,
    Years10,
    All,
}

impl From<KeepWindow> for KeepOffline {
    fn from(w: KeepWindow) -> KeepOffline {
        match w {
            KeepWindow::Days3 => KeepOffline::Days3,
            KeepWindow::Week1 => KeepOffline::Week1,
            KeepWindow::Weeks2 => KeepOffline::Weeks2,
            KeepWindow::Month1 => KeepOffline::Month1,
            KeepWindow::Months3 => KeepOffline::Months3,
            KeepWindow::Months6 => KeepOffline::Months6,
            KeepWindow::Months12 => KeepOffline::Months12,
            KeepWindow::Years2 => KeepOffline::Years2,
            KeepWindow::Years5 => KeepOffline::Years5,
            KeepWindow::Years10 => KeepOffline::Years10,
            KeepWindow::All => KeepOffline::All,
        }
    }
}

impl From<KeepOffline> for KeepWindow {
    fn from(k: KeepOffline) -> KeepWindow {
        match k {
            KeepOffline::Days3 => KeepWindow::Days3,
            KeepOffline::Week1 => KeepWindow::Week1,
            KeepOffline::Weeks2 => KeepWindow::Weeks2,
            KeepOffline::Month1 => KeepWindow::Month1,
            KeepOffline::Months3 => KeepWindow::Months3,
            KeepOffline::Months6 => KeepWindow::Months6,
            KeepOffline::Months12 => KeepWindow::Months12,
            KeepOffline::Years2 => KeepWindow::Years2,
            KeepOffline::Years5 => KeepWindow::Years5,
            KeepOffline::Years10 => KeepWindow::Years10,
            KeepOffline::All => KeepWindow::All,
        }
    }
}

/// What the phone keeps of an account's mail: chosen in Add Account, changed later in Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DownloadSettings {
    /// How far back the account's mail is on the phone, kept in full. Older mail isn't listed.
    pub window: KeepWindow,
    /// The most megabytes of mail the phone keeps, all accounts together.
    pub storage_mb: u64,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        DownloadSettings { window: PHONE_KEEP.into(), storage_mb: PHONE_STORAGE >> 20 }
    }
}

/// The settings of a new account, given it as a command each: its window and download size
/// (`account.edit`), and the phone's storage cap (`account.deviceLimits`).
pub(crate) fn apply_to(session: &mut Session, account: u64, settings: DownloadSettings) -> Result<(), mailcraft_engine::CmdError> {
    let keep = KeepOffline::from(settings.window).code();
    let size = serde_json::to_value(PHONE_DOWNLOAD).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    session.run("account.edit", &serde_json::json!({"id": account, "keepOffline": keep, "downloadInFull": size}))?;
    set_storage(session, settings.storage_mb)
}

/// The phone's storage cap, in megabytes, within the bounds `account.deviceLimits` takes.
pub(crate) fn set_storage(session: &mut Session, megabytes: u64) -> Result<(), mailcraft_engine::CmdError> {
    use mailcraft_model::device_limits::{MAX_STORAGE, MIN_STORAGE};
    let bytes = megabytes.saturating_mul(1 << 20).clamp(MIN_STORAGE, MAX_STORAGE);
    session.run("account.deviceLimits", &serde_json::json!({"storage": bytes})).map(|_| ())
}
