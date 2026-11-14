// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Desktop Alerts (R7.1): the system's notifications, for the window's [`AlertHost`]. Our own
//! code over what the program is already built with, one way per system:
//!
//! - **Linux and the BSDs**: the desktop's notification service (`org.freedesktop.Notifications`,
//!   GNOME's, KDE Plasma's, mako, dunst…) over the session's D-Bus, with the same zbus as the
//!   light or dark preference (`desktop_theme`). Alerts go out from a thread of their own, so a
//!   service that is slow to answer never holds the window up; a click is the service's
//!   `ActionInvoked` for the alert's default action, where the service has actions. The body is
//!   escaped where the service reads markup in it, so that what a message says is shown as text
//!   and nothing in it is fetched. No service: Options says so.
//! - **Windows**: toasts (`Windows.UI.Notifications`), for the AppUserModelID `MailCraft.MailCraft`
//!   that the `.msi` gives its Start menu shortcut: Windows shows a program's toasts only when it
//!   knows the program by such a shortcut, so a copy the `.msi` didn't install says so in Options.
//!   The text goes into the toast's XML as text nodes, never as markup. A click while MailCraft
//!   runs comes back through the toast's `Activated`. (The process doesn't declare the id itself:
//!   that call is `unsafe` in the bindings, which this program forbids; started from the shortcut,
//!   it is the shortcut's.)
//! - **macOS**: the UserNotifications framework, only for a copy that runs from the MailCraft app
//!   (outside one, the framework would end the program). Permission is asked for when the first
//!   alert is due, not at start; refused, Options says it is turned off in System Settings. A click
//!   brings MailCraft in front, which the system does by itself; which alert was clicked isn't
//!   known here (that needs a delegate object, which can't be made without `unsafe`).
//!
//! Nothing of what an alert says is logged. No sound of our own: the system's default, which the
//! owner controls in the system's settings.

use std::sync::mpsc::{Receiver, Sender};

use mailcraft_ui_egui::alerts::AlertHost;

/// The system's notifications for this window, where it has some. Clicks wake `ctx`.
pub fn host(ctx: egui::Context) -> Option<Box<dyn AlertHost>> {
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
    {
        Some(Box::new(freedesktop::Service::start(ctx)))
    }
    #[cfg(target_os = "windows")]
    {
        Some(Box::new(toasts::Toasts::new(ctx)))
    }
    #[cfg(target_os = "macos")]
    {
        let _ = ctx;
        Some(Box::new(apple::Notifications::new()))
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
    {
        let _ = ctx;
        None
    }
}

/// Where clicks come back to the window: the alert's key, and the window is woken.
#[derive(Clone)]
#[cfg_attr(target_os = "macos", allow(dead_code))]
struct Clicks {
    tx: Sender<u64>,
    ctx: egui::Context,
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
impl Clicks {
    fn new(ctx: egui::Context) -> (Clicks, Receiver<u64>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Clicks { tx, ctx }, rx)
    }

    fn clicked(&self, key: u64) {
        let _ = self.tx.send(key);
        self.ctx.request_repaint();
    }
}

/// Clicks waiting for the window, at most a few at a time.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn drain(rx: &Receiver<u64>) -> Vec<u64> {
    rx.try_iter().take(20).collect()
}

/// `text` for a notification service that reads markup in an alert's body: `&`, `<` and `>` as
/// entities, so that it is shown as it is and nothing in it is read as a tag (a picture's tag
/// would have the service fetch it).
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
pub fn escape_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
mod freedesktop {
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use mailcraft_ui_egui::alerts::{AlertHost, AlertKind, AlertTrouble, DesktopAlert};
    use zbus::zvariant::Value;

    use super::{Clicks, drain, escape_markup};

    const SERVICE: &str = "org.freedesktop.Notifications";
    const PATH: &str = "/org/freedesktop/Notifications";
    /// The desktop entry the service finds the program's name and icon in.
    const DESKTOP_ENTRY: &str = "io.github.billsecond.mailcraft";
    const CALL_TIMEOUT: Duration = Duration::from_secs(3);
    /// How many alerts may wait to go out; more are dropped (a flood is one alert anyway).
    const WAITING: usize = 16;
    /// How many shown alerts are remembered, for their clicks.
    const REMEMBERED: usize = 200;

    /// What is known of the service: not yet, there, or not.
    const UNKNOWN: u8 = 0;
    const THERE: u8 = 1;
    const MISSING: u8 = 2;

