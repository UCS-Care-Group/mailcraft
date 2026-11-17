// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tray events stay bounded and restore the window through the normal egui commands.

use super::*;

#[test]
fn tray_events_coalesce_without_an_unbounded_queue() {
    let signals = Signals { pending: AtomicU8::new(0), online: AtomicBool::new(false), ctx: Context::default() };
    for _ in 0..100_000 {
        signals.send(SHOW);
        signals.send(QUIT);
    }
    assert_eq!(signals.pending.swap(0, Ordering::Relaxed), SHOW | QUIT);
    assert_eq!(signals.pending.load(Ordering::Relaxed), 0);
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
#[test]
fn hidden_windows_remain_reachable_and_quit_keeps_using_the_apps_save_path() {
    // No desktop service or mail account is touched: only the tray's I/O is a stand-in.
    for action in ["click", "quit", "trayLost", "optionOff", "notification", "idle"] {
        let ctx = Context::default();
        let signals = Arc::new(Signals { pending: AtomicU8::new(0), online: AtomicBool::new(true), ctx: ctx.clone() });
        let mut tray = Tray { backend: Some(backend::Backend::stand_in()), signals, hidden: true, can_hide: true };
        let mut app = MailApp::new(mailcraft_engine::Session::new(mailcraft_model::Mailbox::default()), mailcraft_ui_egui::Services::default());
        app.ui.close_to_tray = true;
        match action {
            "click" => tray.signals.send(SHOW),
            "quit" => tray.signals.send(QUIT),
            "trayLost" => tray.signals.online(false),
            "optionOff" => app.ui.close_to_tray = false,
            _ => (),
        }
        let out = ctx.run_logic(&egui::RawInput::default(), |ctx| {
            if action == "notification" {
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
            tray.pump(ctx, &mut app);
            tray.after_frame(ctx);
        });
        let commands = out.viewport_commands.get(&egui::ViewportId::ROOT).cloned().unwrap_or_default();
        assert_eq!(commands.contains(&ViewportCommand::Visible(true)), action != "idle", "{action}");
        assert_eq!(tray.hidden, action == "idle", "{action}");
        assert_eq!(app.quit_requested, action == "quit", "{action}");
        assert!(!commands.contains(&ViewportCommand::Close), "Quit must pass through the app's save and unsaved-work checks");
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[test]
fn x11_is_only_probed_and_selected_when_the_option_is_on() {
    assert!(!prefer_x11(false, || panic!("default startup must not probe X11")));
    assert!(!prefer_x11(true, || false), "unreachable X11 leaves default backend selection alone");
    assert!(prefer_x11(true, || true));
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
#[test]
fn tray_lifetime_follows_the_option_and_discards_old_callbacks() {
    let ctx = Context::default();
    let mut tray = Tray::new(&ctx, true);
    assert!(tray.backend.is_none());
    tray.configure(false, |_| panic!("disabled must not start a tray"));
    tray.configure(true, |signals| {
        signals.online(true);
        backend::Backend::stand_in()
    });
    assert!(tray.available());
    let old = tray.signals.clone();
    tray.configure(true, |_| panic!("enabled must not restart each frame"));
    tray.configure(false, |_| panic!("turning off must not start a tray"));
    assert!(tray.backend.is_none());
    assert!(!tray.available());
    tray.configure(true, |_| backend::Backend::stand_in());
    old.online(true);
    old.send(QUIT);
    assert!(!tray.available());
    assert_eq!(tray.signals.pending.load(Ordering::Relaxed), 0);
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
#[test]
fn appkit_hide_tracks_only_tray_closes_and_dock_unhide_ends_them() {
    let ctx = Context::default();
    let mut tray = Tray::new(&ctx, true);
    // The app hidden through Cmd+H isn't a tray hide, even with the option off.
    tray.after_frame_with_visibility(&ctx, Some(true));
    assert!(!tray.hidden);
    let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| {
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("mailcraft.tray.hidden"), true));
        tray.after_frame_with_visibility(ctx, Some(true));
    });
    assert!(tray.hidden, "the native hide wins over a viewport command queued before it");
    tray.after_frame_with_visibility(&ctx, Some(false));
    assert!(!tray.hidden, "the Dock's unhide ends the tray hide");
}
