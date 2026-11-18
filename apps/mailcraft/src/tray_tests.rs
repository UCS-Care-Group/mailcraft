// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tray events stay bounded and restore the window through the normal egui commands.

use super::*;

fn unread_session() -> (mailcraft_engine::Session, mailcraft_model::Id, mailcraft_model::Id) {
    use mailcraft_model::{Account, AccountKind, FolderKind, Message};
    let mut mailbox = mailcraft_model::Mailbox::default();
    let account = mailbox.add_account(Account { kind: AccountKind::Imap, email: "avery@example.org".into(), ..Default::default() });
    let inbox = mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let first = mailbox.add_message(Message { account, folder: inbox, subject: "First synthetic message".into(), ..Default::default() }).unwrap();
    let second = mailbox.add_message(Message { account, folder: inbox, subject: "Second synthetic message".into(), ..Default::default() }).unwrap();
    (mailcraft_engine::Session::new(mailbox), first, second)
}

#[test]
fn badge_stays_until_the_last_unread_message_is_read_and_follows_undo() {
    let (mut session, first, second) = unread_session();
    assert!(has_unread(&mut session));
    session.run("mail.markRead", &serde_json::json!({"id": first})).unwrap();
    assert!(has_unread(&mut session), "reading one message must not clear the other one's badge");
    session.run("mail.markRead", &serde_json::json!({"id": second})).unwrap();
    assert!(!has_unread(&mut session));
    session.run("edit.undo", &serde_json::json!({})).unwrap();
    assert!(has_unread(&mut session));
    session.run("mail.markAllRead", &serde_json::json!({})).unwrap();
    assert!(!has_unread(&mut session));
    session.run("mail.markUnread", &serde_json::json!({"id": first})).unwrap();
    assert!(has_unread(&mut session));
    session.run("mail.delete", &serde_json::json!({"id": first})).unwrap();
    assert!(!has_unread(&mut session), "unread deleted mail must not leave a stale badge");
    let read = session.mailbox.message(second).unwrap();
    let arriving = mailcraft_model::Message { account: read.account, folder: read.folder, subject: "New arrival".into(), ..Default::default() };
    let arriving = session.mailbox.add_message(arriving).unwrap();
    session.mailbox_changed();
    assert!(has_unread(&mut session), "new mail turns the badge on again after the Inbox was read");
    session.mailbox.message_mut(arriving).unwrap().read = true;
    session.mailbox_changed();
    assert!(!has_unread(&mut session), "a read mark received from another device clears it too");
}

#[test]
fn only_inboxes_of_the_owners_server_accounts_count() {
    use mailcraft_model::{Account, AccountKind, FolderKind, Mailbox, Message};
    for (account_kind, own_inbox_counts) in [
        (AccountKind::Imap, true),
        (AccountKind::Microsoft, true),
        (AccountKind::Jmap, true),
        (AccountKind::Exchange, true),
        (AccountKind::Local, false),
        (AccountKind::Unknown, false),
    ] {
        for shared in [false, true] {
            for kind in FolderKind::STANDARD.into_iter().chain([FolderKind::Custom]) {
                let mut mailbox = Mailbox::default();
                let owner = mailbox.add_account(Account { kind: AccountKind::Microsoft, email: "owner@example.org".into(), ..Default::default() });
                let account = mailbox.add_account(Account {
                    kind: account_kind,
                    email: "avery@example.org".into(),
                    shared_of: shared.then_some(owner),
                    ..Default::default()
                });
                let folder = if kind == FolderKind::Custom {
                    mailbox.add_folder(account, None, "Imported archive").unwrap()
                } else {
                    mailbox.folder_of_kind(account, kind).unwrap()
                };
                mailbox.add_message(Message { account, folder, ..Default::default() }).unwrap();
                assert_eq!(
                    has_unread(&mut mailcraft_engine::Session::new(mailbox)),
                    kind == FolderKind::Inbox && own_inbox_counts && !shared,
                    "{account_kind:?}, shared={shared}, {kind:?}"
                );
            }
        }
    }
    assert!(!has_unread(&mut mailcraft_engine::Session::empty()));
}

