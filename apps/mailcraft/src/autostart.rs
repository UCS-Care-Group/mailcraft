// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Options › General › Start MailCraft when you sign in: MailCraft in the system's own
//! list of programs started when the owner signs in, and out of it again.
//!
//! - Linux and the BSDs: `MailCraft`'s entry in the desktop's autostart folder
//!   (`$XDG_CONFIG_HOME/autostart`, else `~/.config/autostart`; the XDG Autostart specification),
//!   naming the AppImage when this copy is one, else the program.
//! - Windows: the value `MailCraft` of the user's own `Run` key; when the owner turned it off in
//!   Task Manager's Startup apps, turning it on here clears that mark (it is their say-so again).
//! - macOS: a launch agent in `~/Library/LaunchAgents` that runs the program at sign-in.
//!
//! The list is the truth: [`reconcile`] reads it at every start, so an entry the owner removed
//! with the system's own tools is off here too, and one that is on is written again so that it
//! names this copy (an AppImage moved, an update installed elsewhere). Nothing but that one entry
//! is ever written or removed, nothing leaves the computer, and the program started from it is
//! given [`BACKGROUND`] and nothing else. A Flatpak can't write the host's list from its sandbox:
//! there is no switch there ([`supported`]).

use std::path::{Path, PathBuf};

/// The argument a start at sign-in is given: the window opens minimized, or in the tray.
pub const BACKGROUND: &str = "--background";

/// The application id the desktop knows MailCraft by (the entry's name, the launch agent's label).
#[cfg_attr(target_os = "windows", allow(dead_code))]
const APP_ID: &str = "io.github.billsecond.mailcraft";

/// The value's name in the Windows `Run` key.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const RUN_NAME: &str = "MailCraft";

/// The longest program path written into an entry.
const MOST_PATH: usize = 4096;

/// Whether this copy can be put in the system's list at all.
pub fn supported() -> bool {
    cfg!(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "windows",
        target_os = "macos"
    )) && !in_flatpak()
}

fn in_flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists()
}

/// At start: whether MailCraft is in the list now, writing an entry that is there anew so that it
/// names this copy. `None` when the list can't be read (the switch then keeps the setting).
pub fn reconcile() -> Option<bool> {
    let on = system::is_on()?;
    if on && let Err(e) = system::set(true) {
        log::warn!("the entry that starts MailCraft at sign-in couldn't be brought up to date: {e}");
    }
    Some(on)
}

/// Put MailCraft in the list (`true`) or take it out (`false`).
pub fn set(on: bool) -> Result<(), String> {
    system::set(on).inspect(|()| log::info!("MailCraft {} started at sign-in", if on { "is now" } else { "is no longer" }))
}

/// The program a start at sign-in runs: the AppImage this copy runs from, else this program.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly")),
    allow(dead_code)
)]
fn program() -> Result<PathBuf, String> {
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_absolute() && p.is_file());
    match appimage {
        Some(path) => Ok(path),
        None => std::env::current_exe().map_err(|e| format!("where MailCraft runs from couldn't be found: {e}")),
    }
}

/// The program as text that may go into an entry: absolute, valid Unicode, without control
/// characters (a line break would end the entry's line), and not absurdly long.
fn usable(program: &Path) -> Result<&str, String> {
    let text = program.to_str().filter(|t| !t.is_empty() && t.len() <= MOST_PATH && !t.chars().any(char::is_control));
    match text {
        Some(text) if program.is_absolute() => Ok(text),
        _ => Err("MailCraft's own path can't be written into the list of programs started at sign-in".into()),
    }
}

/// The desktop's autostart entry for `program` (the Desktop Entry specification: the path quoted
/// as one argument of `Exec`, then escaped as a string value).
pub(crate) fn desktop_entry(program: &Path) -> Result<String, String> {
    let program = usable(program)?;
    let mut quoted = String::from("\"");
    for c in program.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            // A field code would be expanded: `%%` is a percent sign.
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    // A string value's own escape comes on top of the argument's.
    let exec = quoted.replace('\\', "\\\\");
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=MailCraft\nComment=Mail, calendar, contacts and tasks\nExec={exec} {BACKGROUND}\nIcon={APP_ID}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    ))
}

/// Whether an autostart entry's text starts MailCraft: an entry the desktop has been told to
/// skip (`Hidden=true`, GNOME's switch off) is off.
pub(crate) fn desktop_entry_on(text: &str) -> bool {
    let value = |key: &str| text.lines().find_map(|l| l.trim().strip_prefix(key).and_then(|r| r.trim_start().strip_prefix('=')).map(str::trim));
    value("Hidden") != Some("true") && value("X-GNOME-Autostart-enabled") != Some("false") && value("Exec").is_some_and(|e| !e.is_empty())
}

/// The macOS launch agent for `program`: run at sign-in, once, with [`BACKGROUND`].
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn launch_agent(program: &Path) -> Result<String, String> {
    let program = usable(program)?;
    let xml = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;");
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>Label</key>\n\t<string>{APP_ID}</string>\n\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>{}</string>\n\t\t<string>{BACKGROUND}</string>\n\t</array>\n\t<key>RunAtLoad</key>\n\t<true/>\n\t<key>ProcessType</key>\n\t<string>Interactive</string>\n</dict>\n</plist>\n",
        xml(program)
    ))
}

