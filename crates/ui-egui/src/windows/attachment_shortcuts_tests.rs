// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use egui::{Key, vec2};
use egui_kittest::Harness;
use mailcraft_engine::Session;
use mailcraft_model::{Attachment, FolderKind};

use super::{Verdict, What, contents};
use crate::{MailApp, Services, attachment_preview};

#[test]
fn a_native_message_window_keeps_its_delete_shortcut_with_the_attachment_ribbon() {
    const NOW: i64 = 1_791_993_600;
    const MESSAGE: u64 = 13;
    const WINDOW: u64 = 1;

    let mut mailbox = mailcraft_engine::sample::mailbox(NOW, 0);
    let message = mailbox.message_mut(MESSAGE).unwrap();
    message.attachments = vec![Attachment::new("release-notes.pdf", "application/pdf", b"%PDF-1.4".to_vec())];
    let account = message.account;
    let trash = mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    let what = What::Message { message: MESSAGE, seen: mailbox.messages.get(&MESSAGE).unwrap().clone() };
    let app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    let mut h = Harness::builder().with_size(vec2(1000.0, 700.0)).allow_missing_glyphs().build_ui_state(
        |ui, (app, what, closed)| {
            app.hit.clear();
            *closed = matches!(contents(app, ui, WINDOW, what, true, None), Verdict::Close);
        },
        (app, what, false),
    );
    let ctx = h.ctx.clone();
    let app = &mut h.state_mut().0;
    let old_scope = app.hit.scope(WINDOW);
    attachment_preview::queue_click(app, &ctx, MESSAGE, 0);
    app.hit.scope(old_scope);
    h.run_steps(30);
    assert!(h.state().0.hit.rect_in(WINDOW, "attachmentPreviewBack", MESSAGE).is_some());
    h.key_down(Key::Delete);
    h.step();
    assert!(h.state().2, "the message window closed");
    assert_eq!(h.state().0.session.mailbox.message(MESSAGE).unwrap().folder, trash);
}