#[test]
fn reading_or_filing_the_last_inbox_mail_clears_the_badge_despite_other_unread_mail() {
    use mailcraft_model::{Account, AccountKind, FolderKind, Message};
    let (mut session, first, second) = unread_session();
    let owner = session.mailbox.message(first).unwrap().account;
    let shared = session.mailbox.add_account(Account { kind: AccountKind::Microsoft, shared_of: Some(owner), ..Default::default() });
    let local = session.mailbox.add_account(Account::default());
    let imported = session.mailbox.add_folder(local, None, "Imported .pst Inbox").unwrap();
    let custom = session.mailbox.add_folder(owner, None, "Filed by a rule").unwrap();
    let archive = session.mailbox.folder_of_kind(owner, FolderKind::Archive).unwrap();
    for (account, folder) in [
        (shared, session.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap()),
        (local, session.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap()),
        (local, imported),
        (owner, custom),
        (owner, archive),
    ] {
        session.mailbox.add_message(Message { account, folder, ..Default::default() }).unwrap();
    }
    let other = session.mailbox.add_account(Account { kind: AccountKind::Imap, email: "other@example.org".into(), ..Default::default() });
    let inbox = session.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    let third = session.mailbox.add_message(Message { account: other, folder: inbox, ..Default::default() }).unwrap();
    session.mailbox_changed();
    session.run("mail.markRead", &serde_json::json!({"ids": [first, second]})).unwrap();
    assert!(has_unread(&mut session), "the other own account's Inbox still has unread mail");
    session.run("mail.markRead", &serde_json::json!({"id": third})).unwrap();
    assert!(!has_unread(&mut session), "shared, local, imported and filed unread mail must not hold the badge");
    session.run("mail.markUnread", &serde_json::json!({"id": first})).unwrap();
    assert!(has_unread(&mut session));
    session.run("mail.archive", &serde_json::json!({"id": first})).unwrap();
    assert!(!has_unread(&mut session), "filing the last unread Inbox message clears the badge");
    session.run("edit.undo", &serde_json::json!({})).unwrap();
    assert!(has_unread(&mut session), "undoing the move restores the unread Inbox message");
}

#[test]
fn badge_is_a_red_circle_in_the_top_left_and_does_not_change_the_rest_of_the_icon() {
    let [plain, badged] = icons().unwrap();
    assert_eq!((plain.width, plain.height, plain.rgba.len()), (badged.width, badged.height, badged.rgba.len()));
    let mut changed = 0;
    for (i, (before, after)) in plain.rgba.as_chunks::<4>().0.iter().zip(badged.rgba.as_chunks::<4>().0).enumerate() {
        if before != after {
            changed += 1;
            assert!(i % (plain.width as usize) < plain.width as usize / 2);
            assert!(i / (plain.width as usize) < plain.height as usize / 2);
        }
    }
    assert!(changed > 0);
    let center = (plain.width.min(plain.height) as f32 * 0.20) as usize;
    let pixel = (center * plain.width as usize + center) * 4;
    assert_eq!(badged.rgba.get(pixel..pixel + 4), Some([230, 40, 55, 255].as_slice()));
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
#[test]
fn hidden_tray_updates_without_notifications_and_reopening_does_not_acknowledge_unread_mail() {
    let ctx = Context::default();
    let mut tray = Tray::new(&ctx, true);
    tray.configure(true, |signals| {
        signals.online(true);
        backend::Backend::stand_in()
    });
    tray.hidden = true;
    let (session, first, second) = unread_session();
    let mut app = MailApp::new(session, mailcraft_ui_egui::Services::default());
    app.ui.close_to_tray = true;
    app.ui.alert_new_mail = false;
    tray.pump(&ctx, &mut app);
    assert!(tray.backend.as_ref().unwrap().badged());
    tray.signals.send(SHOW);
    tray.pump(&ctx, &mut app);
    assert!(!tray.hidden);
    assert!(tray.backend.as_ref().unwrap().badged(), "showing the window does not read the mail");
    app.session.run("mail.markRead", &serde_json::json!({"ids": [first, second]})).unwrap();
    tray.pump(&ctx, &mut app);
    assert!(!tray.backend.as_ref().unwrap().badged());
    app.session.run("mail.markUnread", &serde_json::json!({"id": first})).unwrap();
    tray.pump(&ctx, &mut app);
    assert!(tray.backend.as_ref().unwrap().badged());
    app.session = mailcraft_engine::Session::empty();
    tray.pump(&ctx, &mut app);
    assert!(!tray.backend.as_ref().unwrap().badged(), "opening an empty mailbox leaves no badge");
}

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