    pub struct Service {
        out: SyncSender<DesktopAlert>,
        clicks: Receiver<u64>,
        state: Arc<AtomicU8>,
    }

    /// Which notification (the service's id) was which alert (the window's key).
    type Shown = Arc<Mutex<VecDeque<(u32, u64)>>>;

    impl Service {
        /// Connect on a thread of its own, which then sends what the window asks for; another
        /// listens for clicks.
        pub fn start(ctx: egui::Context) -> Service {
            let (out, alerts) = std::sync::mpsc::sync_channel::<DesktopAlert>(WAITING);
            let (clicks, rx) = Clicks::new(ctx);
            let state = Arc::new(AtomicU8::new(UNKNOWN));
            let thread_state = state.clone();
            let spawned = std::thread::Builder::new().name("desktop alerts".into()).spawn(move || {
                if let Err(e) = run(&alerts, clicks, &thread_state) {
                    thread_state.store(MISSING, Ordering::Relaxed);
                    log::info!("desktop alerts can't be shown: no notification service answered ({e})");
                }
            });
            if let Err(e) = spawned {
                state.store(MISSING, Ordering::Relaxed);
                log::warn!("desktop alerts can't be shown: {e}");
            }
            Service { out, clicks: rx, state }
        }
    }

    impl AlertHost for Service {
        fn show(&self, alert: &DesktopAlert) -> Result<(), String> {
            if self.state.load(Ordering::Relaxed) == MISSING {
                return Err("no notification service".into());
            }
            match self.out.try_send(alert.clone()) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(_)) => Err("too many alerts waiting".into()),
                Err(TrySendError::Disconnected(_)) => Err("the alerts' thread has ended".into()),
            }
        }
        fn trouble(&self) -> Option<AlertTrouble> {
            (self.state.load(Ordering::Relaxed) == MISSING).then_some(AlertTrouble::NoService)
        }
        fn clicked(&self) -> Vec<u64> {
            drain(&self.clicks)
        }
    }

    /// The sending thread: connect, ask what the service can do, listen for clicks, then send.
    fn run(alerts: &Receiver<DesktopAlert>, clicks: Clicks, state: &AtomicU8) -> zbus::Result<()> {
        let bus = zbus::blocking::connection::Builder::session()?.method_timeout(CALL_TIMEOUT).build()?;
        let proxy = zbus::blocking::proxy::Builder::<zbus::blocking::Proxy<'_>>::new(&bus)
            .destination(SERVICE)?
            .path(PATH)?
            .interface(SERVICE)?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()?;
        // (Also starts a service that is started on demand.)
        let capabilities: Vec<String> = proxy.call("GetCapabilities", &())?;
        state.store(THERE, Ordering::Relaxed);
        let markup = capabilities.iter().any(|c| c == "body-markup");
        let actions = capabilities.iter().any(|c| c == "actions");
        let shown: Shown = Default::default();
        if actions {
            listen(&bus, shown.clone(), clicks);
        }
        for alert in alerts {
            let body = if markup { escape_markup(&alert.body) } else { alert.body.clone() };
            // The default action is a click on the notification itself.
            let offered: Vec<&str> = if actions { vec!["default", "Open"] } else { Vec::new() };
            let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
            hints.insert("desktop-entry", Value::from(DESKTOP_ENTRY));
            if alert.kind == AlertKind::Mail {
                hints.insert("category", Value::from("email.arrived"));
            }
            let sent: zbus::Result<u32> = proxy.call("Notify", &("MailCraft", 0u32, "", alert.title.as_str(), body.as_str(), offered, hints, -1i32));
            match sent {
                Ok(id) => {
                    if let Ok(mut shown) = shown.lock() {
                        if shown.len() >= REMEMBERED {
                            shown.pop_front();
                        }
                        shown.push_back((id, alert.key));
                    }
                }
                Err(e) => log::debug!("a desktop alert wasn't shown: {e}"),
            }
        }
        Ok(())
    }

    /// Listen for clicks on a thread of its own, for as long as the session's bus is there.
    fn listen(bus: &zbus::blocking::Connection, shown: Shown, clicks: Clicks) {
        let bus = bus.clone();
        let spawned = std::thread::Builder::new().name("desktop alert clicks".into()).spawn(move || {
            let listened = || -> zbus::Result<()> {
                let proxy = zbus::blocking::proxy::Builder::<zbus::blocking::Proxy<'_>>::new(&bus)
                    .destination(SERVICE)?
                    .path(PATH)?
                    .interface(SERVICE)?
                    .cache_properties(zbus::proxy::CacheProperties::No)
                    .build()?;
                for signal in proxy.receive_signal("ActionInvoked")? {
                    let Ok((id, action)) = signal.body().deserialize::<(u32, String)>() else { continue };
                    if action != "default" {
                        continue;
                    }
                    let key = shown.lock().ok().and_then(|shown| shown.iter().find(|(shown_id, _)| *shown_id == id).map(|(_, key)| *key));
                    if let Some(key) = key {
                        clicks.clicked(key);
                    }
                }
                Ok(())
            };
            if let Err(e) = listened() {
                log::debug!("clicks on desktop alerts aren't heard: {e}");
            }
        });
        if let Err(e) = spawned {
            log::debug!("clicks on desktop alerts aren't heard: {e}");
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn without_a_service_alerts_say_why() {
            let (out, _alerts) = std::sync::mpsc::sync_channel(1);
            let (_, clicks) = std::sync::mpsc::channel();
            let service = Service { out, clicks, state: Arc::new(AtomicU8::new(MISSING)) };
            assert_eq!(service.trouble(), Some(AlertTrouble::NoService));
            let alert = DesktopAlert {
                key: 1,
                kind: AlertKind::Mail,
                title: "A".into(),
                body: "B".into(),
                opens: mailcraft_ui_egui::alerts::Opens::Reminders,
            };
            assert!(service.show(&alert).is_err());
            // Known to be there: alerts queue, a few at most.
            service.state.store(THERE, Ordering::Relaxed);
            assert!(service.show(&alert).is_ok());
            assert!(service.show(&alert).is_err(), "full");
            assert_eq!(service.trouble(), None);
        }
    }
}

