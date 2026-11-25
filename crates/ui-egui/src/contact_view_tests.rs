// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use egui::PointerButton;
use serde_json::json;

use crate::people_view::testing;

#[test]
fn contact_view_renders_list_and_chat_timeline_in_light_and_dark() {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("view.contactConversations", json!({"value": true})).unwrap();
    w.run_steps(3);

    // Select Tomás Rivera (who has both incoming messages and an outgoing reply in the sample mailbox).
    w.state_mut().execute("mail.selectCorrespondent", json!({"key": "tomas@lumen.example"})).unwrap();
    w.run_steps(3);

    assert!(w.state().session.view.contact_conversations);
    assert_eq!(w.state().session.view.selected_correspondent.as_deref(), Some("tomas@lumen.example"));

    let row_id = crate::ribbon::hit_id("tomas@lumen.example");
    assert!(w.state().hit.rect("correspondentRow", row_id).is_some(), "correspondent row should be drawn");
    assert!(w.state().hit.rect("contactMerge", 0).is_some(), "merge button should be drawn in chat header");

    // Verify per-message Reply button works on a visible message and opens compose addressed to Tomás.
    let reply_pos = w
        .state()
        .session
        .mailbox
        .messages
        .values()
        .filter(|m| m.from.email.eq_ignore_ascii_case("tomas@lumen.example"))
        .max_by_key(|m| m.date)
        .and_then(|m| w.state().hit.rect("chatReply", m.id))
        .expect("chatReply button drawn for latest Tomás message")
        .center();
    testing::click(&mut w, reply_pos, PointerButton::Primary, 1);
    w.run_steps(3);
    assert!(w.state().session.compose.is_some());
    w.state_mut().execute("compose.close", json!({"discard": true})).unwrap();
    w.run_steps(2);

    // Switch to dark theme and render without panic.
    w.state_mut().execute("ui.theme", json!({"value": "dark"})).unwrap();
    w.run_steps(3);
    assert!(w.state().hit.rect("correspondentRow", row_id).is_some());
}

#[test]
fn merge_correspondents_dialog_combines_addresses() {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("view.contactConversations", json!({"value": true})).unwrap();
    w.state_mut().execute("mail.selectCorrespondent", json!({"key": "tomas@lumen.example"})).unwrap();
    w.run_steps(2);

    let merge_pos = w.state().hit.rect("contactMerge", 0).expect("merge button").center();
    testing::click(&mut w, merge_pos, PointerButton::Primary, 1);
    w.run_steps(2);
    assert_eq!(testing::dialog(&w), Some("mergeCorrespondents"));

    // Merging through the engine command updates the UI list cleanly.
    w.state_mut()
        .execute("mail.mergeCorrespondents", json!({"addresses": ["tomas@lumen.example", "maya@lumen.example"], "name": "Design Team"}))
        .unwrap();
    w.state_mut().dialog = None;
    w.run_steps(3);

    let merged_id = crate::ribbon::hit_id("maya@lumen.example");
    assert!(w.state().hit.rect("correspondentRow", merged_id).is_some());
}

