// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which kind of copy of MailCraft this is, and so whether it can update itself, and which file
//! of a release it would be updated from.
//!
//! A copy installed from the Windows `.msi` or a Linux AppImage updates itself, and so does the
//! macOS app (wherever the owner put it) when [`MAC_SELF_INSTALL`] is on; every other copy
//! (`.deb`, `.rpm`, Flatpak, the portable `.zip`, a `.tar.gz`, FreeBSD, one built from source)
//! is told of new versions and nothing more. It is known from where the program runs; when that
//! can't be told, it is the notice only. A macOS app or an AppImage in a folder this program
//! can't write to is the notice only too: it is replaced by this program, not by an installer.

use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path};

use mailcraft_model::update::Version;

/// Whether a macOS app installs new versions by itself. **Off** (#497): the builds are signed
/// ad hoc and not notarized, and an app swapped in place by the program is what macOS's checks
/// then judge; one update that went wrong (0.8.1) left an app that wouldn't start. A Mac copy is
/// told of new versions instead, with a link to the release's page.
///
/// Turn it on only when all of this is true: releases are signed with the project's Developer ID
/// and notarized (with the ticket stapled to the app); `self_update::mac_swap` requires the new
/// app's signature to be the project's Team ID (`codesign -R`), not any valid signature; the
/// check after the swap also asks Gatekeeper (`spctl -a -vv` on the app in its place) or checks
/// for a stapled notarization ticket (`stapler validate`), because `codesign --verify` alone is
/// not enough: it passed on the reporter's app that wouldn't start, which `spctl -a` rejected;
/// and an update from the previous release has been tried on a real Mac, from `/Applications`
/// and from a folder of the owner's. The code that installs it is built and tested meanwhile.
pub const MAC_SELF_INSTALL: bool = false;

/// How this copy came to be installed, as far as updating it goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Install {
    /// From the Windows `.msi`: in the place the installer put it, which it registered.
    Msi,
    /// A macOS `MailCraft.app`, run from where the owner keeps it (not from the disk image).
    MacApp,
    /// A Linux AppImage.
    AppImage,
    /// Anything else: a package manager's, a portable copy, one built from source.
    NoticeOnly,
}

impl Install {
    /// The name of this kind's file in release `version`, as the release workflow names it
    /// (`mailcraft-<version>-<os>-<arch>.<ext>`), for a program built for `arch`
    /// (`std::env::consts::ARCH`). `None` where there is none to update from.
    pub fn file_name(self, version: &Version, arch: &str) -> Option<String> {
        let (os, arch, ext) = match (self, arch) {
            (Install::Msi, "x86_64") => ("windows", "x64", "msi"),
            (Install::Msi, "x86") => ("windows", "x86", "msi"),
            (Install::Msi, "aarch64") => ("windows", "arm64", "msi"),
            // One app for both kinds of Mac.
            (Install::MacApp, "x86_64" | "aarch64") => ("macos", "universal", "dmg"),
            (Install::AppImage, "x86_64") => ("linux", "x86_64", "AppImage"),
            (Install::AppImage, "aarch64") => ("linux", "aarch64", "AppImage"),
            _ => return None,
        };
        Some(format!("mailcraft-{version}-{os}-{arch}.{ext}"))
    }
}

/// This copy, as far as updating it goes: what it was installed as ([`found`]), unless it is a
/// macOS app while [`MAC_SELF_INSTALL`] is off, or it is replaced by this program and the folder
/// it is in can't be written to ([`updating`]).
pub fn this_copy() -> Install {
    let found = found();
    updating(found, MAC_SELF_INSTALL, &mut || crate::self_update::target(found).and_then(|target| target.parent().map(writable)).unwrap_or(false))
}

