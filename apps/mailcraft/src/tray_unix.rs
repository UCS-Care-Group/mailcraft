// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Freedesktop StatusNotifierItem, using the session bus; no GTK or network requests.

use std::sync::{
    Arc,
    mpsc::{Receiver, channel},
};

use ksni::blocking::TrayMethods;

use super::{QUIT, SHOW, SYNC, Signals};
use mailcraft_ui_egui::i18n::{self, Lang};

/// Own the shutdown obligation before handing a service across the startup channel. Dropping
/// an installed handle, a queued result, or a failed send all shuts the service down.
struct ShutdownOnDrop<T> {
    handle: T,
    shutdown: fn(&T),
}

impl<T> Drop for ShutdownOnDrop<T> {
    fn drop(&mut self) {
        (self.shutdown)(&self.handle);
    }
}

type TrayHandle = ShutdownOnDrop<ksni::blocking::Handle<Item>>;

pub struct Backend {
    arriving: Receiver<Result<TrayHandle, String>>,
    handle: Option<TrayHandle>,
    lang: Lang,
    #[cfg(test)]
    stand_in: bool,
}

impl Backend {
    pub fn start(signals: Arc<Signals>) -> Self {
        let (tx, arriving) = channel();
        let wake = signals.clone();
        // Connecting to a desktop that is slow or absent must never hold the window up.
        if let Err(e) = std::thread::Builder::new().name("system-tray".into()).spawn(move || {
            let result = start_item(signals.clone(), |item| {
                item.spawn()
                    .map(|handle| ShutdownOnDrop {
                        handle,
                        shutdown: |h| {
                            h.shutdown();
                        },
                    })
                    .map_err(|e| e.to_string())
            });
            // If the receiver is already gone, SendError drops the guarded handle here.
            let _ = tx.send(result);
            signals.ctx.request_repaint();
        }) {
            log::warn!("system tray couldn't start: {e}");
            wake.online(false);
        }
        Self {
            arriving,
            handle: None,
            lang: i18n::current(),
            #[cfg(test)]
            stand_in: false,
        }
    }

    pub fn poll(&mut self) {
        if let Ok(result) = self.arriving.try_recv() {
            match result {
                Ok(handle) => self.handle = Some(handle),
                Err(e) => log::warn!("system tray unavailable; closing will exit MailCraft: {e}"),
            }
        }
        if self.lang != i18n::current() {
            self.lang = i18n::current();
            if let Some(handle) = &self.handle {
                handle.handle.update(|_| ());
            }
        }
    }

    pub fn ready(&self) -> bool {
        #[cfg(test)]
        if self.stand_in {
            return true;
        }
        self.handle.as_ref().is_some_and(|h| !h.handle.is_closed())
    }

    #[cfg(test)]
    pub fn stand_in() -> Self {
        let (_, arriving) = channel();
        Self { arriving, handle: None, lang: i18n::current(), stand_in: true }
    }
}

fn start_item<T>(signals: Arc<Signals>, spawn: impl FnOnce(Item) -> Result<T, String>) -> Result<T, String> {
    let result = make_item(signals.clone()).and_then(|item| {
        signals.online(true);
        spawn(item)
    });
    if result.is_err() {
        signals.online(false);
    }
    result
}

pub(super) struct Item {
    signals: Arc<Signals>,
    icon: ksni::Icon,
}

fn make_item(signals: Arc<Signals>) -> Result<Item, String> {
    let icon = crate::app_icon().ok_or("MailCraft icon couldn't be read")?;
    let mut argb = Vec::with_capacity(icon.rgba.len());
    for &[r, g, b, a] in icon.rgba.as_chunks::<4>().0 {
        // StatusNotifierItem uses network-order ARGB, unlike egui's RGBA.
        argb.extend_from_slice(&[a, r, g, b]);
    }
    Ok(Item {
        signals,
        icon: ksni::Icon {
            width: i32::try_from(icon.width).map_err(|_| "tray icon is too wide")?,
            height: i32::try_from(icon.height).map_err(|_| "tray icon is too tall")?,
            data: argb,
        },
    })
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        crate::APP_ID.into()
    }
    fn title(&self) -> String {
        "MailCraft".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icon.clone()]
    }
    fn activate(&mut self, _: i32, _: i32) {
        self.signals.send(SHOW);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let (show, sync, quit) = mailcraft_ui_egui::menubar::tray_text();
        vec![
            ksni::menu::StandardItem { label: show, activate: Box::new(|item: &mut Self| item.signals.send(SHOW)), ..Default::default() }.into(),
            ksni::menu::StandardItem { label: sync, activate: Box::new(|item: &mut Self| item.signals.send(SYNC)), ..Default::default() }.into(),
            ksni::MenuItem::Separator,
            ksni::menu::StandardItem { label: quit, activate: Box::new(|item: &mut Self| item.signals.send(QUIT)), ..Default::default() }.into(),
        ]
    }
    fn watcher_offline(&self, _: ksni::OfflineReason) -> bool {
        self.signals.online(false);
        true
    }
    fn watcher_online(&self) {
        self.signals.online(true);
    }
}

#[cfg(test)]
#[path = "tray_unix_tests.rs"]
mod tests;
