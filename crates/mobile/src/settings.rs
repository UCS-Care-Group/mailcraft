// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What Settings reads and writes besides the accounts: Mail to keep offline, a signature for
//! each account, the switches that belong to this device alone, and the estimate behind the
//! free-space question before a big sync.
//!
//! - Mail to keep offline is the engine's own setting of an account (`account.edit`,
//!   `keepOffline`); the phone starts new accounts at one month ([`MOBILE_KEEP_OFFLINE`]), the
//!   desktop at twelve. A shorter window takes nothing off the disk by itself: [`Core::thin_offline`]
//!   does, a few hundred messages a call, which the sync tick (and Settings, at once) calls.
//! - A signature is kept by the engine (`signature.set`) under the account's address, so that each
//!   account has its own on a phone; one an account already has under another name is edited as it is.
//! - The device settings are not the mailbox's: they stay on the device and never reach a server.
//!   They are one small JSON text that the app keeps in its private storage and reads back through
//!   [`parse_device_settings`], which trusts nothing in it. The two picture switches
//!   (`download_pictures`, `sender_images`) are the ones the reading pane and the lists read, so
//!   there is one place for each.
//! - [`offline_estimate`] is a rough size for a window, for a question asked before work that big.

use mailcraft_model::KeepOffline;
use serde_json::{Value, json};

use crate::text;
use crate::{Core, CoreError};

/// What a new account on the phone keeps offline: one month.
pub const MOBILE_KEEP_OFFLINE: &str = "1m";
/// Longest signature taken (characters): a signature is a few lines.
pub const MAX_SIGNATURE: usize = 4_000;
/// Most bytes of device settings that are read: more is the defaults.
const MAX_SETTINGS_BYTES: usize = 16 * 1024;
/// Most messages one thinning call takes back.
const MAX_THIN: u32 = 2_000;

/// The switches that belong to this device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct DeviceSettings {
    /// Download the pictures of every message without asking (off).
    pub download_pictures: bool,
    /// Show sender images (off).
    pub sender_images: bool,
    /// Notify for new mail only: no reminders' wording, no other kinds (off).
    pub new_mail_only: bool,
    /// Notify only for mail that would alert: Primary or Focused (off).
    pub primary_only: bool,
}

/// The settings of nothing yet: every switch off.
#[uniffi::export]
pub fn default_device_settings() -> DeviceSettings {
    DeviceSettings::default()
}

/// Read what [`device_settings_json`] wrote. Anything else (too long, not JSON, a field that is
/// not true or false) leaves that switch off: a file that has gone wrong never turns on what
/// sends something.
#[uniffi::export]
pub fn parse_device_settings(json: String) -> DeviceSettings {
    if json.len() > MAX_SETTINGS_BYTES {
        return DeviceSettings::default();
    }
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&json) else { return DeviceSettings::default() };
    let on = |key: &str| map.get(key).and_then(Value::as_bool).unwrap_or(false);
    DeviceSettings {
        download_pictures: on("downloadPictures"),
        sender_images: on("senderImages"),
        new_mail_only: on("newMailOnly"),
        primary_only: on("primaryOnly"),
    }
}

/// The settings as the text the app keeps.
#[uniffi::export]
pub fn device_settings_json(settings: DeviceSettings) -> String {
    json!({
        "downloadPictures": settings.download_pictures,
        "senderImages": settings.sender_images,
        "newMailOnly": settings.new_mail_only,
        "primaryOnly": settings.primary_only,
    })
    .to_string()
}

/// The windows of Mail to keep offline the phone offers, shortest first, as the codes
/// `account.edit` takes.
#[uniffi::export]
pub fn keep_offline_codes() -> Vec<String> {
    KeepOffline::STEPS.iter().map(|k| k.code().to_string()).collect()
}

/// What a new account keeps offline on the phone.
#[uniffi::export]
pub fn mobile_keep_offline() -> String {
    MOBILE_KEEP_OFFLINE.to_string()
}

/// About how many bytes of mail the window `code` brings: a rough figure for the question that is
/// asked before a big sync, never a promise. A month of an ordinary mailbox is taken as 40 MB
/// (messages, their pictures and small attachments), and the figure is capped at 20 GB. An unknown
/// code is the largest.
#[uniffi::export]
pub fn offline_estimate(code: String) -> u64 {
    const PER_MONTH: u64 = 40 * 1024 * 1024;
    const CAP: u64 = 20 * 1024 * 1024 * 1024;
    // In tenths of a month: three days is a tenth, a week a quarter (rounded to three), two weeks half.
    let tenths: u64 = match KeepOffline::parse(&code) {
        Some(KeepOffline::Days3) => 1,
        Some(KeepOffline::Week1) => 3,
        Some(KeepOffline::Weeks2) => 5,
        Some(KeepOffline::Month1) => 10,
        Some(KeepOffline::Months3) => 30,
        Some(KeepOffline::Months6) => 60,
        Some(KeepOffline::Months12) => 120,
        Some(KeepOffline::Years2) => 240,
        Some(KeepOffline::Years5) => 600,
        Some(KeepOffline::Years10) => 1200,
        Some(KeepOffline::All) | None => return CAP,
    };
    (tenths.saturating_mul(PER_MONTH) / 10).min(CAP)
}

/// How a change of an account's window came out.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct KeepOfflineChange {
    /// The window now, as its code.
    pub code: String,
    /// It is shorter than it was: older mail goes back to its headers ([`Core::thin_offline`]).
    pub shorter: bool,
    /// It is longer than it was: the newly covered mail is fetched by the next sync.
    pub longer: bool,
}

