//! The operating system's notifications, for new mail and reminders while the window is
//! elsewhere (`Services::notify`; what they say is decided in `mailcraft_ui_egui::notify`).
//!
//! `notify-rust` speaks each system's own way: the freedesktop.org notification service over
//! D-Bus on Linux and the BSDs (pure Rust, `zbus`), a toast on Windows, the Notification Center
//! on macOS. Each notice goes out on a thread of its own, as a D-Bus call can take a moment and
//! the window never waits. A system that won't show one (no notification service running, the
//! owner turned MailCraft's notifications off in the system's settings) is logged quietly, and
//! the log line says nothing of what the notice said: a subject is the owner's business.

use mailcraft_ui_egui::Notice;

/// What the system knows MailCraft by: the desktop entry and icon name on Linux and the BSDs,
/// the application id the Start Menu shortcut carries on Windows (`packaging/windows/mailcraft.wxs`),
/// the bundle identifier on macOS (`packaging/macos/Info.plist.in`).
const APP_ID: &str = "io.github.billsecond.mailcraft";

/// The hook for `Services::notify`.
pub fn hook() -> Box<dyn Fn(Notice)> {
    #[cfg(target_os = "macos")]
    {
        // Outside the app bundle (`cargo run`) there is no such application; the system then
        // files the notices under the program that started it, which is fine.
        if let Err(e) = notify_rust::set_application(APP_ID) {
            log::debug!("notifications: {e}");
        }
    }
    Box::new(|notice: Notice| {
        let spawned = std::thread::Builder::new().name("notification".into()).spawn(move || {
            if let Err(e) = show(&notice) {
                log::info!("the system didn't show a notification: {e}");
            }
        });
        if let Err(e) = spawned {
            log::info!("couldn't hand a notification to the system: {e}");
        }
    })
}

fn show(notice: &Notice) -> Result<(), String> {
    let mut n = notify_rust::Notification::new();
    n.summary(&notice.title).body(&notice.body).appname("MailCraft");
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // The icon by its name in the installed theme (the package puts it in hicolor), and the
        // desktop entry, so the notification service shows and groups it as MailCraft's.
        n.icon(APP_ID).hint(notify_rust::Hint::DesktopEntry(APP_ID.into()));
        if notice.kind == mailcraft_ui_egui::NoticeKind::NewMail {
            n.hint(notify_rust::Hint::Category("email.arrived".into()));
        }
    }
    #[cfg(target_os = "windows")]
    {
        n.app_id(APP_ID);
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let _ = notice.kind;
    n.show().map(|_| ()).map_err(|e| e.to_string())
}
