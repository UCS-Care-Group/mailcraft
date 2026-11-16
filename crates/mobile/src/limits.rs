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

use crate::Core;
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

/// What the phone holds of mail, against its cap: for Settings.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DeviceStorage {
    /// The most megabytes of mail the phone keeps, all accounts together.
    pub cap_mb: u64,
    /// About how many bytes of mail are kept on the phone now (what the cap is measured by).
    pub used_bytes: u64,
    /// How many messages are kept.
    pub messages: u64,
    /// The caps Settings offers, smallest first, the present one among them.
    pub choices_mb: Vec<u64>,
}

/// The caps offered, in megabytes (all within `account.deviceLimits`' bounds).
const STORAGE_CHOICES_MB: [u64; 6] = [250, 500, 1024, 2048, 5120, 10240];

/// What the phone holds now, against a cap of `cap_mb`.
pub(crate) fn storage_of(session: &Session) -> DeviceStorage {
    let cap = session.mailbox.device_limits.map_or(PHONE_STORAGE, |l| l.storage);
    let cap_mb = cap >> 20;
    let used_bytes = session.mailbox.messages.values().map(|m| mailcraft_engine::cmd::device_limits::local_bytes(m)).fold(0u64, u64::saturating_add);
    let mut choices_mb = STORAGE_CHOICES_MB.to_vec();
    if !choices_mb.contains(&cap_mb) {
        choices_mb.push(cap_mb);
        choices_mb.sort_unstable();
    }
    DeviceStorage { cap_mb, used_bytes, messages: u64::try_from(session.mailbox.messages.len()).unwrap_or(u64::MAX), choices_mb }
}

/// Messages one step of keeping within the limits thins or takes off the phone: a mailbox moved
/// to the limits from years of headers gets there in a few steps, none holding the session long.
pub(crate) const TRIM_BATCH: u64 = 5_000;
/// How often the limits are looked at again while nothing asks for it.
pub(crate) const TRIM_EVERY: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// When the mailbox was last kept within the phone's limits.
#[derive(Debug, Default)]
pub(crate) struct Trimming {
    /// Due at the next wake: limits changed, a round ended, or more was left.
    pub(crate) due: bool,
    at: Option<std::time::Instant>,
}

impl Trimming {
    /// One step, when one is due: `account.trimDevice` a batch at a time. True when more is left.
    pub(crate) fn step(&mut self, session: &mut Session) -> bool {
        let stale = self.at.is_none_or(|at| at.elapsed() >= TRIM_EVERY);
        if !(self.due || stale) || session.mailbox.device_limits.is_none() {
            return false;
        }
        (self.due, self.at) = (false, Some(std::time::Instant::now()));
        match session.run("account.trimDevice", &serde_json::json!({"most": TRIM_BATCH})) {
            Ok(said) if said.get("left").and_then(serde_json::Value::as_u64).unwrap_or(0) > 0 => {
                self.due = true;
                true
            }
            Ok(_) => false,
            Err(e) => {
                log::warn!("phone: the mailbox couldn't be kept within its limits: {e}");
                false
            }
        }
    }
}

/// The Microsoft account `account` of the mailbox, not a shared mailbox (it follows its owner's).
fn own_account(session: &Session, account: u64) -> Result<(), crate::CoreError> {
    let found = session.mailbox.accounts.iter().any(|a| a.id == account && a.kind == AccountKind::Microsoft && !a.is_shared());
    if found { Ok(()) } else { Err(crate::CoreError::BadRequest { reason: "there is no such account".to_string() }) }
}

#[uniffi::export]
impl Core {
    /// What the phone keeps unless the owner chooses otherwise: what Add Account offers first.
    pub fn default_download_settings(&self) -> DownloadSettings {
        DownloadSettings::default()
    }

    /// What the phone holds of mail against its cap, for Settings.
    pub async fn device_storage(&self) -> Result<DeviceStorage, crate::CoreError> {
        self.call(|s| Ok(storage_of(s))).await
    }

    /// Set the phone's storage cap in megabytes (brought within 50 MB to 64 GB) through
    /// `account.deviceLimits`, whatever the accounts' windows. A smaller cap takes the oldest mail
    /// back to its headers, then off the phone (never off the server), between runs. Answers what
    /// the phone holds now.
    pub async fn set_storage_cap(&self, megabytes: u64) -> Result<DeviceStorage, crate::CoreError> {
        let kept = self
            .call(move |s| {
                set_storage(s, megabytes)?;
                Ok(storage_of(s))
            })
            .await?;
        self.sync.trim_soon();
        Ok(kept)
    }

    /// What the phone keeps of `account`'s mail: its window, and the phone's storage cap.
    pub async fn download_settings(&self, account: u64) -> Result<DownloadSettings, crate::CoreError> {
        self.call(move |s| {
            own_account(s, account)?;
            let storage = s.mailbox.device_limits.map_or(PHONE_STORAGE, |l| l.storage);
            Ok(DownloadSettings { window: s.mailbox.keep_offline_of(account).into(), storage_mb: storage >> 20 })
        })
        .await
    }

    /// Change what the phone keeps of `account`'s mail. A shorter window or a smaller cap takes
    /// what is now outside it off the phone (never off the server) between runs; a longer window
    /// lists the account's folders afresh, bringing the older mail it now covers. The cap is the
    /// phone's, for every account, brought within 50 MB to 64 GB.
    pub async fn set_download_settings(&self, account: u64, settings: DownloadSettings) -> Result<(), crate::CoreError> {
        self.call(move |s| {
            own_account(s, account)?;
            let keep = KeepOffline::from(settings.window).code();
            s.run("account.edit", &serde_json::json!({"id": account, "keepOffline": keep}))?;
            set_storage(s, settings.storage_mb)?;
            Ok(())
        })
        .await?;
        self.sync.trim_soon();
        Ok(())
    }
}