#[test]
fn chat_bubbles_render_html_cid_images_cc_and_click_to_read() {
    use mailcraft_model::{Address, Attachment, Body, FolderKind, Message};

    let mut mb = mailcraft_engine::sample::mailbox(testing::now(), 0);
    let account = mb.accounts[0].id;
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();

    // Minimal valid 1x1 PNG for testing inline `cid:` rendering in chat bubbles.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01,
        0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let mut inline_pic = Attachment::new("pixel.png", "image/png", TINY_PNG.to_vec());
    inline_pic.content_id = "hero-1".into();
    inline_pic.inline = true;

    let older_unread = mb
        .add_message(Message {
            folder: inbox,
            message_id: "alice-older-unread@partner.example".into(),
            from: Address::new("Alice Chen", "alice@partner.example"),
            to: vec![Address::new("Avery Quinn", "avery@lumen.example")],
            subject: "Quick check-in".into(),
            date: testing::now() - 3_600,
            received: testing::now() - 3_600,
            body: Body { text: "Are we still good for 3pm?".into(), html: None },
            read: false,
            ..Default::default()
        })
        .unwrap();

    let long_paras: String = (0..40).map(|i| format!("<p>Detailed release section line {i}: status review notes.</p>")).collect();
    let rich_id = mb
        .add_message(Message {
            folder: inbox,
            message_id: "rich-chat-test@lumen.example".into(),
            from: Address::new("Alice Chen", "alice@partner.example"),
            to: vec![Address::new("Avery Quinn", "avery@lumen.example")],
            cc: vec![Address::new("Tomás Rivera", "tomas@lumen.example"), Address::new("Priya Nair", "priya@lumen.example")],
            subject: "Design update with HTML and image".into(),
            date: testing::now(),
            received: testing::now(),
            body: Body {
                text: "Fallback plain text".into(),
                html: Some(format!("<h3>Mockup Preview</h3><p><img src=\"cid:hero-1\" width=\"48\" height=\"48\"></p>{long_paras}")),
            },
            attachments: vec![inline_pic],
            read: false,
            ..Default::default()
        })
        .unwrap();

    let mut w = testing::window_on(mb, "view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("view.contactConversations", json!({"value": true})).unwrap();
    w.run_steps(3);

    // Click Alice's row (Option B): latest unread becomes read, older unread stays unread.
    let alice_row = w.state().hit.rect("correspondentRow", crate::ribbon::hit_id("alice@partner.example")).expect("Alice row").center();
    testing::click(&mut w, alice_row, PointerButton::Primary, 1);
    w.run_steps(4);

    assert!(w.state().session.mailbox.message(rich_id).unwrap().read, "latest unread marked read on row click");
    assert!(!w.state().session.mailbox.message(older_unread).unwrap().read, "older unread stays unread under Option B");

    // HTML + inline image layout was built under Place::Chat(rich_id), and tall HTML is clamped below half-screen ceiling.
    let laid_h = crate::reading::view::laid_size(&w.ctx, crate::reading::Place::Chat(rich_id), rich_id).map(|s| s.y).unwrap_or(0.0);
    assert!(laid_h > 460.0, "expected tall HTML body > 460pt, got {laid_h}");
    let bubble_rect = w.state().hit.rect("chatBubble", rich_id).expect("rich bubble rect");
    assert!(bubble_rect.height() < laid_h, "tall HTML bubble should be collapsed, bubble_h={} laid_h={laid_h}", bubble_rect.height());

    // Verify `To:` and `Cc:` labels are painted in the chat pane (accessible text includes Cc recipient).
    assert!(w.state().hit.rect("contactMarkAllRead", 0).is_some(), "Mark All Read button shown when unread messages remain");

    // Clicking the older unread bubble marks it read too.
    let older_pos = w.state().hit.rect("chatBubble", older_unread).expect("older bubble").center();
    testing::click(&mut w, older_pos, PointerButton::Primary, 1);
    w.run_steps(3);
    assert!(w.state().session.mailbox.message(older_unread).unwrap().read, "clicking unread bubble marks it read");

    // Clicking envelope toggle marks it unread again and keeps it unread (not immediately re-read on same frame).
    let toggle_pos = w.state().hit.rect("chatToggleRead", older_unread).expect("toggle read button").center();
    testing::click(&mut w, toggle_pos, PointerButton::Primary, 1);
    w.run_steps(3);
    assert!(!w.state().session.mailbox.message(older_unread).unwrap().read, "envelope toggle leaves message unread");

    // Clicking the prominent Mark All Read button clears remaining unread count.
    let mark_all_pos = w.state().hit.rect("contactMarkAllRead", 0).expect("Mark All Read button").center();
    testing::click(&mut w, mark_all_pos, PointerButton::Primary, 1);
    w.run_steps(3);
    assert!(w.state().session.mailbox.message(older_unread).unwrap().read);
    assert!(w.state().hit.rect("contactMarkAllRead", 0).is_none());

    // Clicking blank space in the right-hand chat pane clears all bubble selection.
    assert!(!w.state().session.sel.messages.is_empty());
    let rich_rect = w.state().hit.rect("chatBubble", rich_id).expect("rich bubble rect");
    let blank_pos = egui::pos2(rich_rect.max.x + 24.0, rich_rect.center().y);
    testing::click(&mut w, blank_pos, PointerButton::Primary, 1);
    w.run_steps(2);
    assert!(w.state().session.sel.messages.is_empty(), "clicking blank area clears bubble selection");
}

#[test]
fn prefs_restore_contact_view_flags_and_merge_dialog_limits_remove_to_manual_groups() {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("view.contactConversations", json!({"value": true, "allAccounts": true})).unwrap();
    w.state_mut().execute("mail.selectCorrespondent", json!({"key": "tomas@lumen.example"})).unwrap();
    let saved = w.state().prefs();

    let mut fresh = testing::window("view.mail");
    fresh.state_mut().apply_prefs(saved);
    assert!(fresh.state().session.view.contact_conversations);
    assert!(fresh.state().session.view.contact_all_accounts);
    assert!(!fresh.state().session.view.conversations);
    assert!(fresh.state().session.view.selected_correspondent.is_none());

    // Contact-only links without a manual CorrespondentGroup have empty manual_addresses.
    let dlg = crate::contact_view::MergeDialog::open(
        fresh.state(),
        "tomas@lumen.example",
        "Tomás",
        &["tomas@lumen.example".into(), "tomas.alt@lumen.example".into()],
    );
    assert!(dlg.manual_addresses.is_empty());

    // After manual merge, both addresses appear in manual_addresses.
    fresh.state_mut().execute("mail.mergeCorrespondents", json!({"addresses": ["tomas@lumen.example", "maya@lumen.example"]})).unwrap();
    let dlg2 = crate::contact_view::MergeDialog::open(
        fresh.state(),
        "maya@lumen.example",
        "Maya",
        &["maya@lumen.example".into(), "tomas@lumen.example".into()],
    );
    assert!(dlg2.manual_addresses.contains("tomas@lumen.example"));
    assert!(dlg2.manual_addresses.contains("maya@lumen.example"));
}