/// What a shorter window would take back, before the owner says yes.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct OfflineFreed {
    pub messages: u64,
    pub bytes: u64,
}

/// What thinning did.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ThinResult {
    pub thinned: u32,
    pub bytes: u64,
    /// Still to go.
    pub left: u32,
}

/// An account's signature as the phone edits it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AccountSignature {
    /// The words, as plain text (empty for none).
    pub text: String,
    /// False for a signature with a picture or its own HTML, which a plain text box would flatten.
    pub editable: bool,
}

fn bad(message: &str) -> CoreError {
    CoreError::BadRequest { reason: message.to_string() }
}

fn parse_code(code: &str) -> Result<KeepOffline, CoreError> {
    KeepOffline::parse(code).ok_or_else(|| bad("that is not a window for Mail to keep offline"))
}

fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

#[uniffi::export]
impl Core {
    /// The window of Mail to keep offline of `account`, as its code (a shared mailbox follows its owner's).
    pub async fn keep_offline(&self, account: u64) -> Result<String, CoreError> {
        self.call(move |s| {
            s.mailbox.account(account).ok_or_else(|| bad("there is no such account"))?;
            Ok(s.mailbox.keep_offline_of(account).code().to_string())
        })
        .await
    }

    /// Set the window of `account` (a Microsoft account, not a shared mailbox) through `account.edit`.
    pub async fn set_keep_offline(&self, account: u64, code: String) -> Result<KeepOfflineChange, CoreError> {
        let wanted = parse_code(&code)?;
        self.call(move |s| {
            let a = s.mailbox.account(account).ok_or_else(|| bad("there is no such account"))?;
            if a.is_shared() {
                return Err(bad("a shared mailbox keeps what the account it is opened through keeps"));
            }
            let was = s.mailbox.keep_offline_of(account);
            if was != wanted {
                s.run("account.edit", &json!({"id": account, "keepOffline": wanted.code()}))?;
            }
            Ok(KeepOfflineChange { code: wanted.code().to_string(), shorter: wanted < was, longer: wanted > was })
        })
        .await
    }

    /// How much the window `code` would take back for `account` now; nothing changes.
    pub async fn offline_freed(&self, account: u64, code: String) -> Result<OfflineFreed, CoreError> {
        let wanted = parse_code(&code)?;
        self.call(move |s| {
            let a = s.mailbox.account(account).ok_or_else(|| bad("there is no such account"))?;
            if !a.kind.has_offline_window() || a.is_shared() {
                return Ok(OfflineFreed { messages: 0, bytes: 0 });
            }
            let (messages, bytes) = mailcraft_engine::cmd::offline::would_free(s, account, wanted);
            Ok(OfflineFreed { messages: count(messages), bytes })
        })
        .await
    }

    /// Take mail outside the windows back to its headers and preview, at most `most` messages
    /// (1 to 2,000) a call, so a long job is a number of short ones. Nothing is left to do when
    /// `left` is 0; an account that keeps all has nothing.
    pub async fn thin_offline(&self, most: u32) -> Result<ThinResult, CoreError> {
        let most = most.clamp(1, MAX_THIN);
        self.call(move |s| {
            if !s.mailbox.accounts.iter().any(|a| a.kind.has_offline_window()) {
                return Ok(ThinResult { thinned: 0, bytes: 0, left: 0 });
            }
            let said = s.run("account.thinOffline", &json!({"most": most}))?;
            let number = |key: &str| said.get(key).and_then(Value::as_u64).unwrap_or(0);
            Ok(ThinResult {
                thinned: u32::try_from(number("thinned")).unwrap_or(u32::MAX),
                bytes: number("bytes"),
                left: u32::try_from(number("left")).unwrap_or(u32::MAX),
            })
        })
        .await
    }

    /// The signature `account` starts its new messages with, as plain text.
    pub async fn account_signature(&self, account: u64) -> Result<AccountSignature, CoreError> {
        self.call(move |s| {
            let a = s.mailbox.account(account).ok_or_else(|| bad("there is no such account"))?;
            let sig = s.mailbox.signature(&a.signature);
            Ok(match sig {
                Some(sig) => {
                    AccountSignature { text: text::clip(&sig.plain(), MAX_SIGNATURE), editable: sig.html().is_none() && sig.pictures().is_empty() }
                }
                None => AccountSignature { text: String::new(), editable: true },
            })
        })
        .await
    }

    /// Make `text` the signature of `account` for new messages, replies and forwards, or none when
    /// it is only white space. The signature is named by the account's address, unless the account
    /// already uses one by another name, which is edited as it is.
    pub async fn set_account_signature(&self, account: u64, text: String) -> Result<(), CoreError> {
        if text.chars().count() > MAX_SIGNATURE {
            return Err(bad("the signature is too long"));
        }
        self.call(move |s| {
            let a = s.mailbox.account(account).ok_or_else(|| bad("there is no such account"))?;
            let name = if s.mailbox.signature(&a.signature).is_some() { a.signature.clone() } else { a.email.clone() };
            if name.trim().is_empty() {
                return Err(bad("the account has no address to name the signature by"));
            }
            if let Some(old) = s.mailbox.signature(&name)
                && (old.html().is_some() || !old.pictures().is_empty())
            {
                return Err(bad("that signature has a picture or its own layout; change it on the computer"));
            }
            if text.trim().is_empty() {
                if s.mailbox.signature(&name).is_some() {
                    s.run("signature.remove", &json!({"name": name}))?;
                }
                return Ok(());
            }
            s.run("signature.set", &json!({"name": name, "text": text, "default": true, "account": account}))?;
            Ok(())
        })
        .await
    }
}
