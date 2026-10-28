// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #344: column commands, saved settings and metadata-only sort keys.
use super::*;
use crate::{Prefs, sample};
use mailcraft_model::{Attachment, FlagStatus};

const NOW: i64 = 1_791_993_600;
fn session() -> Session {
    Session::at(sample::mailbox(NOW, 0), NOW, 0)
}

#[test]
fn configuring_columns_is_atomic_and_preserves_mail_and_selection() {
    let mut s = session();
    let mailbox = s.mailbox.clone();
    let selection = s.sel.clone();
    let want = json!({"enabled": true, "items": [{"column":"subject","width":300},{"column":"from","width":180}]});
    assert_eq!(s.run("view.columns", &want).unwrap(), want);
    let before = s.prefs.columns.clone();
    for bad in [
        json!({"enabled":"yes"}),
        json!({"items":[]}),
        json!({"items":[{"column":"from","width":100}]}),
        json!({"items":[{"column":"subject","width":0}]}),
        json!({"items":[{"column":"future","width":100}]}),
        json!({"items":[{"width":100}]}),
        json!({"items":[{"column":"subject"}]}),
        json!({"items":[{"column":"subject","width":100},{"column":"subject","width":200}]}),
    ] {
        assert!(s.run("view.columns", &bad).is_err(), "{bad}");
        assert_eq!(s.prefs.columns, before);
    }
    assert_eq!(s.mailbox, mailbox);
    assert_eq!(s.sel, selection);
    assert!(!s.can_undo());
    assert_eq!(s.run("view.columns", &json!({})).unwrap(), want);
}

#[test]
fn old_and_future_preferences_keep_a_usable_column_layout() {
    let old: Prefs = serde_json::from_value(json!({"oneLine":true})).unwrap();
    assert!(!old.columns.enabled);
    assert_eq!(old.columns, Columns::default());
    let mut future: Columns = serde_json::from_value(
        json!({"enabled":true,"items":[{"column":"future","width":50},{"column":"from","width":0},{"column":"from","width":65535},{"column":"size"}]}),
    )
    .unwrap();
    future.sanitize();
    // A column written without a width has its own.
    assert_eq!(future.items, vec![ColumnSpec { column: Column::From, width: 36 }, ColumnSpec::new(Column::Size), ColumnSpec::new(Column::Subject)]);
    let back: Columns = serde_json::from_value(json!(future)).unwrap();
    assert_eq!(back, future);
    // An arrangement a later build saved reads as By Date; the rest of the view stays.
    let view: crate::ViewState = serde_json::from_value(json!({"sort": "future", "sortDescending": true, "conversations": true})).unwrap();
    assert_eq!((view.sort, view.sort_descending, view.conversations), (SortBy::Date, true, true));
    let view: crate::ViewState = serde_json::from_value(json!({"sort": "flag"})).unwrap();
    assert_eq!(view.sort, SortBy::Flag);
    for saved in [json!({"sort": null}), json!({"sort": 3}), json!({})] {
        assert_eq!(serde_json::from_value::<crate::ViewState>(saved).unwrap().sort, SortBy::Date);
    }
}

#[test]
fn attachment_and_flag_sorts_reverse_and_keep_pins_on_top() {
    let mut s = session();
    let ids: Vec<_> = s.rows().iter().take(4).map(|r| r.id).collect();
    assert_eq!(ids.len(), 4);
    for m in s.mailbox.messages.values_mut() {
        let m = std::sync::Arc::make_mut(m);
        m.pinned = false;
        m.flag = FlagStatus::None;
        m.attachments.clear();
        m.attachments_on_server = false;
    }
    s.mailbox.message_mut(ids[0]).unwrap().pinned = true;
    s.mailbox.message_mut(ids[1]).unwrap().attachments.push(Attachment::new("a.pdf", "application/pdf", vec![1]));
    s.mailbox.message_mut(ids[1]).unwrap().flag = FlagStatus::Flagged;
    s.mailbox.message_mut(ids[2]).unwrap().flag = FlagStatus::Complete;
    s.mailbox_changed();
    for by in ["attachments", "flag"] {
        s.run("view.sort", &json!({"by":by})).unwrap();
        let rows = s.rows();
        assert_eq!(rows[0].id, ids[0]);
        assert_eq!(rows[1].id, ids[1]);
        s.run("view.reverseSort", &json!({})).unwrap();
        let rows = s.rows();
        assert_eq!(rows[0].id, ids[0]);
        assert_eq!(rows.last().unwrap().id, ids[1]);
    }
}