#[cfg(target_os = "windows")]
mod toasts {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::mpsc::Receiver;

    use mailcraft_ui_egui::alerts::{AlertHost, AlertTrouble, DesktopAlert};
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{NotificationSetting, ToastNotification, ToastNotificationManager, ToastTemplateType};
    use windows::core::{HSTRING, IInspectable};

    use super::{Clicks, drain};

    /// What the `.msi`'s Start menu shortcut tells Windows the program is (its
    /// `System.AppUserModel.ID`, `packaging/windows/mailcraft.wxs`). Toasts use it.
    const APP_ID: &str = "MailCraft.MailCraft";
    /// How many toasts are kept, so that a click on one is still heard.
    const KEPT: usize = 50;

    pub struct Toasts {
        clicks: Clicks,
        rx: Receiver<u64>,
        installed: bool,
        turned_off: Mutex<bool>,
        kept: Mutex<VecDeque<ToastNotification>>,
    }

    impl Toasts {
        pub fn new(ctx: egui::Context) -> Toasts {
            let (clicks, rx) = Clicks::new(ctx);
            let installed = crate::install::this_copy() == crate::install::Install::Msi;
            Toasts { clicks, rx, installed, turned_off: Mutex::new(false), kept: Mutex::new(VecDeque::new()) }
        }

        fn toast(&self, alert: &DesktopAlert) -> windows::core::Result<()> {
            let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?;
            if let Ok(setting) = notifier.Setting() {
                let off = setting != NotificationSetting::Enabled;
                if let Ok(mut turned_off) = self.turned_off.lock() {
                    *turned_off = off;
                }
                if off {
                    return Err(windows::core::Error::empty());
                }
            }
            // A title line and a body line, as text nodes: nothing of the message is read as XML.
            let xml: XmlDocument = ToastNotificationManager::GetTemplateContent(ToastTemplateType::ToastText02)?;
            let lines = xml.GetElementsByTagName(&HSTRING::from("text"))?;
            for (at, text) in [(0u32, alert.title.as_str()), (1, alert.body.as_str())] {
                let node = xml.CreateTextNode(&HSTRING::from(text))?;
                lines.GetAt(at)?.AppendChild(&node)?;
            }
            let toast = ToastNotification::CreateToastNotification(&xml)?;
            let (clicks, key) = (self.clicks.clone(), alert.key);
            toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, _| {
                clicks.clicked(key);
                Ok(())
            }))?;
            notifier.Show(&toast)?;
            if let Ok(mut kept) = self.kept.lock() {
                if kept.len() >= KEPT {
                    kept.pop_front();
                }
                kept.push_back(toast);
            }
            Ok(())
        }
    }

    impl AlertHost for Toasts {
        fn show(&self, alert: &DesktopAlert) -> Result<(), String> {
            if !self.installed {
                return Err("not installed by the .msi".into());
            }
            self.toast(alert).map_err(|e| e.to_string())
        }
        fn trouble(&self) -> Option<AlertTrouble> {
            if !self.installed {
                return Some(AlertTrouble::NotInstalled);
            }
            self.turned_off.lock().ok().filter(|off| **off).map(|_| AlertTrouble::TurnedOff)
        }
        fn clicked(&self) -> Vec<u64> {
            drain(&self.rx)
        }
    }
}