/// The Windows `Run` value for `program`: the path quoted (a Windows path holds no `"`).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn run_value(program: &Path) -> Result<String, String> {
    let program = usable(program)?;
    if program.contains('"') {
        return Err("MailCraft's own path can't be written into the list of programs started at sign-in".into());
    }
    Ok(format!("\"{program}\" {BACKGROUND}"))
}

/// Write `text` to `path` through a new file renamed into place, so that a half-written entry is
/// never left; the folder is made when it isn't there.
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn write_entry(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write as _;
    let dir = path.parent().ok_or("the folder of programs started at sign-in couldn't be found")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{} couldn't be made: {e}", dir.display()))?;
    let mut file = tempfile::NamedTempFile::new_in(dir).map_err(|e| format!("{} couldn't be written to: {e}", dir.display()))?;
    file.write_all(text.as_bytes()).and_then(|()| file.as_file().sync_all()).map_err(|e| format!("{} couldn't be written: {e}", path.display()))?;
    file.persist(path).map_err(|e| format!("{} couldn't be written: {}", path.display(), e.error))?;
    Ok(())
}

/// Remove the entry at `path`; one that isn't there is already gone.
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn remove_entry(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{} couldn't be removed: {e}", path.display())),
    }
}

/// The owner's home folder, from the environment alone.
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_absolute())
}

/// `$XDG_CONFIG_HOME/autostart/<id>.desktop`, else `~/.config/autostart/<id>.desktop`.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly")),
    allow(dead_code)
)]
pub(crate) fn autostart_path(config_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let config = config_home.filter(|p| p.is_absolute()).or_else(|| home.map(|h| h.join(".config")))?;
    Some(config.join("autostart").join(format!("{APP_ID}.desktop")))
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
mod system {
    use super::*;

    fn path() -> Result<PathBuf, String> {
        autostart_path(std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from), home()).ok_or_else(|| "the autostart folder couldn't be found".into())
    }

    pub fn is_on() -> Option<bool> {
        match std::fs::read(path().ok()?) {
            // (Our entry is small; more than this is somebody else's and is not ours to read.)
            Ok(bytes) if bytes.len() <= 64 * 1024 => Some(desktop_entry_on(&String::from_utf8_lossy(&bytes))),
            Ok(_) => Some(false),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
            Err(_) => None,
        }
    }

    pub fn set(on: bool) -> Result<(), String> {
        let path = path()?;
        if on { write_entry(&path, &desktop_entry(&program()?)?) } else { remove_entry(&path) }
    }
}

#[cfg(target_os = "macos")]
mod system {
    use super::*;

    fn path() -> Result<PathBuf, String> {
        home()
            .map(|h| h.join("Library").join("LaunchAgents").join(format!("{APP_ID}.plist")))
            .ok_or_else(|| "the LaunchAgents folder couldn't be found".into())
    }

    pub fn is_on() -> Option<bool> {
        match std::fs::metadata(path().ok()?) {
            Ok(meta) => Some(meta.is_file()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
            Err(_) => None,
        }
    }

    pub fn set(on: bool) -> Result<(), String> {
        let path = path()?;
        if on {
            let program = std::env::current_exe().map_err(|e| format!("where MailCraft runs from couldn't be found: {e}"))?;
            write_entry(&path, &launch_agent(&program)?)
        } else {
            remove_entry(&path)
        }
    }
}

#[cfg(target_os = "windows")]
mod system {
    use super::*;
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    /// Where Task Manager's Startup apps notes an entry the owner turned off.
    const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

    pub fn is_on() -> Option<bool> {
        let user = RegKey::predef(HKEY_CURRENT_USER);
        let present = match user.open_subkey_with_flags(RUN, KEY_READ) {
            Ok(run) => run.get_value::<String, _>(RUN_NAME).is_ok_and(|v| !v.trim().is_empty()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return None,
        };
        // Its first byte is odd when the owner turned the entry off.
        let disabled = user
            .open_subkey_with_flags(APPROVED, KEY_READ)
            .and_then(|k| k.get_raw_value(RUN_NAME))
            .is_ok_and(|v| v.bytes.first().is_some_and(|b| b & 1 == 1));
        Some(present && !disabled)
    }

    pub fn set(on: bool) -> Result<(), String> {
        let user = RegKey::predef(HKEY_CURRENT_USER);
        if on {
            let program = std::env::current_exe().map_err(|e| format!("where MailCraft runs from couldn't be found: {e}"))?;
            let value = run_value(&program)?;
            let (run, _) = user.create_subkey(RUN).map_err(|e| format!("the list of programs started at sign-in couldn't be opened: {e}"))?;
            run.set_value(RUN_NAME, &value).map_err(|e| format!("MailCraft couldn't be added to the programs started at sign-in: {e}"))?;
            if let Ok(approved) = user.open_subkey_with_flags(APPROVED, KEY_SET_VALUE) {
                let _ = approved.delete_value(RUN_NAME);
            }
            Ok(())
        } else {
            match user.open_subkey_with_flags(RUN, KEY_SET_VALUE) {
                Ok(run) => match run.delete_value(RUN_NAME) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(format!("MailCraft couldn't be taken out of the programs started at sign-in: {e}")),
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("the list of programs started at sign-in couldn't be opened: {e}")),
            }
        }
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "windows",
    target_os = "macos"
)))]
mod system {
    pub fn is_on() -> Option<bool> {
        None
    }

    pub fn set(_on: bool) -> Result<(), String> {
        Err("this system has no list of programs started at sign-in that MailCraft knows".into())
    }
}

#[cfg(test)]
#[path = "autostart_tests.rs"]
mod autostart_tests;
