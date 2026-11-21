// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A red dot on the taskbar icon (Windows). The window decides when new mail is waiting
//! ([`mailcraft_ui_egui::MailApp::taskbar_badge`]); this only shows the dot, and takes it off
//! when that count is 0.
//!
//! The dot is an overlay icon, which Windows puts at the button's lower corner. Painting into
//! the icon cannot leave the icon's square.

/// What was last handed to the taskbar, and whether a failure was already said.
pub struct Icons {
    /// Whether the red dot is on the button.
    dot: bool,
    #[cfg(target_os = "windows")]
    said_dot: bool,
    /// The main window. Learned while it is in front, and, before it ever has been, found among
    /// this process's own top-level windows, so mail that arrives first still gets a dot. A
    /// compose or reminder window is another window of this process; only a window with no owner
    /// is kept, and the dot is only ever put on that handle.
    #[cfg(target_os = "windows")]
    main: Option<winsafe::HWND>,
    /// The icon, built once. winsafe 0.0.29 has no `CreateIconIndirect`, so the bytes are loaded
    /// through a file that is removed as soon as the icon exists.
    #[cfg(target_os = "windows")]
    icon: Option<winsafe::guard::DestroyIconGuard>,
    /// When a failed call may be tried again. A broken shell is not asked on every frame.
    #[cfg(target_os = "windows")]
    retry_at: Option<std::time::Instant>,
}

impl Icons {
    pub fn new() -> Icons {
        Icons {
            dot: false,
            #[cfg(target_os = "windows")]
            said_dot: false,
            #[cfg(target_os = "windows")]
            main: None,
            #[cfg(target_os = "windows")]
            icon: None,
            #[cfg(target_os = "windows")]
            retry_at: None,
        }
    }

    /// Show the red dot when `count` is not 0, and take it off when it is. `in_front` is the main
    /// window. `words` is what a screen reader hears for the overlay. Windows only: elsewhere the
    /// taskbar button is not the window's to mark. A refusal is said once and tried again after
    /// a few seconds.
    pub fn apply(&mut self, count: u32, in_front: bool, words: &str) {
        let want = count > 0;
        #[cfg(target_os = "windows")]
        self.hand_dot(want, in_front, words);
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (in_front, words);
            self.dot = want;
        }
    }
}

/// A failure is said once.
#[cfg(target_os = "windows")]
fn note(said: &mut bool, err: &str) {
    if !*said {
        log::warn!("taskbar dot: {err}");
        *said = true;
    }
}

/// How long a failed call waits before the shell is asked again.
#[cfg(target_os = "windows")]
const RETRY: std::time::Duration = std::time::Duration::from_secs(3);

#[cfg(target_os = "windows")]
fn ours(hwnd: &winsafe::HWND) -> bool {
    let (_, owner) = hwnd.GetWindowThreadProcessId();
    owner == std::process::id()
}

/// A window another of this process owns (a compose or reminder window) is not the main one.
#[cfg(target_os = "windows")]
fn unowned(hwnd: &winsafe::HWND) -> bool {
    hwnd.GetWindow(winsafe::co::GW::OWNER).is_err()
}

#[cfg(target_os = "windows")]
impl Icons {
    fn remember_main(&mut self) {
        let Some(hwnd) = winsafe::HWND::GetForegroundWindow() else { return };
        if ours(&hwnd) && unowned(&hwnd) {
            self.main = Some(hwnd);
        }
    }

    /// A top-level window of this process that nobody owns: the main window, even when it has
    /// never been in front.
    fn find_main() -> Option<winsafe::HWND> {
        let mut found = None;
        let _ = winsafe::EnumWindows(|hwnd| {
            if found.is_none() && ours(&hwnd) && unowned(&hwnd) && hwnd.IsWindowVisible() {
                found = Some(hwnd);
            }
            found.is_none()
        });
        found
    }