#[test]
fn an_invitations_calendar_part_is_not_a_file() {
    use mailcraft_model::Message;
    let mut m = Message::default();
    assert!(!has_files(&m));
    // A calendar exported to a file, or one that can't be read, is a file as any other.
    m.attachments.push(Attachment::new("holidays.ics", "text/calendar", b"BEGIN:VCALENDAR".to_vec()));
    assert!(has_files(&m));
    m.attachments[0] = Attachment::new("invite.ics", "text/calendar", b"BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT".to_vec());
    assert!(m.has_attachments());
    assert!(!has_files(&m));
    m.attachments.push(Attachment::new("agenda.pdf", "application/pdf", vec![1]));
    assert!(has_files(&m));
    let unfetched = Message { unfetched: true, attachments_on_server: true, ..Default::default() };
    assert!(has_files(&unfetched));
}

#[test]
fn arranged_by_attachments_an_invitation_counts_as_without() {
    let mut s = session();
    for m in s.mailbox.messages.values_mut() {
        let m = std::sync::Arc::make_mut(m);
        m.pinned = false;
        m.attachments.clear();
        m.attachments_on_server = false;
    }
    let ids: Vec<_> = s.rows().iter().map(|r| r.id).collect();
    let (invite, file) = (ids[ids.len() - 1], ids[ids.len() - 2]);
    s.mailbox.message_mut(invite).unwrap().attachments.push(Attachment::new(
        "invite.ics",
        "text/calendar",
        b"BEGIN:VCALENDAR\r\nMETHOD:REQUEST".to_vec(),
    ));
    s.mailbox.message_mut(file).unwrap().attachments.push(Attachment::new("a.pdf", "application/pdf", vec![1]));
    s.mailbox_changed();
    s.run("view.sort", &json!({"by": "attachments"})).unwrap();
    let rows = s.rows();
    assert_eq!(rows[0].id, file);
    assert_ne!(rows[1].id, invite);
    assert_eq!(rows.last().unwrap().id, invite);
}

#[test]
fn in_sent_items_from_arranges_by_who_the_mail_is_to() {
    use mailcraft_model::{Address, FolderKind, Message};
    let mut s = session();
    let account = s.mailbox.default_account().unwrap().id;
    let sent = s.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    let to = |name: &str, from: &str| Message {
        folder: sent,
        account,
        date: NOW,
        subject: format!("To {name}"),
        from: Address::new(from, format!("{}@example.com", from.to_lowercase())),
        to: vec![Address::new(name, format!("{}@example.com", name.to_lowercase()))],
        ..Default::default()
    };
    let abe = s.mailbox.add_message(to("Abe", "Zoe")).unwrap();
    let zed = s.mailbox.add_message(to("Zed", "Ann")).unwrap();
    let nobody = s.mailbox.add_message(Message { to: Vec::new(), ..to("", "Bea") }).unwrap();
    s.mailbox_changed();
    s.run("folder.select", &json!({"id": sent})).unwrap();
    s.run("view.sort", &json!({"by": "from", "descending": false})).unwrap();
    let order: Vec<_> = s.rows().iter().map(|r| r.id).filter(|id| [abe, zed].contains(id)).collect();
    assert_eq!(order, [abe, zed], "by the recipient, not the sender");
    // And headed by them too; mail to nobody yet as its row says it.
    let groups: Vec<_> = s.rows().iter().filter(|r| [abe, zed, nobody].contains(&r.id)).map(|r| r.group.clone()).collect();
    assert_eq!(groups, [Some(NO_RECIPIENTS.to_string()), Some("Abe".to_string()), Some("Zed".to_string())]);
}
