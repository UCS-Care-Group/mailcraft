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
    /// The window the dot was put on. It is taken off that same window: a compose or reminder
    /// window is another window of this process, and clearing whichever is in front can leave
    /// the dot on the one that received it.
    #[cfg(target_os = "windows")]
    marked: Option<winsafe::HWND>,
}

impl Icons {
    pub fn new() -> Icons {
        Icons {
            dot: false,
            #[cfg(target_os = "windows")]
            said_dot: false,
            #[cfg(target_os = "windows")]
            marked: None,
        }
    }

    /// Show the red dot when `count` is not 0, and take it off when it is. Windows only:
    /// elsewhere the taskbar button is not the window's to mark. A refusal is said once and
    /// tried again.
    pub fn apply(&mut self, frame: &eframe::Frame, count: u32) {
        let want = count > 0;
        #[cfg(target_os = "windows")]
        self.hand_dot(frame, want);
        #[cfg(not(target_os = "windows"))]
        {
            let _ = frame;
            self.dot = want;
        }
    }
}

/// A failure is said once. The next frame tries again.
#[cfg(target_os = "windows")]
fn note(said: &mut bool, err: &str) {
    if !*said {
        log::warn!("taskbar dot: {err}");
        *said = true;
    }
}

#[cfg(target_os = "windows")]
impl Icons {
    fn hand_dot(&mut self, frame: &eframe::Frame, want: bool) {
        if self.dot == want {
            return;
        }
        match overlay(frame, want, self.marked.as_ref()) {
            Ok(marked) => {
                self.dot = want;
                self.marked = marked;
            }
            Err(e) => note(&mut self.said_dot, &e),
        }
    }
}

/// The red dot, or nothing, on the taskbar button. Windows puts an overlay at the button's
/// lower corner. Putting the dot on returns the window it was put on; taking it off returns
/// nothing, and uses `marked` when that window still exists.
#[cfg(target_os = "windows")]
fn overlay(frame: &eframe::Frame, on: bool, marked: Option<&winsafe::HWND>) -> Result<Option<winsafe::HWND>, String> {
    let _ = frame;
    if on {
        let hwnd = our_window().ok_or_else(|| "the window isn't on the taskbar yet".to_string())?;
        set_overlay(&hwnd, true)?;
        return Ok(Some(hwnd));
    }
    let fallback;
    let hwnd = if let Some(hwnd) = marked.filter(|hwnd| hwnd.IsWindow()) {
        hwnd
    } else {
        fallback = our_window().ok_or_else(|| "the window isn't on the taskbar yet".to_string())?;
        &fallback
    };
    set_overlay(hwnd, false)?;
    Ok(None)
}

/// The red dot, or nothing, on `hwnd`'s taskbar button.
#[cfg(target_os = "windows")]
fn set_overlay(hwnd: &winsafe::HWND, on: bool) -> Result<(), String> {
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
    let path = dot_file()?;
    let icon = winsafe::HINSTANCE::NULL
        .LoadImageIcon(winsafe::IdOicStr::from_str(&path), winsafe::SIZE { cx: 16, cy: 16 }, winsafe::co::LR::LOADFROMFILE)
        .map_err(|e| e.to_string())?;
    list.SetOverlayIcon(hwnd, Some(&icon), "New mail").map_err(|e| e.to_string())
}

/// This process's window, the one the taskbar button belongs to.
#[cfg(target_os = "windows")]
fn our_window() -> Option<winsafe::HWND> {
    let pid = std::process::id();
    let mut found = None;
    let _ = winsafe::EnumWindows(|hwnd: winsafe::HWND| {
        let (_, owner) = hwnd.GetWindowThreadProcessId();
        let titled = hwnd.GetWindowText().ok().is_some_and(|text| !text.is_empty());
        if owner == pid && hwnd.IsWindowVisible() && titled {
            found = Some(hwnd);
            return false;
        }
        true
    });
    found
}

/// Where the dot's icon was written, once.
#[cfg(target_os = "windows")]
fn dot_file() -> Result<String, String> {
    static PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(path) = PATH.get() {
        return Ok(path.clone());
    }
    let path = std::env::temp_dir().join("mailcraft-taskbar-dot.ico");
    std::fs::write(&path, dot_icon()).map_err(|e| e.to_string())?;
    let path = path.to_str().ok_or_else(|| "the dot's icon couldn't be kept".to_string())?.to_string();
    Ok(PATH.get_or_init(|| path).clone())
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
            if !dot_pixel(x, y) {
                row[x / 8] |= 1 << (7 - (x % 8));
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