    fn hand_dot(&mut self, want: bool, in_front: bool, words: &str) {
        if in_front {
            self.remember_main();
        }
        if self.dot == want && self.retry_at.is_none() {
            return;
        }
        if self.retry_at.is_some_and(|at| std::time::Instant::now() < at) {
            return;
        }
        if self.main.is_none() {
            self.main = Self::find_main();
        }
        let Some(hwnd) = self.main.take().filter(|hwnd| hwnd.IsWindow()) else { return };
        match self.set_overlay(&hwnd, want, words) {
            Ok(()) => {
                self.main = Some(hwnd);
                self.dot = want;
                self.retry_at = None;
            }
            Err(e) => {
                self.main = Some(hwnd);
                self.retry_at = Some(std::time::Instant::now() + RETRY);
                note(&mut self.said_dot, &e);
            }
        }
    }

    /// The red dot, or nothing, on the main window's taskbar button.
    fn set_overlay(&mut self, hwnd: &winsafe::HWND, on: bool, words: &str) -> Result<(), String> {
        use winsafe::prelude::*;
        let list = winsafe::CoCreateInstance::<winsafe::ITaskbarList3>(
            &winsafe::co::CLSID::TaskbarList,
            None::<&winsafe::IUnknown>,
            winsafe::co::CLSCTX::INPROC_SERVER,
        )
        .map_err(|e| e.to_string())?;
        list.HrInit().map_err(|e| e.to_string())?;
        if !on {
            return list.SetOverlayIcon(hwnd, None, "").map_err(|e| e.to_string());
        }
        if self.icon.is_none() {
            self.icon = Some(load_icon()?);
        }
        let Some(icon) = self.icon.as_ref() else {
            return Err("the dot's icon couldn't be made".to_string());
        };
        list.SetOverlayIcon(hwnd, Some(icon), words).map_err(|e| e.to_string())
    }
}

/// The icon, loaded from a file that exists only for that load. The name is not a fixed one, and
/// the file is removed once the icon is in memory.
#[cfg(target_os = "windows")]
fn load_icon() -> Result<winsafe::guard::DestroyIconGuard, String> {
    use std::io::Write;
    use winsafe::prelude::*;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let path = std::env::temp_dir().join(format!("mailcraft-dot-{}-{nanos}.ico", std::process::id()));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
    file.write_all(&dot_icon()).map_err(|e| e.to_string())?;
    drop(file);
    let text = path.to_str().ok_or_else(|| "the dot's icon couldn't be kept".to_string())?;
    let icon = winsafe::HINSTANCE::NULL
        .LoadImageIcon(winsafe::IdOicStr::from_str(text), winsafe::SIZE { cx: 16, cy: 16 }, winsafe::co::LR::LOADFROMFILE)
        .map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&path);
    icon
}

/// A 16×16 icon: a red dot on a clear square. The corner pixels are clear.
#[cfg(target_os = "windows")]
fn dot_icon() -> Vec<u8> {
    const SIDE: usize = 16;
    let xor = SIDE * SIDE * 4;
    let mask = 4 * SIDE;
    let image = 40 + xor + mask;
    let mut bytes = Vec::with_capacity(22 + image);
    bytes.extend_from_slice(&[0, 0, 1, 0, 1, 0, SIDE as u8, SIDE as u8, 0, 0]);
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(&(image as u32).to_le_bytes());
    bytes.extend_from_slice(&22u32.to_le_bytes());
    bytes.extend_from_slice(&40u32.to_le_bytes());
    bytes.extend_from_slice(&(SIDE as i32).to_le_bytes());
    bytes.extend_from_slice(&((SIDE as i32) * 2).to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&(xor as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 16]);
    for y in (0..SIDE).rev() {
        for x in 0..SIDE {
            if dot_pixel(x, y) {
                bytes.extend_from_slice(&[32, 32, 220, 255]);
            } else {
                bytes.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    for y in (0..SIDE).rev() {
        let mut row = [0u8; 4];
        for x in 0..SIDE {
            if !dot_pixel(x, y)
                && let Some(cell) = row.get_mut(x / 8)
            {
                *cell |= 1 << (7 - (x % 8));
            }
        }
        bytes.extend_from_slice(&row);
    }
    bytes
}

/// Whether `(x, y)` is inside the dot. The middle of the 16×16 square, a small radius.
#[cfg(target_os = "windows")]
fn dot_pixel(x: usize, y: usize) -> bool {
    let dx = x as i32 - 7;
    let dy = y as i32 - 7;
    dx * dx + dy * dy <= 16
}

#[cfg(all(test, target_os = "windows"))]
#[path = "taskbar_count_tests.rs"]
mod taskbar_count_tests;