#[cfg(target_os = "macos")]
mod apple {
    use std::sync::{Arc, Mutex};

    use block2::RcBlock;
    use mailcraft_ui_egui::alerts::{AlertHost, AlertTrouble, DesktopAlert};
    use objc2::runtime::Bool;
    use objc2_foundation::{NSBundle, NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest, UNNotificationSound, UNUserNotificationCenter,
    };

    /// How many alerts wait for the owner's answer about permission; more are dropped.
    const WAITING: usize = 20;

    #[derive(Default)]
    enum Permission {
        /// Not asked yet.
        #[default]
        Unknown,
        /// Asked: these wait for the answer.
        Asking(Vec<DesktopAlert>),
        Granted,
        Refused,
    }

    pub struct Notifications {
        in_app: bool,
        permission: Arc<Mutex<Permission>>,
    }

    impl Notifications {
        pub fn new() -> Notifications {
            // Outside an app bundle there is no notification centre for the program, and asking
            // for it ends the program.
            let bundle = NSBundle::mainBundle();
            let in_app = bundle.bundleIdentifier().is_some() && bundle.bundlePath().to_string().ends_with(".app");
            Notifications { in_app, permission: Default::default() }
        }
    }

    /// Hand one alert to the notification centre.
    fn post(alert: &DesktopAlert) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&alert.title));
        content.setBody(&NSString::from_str(&alert.body));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        let id = NSString::from_str(&format!("mailcraft-alert-{}", alert.key));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);
        UNUserNotificationCenter::currentNotificationCenter().addNotificationRequest_withCompletionHandler(&request, None);
    }

    impl AlertHost for Notifications {
        fn show(&self, alert: &DesktopAlert) -> Result<(), String> {
            if !self.in_app {
                return Err("not running from the app".into());
            }
            let Ok(mut permission) = self.permission.lock() else { return Err("the permission's state is lost".into()) };
            match &mut *permission {
                Permission::Granted => {
                    post(alert);
                    Ok(())
                }
                Permission::Refused => Err("turned off in System Settings".into()),
                Permission::Asking(waiting) => {
                    if waiting.len() < WAITING {
                        waiting.push(alert.clone());
                    }
                    Ok(())
                }
                Permission::Unknown => {
                    *permission = Permission::Asking(vec![alert.clone()]);
                    drop(permission);
                    // Asked once; afterwards the system answers at once with what the owner chose.
                    let shared = self.permission.clone();
                    let answered = RcBlock::new(move |granted: Bool, _: *mut NSError| {
                        let Ok(mut permission) = shared.lock() else { return };
                        let waiting = match std::mem::take(&mut *permission) {
                            Permission::Asking(waiting) => waiting,
                            _ => Vec::new(),
                        };
                        *permission = if granted.as_bool() { Permission::Granted } else { Permission::Refused };
                        if granted.as_bool() {
                            waiting.iter().for_each(post);
                        }
                    });
                    let options = UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound;
                    UNUserNotificationCenter::currentNotificationCenter().requestAuthorizationWithOptions_completionHandler(options, &answered);
                    Ok(())
                }
            }
        }
        fn trouble(&self) -> Option<AlertTrouble> {
            if !self.in_app {
                return Some(AlertTrouble::OutsideApp);
            }
            matches!(self.permission.lock().as_deref(), Ok(Permission::Refused)).then_some(AlertTrouble::TurnedOff)
        }
    }
}

#[cfg(test)]
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
mod tests {
    use super::*;

    #[test]
    fn markup_in_a_body_is_shown_as_text() {
        assert_eq!(
            escape_markup("<img src=\"http://track.example/p.gif\"> & <b>x</b>"),
            "&lt;img src=\"http://track.example/p.gif\"&gt; &amp; &lt;b&gt;x&lt;/b&gt;"
        );
        assert_eq!(escape_markup("plain text, ünïcödé"), "plain text, ünïcödé");
        assert_eq!(escape_markup(""), "");
    }
}
