// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's part in a copy that updates itself: whether the organisation has turned that off,
//! where downloads are kept, and the download itself on a background thread
//! (`mailcraft_net::update_download`, which checks it by the release's own signature). Nothing
//! is installed here.

use std::path::Path;

use mailcraft_model::update::Release;
use mailcraft_net::update_download::{DownloadError, Downloaded, Watch};

use crate::install::Install;
use crate::self_update::Verified;

/// The folder of the data folder that downloads are kept in, until they are installed or refused.
pub const FOLDER: &str = "updates";

/// The file whose presence turns updating by itself off for every user of the computer, on
/// macOS and on Linux and FreeBSD. What is in it doesn't matter.
#[cfg(target_os = "macos")]
pub const POLICY_FILE: &str = "/Library/Application Support/MailCraft/no-automatic-updates";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const POLICY_FILE: &str = "/etc/mailcraft/no-automatic-updates";

/// On Windows: `DisableAutomaticUpdates`, a DWORD other than 0 under this key of the machine's.
#[cfg(target_os = "windows")]
pub const POLICY_KEY: &str = r"Software\Policies\MailCraft";
#[cfg(target_os = "windows")]
pub const POLICY_VALUE: &str = "DisableAutomaticUpdates";

/// Whether the organisation has turned updating MailCraft by itself off for this computer.
pub fn turned_off_by_organisation() -> bool {
    #[cfg(target_os = "windows")]
    {
        use winreg::RegKey;
        use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags(POLICY_KEY, KEY_READ)
            .and_then(|key| key.get_value::<u32, _>(POLICY_VALUE))
            .is_ok_and(|value| value != 0)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Path::new(POLICY_FILE).exists()
    }
}

/// Download `release`'s file for `install` into `dir`, checked, with `fetch` doing the
/// downloading (`mailcraft_net::update_download::download_watched`, or a stand-in in the tests),
/// which tells `watch` how far it has come and stops when it says so. Where the file is and the
/// SHA-256 the release's signed list gives for it (never one worked out from the file again,
/// which may have been changed since it was checked), or why there is none in words that name
/// no address.
pub fn download(
    release: &Release,
    install: Install,
    dir: &Path,
    watch: Watch<'_>,
    fetch: impl Fn(&Release, &str, &Path, Watch<'_>) -> Result<Downloaded, DownloadError>,
) -> Result<Verified, String> {
    let name = install.file_name(&release.version, std::env::consts::ARCH).ok_or_else(|| "this copy has no file in a release".to_string())?;
    let got = fetch(release, &name, dir, watch).map_err(|e| e.to_string())?;
    Ok(Verified { version: release.version.clone(), path: got.path, sha256: got.sha256 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_is_read_where_the_organisation_puts_it() {
        // The tests' computer has none, and asking never fails.
        let _ = turned_off_by_organisation();
        #[cfg(not(target_os = "windows"))]
        assert!(POLICY_FILE.starts_with('/') && POLICY_FILE.ends_with("no-automatic-updates"));
    }
}