/// Of a copy installed as `found`, the kind that updates it: a macOS app only with
/// `mac_self_install`, and a macOS app or an AppImage, which this program replaces itself, only
/// where `parent_writable` (asked only then) says that the folder it is in can be written to.
/// The `.msi` is installed by Windows Installer, which asks for an administrator's say-so.
pub fn updating(found: Install, mac_self_install: bool, parent_writable: &mut dyn FnMut() -> bool) -> Install {
    match found {
        Install::MacApp if !mac_self_install => Install::NoticeOnly,
        Install::MacApp | Install::AppImage if !parent_writable() => Install::NoticeOnly,
        other => other,
    }
}

/// Whether this program can make and remove a file in `dir`: tried, as permissions alone don't
/// tell (access lists, a read-only disk, macOS's protections).
pub fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".mailcraft-write-test-{}", std::process::id()));
    match fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(file) => {
            drop(file);
            fs::remove_file(&probe).is_ok()
        }
        Err(_) => false,
    }
}

/// What this copy was installed as, whether or not it updates itself.
pub fn found() -> Install {
    let Ok(exe) = std::env::current_exe() else { return Install::NoticeOnly };
    #[cfg(target_os = "windows")]
    {
        windows(&exe, registered_path().as_deref())
    }
    #[cfg(target_os = "macos")]
    {
        mac(&exe)
    }
    #[cfg(target_os = "linux")]
    {
        let (image, dir) = (std::env::var_os("APPIMAGE"), std::env::var_os("APPDIR"));
        linux(image.as_deref(), dir.as_deref(), std::env::var_os("FLATPAK_ID").is_some(), &exe)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = exe;
        Install::NoticeOnly
    }
}

/// Where the `.msi` says it put `mailcraft.exe` (its App Paths entry); the portable `.zip`
/// registers nothing.
#[cfg(target_os = "windows")]
fn registered_path() -> Option<std::path::PathBuf> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\App Paths\mailcraft.exe", KEY_READ)
        .ok()?;
    let path: String = key.get_value("").ok()?;
    let path = path.trim().trim_matches('"');
    (!path.is_empty()).then(|| std::path::PathBuf::from(path))
}

/// An `.msi` copy is the one the installer registered: the program is the file it names.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn windows(exe: &Path, registered: Option<&Path>) -> Install {
    let same = |a: &Path, b: &Path| a.as_os_str().to_string_lossy().eq_ignore_ascii_case(&b.as_os_str().to_string_lossy());
    match registered {
        Some(registered) if same(exe, registered) => Install::Msi,
        _ => Install::NoticeOnly,
    }
}

/// A macOS app: the program is `<something>.app/Contents/MacOS/<program>`, run from a place
/// the owner keeps it, not from a disk image (`/Volumes/…`) nor from the copy macOS makes of an
/// app it hasn't been moved yet (App Translocation).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn mac(exe: &Path) -> Install {
    let parts: Vec<&OsStr> = exe.components().filter_map(|c| if let Component::Normal(part) = c { Some(part) } else { None }).collect();
    let bundle = match parts.as_slice() {
        [.., app, contents, macos, _program] if *contents == "Contents" && *macos == "MacOS" => Path::new(app).extension() == Some(OsStr::new("app")),
        _ => false,
    };
    let from_image = exe.starts_with("/Volumes");
    let translocated = parts.iter().any(|part| *part == "AppTranslocation");
    if bundle && exe.is_absolute() && !from_image && !translocated { Install::MacApp } else { Install::NoticeOnly }
}

/// An AppImage: its runtime says where the image is (`APPIMAGE`, an absolute path to a file
/// named `….AppImage`) and where it is mounted (`APPDIR`), and the program runs from inside
/// that. A Flatpak is never one.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn linux(image: Option<&OsStr>, dir: Option<&OsStr>, flatpak: bool, exe: &Path) -> Install {
    let (Some(image), Some(dir)) = (image.map(Path::new), dir.map(Path::new)) else { return Install::NoticeOnly };
    let named = image.extension() == Some(OsStr::new("AppImage"));
    if !flatpak && named && image.is_absolute() && dir.is_absolute() && exe.starts_with(dir) { Install::AppImage } else { Install::NoticeOnly }
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod install_tests;
