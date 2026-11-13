// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whether the system plays animations, for File › Options › Animations › Use system setting
//! (#1183). On Windows it is Settings › Accessibility › Visual effects › Animation effects, read
//! through the Windows Runtime's `UISettings`, which the `windows` crate projects as safe calls.
//! Elsewhere nothing is read yet, and animations play unless the owner turns them off. A failed
//! read counts as on: that is what the window did before there was a choice.

/// What reads the system's setting, when there is one to read.
#[cfg(target_os = "windows")]
pub fn reader() -> Option<Box<dyn Fn() -> bool>> {
    Some(Box::new(|| windows::UI::ViewManagement::UISettings::new().and_then(|settings| settings.AnimationsEnabled()).unwrap_or(true)))
}

/// What reads the system's setting, when there is one to read.
#[cfg(not(target_os = "windows"))]
pub fn reader() -> Option<Box<dyn Fn() -> bool>> {
    None
}
