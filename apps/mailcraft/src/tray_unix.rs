// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Freedesktop StatusNotifierItem, using the session bus; no GTK or network requests.

use std::sync::{
    Arc,
    mpsc::{Receiver, channel},
};

use ksni::blocking::TrayMethods;

use super::{QUIT, SHOW, Signals};

pub struct Backend {
    arriving: Receiver<Result<ksni::blocking::Handle<Item>, String>>,
    handle: Option<ksni::blocking::Handle<Item>>,
    #[cfg(test)]
    stand_in: bool,
}

impl Backend {
    pub fn start(signals: Arc<Signals>) -> Self {
        let (tx, arriving) = channel();
        let wake = signals.clone();
        // Connecting to a desktop that is slow or absent must never hold the window up.
        if let Err(e) = std::thread::Builder::new().name("system-tray".into()).spawn(move || {
            let result = make_item(signals.clone()).and_then(|item| item.spawn().map_err(|e| e.to_string()));
            signals.online(result.is_ok());
            if let Err(unsent) = tx.send(result)
                && let Ok(handle) = unsent.0
            {
                handle.shutdown();
            }
            signals.ctx.request_repaint();
        }) {
            log::warn!("system tray couldn't start: {e}");
            wake.online(false);
        }
        Self {
            arriving,
            handle: None,
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
    }

    pub fn ready(&self) -> bool {
        #[cfg(test)]
        if self.stand_in {
            return true;
        }
        self.handle.as_ref().is_some_and(|h| !h.is_closed())
    }

    #[cfg(test)]
    pub fn stand_in() -> Self {
        let (_, arriving) = channel();
        Self { arriving, handle: None, stand_in: true }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.shutdown();
        }
    }
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
        let (show, quit) = mailcraft_ui_egui::menubar::tray_text();
        vec![
            ksni::menu::StandardItem { label: show, activate: Box::new(|item: &mut Self| item.signals.send(SHOW)), ..Default::default() }.into(),
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
