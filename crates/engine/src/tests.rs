// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use mailcraft_model::{FlagStatus, FolderKind, Id, Importance};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::*;

/// Wednesday 2026-10-14 16:00 UTC.
fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn session() -> Session {
    Session::at(sample::mailbox(now(), 0), now(), 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn kind(s: &Session, k: FolderKind) -> Id {
    s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, k).unwrap()
}

fn subjects(s: &mut Session) -> Vec<String> {
    let rows = s.rows();
    rows.iter().map(|r| s.mailbox.message(r.id).unwrap().subject.clone()).collect()
}

fn first(s: &mut Session) -> Id {
    s.rows()[0].id
}

#[test]
fn command_ids_are_unique_and_well_formed() {
    let s = session();
    let mut seen = std::collections::HashSet::new();
    for c in s.registry.all() {
        assert!(seen.insert(c.id), "duplicate id {}", c.id);
        assert!(c.id.contains('.') && c.id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '.'), "{}", c.id);
        assert!(!c.label.is_empty() && !c.location.is_empty(), "{}", c.id);
        assert!(c.params.starts_with('{'), "{}: {}", c.id, c.params);
    }
    assert!(s.registry.all().len() >= 120, "{}", s.registry.all().len());
    assert_eq!(s.registry.by_shortcut("mod+r").map(|c| c.id), Some("mail.reply"));
    assert_eq!(s.registry.by_shortcut("Mod+D").map(|c| c.id), Some("mail.delete"));
}

#[test]
fn shortcuts_are_not_bound_twice() {
    let s = session();
    let mut seen: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
    for c in s.registry.all().iter().filter(|c| !c.shortcut.is_empty()) {
        for key in c.shortcut.split(" / ") {
            // The same key may serve different modules (Delete in mail, people and tasks).
            let scope = c.id.split('.').next().unwrap();
            if let Some(other) = seen.insert(format!("{scope}:{}", key.to_lowercase()), c.id) {
                panic!("{key} is bound to both {other} and {}", c.id);
            }
        }
    }
}

#[test]
fn the_sample_mailbox_opens_on_a_grouped_inbox() {
    let mut s = session();
    assert_eq!(s.view.folder, kind(&s, FolderKind::Inbox));
    let rows = s.rows();
    let groups: Vec<&str> = rows.iter().filter_map(|r| r.group.as_deref()).collect();
    assert_eq!(groups, ["Pinned", "Today", "Yesterday", "Monday", "Sunday", "Last Week", "Two Weeks Ago", "Last Month"]);
    assert_eq!(subjects(&mut s)[..2], ["Welcome to the Atlas project", "Homepage mockups, round three"]);
    assert_eq!(s.mailbox.unread_count(s.view.folder), 4);
    assert!(s.mailbox.repair().is_empty());
}

#[test]
fn show_in_groups_off_leaves_rows_without_headings_and_is_remembered() {
    let mut s = session();
    let sorted: Vec<Id> = s.rows().iter().map(|r| r.id).collect();
    assert!(s.rows().iter().any(|r| r.group.is_some()));
    assert_eq!(s.run("view.groups", &json!({})).unwrap()["value"], false);
    let rows = s.rows();
    assert!(rows.iter().all(|r| r.group.is_none()));
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), sorted);
    // Saved with the other preferences, and on again by the same command.
    let saved = serde_json::to_value(&s.prefs).unwrap();
    assert_eq!(saved["showGroups"], false);
    assert!(!serde_json::from_value::<crate::Prefs>(saved).unwrap().show_groups);
    assert!(serde_json::from_value::<crate::Prefs>(json!({})).unwrap().show_groups);
    assert_eq!(s.run("view.groups", &json!({"value": true})).unwrap()["value"], true);
    assert!(s.rows().iter().any(|r| r.group.is_some()));
}

#[test]
fn the_single_line_list_is_a_toggled_preference() {
    let mut s = session();
    assert!(!s.prefs.one_line);
    assert_eq!(s.run("view.oneLine", &json!({})).unwrap()["value"], true);
    assert!(s.prefs.one_line);
    assert_eq!(serde_json::to_value(&s.prefs).unwrap()["oneLine"], true);
    assert_eq!(s.run("view.oneLine", &json!({"value": false})).unwrap()["value"], false);
    assert!(serde_json::to_value(&s.prefs).unwrap().get("oneLine").is_none());
}

#[test]
fn selecting_marks_read_without_an_undo_step() {
    let mut s = session();
    let id = s.rows()[1].id;
    assert!(!s.mailbox.message(id).unwrap().read);
    run(&mut s, "mail.select", json!({"id": id}));
    assert!(s.mailbox.message(id).unwrap().read);
    assert!(!s.can_undo() && s.dirty);
    s.prefs.mark_read_on_open = false;
    run(&mut s, "mail.next", json!({}));
    assert!(!s.focused_message().unwrap().read);
    assert!(s.run("mail.select", &json!({"id": 424242})).is_err());
}

#[test]
fn delete_walks_down_the_list_and_undo_brings_it_back() {
    let mut s = session();
    let before = subjects(&mut s);
    let id = first(&mut s);
    run(&mut s, "mail.select", json!({"id": id}));
    run(&mut s, "mail.delete", json!({}));
    assert_eq!(subjects(&mut s), before[1..]);
    // The next message took its place and is selected.
    assert_eq!(s.focused_message().unwrap().subject, before[1]);
    assert_eq!(s.mailbox.message(id).unwrap().folder, kind(&s, FolderKind::Deleted));
    assert_eq!(s.undo_label(), Some("Delete"));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(subjects(&mut s), before);
    assert_eq!(s.sel.focus(), Some(id));
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(subjects(&mut s).len(), before.len() - 1);
    // Deleting from Deleted Items removes for good.
    run(&mut s, "mail.delete", json!({"id": id}));
    assert!(s.mailbox.message(id).is_none());
}

#[test]
fn a_failed_command_changes_nothing() {
    let mut s = session();
    let before = s.mailbox.clone();
    let ids: Vec<Id> = s.rows().iter().take(2).map(|r| r.id).collect();
    // The first message would move before the second one fails.
    let r = s.run("mail.move", &json!({"ids": [ids[0], 424242], "folder": kind(&s, FolderKind::Archive)}));
    assert!(r.is_err());
    assert!(s.mailbox == before && !s.can_undo() && !s.dirty);
    assert!(matches!(s.run("nope.nothing", &json!({})), Err(CmdError::Unknown(_))));
    assert!(matches!(s.run("mail.reply", &json!({})), Err(CmdError::Disabled(_))));
    // Naming the message lifts a selection-only disable.
    assert!(s.run("mail.markRead", &json!({"id": ids[0]})).is_ok());
}

#[test]
fn flags_categories_and_read_state() {
    let mut s = session();
    let ids: Vec<Id> = s.rows().iter().skip(1).take(3).map(|r| r.id).collect();
    run(&mut s, "mail.select", json!({"ids": ids}));
    run(&mut s, "mail.toggleRead", json!({}));
    assert!(ids.iter().all(|id| s.mailbox.message(*id).unwrap().read));
    run(&mut s, "mail.toggleRead", json!({}));
    assert!(ids.iter().all(|id| !s.mailbox.message(*id).unwrap().read));
    run(&mut s, "mail.flag", json!({"due": "2026-10-16"}));
    let m = s.mailbox.message(ids[0]).unwrap();
    assert_eq!((m.flag, s.local(m.flag_due.unwrap()).iso()), (FlagStatus::Flagged, "2026-10-16T17:00:00".to_string()));
    run(&mut s, "mail.complete", json!({"id": ids[0]}));
    assert_eq!(s.mailbox.message(ids[0]).unwrap().flag, FlagStatus::Complete);
    run(&mut s, "mail.categorize", json!({"category": "red CATEGORY"}));
    assert_eq!(s.mailbox.message(ids[1]).unwrap().categories.last().map(String::as_str), Some("Red category"));
    run(&mut s, "mail.categorize", json!({"category": "Waiting"}));
    assert!(s.mailbox.category("waiting").is_some());
    run(&mut s, "mail.categorize", json!({"category": "Waiting"}));
    assert!(!s.mailbox.message(ids[1]).unwrap().has_category("Waiting"));
    // One undo step per command, whatever the number of messages.
    assert_eq!(s.undo_label(), Some("Categorize"));
    // Marking read what is read already changes nothing and adds no step.
    run(&mut s, "mail.markUnread", json!({}));
    let steps = (0..50).take_while(|_| s.undo()).count();
    assert_eq!(steps, 7);
}

#[test]
fn replies_address_the_right_people_and_quote() {
    let mut s = session();
    let id = s.rows().iter().map(|r| r.id).find(|id| s.mailbox.message(*id).unwrap().subject == "RE: Release checklist").unwrap();
    run(&mut s, "mail.select", json!({"id": id}));
    run(&mut s, "mail.replyAll", json!({}));
    let c = s.compose.clone().unwrap();
    assert_eq!(c.to, "Tomás Rivera <tomas@lumen.example>; Maya Okafor <maya@lumen.example>");
    assert_eq!((c.cc.as_str(), c.subject.as_str()), ("", "RE: Release checklist"));
    assert!(c.body.plain().starts_with("\n\nAvery Quinn\nLumen Studio\n\n-----Original Message-----\nFrom: Tomás Rivera <tomas@lumen.example>\nSent: Tuesday, October 13, 2026 4:20 PM\n"), "{}", c.body.plain());
    assert_eq!(c.references.last().map(String::as_str), Some(s.mailbox.message(id).unwrap().message_id.as_str()));
    run(&mut s, "mail.reply", json!({}));
    assert_eq!(s.compose.as_ref().unwrap().to, "Tomás Rivera <tomas@lumen.example>");
    let first_id = first(&mut s);
    run(&mut s, "mail.forward", json!({"id": first_id}));
    let c = s.compose.clone().unwrap();
    assert_eq!((c.to.as_str(), c.subject.as_str()), ("", "FW: Welcome to the Atlas project"));
    // Composing is not an edit of the mailbox.
    assert!(!s.can_undo());
}

#[test]
fn sending_files_the_message_and_marks_the_original() {
    let mut s = session();
    let id = s.rows()[1].id;
    run(&mut s, "mail.reply", json!({"id": id}));
    assert!(s.run("compose.send", &json!({})).is_ok());
    assert!(s.compose.is_none());
    assert!(s.mailbox.message(id).unwrap().answered);
    let sent = kind(&s, FolderKind::Sent);
    let m = s.mailbox.messages_in(sent).max_by_key(|m| m.id).unwrap();
    assert_eq!((m.subject.as_str(), m.date, m.read), ("RE: Homepage mockups, round three", now(), true));
    assert!(m.message_id.ends_with("@lumen.example") && m.in_reply_to == s.mailbox.message(id).unwrap().message_id);
    assert_eq!(m.from.full(), "Avery Quinn <avery@lumen.example>");

    // Bad recipients stop a send and keep the message open.
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example; not an address", "subject": "x"}));
    let e = s.run("compose.send", &json!({})).unwrap_err();
    assert!(e.to_string().contains("not an address"), "{e}");
    assert!(s.compose.is_some());
    run(&mut s, "compose.set", json!({"to": ""}));
    assert!(s.run("compose.send", &json!({})).unwrap_err().to_string().contains("recipient"));
    // A message to myself arrives in my own Inbox.
    run(&mut s, "compose.set", json!({"to": "AVERY@lumen.example", "bcc": "maya@lumen.example", "body": "note to self"}));
    let r = run(&mut s, "compose.send", json!({}));
    assert_eq!(r["delivered"], 1);
    let inbox = kind(&s, FolderKind::Inbox);
    let copy = s.mailbox.messages_in(inbox).max_by_key(|m| m.id).unwrap();
    assert!(!copy.read && copy.bcc.is_empty() && copy.body.text.starts_with("note to self"));
}

/// A `mailto:` link starts a message with its recipients, subject and body, and with nothing else
/// it names; it can't make itself the sender, add an attachment or smuggle a header in.
#[test]
fn a_mailto_link_starts_a_message() {
    let mut s = session();
    run(
        &mut s,
        "mail.new",
        json!({"mailto": "mailto:maya@lumen.example,lin@lumen.example?cc=jonas@lumen.example&subject=Lunch%20on%20Friday%3F&body=Noon%0D%0Aat%20the%20usual%20place"}),
    );
    let c = s.compose.clone().unwrap();
    assert_eq!((c.to.as_str(), c.cc.as_str(), c.bcc.as_str()), ("maya@lumen.example; lin@lumen.example", "jonas@lumen.example", ""));
    assert_eq!(c.subject, "Lunch on Friday?");
    assert!(c.body.plain().starts_with("Noon\nat the usual place"), "{:?}", c.body.plain());
    // It sends like any other message.
    run(&mut s, "compose.send", json!({}));
    let sent = kind(&s, FolderKind::Sent);
    let m = s.mailbox.messages_in(sent).max_by_key(|m| m.id).unwrap();
    assert_eq!(m.from.full(), "Avery Quinn <avery@lumen.example>");
    assert_eq!((m.to.len(), m.cc.len(), m.attachments.len()), (2, 1, 0));

    // What the link has no say in, and what the caller says itself.
    run(
        &mut s,
        "mail.new",
        json!({"mailto": "MAILTO:a@lumen.example?from=boss@lumen.example&attach=/etc/passwd&subject=Hi%0D%0ABcc:%20x@evil.example&bcc=b@lumen.example", "to": "maya@lumen.example"}),
    );
    let c = s.compose.clone().unwrap();
    assert_eq!((c.to.as_str(), c.bcc.as_str()), ("maya@lumen.example", "b@lumen.example"));
    assert!(!c.subject.contains(['\r', '\n']) && c.attachments.is_empty(), "{:?}", c.subject);
    run(&mut s, "compose.discard", json!({}));

    // Not a link: an error, and no message is started.
    for junk in ["", "maya@lumen.example", "https://lumen.example/?mailto:x", "mail"] {
        assert!(s.run("mail.new", &json!({"mailto": junk})).is_err(), "{junk:?}");
        assert!(s.compose.is_none());
    }
    assert!(s.run("mail.new", &json!({"mailto": "mailto:"})).is_ok());
}

#[test]
fn drafts_save_reopen_and_discard() {
    let mut s = session();
    let drafts = kind(&s, FolderKind::Drafts);
    let n = s.mailbox.count(drafts);
    run(&mut s, "mail.new", json!({"to": "Lin", "subject": "Half a thought"}));
    let d = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    assert_eq!(s.mailbox.count(drafts), n + 1);
    assert!(s.mailbox.message(d).unwrap().draft && !s.compose.as_ref().unwrap().dirty);
    // Saving again updates the same draft.
    run(&mut s, "compose.set", json!({"body": "more"}));
    assert!(s.compose.as_ref().unwrap().dirty);
    run(&mut s, "compose.saveDraft", json!({}));
    assert_eq!((s.mailbox.count(drafts), s.mailbox.message(d).unwrap().body.text.as_str()), (n + 1, "more"));
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": d}));
    assert_eq!(s.compose.as_ref().unwrap().subject, "Half a thought");
    // Check Names turns the one matching contact into an address.
    assert_eq!(run(&mut s, "compose.checkNames", json!({}))["unresolved"], json!([]));
    assert_eq!(s.compose.as_ref().unwrap().to, "Lin Zhao <lin.zhao@harborworks.example>");
    run(&mut s, "compose.discard", json!({}));
    assert!(s.mailbox.message(d).is_none() && s.compose.is_none());
    let first_id = first(&mut s);
    assert!(s.run("mail.editDraft", &json!({"id": first_id})).is_err());
}

#[test]
fn attachments_come_and_go() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example"}));
    let r = run(&mut s, "compose.attach", json!({"name": "C:\\Users\\me\\notes.txt", "data": "aGVsbG8="}));
    assert_eq!((r["name"].as_str(), r["size"].as_u64()), (Some("notes.txt"), Some(5)));
    assert!(s.run("compose.attach", &json!({"name": "x.bin", "data": "***"})).is_err());
    assert!(s.run("compose.removeAttachment", &json!({"index": 5})).is_err());
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let got = run(&mut s, "mail.saveAttachment", json!({"id": id, "index": 0}));
    assert_eq!((got["name"].as_str(), got["data"].as_str()), (Some("notes.txt"), Some("aGVsbG8=")));
    run(&mut s, "mail.removeAttachment", json!({"id": id, "index": 0}));
    assert!(s.mailbox.message(id).unwrap().attachments.is_empty());
}

#[test]
fn saved_attachments_and_exports_are_named_safely() {
    let mut s = session();
    let id = first(&mut s);
    let names = ["invoice\u{202e}fdp.exe", "CON.txt", "../../x", "a:b", "re\u{200b}port.pdf", ""];
    for name in names {
        s.mailbox.message_mut(id).unwrap().attachments.push(mailcraft_model::Attachment::new(name, "application/octet-stream", b"hi".to_vec()));
    }
    let base = s.mailbox.message(id).unwrap().attachments.len() - names.len();
    let saved: Vec<String> = (0..names.len())
        .map(|i| run(&mut s, "mail.saveAttachment", json!({"id": id, "index": base + i}))["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(saved, ["invoice_fdp.exe", "_CON.txt", "x.bin", "a_b.bin", "re_port.pdf", "attachment.bin"]);
    s.mailbox.message_mut(id).unwrap().subject = "CON".into();
    assert_eq!(run(&mut s, "file.exportMessage", json!({"id": id}))["name"], "_CON.eml");
    s.mailbox.message_mut(id).unwrap().subject = "Q1/Q2\u{202e}fdp".into();
    assert_eq!(run(&mut s, "file.exportMessage", json!({"id": id}))["name"], "Q1_Q2_fdp.eml");
}

#[test]
fn attachments_are_opened_only_in_the_window() {
    let mut s = session();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example"}));
    run(&mut s, "compose.attach", json!({"name": "notes.txt", "data": "aGVsbG8="}));
    run(&mut s, "compose.attach", json!({"name": "setup.exe", "data": "aGVsbG8="}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    // Outside the window nothing is opened: an ordinary file says where it would be...
    let e = s.run("mail.openAttachment", &json!({"id": id, "index": 0})).unwrap_err().to_string();
    assert!(e.contains("window") && e.contains("mail.saveAttachment"), "{e}");
    // ...and a program is refused for what it is.
    let e = s.run("mail.openAttachment", &json!({"id": id, "index": 1})).unwrap_err().to_string();
    assert!(e.contains("doesn't open setup.exe") && e.contains("Save As"), "{e}");
    assert!(s.run("mail.openAttachment", &json!({"id": id, "index": 9})).is_err());
    assert!(s.run("mail.openAttachment", &json!({"id": id})).is_err());
    // Agents may not ask for it at all.
    assert_eq!(crate::agent::access("mail.openAttachment"), Some(crate::agent::Access::Never));
}

#[test]
fn folders_and_views() {
    let mut s = session();
    let r = run(&mut s, "folder.select", json!({"path": "inbox/projects/atlas"}));
    assert_eq!(r["messages"], 2);
    let id = run(&mut s, "folder.new", json!({"name": "Decisions"}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.folder_path(id), "Inbox/Projects/Atlas/Decisions");
    run(&mut s, "folder.rename", json!({"id": id, "name": "Log"}));
    run(&mut s, "folder.move", json!({"id": id, "parent": null}));
    assert_eq!(s.mailbox.folder_path(id), "Log");
    run(&mut s, "folder.select", json!({"id": id}));
    run(&mut s, "folder.delete", json!({}));
    assert_eq!(s.mailbox.folder_path(id), "Deleted Items/Log");
    assert_eq!(s.view.folder, kind(&s, FolderKind::Inbox));
    assert!(s.run("folder.rename", &json!({"id": s.view.folder, "name": "In"})).is_err());

    run(&mut s, "folder.select", json!({"kind": "junk"}));
    assert_eq!(run(&mut s, "folder.empty", json!({}))["removed"], 1);
    assert!(matches!(s.run("folder.empty", &json!({})), Err(CmdError::Disabled(_))));

    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.sort", json!({"by": "from"}));
    assert!(!s.view.sort_descending);
    let rows = s.rows();
    let senders: Vec<&str> = rows.iter().filter_map(|r| r.group.as_deref()).collect();
    assert_eq!(senders[..3], ["Pinned", "Cloudline Billing", "Dev Patel"]);
    // By size, largest on top, a message of which only the start is here sorts by the size its
    // server said (#273).
    let unpinned =
        |s: &mut Session| -> Vec<Id> { s.rows().iter().map(|r| r.id).filter(|id| s.mailbox.message(*id).is_some_and(|m| !m.pinned)).collect() };
    run(&mut s, "view.sort", json!({"by": "size", "descending": true}));
    let order = unpinned(&mut s);
    let partial = *order.last().unwrap();
    let m = s.mailbox.message_mut(partial).unwrap();
    m.body = mailcraft_model::Body { text: "Only the start".into(), html: None };
    m.attachments.clear();
    m.unfetched = true;
    m.server = Some(mailcraft_model::sync::ServerMessage { size: 50 << 20, ..Default::default() });
    // Another order and back, for the list to be made again.
    run(&mut s, "view.sort", json!({"by": "date"}));
    s.rows();
    run(&mut s, "view.sort", json!({"by": "size", "descending": true}));
    assert_eq!(unpinned(&mut s).first(), Some(&partial));
    assert_ne!(order.first(), Some(&partial));
    // One whose size isn't known counts as the smallest: last with the largest on top, first
    // with the smallest.
    let unknown = order[0];
    s.mailbox.message_mut(unknown).unwrap().headers.push((mailcraft_model::NOT_DOWNLOADED.into(), "62914560".into()));
    run(&mut s, "view.sort", json!({"by": "date"}));
    s.rows();
    run(&mut s, "view.sort", json!({"by": "size", "descending": true}));
    assert_eq!(unpinned(&mut s).last(), Some(&unknown));
    run(&mut s, "view.sort", json!({"by": "size", "descending": false}));
    assert_eq!(unpinned(&mut s).first(), Some(&unknown));
    run(&mut s, "view.sort", json!({"by": "date"}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    assert_eq!(s.rows().len(), 4);
    run(&mut s, "view.filter", json!({"value": "all"}));
    run(&mut s, "view.conversations", json!({"value": true}));
    let rows = s.rows();
    let thread = rows.iter().find(|r| r.members.len() > 1).unwrap();
    assert_eq!((s.mailbox.message(thread.id).unwrap().subject.as_str(), thread.members.len()), ("RE: Release checklist", 2));
}

/// Arranged by categories, messages run A to Z by the categories they carry, those with the same
/// ones together whatever order or capitals they were given in, after those with none.
#[test]
fn the_list_arranges_by_categories() {
    let mut s = session();
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    let ids: Vec<Id> = s.rows().iter().map(|r| r.id).filter(|id| !s.mailbox.message(*id).unwrap().pinned).collect();
    for (id, names) in ids.iter().zip([vec!["Red category", "Blue category"], vec!["blue category", "red category"], vec!["Green category"]]) {
        s.mailbox.message_mut(*id).unwrap().categories = names.into_iter().map(String::from).collect();
    }
    let r = run(&mut s, "view.sort", json!({"by": "categories"}));
    assert_eq!((&r["by"], &r["descending"]), (&json!("categories"), &json!(false)));
    let rows = s.rows();
    let groups: Vec<&str> = rows.iter().filter_map(|r| r.group.as_deref()).filter(|g| *g != "Pinned").collect();
    // As Outlook arranges them: a message is listed under each of its categories.
    assert_eq!(groups, ["No category", "Blue category", "Green category", "Red category"]);
    let copies = |id: Id| rows.iter().filter(|r| r.id == id).map(|r| r.copy).collect::<Vec<_>>();
    assert_eq!((copies(ids[0]), copies(ids[1]), copies(ids[2])), (vec![0, 1], vec![0, 1], vec![0]));
    assert_eq!(crate::list::listed(&rows), rows.len() - 2, "a message listed twice counts once");

    // Down from a message's second row goes on from there, not from its first.
    let second = crate::list::position_of(&rows, ids[0], 1).unwrap();
    run(&mut s, "mail.select", json!({"id": ids[0], "copy": 1}));
    assert_eq!(s.sel.focus_copy(), 1);
    run(&mut s, "mail.next", json!({}));
    let after = rows.get(second + 1).unwrap_or(&rows[second]);
    assert_eq!((s.sel.focus(), s.sel.focus_copy()), (Some(after.id), after.copy));
    // Select All and a range take each message once.
    run(&mut s, "mail.selectAll", json!({}));
    assert_eq!(s.sel.messages.len(), crate::list::listed(&rows));
    run(&mut s, "mail.select", json!({"id": rows[0].id}));
    run(&mut s, "mail.select", json!({"id": rows[rows.len() - 1].id, "range": true}));
    let mut unique = s.sel.messages.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), s.sel.messages.len());

    // Turned over, Z to A and no category last; still newest first under each.
    run(&mut s, "view.reverseSort", json!({}));
    let rows = s.rows();
    let groups: Vec<&str> = rows.iter().filter_map(|r| r.group.as_deref()).filter(|g| *g != "Pinned").collect();
    assert_eq!(groups, ["Red category", "Green category", "Blue category", "No category"]);

    // Show in Groups off changes nothing here, as in Outlook: the headings stay, and a message is
    // still under each of its categories.
    run(&mut s, "view.groups", json!({"value": false}));
    assert_eq!(s.rows(), rows);
    run(&mut s, "view.groups", json!({"value": true}));
    assert!(s.run("view.sort", &json!({"by": "colour"})).is_err());
}

/// Names that differ only in case are one category, beyond ASCII too (`Été` and `été`), spelt as
/// the mailbox's list spells it; a name the list doesn't have is headed as the first message under
/// it spells it.
#[test]
fn categories_that_differ_only_in_case_are_one_heading() {
    let mut s = session();
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    s.mailbox.categories.push(mailcraft_model::Category { name: "Été".into(), color: Default::default() });
    let ids: Vec<Id> = s.rows().iter().map(|r| r.id).filter(|id| !s.mailbox.message(*id).unwrap().pinned).collect();
    for id in &ids {
        s.mailbox.message_mut(*id).unwrap().categories.clear();
    }
    for (id, names) in ids.iter().zip([vec!["été", "ÉTÉ"], vec!["Été"], vec!["Ölfarbe"], vec!["ölfarbe"]]) {
        s.mailbox.message_mut(*id).unwrap().categories = names.into_iter().map(String::from).collect();
    }
    run(&mut s, "view.sort", json!({"by": "categories"}));
    let rows = s.rows();
    let groups: Vec<&str> = rows.iter().filter_map(|r| r.group.as_deref()).filter(|g| *g != "Pinned").collect();
    assert_eq!(groups, ["No category", "Été", "Ölfarbe"]);
    assert_eq!(rows.iter().filter(|r| r.id == ids[0]).count(), 1, "one message, one heading");
}

/// Contacts arranged by categories are listed by name, each once (the People list puts each
/// under each of its categories); arranged by name, favourites come first.
#[test]
fn contacts_arrange_by_categories() {
    let mut s = session();
    let by_name = crate::cmd::people::sorted(&s, "");
    let (a, b) = (by_name[0], by_name[1]);
    s.mailbox.contact_mut(a).unwrap().categories = vec!["Red category".into()];
    s.mailbox.contact_mut(b).unwrap().categories = vec!["Blue category".into(), "Red category".into()];
    assert_eq!(run(&mut s, "people.arrange", json!({"by": "categories"}))["by"], "categories");
    let order = crate::cmd::people::sorted(&s, "");
    let mut alphabetical = order.clone();
    alphabetical.sort_by_key(|id| {
        let c = s.mailbox.contact(*id).unwrap();
        (c.file_as().to_lowercase(), c.id)
    });
    assert_eq!(order, alphabetical, "by name alone, each once");
    assert!(s.run("people.arrange", &json!({"by": "colour"})).is_err());
    run(&mut s, "people.arrange", json!({"by": "name"}));
    assert_eq!(crate::cmd::people::sorted(&s, ""), by_name);
}

#[test]
fn search_filters_the_list_and_can_look_everywhere() {
    let mut s = session();
    assert_eq!(run(&mut s, "view.search", json!({"query": "from:tomás"}))["found"], 2);
    assert_eq!(run(&mut s, "view.search", json!({"query": "has:attachment is:unread"}))["found"], 2);
    assert_eq!(run(&mut s, "view.search", json!({"query": "atlas"}))["found"], 2);
    // Everywhere: the Atlas folder and Archive too, but not Junk or Deleted.
    assert_eq!(run(&mut s, "view.search", json!({"query": "atlas", "scope": "everywhere"}))["found"], 5);
    assert_eq!(run(&mut s, "view.search", json!({"query": "lift OR prize", "scope": "everywhere"}))["found"], 0);
    run(&mut s, "view.clearSearch", json!({}));
    assert_eq!(s.rows().len(), 13);
    let first_id = first(&mut s);
    run(&mut s, "mail.select", json!({"id": first_id}));
    assert_eq!(run(&mut s, "mail.findRelated", json!({}))["found"], 1);
}

/// A list shows what a message says to its reader: the HTML's words when there is HTML, never
/// the padding, references and addresses a sender's plain-text part is full of. Nothing of it is
/// kept with the message, so mail that was already here reads the same way.
#[test]
fn previews_say_what_a_message_says() {
    use mailcraft_model::{Body, Message};

    let padding = "&zwnj; &zwnj; &zwnj; &zwnj; &nbsp; &zwnj;";
    let newsletter = Message {
        body: Body {
            text: format!("{padding}\n[image: Fernhollow]\nhttps://click.fernhollow.example/t/v1/9f3a7c?u=88213\nWool layers are back"),
            html: Some(format!(
                "<html><head><style>p {{ margin: 0 }}</style></head><body><div style=\"display:none\">Members get first pick this weekend.{}</div>\
                 <p><a href=\"https://click.fernhollow.example/t/v1/9f3a7c?u=88213\"><img alt=\"Fernhollow\" src=\"cid:mark\"></a></p>\
                 <p>Wool layers are <b>back</b></p></body></html>",
                "&zwnj;&nbsp;&#847;".repeat(80)
            )),
        },
        ..Message::default()
    };
    assert_eq!(list::preview(&newsletter, 200), "Members get first pick this weekend. Wool layers are back");
    assert_eq!(list::preview(&newsletter, 20), "Members get first…");
    // The text part alone, as a message without HTML has it (and as one that came before this did).
    let text_only = Message { body: Body { text: newsletter.body.text.clone(), html: None }, ..Message::default() };
    assert_eq!(list::preview(&text_only, 200), "Wool layers are back");
    // HTML that says nothing in words leaves the text to speak.
    let pictures = Message { body: Body { text: "See the pictures attached.".into(), html: Some("<p>&nbsp;</p><br>".into()) }, ..Message::default() };
    assert_eq!(list::preview(&pictures, 200), "See the pictures attached.");
    assert_eq!(list::preview(&Message::default(), 200), "");
    // What a script lists is the same line, and the message itself is as it was.
    let mut s = session();
    let before = newsletter.clone();
    let id = s.mailbox.add_message(Message { folder: s.view.folder, ..newsletter }).unwrap();
    let listed = run(&mut s, "mail.get", json!({"id": id}));
    assert_eq!(listed["snippet"], "Members get first pick this weekend. Wool layers are back");
    assert_eq!(s.mailbox.message(id).unwrap().body, before.body);
    // The sample's own mail reads as one clean line, every message of it.
    for m in s.mailbox.messages.values() {
        let line = list::preview(m, 60);
        assert_eq!(line.is_empty(), m.body.text.trim().is_empty(), "{}", m.subject);
        assert!(line.chars().count() <= 61 && !line.contains('\n') && !line.contains("  ") && !line.contains("http"), "{line:?}");
    }
}

#[test]
fn calendar_events_recur_move_and_lose_single_days() {
    let mut s = session();
    let week = run(&mut s, "calendar.list", json!({}));
    assert_eq!((week["from"].as_str(), week["to"].as_str()), (Some("2026-10-11"), Some("2026-10-17")));
    let titles: Vec<&str> = week["events"].as_array().unwrap().iter().map(|e| e["title"].as_str().unwrap()).collect();
    assert_eq!(titles.iter().filter(|t| **t == "Team stand-up").count(), 5);
    assert!(titles.contains(&"1:1 with Priya") && titles.contains(&"Budget sync") && titles.contains(&"Dentist"));

    let id = run(&mut s, "calendar.new", json!({"title": "Sprint demo", "start": "2026-10-16T15:00", "end": "2026-10-16T16:00", "attendees": "maya@lumen.example", "recurrence": {"freq": "weekly", "interval": 2, "count": 3}}))["id"].as_u64().unwrap();
    let e = run(&mut s, "calendar.get", json!({"id": id}));
    assert_eq!(e["recurrence"]["text"], "Every 2 weeks on Friday, 3 times");
    assert_eq!((e["organizer"].as_str(), e["reminder"].as_u64()), (Some("Avery Quinn <avery@lumen.example>"), Some(15)));
    let all = run(&mut s, "calendar.list", json!({"from": "2026-10-01", "to": "2026-12-31"}));
    let days: Vec<&str> = all["events"].as_array().unwrap().iter().filter(|e| e["id"] == id).map(|e| e["start"].as_str().unwrap()).collect();
    assert_eq!(days, ["2026-10-16T15:00:00", "2026-10-30T15:00:00", "2026-11-13T15:00:00"]);
    run(&mut s, "calendar.delete", json!({"id": id, "occurrence": "2026-10-30"}));
    run(&mut s, "calendar.move", json!({"id": id, "start": "2026-10-16T09:00"}));
    let e = run(&mut s, "calendar.get", json!({"id": id}));
    assert_eq!(
        (e["start"].as_str(), e["end"].as_str(), e["exceptions"][0].as_str()),
        (Some("2026-10-16T09:00:00"), Some("2026-10-16T10:00:00"), Some("2026-10-30"))
    );
    run(&mut s, "calendar.edit", json!({"id": id, "allDay": true, "recurrence": null}));
    let e = run(&mut s, "calendar.get", json!({"id": id}));
    assert_eq!((e["start"].as_str(), e["end"].as_str(), e["time"].as_str()), (Some("2026-10-16"), Some("2026-10-16"), Some("All day")));
    run(&mut s, "calendar.edit", json!({"id": id, "end": "2026-10-18"}));
    assert_eq!(s.mailbox.event(id).unwrap().duration_minutes(), 3 * 24 * 60);
    assert!(s.run("calendar.edit", &json!({"id": id, "end": "2026-10-01"})).is_err());
    assert!(s.run("calendar.new", &json!({"title": ""})).is_err());
    assert!(s.run("calendar.newMeeting", &json!({"title": "No one"})).is_err());
    run(&mut s, "calendar.delete", json!({"id": id}));
    assert!(s.mailbox.event(id).is_none());

    run(&mut s, "view.month", json!({}));
    let r = run(&mut s, "view.next", json!({}));
    assert_eq!((r["from"].as_str(), r["to"].as_str()), (Some("2026-11-01"), Some("2026-12-05")));
    assert_eq!(run(&mut s, "view.today", json!({}))["date"], "2026-10-14");
}

#[test]
fn a_new_appointment_starts_at_the_next_half_hour() {
    let mut s = session();
    let id = run(&mut s, "calendar.new", json!({"title": "Quick sync"}))["id"].as_u64().unwrap();
    let e = s.mailbox.event(id).unwrap();
    assert_eq!((e.start.iso().as_str(), e.duration_minutes()), ("2026-10-14T16:30:00", 30));
    run(&mut s, "view.goToDate", json!({"date": "2026-10-20"}));
    let id = run(&mut s, "calendar.new", json!({"title": "Later"}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.event(id).unwrap().start.iso(), "2026-10-20T08:00:00");
}

#[test]
fn people_and_tasks() {
    let mut s = session();
    let c = run(
        &mut s,
        "people.new",
        json!({"first": "Noor", "last": "Haddad", "email": "noor@example.org", "phone": "+1 555 0190", "birthday": "1990-05-02"}),
    )["id"]
        .as_u64()
        .unwrap();
    run(&mut s, "people.edit", json!({"id": c, "company": "Haddad Studio", "email": "noor@haddad.example"}));
    let got = run(&mut s, "people.get", json!({}));
    assert_eq!(
        (got["fileAs"].as_str(), got["emails"][0]["value"].as_str(), got["birthday"].as_str()),
        (Some("Haddad, Noor"), Some("noor@haddad.example"), Some("1990-05-02"))
    );
    assert_eq!(run(&mut s, "people.search", json!({"query": "lumen"}))["found"], 6);
    run(&mut s, "people.email", json!({"id": c}));
    assert_eq!(s.compose.as_ref().unwrap().to, "Noor Haddad <noor@haddad.example>");
    run(&mut s, "people.delete", json!({"id": c}));
    assert!(s.run("people.new", &json!({})).is_err());
    // The sender of a message becomes a contact once.
    run(&mut s, "folder.select", json!({"path": "Receipts"}));
    let first_id = first(&mut s);
    run(&mut s, "mail.select", json!({"id": first_id}));
    let added = run(&mut s, "people.fromMessage", json!({}));
    assert_eq!(added["existing"], false);
    assert_eq!(run(&mut s, "people.fromMessage", json!({}))["existing"], true);

    let active = run(&mut s, "tasks.list", json!({}));
    let titles: Vec<&str> = active["tasks"].as_array().unwrap().iter().map(|t| t["title"].as_str().unwrap()).collect();
    assert_eq!(titles[..2], ["Submit the September expense report", "Review the Q4 roadmap draft"]);
    assert_eq!(active["tasks"][0]["overdue"], true);
    let t = run(&mut s, "tasks.new", json!({"title": "Call the venue", "due": "2026-10-15", "importance": "high"}))["id"].as_u64().unwrap();
    run(&mut s, "tasks.complete", json!({}));
    assert!(s.mailbox.task(t).unwrap().is_complete());
    assert_eq!(run(&mut s, "tasks.list", json!({"filter": "completed"}))["tasks"].as_array().unwrap().len(), 2);
    // A to-do made from a message flags it; completing the to-do completes the flag.
    let made = run(&mut s, "tasks.fromMessage", json!({"due": "2026-10-20"}))["id"].as_u64().unwrap();
    let mid = s.mailbox.task(made).unwrap().message.unwrap();
    assert_eq!(s.mailbox.message(mid).unwrap().flag, FlagStatus::Flagged);
    run(&mut s, "tasks.complete", json!({"id": made}));
    assert_eq!(s.mailbox.message(mid).unwrap().flag, FlagStatus::Complete);
}

#[test]
fn rules_and_accounts() {
    let mut s = session();
    let news = run(&mut s, "folder.new", json!({"name": "Newsletters", "parent": null}))["id"].as_u64().unwrap();
    let book = s.rows().iter().map(|r| r.id).find(|id| s.mailbox.message(*id).unwrap().from.email.contains("harborbooks")).unwrap();
    let r = run(&mut s, "rules.fromMessage", json!({"id": book, "folder": news}));
    assert_eq!(r["matched"], 1);
    assert_eq!(s.mailbox.message(book).unwrap().folder, news);
    assert!(s.mailbox.message(book).unwrap().has_category("Purple category"));
    assert!(s.run("rules.add", &json!({"name": "Bad", "conditions": [], "actions": [{"type": "flag"}]})).is_err());
    assert!(
        s.run("rules.add", &json!({"name": "Bad", "conditions": [{"type": "hasAttachment"}], "actions": [{"type": "moveTo", "value": 424242}]}))
            .is_err()
    );
    let id = run(&mut s, "rules.add", json!({"name": "Invoices", "conditions": [{"type": "subjectContains", "value": "invoice"}, {"type": "hasAttachment"}], "actions": [{"type": "setImportance", "value": "high"}], "stop": true}))["id"].as_u64().unwrap();
    assert_eq!(run(&mut s, "rules.run", json!({}))["matched"], 1);
    assert!(s.mailbox.messages.values().any(|m| m.subject.contains("invoice") && m.importance == Importance::High));
    run(&mut s, "rules.enable", json!({"id": id, "value": false}));
    run(&mut s, "rules.remove", json!({"id": id}));

    assert!(matches!(s.run("sync.request", &json!({})), Err(CmdError::Disabled(_))));
    let a = run(&mut s, "account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example", "port": 587, "security": "starttls"}}))["id"].as_u64().unwrap();
    assert!(s.run("account.add", &json!({"email": "AVERY@mail.example"})).is_err());
    assert!(s.run("account.add", &json!({"email": "x@y.example", "incoming": {"host": "imap.example/evil path"}})).is_err());
    // Mail from a server account waits in its Outbox.
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "subject": "Queued", "account": a}));
    assert_eq!(run(&mut s, "compose.send", json!({}))["queued"], true);
    let req = cmd::account::sync_request(&s, a).unwrap();
    assert_eq!((req.outbox.len(), req.folders.len(), req.password.as_str()), (1, 7, ""));
    assert_eq!(run(&mut s, "sync.request", json!({"account": a}))["outbox"][0]["subject"], "Queued");
}

#[test]
fn accounts_that_sign_in_on_the_providers_page() {
    use mailcraft_model::{AccountKind, OAuthProvider};
    let mut s = session();
    // The suggestion for an Outlook.com address, taken as it comes.
    let preset = run(&mut s, "account.preset", json!({"email": "avery@outlook.example.org"}));
    assert_eq!((preset["known"].as_bool(), preset["incoming"]["host"].as_str()), (Some(false), Some("imap.outlook.example.org")));
    assert_eq!(run(&mut s, "account.preset", json!({"email": " Avery@Hotmail.CO.UK "}))["provider"], "microsoft");
    let mut preset = run(&mut s, "account.preset", json!({"email": "avery@hotmail.co.uk"}));
    assert_eq!((preset["auth"].as_str(), preset["provider"].as_str()), (Some("oauth2"), Some("microsoft")));
    assert_eq!(
        (preset["incoming"]["host"].as_str(), preset["outgoing"]["host"].as_str()),
        (Some("outlook.office365.com"), Some("smtp-mail.outlook.com"))
    );
    assert_eq!((preset["outgoing"]["port"].as_u64(), preset["outgoing"]["security"].as_str()), (Some(587), Some("starttls")));
    // A work address isn't told apart by its name; saying it is Microsoft's is enough.
    let work = run(&mut s, "account.preset", json!({"email": "avery@lumen.example", "provider": "microsoft"}));
    assert_eq!((work["auth"].as_str(), work["outgoing"]["host"].as_str()), (Some("oauth2"), Some("smtp.office365.com")));
    assert_eq!(run(&mut s, "account.preset", json!({"email": "a@live.somecompany.example"}))["auth"], "password");
    // A company's own domain: the mail exchangers a host with sockets looked up say whose it is.
    let mx = run(&mut s, "account.preset", json!({"email": "avery@lumen.example", "mx": ["lumen-example.mail.protection.outlook.com"]}));
    assert_eq!((mx["via"].as_str(), mx["service"].as_str(), mx["auth"].as_str()), (Some("mx"), Some("Microsoft 365"), Some("oauth2")));
    assert_eq!((mx["incoming"]["host"].as_str(), mx["outgoing"]["host"].as_str()), (Some("outlook.office365.com"), Some("smtp.office365.com")));
    let mx = run(&mut s, "account.preset", json!({"email": "avery@lumen.example", "mx": ["mx.lumen.example", 7, null, "aspmx.l.google.com"]}));
    assert_eq!((mx["service"].as_str(), mx["auth"].as_str(), mx["provider"].is_null()), (Some("Google Workspace"), Some("password"), true));
    let mx = run(&mut s, "account.preset", json!({"email": "avery@lumen.example", "mx": ["mx.lumen.example", "evil.example/../mx.microsoft"]}));
    assert_eq!(
        (mx["known"].as_bool(), mx["via"].as_str(), mx["incoming"]["host"].as_str()),
        (Some(false), Some("guess"), Some("imap.lumen.example"))
    );
    // Or the owner says whose it is.
    let named = run(&mut s, "account.preset", json!({"email": "avery@lumen.example", "service": "Outlook.com"}));
    assert_eq!((named["via"].as_str(), named["outgoing"]["host"].as_str()), (Some("asked"), Some("smtp-mail.outlook.com")));
    assert_eq!(run(&mut s, "account.preset", json!({"email": "avery@lumen.example", "service": "google"}))["service"], "Gmail");
    assert!(s.run("account.preset", &json!({"email": "avery@lumen.example", "service": "nobody"})).is_err());
    // The provider's own domain needs nobody's exchangers.
    assert_eq!(run(&mut s, "account.preset", json!({"email": "a@outlook.com", "mx": ["aspmx.l.google.com"]}))["service"], "Outlook.com");
    assert_eq!(run(&mut s, "account.preset", json!({"email": "a@gmail.com"}))["incoming"]["host"], "imap.gmail.com");
    for bad in [
        json!({}),
        json!({"email": "nobody"}),
        json!({"email": "a@b"}),
        json!({"email": "a@b c.example"}),
        json!({"email": "a@b.example", "provider": "nobody"}),
    ] {
        assert!(s.run("account.preset", &bad).is_err(), "{bad}");
    }

    preset["email"] = json!("avery@hotmail.co.uk");
    preset["clientId"] = json!("0f0f0f0f-1111-2222-3333-444444444444");
    let id = run(&mut s, "account.add", preset)["id"].as_u64().unwrap();
    let account = s.mailbox.account(id).unwrap().clone();
    let oauth = account.oauth.clone().unwrap();
    assert_eq!(
        (oauth.provider, oauth.client_id.as_str(), oauth.tenant.as_str()),
        (OAuthProvider::Microsoft, "0f0f0f0f-1111-2222-3333-444444444444", "")
    );
    assert_eq!((account.kind, account.login()), (AccountKind::Imap, "avery@hotmail.co.uk"));
    let listed = run(&mut s, "account.list", json!({}));
    let row = listed["accounts"].as_array().unwrap().iter().find(|a| a["id"] == id).unwrap();
    assert_eq!((row["auth"].as_str(), row["oauth"]["provider"].as_str()), (Some("oauth2"), Some("microsoft")));
    // Nothing secret is part of the request the host is handed.
    let req = cmd::account::sync_request(&s, id).unwrap();
    assert!(req.account.oauth.is_some() && req.secret().is_empty());

    run(&mut s, "account.edit", json!({"id": id, "tenant": "consumers"}));
    assert_eq!(s.mailbox.account(id).unwrap().oauth.as_ref().unwrap().tenant, "consumers");
    for bad in [
        json!({"id": id, "tenant": "evil/../path"}),
        json!({"id": id, "clientId": "a b"}),
        json!({"id": id, "auth": "magic"}),
        json!({"id": id, "provider": "nobody"}),
    ] {
        assert!(s.run("account.edit", &bad).is_err(), "{bad}");
    }
    assert_eq!(s.mailbox.account(id).unwrap().oauth.as_ref().unwrap().tenant, "consumers", "a refused edit changes nothing");
    run(&mut s, "account.edit", json!({"id": id, "auth": "password"}));
    assert_eq!(s.mailbox.account(id).unwrap().oauth, None);
    // Without `auth`, the sign-in settings alone don't turn a password account into another kind.
    run(&mut s, "account.edit", json!({"id": id, "tenant": "consumers", "provider": "microsoft"}));
    assert_eq!(s.mailbox.account(id).unwrap().oauth, None);
}

#[test]
fn a_sync_result_lands_in_the_right_folders() {
    use mailcraft_model::sync::{FolderDelta, RemoteFlags, SyncResult};
    use mailcraft_model::{Address, Message};
    let mut s = session();
    let a = run(&mut s, "account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}}))["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "subject": "Queued", "account": a}));
    let queued = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let mail =
        |uid: u32, subject: &str| Message { uid, subject: subject.into(), from: Address::new("", "x@example.org"), date: 100, ..Default::default() };
    let first = SyncResult {
        account: a,
        sent: vec![queued],
        folders: vec![
            FolderDelta {
                id: None,
                remote: "INBOX".into(),
                kind: FolderKind::Inbox,
                uid_validity: 7,
                uid_seen: 12,
                new: vec![mail(11, "One"), mail(12, "Two")],
                ..Default::default()
            },
            FolderDelta {
                id: None,
                remote: "Lists/Rust".into(),
                kind: FolderKind::Custom,
                uid_validity: 3,
                uid_seen: 1,
                new: vec![mail(1, "Weekly")],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let step = s.undo_label();
    let r = run(&mut s, "sync.apply", json!({"result": first}));
    assert_eq!((r["new"].as_u64(), r["sent"].as_u64(), r["summary"].as_str()), (Some(3), Some(1), Some("3 new messages, 1 sent")));
    assert_eq!(s.mailbox.message(queued).unwrap().folder, s.mailbox.folder_of_kind(a, FolderKind::Sent).unwrap());
    let f = s.mailbox.folder(inbox).unwrap();
    assert_eq!((f.remote.as_str(), f.uid_validity, f.uid_seen, s.mailbox.count(inbox)), ("INBOX", 7, 12, 2));
    let rust = s.mailbox.folders.iter().find(|f| f.remote == "Lists/Rust").unwrap().clone();
    assert_eq!((rust.name.as_str(), rust.account, s.mailbox.count(rust.id)), ("Rust", a, 1));
    // Mail that went out can't be called back: the steps that led there are forgotten.
    assert!(s.sync_status.starts_with("3 new messages, 1 sent — ") && step.is_some() && !s.can_undo());
    run(&mut s, "mail.flag", json!({"id": queued}));
    let step = s.undo_label();

    // Second run: flags changed, one message gone, one repeated (ignored), one new.
    let second = SyncResult {
        account: a,
        folders: vec![FolderDelta {
            id: Some(inbox),
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 7,
            uid_seen: 13,
            uid_from: 0,
            new: vec![mail(12, "Two again"), mail(13, "Three")],
            flags: vec![RemoteFlags { uid: 12, read: true, flagged: true, ..Default::default() }],
            removed: vec![11],
        }],
        ..Default::default()
    };
    let r = run(&mut s, "sync.apply", json!({"result": second}));
    assert_eq!((r["new"].as_u64(), r["removed"].as_u64(), r["updated"].as_u64()), (Some(1), Some(1), Some(1)));
    // Receiving mail is not an undo step, and takes none away.
    assert!(step.is_some() && s.undo_label() == step);
    let mut got: Vec<(u32, bool, FlagStatus)> = s.mailbox.messages_in(inbox).map(|m| (m.uid, m.read, m.flag)).collect();
    got.sort_by_key(|x| x.0);
    assert_eq!(got, [(12, true, FlagStatus::Flagged), (13, false, FlagStatus::None)]);
    // Deleted on a phone: gone from the Inbox, and its copy turns up in Deleted Items. Nothing
    // came to the owner, so nothing is new.
    let trash = |uid_seen: u32, new: Vec<Message>| FolderDelta {
        id: None,
        remote: "Deleted Items".into(),
        kind: FolderKind::Deleted,
        uid_validity: 5,
        uid_seen,
        new,
        ..Default::default()
    };
    // Its first download counts, as any folder's does.
    let first_listing = SyncResult { account: a, folders: vec![trash(1, vec![mail(1, "Old")])], ..Default::default() };
    assert_eq!(run(&mut s, "sync.apply", json!({"result": first_listing}))["new"], 1);
    let deleted = SyncResult {
        account: a,
        folders: vec![
            FolderDelta {
                id: Some(inbox),
                remote: "INBOX".into(),
                kind: FolderKind::Inbox,
                uid_validity: 7,
                uid_seen: 13,
                removed: vec![13],
                ..Default::default()
            },
            trash(2, vec![mail(2, "Three")]),
        ],
        ..Default::default()
    };
    let r = run(&mut s, "sync.apply", json!({"result": deleted}));
    assert_eq!((r["new"].as_u64(), r["removed"].as_u64()), (Some(0), Some(1)));
    assert!(s.sync_status.starts_with("Up to date"), "{}", s.sync_status);
    // A new UIDVALIDITY: everything from the server is replaced.
    let reset = SyncResult {
        account: a,
        folders: vec![FolderDelta {
            id: Some(inbox),
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 8,
            uid_seen: 1,
            new: vec![mail(1, "Fresh")],
            ..Default::default()
        }],
        ..Default::default()
    };
    run(&mut s, "sync.apply", json!({"result": reset}));
    let f = s.mailbox.folder(inbox).unwrap();
    assert_eq!((s.mailbox.count(inbox), f.uid_validity, f.uid_seen), (1, 8, 1));
    assert!(s.run("sync.apply", &json!({"result": {"account": 424242}})).is_err());

    // The way the app takes mail in (not through `run`): the list on screen and whatever else
    // follows the revision must notice, or new mail stays invisible until something else changes.
    run(&mut s, "folder.select", json!({"id": inbox}));
    let (rev, shown) = (s.rev(), s.rows().len());
    let more = SyncResult {
        account: a,
        folders: vec![FolderDelta {
            id: Some(inbox),
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 8,
            uid_seen: 2,
            new: vec![mail(2, "Just in")],
            ..Default::default()
        }],
        ..Default::default()
    };
    cmd::account::apply_sync(&mut s, more).unwrap();
    assert!(s.rev() > rev && s.dirty);
    assert_eq!(s.rows().len(), shown + 1);
}

// ---- synchronising both ways ----
//
// The server's side of each run is written out by hand here, as `mailcraft-net` answers it (the
// engine can't reach that crate; its tests check, against the fake servers, that it does answer
// so).

mod two_way {
    use mailcraft_model::sync::{Created, Deleted, FlagsPushed, FolderDelta, Moved, RemoteFlags, RemoteOp, SyncRequest, SyncResult};
    use mailcraft_model::{Address, Message, Pending};

    use super::*;

    /// A message as it comes down from the server.
    fn from_server(uid: u32, subject: &str) -> Message {
        Message {
            uid,
            subject: subject.into(),
            message_id: format!("{}@mail.example", subject.to_lowercase().replace(' ', "-")),
            from: Address::new("", "x@example.org"),
            date: 100 + i64::from(uid),
            ..Default::default()
        }
    }

    /// What a run read in one server mailbox: its new messages, and the flags of the ones known.
    fn news(id: Option<Id>, remote: &str, kind: FolderKind, uid_validity: u32, new: Vec<Message>, flags: Vec<RemoteFlags>) -> FolderDelta {
        let uid_seen = new.iter().map(|m| m.uid).chain(flags.iter().map(|f| f.uid)).max().unwrap_or(0);
        FolderDelta { id, remote: remote.into(), kind, uid_validity, uid_seen, new, flags, ..Default::default() }
    }

    fn has(uid: u32, read: bool, flagged: bool) -> RemoteFlags {
        RemoteFlags { uid, read, flagged, ..Default::default() }
    }

    fn pushed(folder: &str, uid: u32, read: Option<bool>, flagged: Option<bool>) -> FlagsPushed {
        FlagsPushed { folder: folder.into(), uid, read, flagged, answered: None }
    }

    fn flags(folder: &str, uid: u32, read: Option<bool>, flagged: Option<bool>, answered: Option<bool>) -> RemoteOp {
        RemoteOp::Flags { folder: folder.into(), uid, read, flagged, answered }
    }

    fn moved(message: Id, from: &str, uid: u32, to_folder: Id, new_uid: u32, uid_validity: u32) -> Moved {
        Moved { message, from: from.into(), uid, to_folder, new_uid, uid_validity }
    }

    fn apply(s: &mut Session, result: SyncResult) -> Value {
        let applied = cmd::account::apply_sync(s, result).unwrap();
        // Whatever came back, the mailbox hangs together.
        assert!(s.mailbox.clone().repair().is_empty(), "{:?}", s.mailbox.clone().repair());
        applied
    }

    fn request(s: &Session, account: Id) -> SyncRequest {
        cmd::account::sync_request(s, account).unwrap()
    }

    fn unpushed(s: &Session, account: Id) -> usize {
        cmd::account::unpushed(s, account)
    }

    fn folder(s: &Session, account: Id, kind: FolderKind) -> Id {
        s.mailbox.folder_of_kind(account, kind).unwrap()
    }

    /// The account's message with a subject.
    fn titled(s: &Session, account: Id, subject: &str) -> Id {
        s.mailbox.messages.values().find(|m| m.account == account && m.subject == subject).unwrap().id
    }

    /// Where a message is, and what it is to the server and to its reader.
    fn state(s: &Session, id: Id) -> (Id, u32, bool, FlagStatus) {
        let m = s.mailbox.message(id).unwrap();
        (m.folder, m.uid, m.read, m.flag)
    }

    /// The UIDs a request says the mailbox has of a folder, in order.
    fn known(req: &SyncRequest, folder: Id) -> Vec<u32> {
        let mut uids = req.folders.iter().find(|f| f.id == folder).unwrap().known_uids.clone();
        uids.sort_unstable();
        uids
    }

    /// A session with an account on a server, as its first Send/Receive leaves it: the Inbox
    /// follows "INBOX" (UIDs 1 to 3, the first one read), Archive "Archive" (one message), Sent
    /// Items "Sent" and Deleted Items "Trash". Returns the account, its Inbox and its Archive.
    fn synced() -> (Session, Id, Id, Id) {
        let mut s = session();
        let servers = json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}});
        let a = run(&mut s, "account.add", servers)["id"].as_u64().unwrap();
        let inbox = vec![Message { read: true, ..from_server(1, "One") }, from_server(2, "Two"), from_server(3, "Three")];
        let first = SyncResult {
            account: a,
            folders: vec![
                news(None, "INBOX", FolderKind::Inbox, 7, inbox, vec![]),
                news(None, "Archive", FolderKind::Archive, 8, vec![from_server(1, "Filed")], vec![]),
                news(None, "Sent", FolderKind::Sent, 9, vec![], vec![]),
                news(None, "Trash", FolderKind::Deleted, 10, vec![], vec![]),
            ],
            ..Default::default()
        };
        assert_eq!(apply(&mut s, first)["new"], 4);
        let (inbox, archive) = (folder(&s, a, FolderKind::Inbox), folder(&s, a, FolderKind::Archive));
        // What came down is as the server has it: nothing to push.
        assert_eq!((unpushed(&s, a), request(&s, a).ops.len(), known(&request(&s, a), inbox)), (0, 0, vec![1, 2, 3]));
        (s, a, inbox, archive)
    }

    #[test]
    fn flags_changed_here_go_up_and_stay_as_they_are() {
        let (mut s, a, inbox, _) = synced();
        let (two, three) = (titled(&s, a, "Two"), titled(&s, a, "Three"));
        run(&mut s, "mail.markRead", json!({"id": two}));
        run(&mut s, "mail.flag", json!({"id": three}));
        assert_eq!(unpushed(&s, a), 2);
        // Only what was changed here is named: the server's other flags are not ours to set.
        assert_eq!(request(&s, a).ops, [flags("INBOX", 2, Some(true), None, None), flags("INBOX", 3, None, Some(true), None)]);
        // The run that used to undo it: the server answers with what it has. It took the changes.
        let took = SyncResult {
            account: a,
            flagged: vec![pushed("INBOX", 2, Some(true), None), pushed("INBOX", 3, None, Some(true))],
            folders: vec![news(
                Some(inbox),
                "INBOX",
                FolderKind::Inbox,
                7,
                vec![],
                vec![has(1, true, false), has(2, true, false), has(3, false, true)],
            )],
            ..Default::default()
        };
        let r = apply(&mut s, took.clone());
        assert_eq!((r["pushed"].as_u64(), r["updated"].as_u64(), r["summary"].as_str()), (Some(2), Some(0), Some("Up to date")));
        assert_eq!((state(&s, two).2, state(&s, three).3), (true, FlagStatus::Flagged));
        assert_eq!((unpushed(&s, a), request(&s, a).ops.len()), (0, 0));
        // And the run after that, and the one after.
        apply(&mut s, SyncResult { flagged: vec![], ..took.clone() });
        assert_eq!((state(&s, two).2, state(&s, three).3, unpushed(&s, a)), (true, FlagStatus::Flagged, 0));

        // A run that didn't get the change through: the server still has the message as read.
        run(&mut s, "mail.markUnread", json!({"id": two}));
        let failed = SyncResult { flagged: vec![], errors: vec!["Couldn't change flags in \"INBOX\": the server said: no".into()], ..took };
        let r = apply(&mut s, failed);
        assert_eq!(r["summary"], "Couldn't change flags in \"INBOX\": the server said: no");
        // It stays as it was made here, and is asked for again.
        assert!(!state(&s, two).2);
        assert_eq!(request(&s, a).ops, [flags("INBOX", 2, Some(false), None, None)]);
        // Changing it back leaves nothing to ask.
        run(&mut s, "mail.markRead", json!({"id": two}));
        assert_eq!(unpushed(&s, a), 0);
    }

    #[test]
    fn the_servers_flag_changes_merge_with_those_made_here() {
        let (mut s, a, inbox, _) = synced();
        let (one, two, three) = (titled(&s, a, "One"), titled(&s, a, "Two"), titled(&s, a, "Three"));
        let server_says = |flags: Vec<RemoteFlags>| SyncResult {
            account: a,
            folders: vec![news(Some(inbox), "INBOX", FolderKind::Inbox, 7, vec![], flags)],
            ..Default::default()
        };
        let answered = RemoteFlags { uid: 2, answered: true, ..Default::default() };
        // Changed on the server only: it comes down.
        let r = apply(&mut s, server_says(vec![has(1, false, true), answered, has(3, false, false)]));
        assert_eq!(r["updated"], 2);
        assert_eq!((state(&s, one).2, state(&s, one).3, s.mailbox.message(two).unwrap().answered), (false, FlagStatus::Flagged, true));
        assert_eq!(unpushed(&s, a), 0);

        // Different flags of one message changed on both sides: both changes hold.
        run(&mut s, "mail.markRead", json!({"id": two}));
        apply(&mut s, server_says(vec![has(1, false, true), has(2, false, true), has(3, false, false)]));
        let m = s.mailbox.message(two).unwrap();
        assert_eq!((m.read, m.flag, m.answered), (true, FlagStatus::Flagged, false));
        // What goes up is the change made here, and no word about the flags the server changed.
        assert_eq!(request(&s, a).ops, [flags("INBOX", 2, Some(true), None, None)]);

        // The same flag changed on both sides: the change made here wins, whatever the server has.
        run(&mut s, "mail.clearFlag", json!({"id": one}));
        run(&mut s, "mail.flag", json!({"id": three}));
        apply(&mut s, server_says(vec![has(1, true, true), has(2, false, true), has(3, true, true)]));
        // One: unflagged here stays unflagged (and its read state, untouched here, follows the server).
        assert_eq!((state(&s, one).2, state(&s, one).3), (true, FlagStatus::None));
        // Three: flagged on both sides is simply flagged.
        assert_eq!((state(&s, three).2, state(&s, three).3), (true, FlagStatus::Flagged));
        assert_eq!(request(&s, a).ops, [flags("INBOX", 1, None, Some(false), None), flags("INBOX", 2, Some(true), None, None)]);
    }

    #[test]
    fn a_flag_changed_while_the_synchronisation_ran_is_not_lost() {
        let (mut s, a, inbox, _) = synced();
        let (two, three) = (titled(&s, a, "Two"), titled(&s, a, "Three"));
        run(&mut s, "mail.markRead", json!({"id": two}));
        // The host leaves with this...
        assert_eq!(request(&s, a).ops, [flags("INBOX", 2, Some(true), None, None)]);
        // ...and while it talks to the server, the owner flags the message, thinks again about
        // having read it, and reads another one.
        run(&mut s, "mail.flag", json!({"id": two}));
        run(&mut s, "mail.markUnread", json!({"id": two}));
        run(&mut s, "mail.markRead", json!({"id": three}));
        // The host comes back with what it was sent for.
        let back = SyncResult {
            account: a,
            flagged: vec![pushed("INBOX", 2, Some(true), None)],
            folders: vec![news(
                Some(inbox),
                "INBOX",
                FolderKind::Inbox,
                7,
                vec![],
                vec![has(1, true, false), has(2, true, false), has(3, false, false)],
            )],
            ..Default::default()
        };
        assert_eq!(apply(&mut s, back)["updated"], 0);
        // Everything is as the owner left it, and the next run takes it up.
        assert_eq!((state(&s, two).2, state(&s, two).3, state(&s, three).2), (false, FlagStatus::Flagged, true));
        assert_eq!(request(&s, a).ops, [flags("INBOX", 2, Some(false), Some(true), None), flags("INBOX", 3, Some(true), None, None)]);
    }

    #[test]
    fn a_follow_up_that_is_done_with_is_no_flag_on_the_server_and_stays_done() {
        let (mut s, a, inbox, _) = synced();
        let three = titled(&s, a, "Three");
        let server_says = |flagged: bool| SyncResult {
            account: a,
            folders: vec![news(Some(inbox), "INBOX", FolderKind::Inbox, 7, vec![], vec![has(3, false, flagged)])],
            ..Default::default()
        };
        // Flagged on the server, done with here.
        apply(&mut s, server_says(true));
        run(&mut s, "mail.complete", json!({"id": three}));
        assert_eq!(request(&s, a).ops, [flags("INBOX", 3, None, Some(false), None)]);
        apply(&mut s, SyncResult { flagged: vec![pushed("INBOX", 3, None, Some(false))], ..server_says(false) });
        assert_eq!((state(&s, three).3, unpushed(&s, a)), (FlagStatus::Complete, 0));
        // Flagged again elsewhere: what is done here stays done, and the server is told so.
        apply(&mut s, server_says(true));
        assert_eq!(state(&s, three).3, FlagStatus::Complete);
        assert_eq!(request(&s, a).ops, [flags("INBOX", 3, None, Some(false), None)]);
    }

    #[test]
    fn a_move_goes_to_the_server_and_the_message_is_there_once() {
        let (mut s, a, inbox, archive) = synced();
        let two = titled(&s, a, "Two");
        // Read, then filed: the commonest thing done to a message.
        run(&mut s, "mail.select", json!({"id": two}));
        assert!(state(&s, two).2);
        run(&mut s, "mail.archive", json!({"id": two}));
        assert_eq!((state(&s, two), unpushed(&s, a)), ((archive, 0, true, FlagStatus::None), 2));
        let req = request(&s, a);
        // It goes read into its new mailbox: the flag first, where the server still has it.
        assert_eq!(
            req.ops,
            [
                flags("INBOX", 2, Some(true), None, None),
                RemoteOp::Move { message: two, from: "INBOX".into(), uid: 2, to: "Archive".into(), to_folder: archive, create: vec![] },
            ]
        );
        // Until it has, the server has it in INBOX: known there, so that it doesn't come down again.
        assert_eq!((known(&req, inbox), known(&req, archive)), (vec![1, 2, 3], vec![1]));

        // The server did both, and said where the message is now.
        let done = SyncResult {
            account: a,
            flagged: vec![pushed("INBOX", 2, Some(true), None)],
            moved: vec![moved(two, "INBOX", 2, archive, 2, 8)],
            folders: vec![
                news(Some(inbox), "INBOX", FolderKind::Inbox, 7, vec![], vec![has(1, true, false), has(3, false, false)]),
                news(Some(archive), "Archive", FolderKind::Archive, 8, vec![], vec![has(1, false, false), has(2, true, false)]),
            ],
            ..Default::default()
        };
        let r = apply(&mut s, done);
        assert_eq!((r["new"].as_u64(), r["pushed"].as_u64(), r["updated"].as_u64()), (Some(0), Some(2), Some(0)));
        assert_eq!(state(&s, two), (archive, 2, true, FlagStatus::None));
        assert_eq!(s.mailbox.message(two).unwrap().synced, Some(has(2, true, false)));
        assert!(s.mailbox.pending.is_empty());
        assert_eq!((s.mailbox.count(inbox), s.mailbox.count(archive), unpushed(&s, a)), (2, 2, 0));
        let req = request(&s, a);
        assert_eq!((known(&req, archive), req.ops.len()), (vec![1, 2], 0));
        // What went to the server can't be taken back by an undo here.
        assert!(!s.can_undo() && !s.can_redo());

        // Should a run bring the message down as well (it didn't know the UID was known): still once.
        let again = SyncResult {
            account: a,
            folders: vec![news(Some(archive), "Archive", FolderKind::Archive, 8, vec![from_server(2, "Two")], vec![])],
            ..Default::default()
        };
        assert_eq!(apply(&mut s, again)["new"], 0);
        assert_eq!(s.mailbox.count(archive), 2);
    }

    #[test]
    fn a_moved_message_is_matched_with_the_servers_copy_when_its_new_uid_was_not_said() {
        let (mut s, a, inbox, archive) = synced();
        let (two, three) = (titled(&s, a, "Two"), titled(&s, a, "Three"));
        run(&mut s, "mail.move", json!({"ids": [two, three], "folder": archive}));
        assert_eq!(unpushed(&s, a), 2);
        // Moved, and not a word about the new UIDs. In the same run the copy of one comes down,
        // with a message that really is new.
        let done = SyncResult {
            account: a,
            moved: vec![moved(two, "INBOX", 2, archive, 0, 0), moved(three, "INBOX", 3, archive, 0, 0)],
            folders: vec![
                news(Some(inbox), "INBOX", FolderKind::Inbox, 7, vec![], vec![has(1, true, false)]),
                news(
                    Some(archive),
                    "Archive",
                    FolderKind::Archive,
                    8,
                    vec![from_server(2, "Two"), from_server(4, "News")],
                    vec![has(1, false, false)],
                ),
            ],
            ..Default::default()
        };
        let r = apply(&mut s, done);
        // The copy is the message that was moved: it gets its UID, and nothing shows twice. What
        // was filed in Archive elsewhere is taken in, and is not new mail.
        assert_eq!((r["new"].as_u64(), r["matched"].as_u64(), r["summary"].as_str()), (Some(0), Some(1), Some("Up to date")));
        assert_eq!((state(&s, two), s.mailbox.count(archive)), ((archive, 2, false, FlagStatus::None), 4));
        assert!(s.mailbox.pending.is_empty() && unpushed(&s, a) == 0);
        // The other is known to be there, and waits for its copy: no UID yet, nothing to push.
        let m = s.mailbox.message(three).unwrap();
        assert_eq!((m.folder, m.uid, m.synced), (archive, 0, Some(has(0, false, false))));
        assert_eq!(known(&request(&s, a), archive), vec![1, 2, 4]);
        // Read while it waits; the copy that comes down with a later run is unread, and flagged
        // by another program meanwhile.
        run(&mut s, "mail.markRead", json!({"id": three}));
        let later = SyncResult {
            account: a,
            folders: vec![news(
                Some(archive),
                "Archive",
                FolderKind::Archive,
                8,
                vec![Message { flag: FlagStatus::Flagged, ..from_server(3, "Three") }],
                vec![],
            )],
            ..Default::default()
        };
        let r = apply(&mut s, later);
        assert_eq!((r["new"].as_u64(), r["matched"].as_u64(), s.mailbox.count(archive)), (Some(0), Some(1), 4));
        // Both changes hold, and the one made here goes up.
        assert_eq!(state(&s, three), (archive, 3, true, FlagStatus::Flagged));
        assert_eq!(request(&s, a).ops, [flags("Archive", 3, Some(true), None, None)]);

        // Mail that merely carries the Message-ID of a message that was never on the server
        // (one brought in from a file) is mail of its own.
        let imported = s.mailbox.add_message(Message { folder: inbox, ..from_server(0, "From a file") }).unwrap();
        let claims = SyncResult {
            account: a,
            folders: vec![news(Some(inbox), "INBOX", FolderKind::Inbox, 7, vec![from_server(9, "From a file")], vec![])],
            ..Default::default()
        };
        let r = apply(&mut s, claims);
        assert_eq!((r["new"].as_u64(), r["matched"].as_u64()), (Some(1), Some(0)));
        assert_eq!(s.mailbox.message(imported).map(|m| m.uid), Some(0));
    }

    #[test]
    fn a_folder_that_only_exists_here_is_asked_for_and_then_follows_its_mailbox() {
        let (mut s, a, inbox, _) = synced();
        let (two, three) = (titled(&s, a, "Two"), titled(&s, a, "Three"));
        let receipts = run(&mut s, "folder.new", json!({"name": "Receipts", "parent": null, "account": a}))["id"].as_u64().unwrap();
        let projects = run(&mut s, "folder.new", json!({"name": "Projects", "parent": inbox}))["id"].as_u64().unwrap();
        let apollo = run(&mut s, "folder.new", json!({"name": "Apollo", "parent": projects}))["id"].as_u64().unwrap();
        run(&mut s, "mail.move", json!({"id": two, "folder": receipts}));
        run(&mut s, "mail.move", json!({"id": three, "folder": apollo}));
        // Each under the nearest folder the server has: the top level, and the Inbox.
        assert_eq!(
            request(&s, a).ops,
            [
                RemoteOp::Move {
                    message: two,
                    from: "INBOX".into(),
                    uid: 2,
                    to: String::new(),
                    to_folder: receipts,
                    create: vec!["Receipts".into()]
                },
                RemoteOp::Move {
                    message: three,
                    from: "INBOX".into(),
                    uid: 3,
                    to: "INBOX".into(),
                    to_folder: apollo,
                    create: vec!["Projects".into(), "Apollo".into()]
                },
            ]
        );
        // The first is made and filled; the second is made, but its move fails.
        let done = SyncResult {
            account: a,
            created: vec![
                Created { folder: receipts, remote: "Receipts".into() },
                Created { folder: apollo, remote: "INBOX/Projects/Apollo".into() },
            ],
            moved: vec![moved(two, "INBOX", 2, receipts, 1, 907)],
            errors: vec!["Couldn't move messages from \"INBOX\" to \"INBOX/Projects/Apollo\": the server said: no".into()],
            ..Default::default()
        };
        apply(&mut s, done);
        let f = s.mailbox.folder(receipts).unwrap();
        assert_eq!((f.remote.as_str(), f.uid_validity), ("Receipts", 907));
        assert_eq!((state(&s, two).0, state(&s, two).1), (receipts, 1));
        // The other move waits on, for a mailbox that is there now.
        assert_eq!(s.mailbox.folder(apollo).unwrap().remote, "INBOX/Projects/Apollo");
        assert_eq!(
            request(&s, a).ops,
            [RemoteOp::Move { message: three, from: "INBOX".into(), uid: 3, to: "INBOX/Projects/Apollo".into(), to_folder: apollo, create: vec![] }]
        );
        // The mailbox's own news goes to the folder it was made for, not to a second one.
        let folders = s.mailbox.folders.len();
        let listed = SyncResult {
            account: a,
            folders: vec![news(None, "Receipts", FolderKind::Custom, 907, vec![], vec![has(1, false, false)])],
            ..Default::default()
        };
        apply(&mut s, listed);
        assert_eq!((s.mailbox.folders.len(), s.mailbox.count(receipts)), (folders, 1));
        // A mailbox that another folder follows is never given to a second one.
        let taken = SyncResult {
            account: a,
            created: vec![Created { folder: projects, remote: "Receipts".into() }, Created { folder: inbox, remote: "Elsewhere".into() }],
            ..Default::default()
        };
        apply(&mut s, taken);
        assert_eq!((s.mailbox.folder(projects).unwrap().remote.as_str(), s.mailbox.folder(inbox).unwrap().remote.as_str()), ("", "INBOX"));
    }

    #[test]
    fn deleting_moves_to_the_servers_trash_and_deleting_for_good_deletes_there() {
        let (mut s, a, inbox, _) = synced();
        let deleted = folder(&s, a, FolderKind::Deleted);
        let (one, two, three) = (titled(&s, a, "One"), titled(&s, a, "Two"), titled(&s, a, "Three"));
        // Delete: into Deleted Items, which is a move on the server too.
        run(&mut s, "mail.delete", json!({"id": one}));
        assert_eq!(
            request(&s, a).ops,
            [RemoteOp::Move { message: one, from: "INBOX".into(), uid: 1, to: "Trash".into(), to_folder: deleted, create: vec![] }]
        );
        apply(&mut s, SyncResult { account: a, moved: vec![moved(one, "INBOX", 1, deleted, 5, 10)], ..Default::default() });
        assert_eq!(state(&s, one), (deleted, 5, true, FlagStatus::None));
        // Deleted again there: for good, where the server has it now.
        run(&mut s, "mail.delete", json!({"id": one}));
        assert!(s.mailbox.message(one).is_none());
        assert_eq!(request(&s, a).ops, [RemoteOp::Delete { from: "Trash".into(), uid: 5 }]);
        // Deleted and then deleted for good before the server heard of either: deleted where it still is.
        run(&mut s, "mail.delete", json!({"id": two}));
        run(&mut s, "mail.delete", json!({"id": two}));
        // For good straight away.
        run(&mut s, "mail.deletePermanently", json!({"id": three}));
        let gone = [
            RemoteOp::Delete { from: "Trash".into(), uid: 5 },
            RemoteOp::Delete { from: "INBOX".into(), uid: 2 },
            RemoteOp::Delete { from: "INBOX".into(), uid: 3 },
        ];
        assert_eq!((request(&s, a).ops, unpushed(&s, a)), (gone.to_vec(), 3));
        // Known where the server still has them, so that they don't come down again.
        assert_eq!(known(&request(&s, a), inbox), vec![2, 3]);

        // Two go through; the third is refused. It waits, and nothing comes back.
        let partly = SyncResult {
            account: a,
            deleted: vec![Deleted { from: "Trash".into(), uid: 5 }, Deleted { from: "INBOX".into(), uid: 2 }],
            errors: vec!["Couldn't delete messages from \"INBOX\": the server said: no".into()],
            folders: vec![news(Some(inbox), "INBOX", FolderKind::Inbox, 7, vec![], vec![])],
            ..Default::default()
        };
        apply(&mut s, partly);
        assert_eq!(s.mailbox.pending, [Pending::Delete { account: a, from: "INBOX".into(), uid: 3, message_id: "three@mail.example".into() }]);
        assert_eq!((s.mailbox.count(inbox), s.mailbox.count(deleted), unpushed(&s, a)), (0, 0, 1));
        // A download that still lists the message a deletion is waiting for doesn't bring it back.
        let still_there = SyncResult {
            account: a,
            folders: vec![news(Some(inbox), "INBOX", FolderKind::Inbox, 7, vec![from_server(3, "Three")], vec![])],
            ..Default::default()
        };
        assert_eq!(apply(&mut s, still_there)["new"], 0);
        assert_eq!(s.mailbox.count(inbox), 0);
    }

    #[test]
    fn there_and_back_before_a_run_is_nothing_to_push() {
        let (mut s, a, inbox, archive) = synced();
        let two = titled(&s, a, "Two");
        let before = s.mailbox.message(two).cloned();
        run(&mut s, "mail.archive", json!({"id": two}));
        run(&mut s, "mail.markRead", json!({"id": two}));
        run(&mut s, "mail.move", json!({"id": two, "folder": inbox}));
        // It is what it was to the server, but for having been read on the way.
        assert!(s.mailbox.pending.is_empty());
        assert_eq!(s.mailbox.message(two).cloned(), before.map(|m| Message { read: true, ..m }));
        assert_eq!(request(&s, a).ops, [flags("INBOX", 2, Some(true), None, None)]);
        assert_eq!(known(&request(&s, a), inbox), vec![1, 2, 3]);
        // The same by undoing the move.
        run(&mut s, "mail.archive", json!({"id": two}));
        assert_eq!((state(&s, two).0, unpushed(&s, a)), (archive, 2));
        run(&mut s, "edit.undo", json!({}));
        assert_eq!((state(&s, two).0, state(&s, two).1, s.mailbox.pending.len(), unpushed(&s, a)), (inbox, 2, 0, 1));
    }

    #[test]
    fn a_failed_push_keeps_waiting_and_loses_nothing() {
        let (mut s, a, inbox, archive) = synced();
        let (one, two, three) = (titled(&s, a, "One"), titled(&s, a, "Two"), titled(&s, a, "Three"));
        run(&mut s, "mail.archive", json!({"id": two}));
        run(&mut s, "mail.deletePermanently", json!({"id": three}));
        run(&mut s, "mail.markUnread", json!({"id": one}));
        let asked = request(&s, a).ops;
        assert_eq!(asked.len(), 3);
        let steps = s.undo_label();
        // The server was down, or said no to everything: errors, and nothing finished.
        for errors in [
            vec!["Couldn't check for new mail: couldn't reach imap.mail.example: timed out".to_string()],
            vec!["Couldn't move messages".to_string(), "x".to_string()],
        ] {
            let r = apply(&mut s, SyncResult { account: a, errors: errors.clone(), ..Default::default() });
            assert_eq!((r["summary"].as_str(), r["pushed"].as_u64()), (Some(errors[0].as_str()), Some(0)));
            // Everything is as the owner left it, and is asked for again, word for word.
            assert_eq!(request(&s, a).ops, asked);
            assert_eq!((state(&s, two).0, s.mailbox.count(inbox), unpushed(&s, a)), (archive, 1, 3));
            // Nothing reached the server, so the steps can still be undone.
            assert_eq!(s.undo_label(), steps);
        }
        // Undoing them leaves nothing to push at all.
        while s.undo_label().is_some_and(|label| label != "Add Account") {
            run(&mut s, "edit.undo", json!({}));
        }
        assert_eq!((unpushed(&s, a), s.mailbox.pending.len(), s.mailbox.count(inbox)), (0, 0, 3));
    }

    #[test]
    fn a_mailbox_made_anew_keeps_what_waited_for_it() {
        let (mut s, a, inbox, archive) = synced();
        let deleted = folder(&s, a, FolderKind::Deleted);
        let (one, two, three) = (titled(&s, a, "One"), titled(&s, a, "Two"), titled(&s, a, "Three"));
        let filed = titled(&s, a, "Filed");
        run(&mut s, "mail.archive", json!({"id": two}));
        run(&mut s, "mail.deletePermanently", json!({"id": three}));
        run(&mut s, "mail.markUnread", json!({"id": one}));
        run(&mut s, "mail.delete", json!({"id": filed}));
        assert_eq!(unpushed(&s, a), 4);
        // The Inbox has a new UIDVALIDITY: its UIDs name other messages now, or none.
        let anew = SyncResult {
            account: a,
            folders: vec![FolderDelta {
                removed: vec![1],
                ..news(
                    Some(inbox),
                    "INBOX",
                    FolderKind::Inbox,
                    70,
                    vec![from_server(1, "Another"), Message { read: true, ..from_server(2, "One") }],
                    vec![],
                )
            }],
            ..Default::default()
        };
        let r = apply(&mut s, anew);
        // One came back and is the message it was, still unread here; Another is new.
        assert_eq!((r["new"].as_u64(), r["removed"].as_u64(), r["matched"].as_u64()), (Some(1), Some(0), Some(1)));
        assert_eq!(state(&s, one), (inbox, 2, false, FlagStatus::None));
        // What was to be pushed about the old UIDs waits for its copy to be found by Message-ID;
        // what waits for another mailbox waits on.
        assert_eq!(
            s.mailbox.pending,
            [
                Pending::Move { account: a, message: two, from: "INBOX".into(), uid: 0, synced: Some(has(0, false, false)), to: archive },
                Pending::Delete { account: a, from: "INBOX".into(), uid: 0, message_id: "three@mail.example".into() },
                Pending::Move { account: a, message: filed, from: "Archive".into(), uid: 1, synced: Some(has(1, false, false)), to: deleted },
            ]
        );
        assert_eq!(
            request(&s, a).ops,
            [
                RemoteOp::Move { message: filed, from: "Archive".into(), uid: 1, to: "Trash".into(), to_folder: deleted, create: vec![] },
                flags("INBOX", 2, Some(false), None, None),
            ]
        );
        // Everything of the old life is gone from the Inbox, and what is there now came down.
        let mut now: Vec<(u32, String)> = s.mailbox.messages_in(inbox).map(|m| (m.uid, m.subject.clone())).collect();
        now.sort();
        assert_eq!(now, [(1, "Another".to_string()), (2, "One".to_string())]);
        let f = s.mailbox.folder(inbox).unwrap();
        assert_eq!((f.uid_validity, f.uid_seen), (70, 2));
        // The message that was moved out before stays where it was put, off the server.
        assert_eq!((state(&s, two).0, state(&s, two).1), (archive, 0));
    }

    #[test]
    fn sent_mail_is_matched_with_the_copy_the_server_keeps() {
        let (mut s, a, _, _) = synced();
        let sent_items = folder(&s, a, FolderKind::Sent);
        let send = |s: &mut Session, subject: &str| {
            run(s, "mail.new", json!({"to": "maya@lumen.example", "subject": subject, "account": a}));
            run(s, "compose.send", json!({}))["id"].as_u64().unwrap()
        };
        let (first, second, third) = (send(&mut s, "First"), send(&mut s, "Second"), send(&mut s, "Third"));
        assert_eq!((request(&s, a).outbox.len(), s.can_undo()), (3, true));
        let copy = |uid: u32, of: Id, s: &Session| Message { uid, read: true, ..s.mailbox.message(of).cloned().unwrap() };
        // All three went out. The copy of the first came down in the same run; the second's is
        // on the server but not down yet; the third has none (no mailbox for sent mail).
        let went = SyncResult {
            account: a,
            sent: vec![first, second, third],
            filed: vec![first, second],
            folders: vec![news(Some(sent_items), "Sent", FolderKind::Sent, 9, vec![copy(1, first, &s)], vec![])],
            ..Default::default()
        };
        let r = apply(&mut s, went);
        assert_eq!(
            (r["sent"].as_u64(), r["new"].as_u64(), r["matched"].as_u64(), r["summary"].as_str()),
            (Some(3), Some(0), Some(1), Some("3 messages sent"))
        );
        assert_eq!((s.mailbox.count(sent_items), request(&s, a).outbox.len()), (3, 0));
        assert_eq!((state(&s, first).1, state(&s, second).1, state(&s, third).1), (1, 0, 0));
        assert!(s.mailbox.message(second).unwrap().synced.is_some() && s.mailbox.message(third).unwrap().synced.is_none());
        // Mail that went out can't be called back by an undo, which would queue it again.
        assert!(!s.can_undo() && !s.can_redo());

        // The second's copy comes with a later run. The server gave it a Message-ID of its own on
        // the way out and kept ours in a header, as Microsoft's do.
        let ours = s.mailbox.message(second).unwrap().message_id.clone();
        let theirs = Message {
            message_id: "PH0PR01MB0001@prod.outlook.example".into(),
            headers: vec![("X-Microsoft-Original-Message-ID".into(), format!("<{ours}>"))],
            ..copy(2, second, &s)
        };
        // A copy of the third never comes: what does come with its subject is other mail.
        let other = Message { message_id: "other@mail.example".into(), ..copy(3, third, &s) };
        let later = SyncResult {
            account: a,
            folders: vec![news(Some(sent_items), "Sent", FolderKind::Sent, 9, vec![theirs, other], vec![has(1, true, false)])],
            ..Default::default()
        };
        let r = apply(&mut s, later);
        assert_eq!((r["new"].as_u64(), r["matched"].as_u64(), s.mailbox.count(sent_items)), (Some(0), Some(1), 4));
        let m = s.mailbox.message(second).unwrap();
        assert_eq!((m.uid, m.message_id.as_str(), m.synced.map(|f| f.read)), (2, "PH0PR01MB0001@prod.outlook.example", Some(true)));
        assert_eq!((state(&s, third).1, unpushed(&s, a)), (0, 0));
        // A result naming messages that aren't in the Outbox files nothing.
        let r = apply(&mut s, SyncResult { account: a, sent: vec![first, 424242], filed: vec![first], ..Default::default() });
        assert_eq!((r["sent"].as_u64(), state(&s, first).1), (Some(0), 1));
    }

    #[test]
    fn a_copy_that_came_back_under_another_id_is_matched_by_what_it_says() {
        let (mut s, a, _, _) = synced();
        let sent_items = folder(&s, a, FolderKind::Sent);
        let mut send = |subject: &str| {
            run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "cc": "lin@lumen.example", "subject": subject, "account": a}));
            run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap()
        };
        let (alpha, beta) = (send("Alpha"), send("Beta  report"));
        let copy = |uid: u32, of: Id, s: &Session| Message {
            uid,
            read: true,
            message_id: format!("{uid}@server.example"),
            ..s.mailbox.message(of).cloned().unwrap()
        };
        // The server filed both under ids of its own. Beta's copy is down; with it come two that
        // only look like Alpha: sent a second later, and to somebody else.
        let later = Message { date: now() + 1, ..copy(2, alpha, &s) };
        let elsewhere = Message { to: vec![Address::new("", "dev@lumen.example")], ..copy(3, alpha, &s) };
        let beta_back = Message { subject: "Beta report".into(), ..copy(1, beta, &s) };
        let went = SyncResult {
            account: a,
            sent: vec![alpha, beta],
            filed: vec![alpha, beta],
            folders: vec![news(Some(sent_items), "Sent", FolderKind::Sent, 9, vec![beta_back, later, elsewhere], vec![])],
            ..Default::default()
        };
        let r = apply(&mut s, went);
        assert_eq!((r["new"].as_u64(), r["matched"].as_u64(), s.mailbox.count(sent_items)), (Some(2), Some(1), 4));
        let m = s.mailbox.message(beta).unwrap();
        assert_eq!((m.uid, m.message_id.as_str()), (1, "1@server.example"));
        // Alpha still waits for its own.
        assert_eq!((state(&s, alpha).1, s.mailbox.message(alpha).unwrap().synced.is_some()), (0, true));
        let alpha_back = copy(4, alpha, &s);
        let r = apply(
            &mut s,
            SyncResult {
                account: a,
                folders: vec![news(Some(sent_items), "Sent", FolderKind::Sent, 9, vec![alpha_back], vec![])],
                ..Default::default()
            },
        );
        assert_eq!((r["matched"].as_u64(), state(&s, alpha).1, s.mailbox.count(sent_items)), (Some(1), 4, 4));
    }

    #[test]
    fn what_the_owner_did_while_the_synchronisation_ran_is_carried_on() {
        // The host left to move a message to Archive; before it is back, the owner...
        let left = || {
            let (mut s, a, inbox, archive) = synced();
            let two = titled(&s, a, "Two");
            run(&mut s, "mail.archive", json!({"id": two}));
            assert_eq!(request(&s, a).ops.len(), 1);
            (s, a, inbox, archive, two)
        };
        let came_back = |a: Id, two: Id, archive: Id, new_uid: u32| SyncResult {
            account: a,
            moved: vec![moved(two, "INBOX", 2, archive, new_uid, if new_uid == 0 { 0 } else { 8 })],
            ..Default::default()
        };

        // ...undoes the move. The server has it in Archive now: it is asked to move it back.
        let (mut s, a, inbox, archive, two) = left();
        run(&mut s, "edit.undo", json!({}));
        assert_eq!((state(&s, two).0, state(&s, two).1), (inbox, 2));
        apply(&mut s, came_back(a, two, archive, 5));
        assert_eq!((state(&s, two).0, state(&s, two).1), (inbox, 0));
        assert_eq!(
            request(&s, a).ops,
            [RemoteOp::Move { message: two, from: "Archive".into(), uid: 5, to: "INBOX".into(), to_folder: inbox, create: vec![] }]
        );

        // ...moves it on. The next run takes it from where the server put it.
        let (mut s, a, _, archive, two) = left();
        let deleted = folder(&s, a, FolderKind::Deleted);
        run(&mut s, "mail.delete", json!({"id": two}));
        apply(&mut s, came_back(a, two, archive, 5));
        assert_eq!((state(&s, two).0, state(&s, two).1), (deleted, 0));
        assert_eq!(
            request(&s, a).ops,
            [RemoteOp::Move { message: two, from: "Archive".into(), uid: 5, to: "Trash".into(), to_folder: deleted, create: vec![] }]
        );

        // ...deletes it for good. The next run deletes it where the server put it.
        let (mut s, a, _, archive, two) = left();
        run(&mut s, "mail.deletePermanently", json!({"id": two}));
        apply(&mut s, came_back(a, two, archive, 5));
        assert_eq!(request(&s, a).ops, [RemoteOp::Delete { from: "Archive".into(), uid: 5 }]);

        // When the server didn't say where it put the message, there is nothing to go on: the
        // server's copy comes down as a message of its own, and nothing is asked that could hit
        // another message.
        let (mut s, a, _, archive, two) = left();
        run(&mut s, "mail.deletePermanently", json!({"id": two}));
        apply(&mut s, came_back(a, two, archive, 0));
        assert_eq!((request(&s, a).ops.len(), s.mailbox.pending.len()), (0, 0));

        // A deletion the owner took back while it was carried out: the server no longer has the
        // message, so the one here is the only one. It stays, off the server.
        let (mut s, a, inbox, _) = synced();
        let three = titled(&s, a, "Three");
        run(&mut s, "mail.deletePermanently", json!({"id": three}));
        assert_eq!(request(&s, a).ops, [RemoteOp::Delete { from: "INBOX".into(), uid: 3 }]);
        run(&mut s, "edit.undo", json!({}));
        let gone = SyncResult {
            account: a,
            deleted: vec![Deleted { from: "INBOX".into(), uid: 3 }],
            folders: vec![news(Some(inbox), "INBOX", FolderKind::Inbox, 7, vec![], vec![has(1, true, false), has(2, false, false)])],
            ..Default::default()
        };
        apply(&mut s, gone);
        assert_eq!((state(&s, three).0, state(&s, three).1, s.mailbox.count(inbox)), (inbox, 0, 3));
        assert_eq!((known(&request(&s, a), inbox), unpushed(&s, a)), (vec![1, 2], 0));
    }

    #[test]
    fn only_what_the_server_can_be_told_is_asked() {
        let (mut s, a, inbox, archive) = synced();
        let (one, two, three) = (titled(&s, a, "One"), titled(&s, a, "Two"), titled(&s, a, "Three"));
        // Into the Outbox: no place on a server. The move waits until the message is elsewhere.
        let outbox = folder(&s, a, FolderKind::Outbox);
        run(&mut s, "mail.move", json!({"id": one, "folder": outbox}));
        assert_eq!((s.mailbox.pending.len(), unpushed(&s, a), request(&s, a).ops.len()), (1, 0, 0));
        run(&mut s, "mail.move", json!({"id": one, "folder": archive}));
        assert_eq!(unpushed(&s, a), 1);
        // To another account: its own server keeps it where it is.
        let local = kind(&s, FolderKind::Inbox);
        run(&mut s, "mail.move", json!({"id": two, "folder": local}));
        assert_eq!((s.mailbox.pending.len(), unpushed(&s, a)), (1, 1));
        // A copy is not on the server, and changing it is nobody's business.
        let copy = run(&mut s, "mail.copy", json!({"id": three, "folder": inbox}))["copies"][0].as_u64().unwrap();
        run(&mut s, "mail.markRead", json!({"id": copy}));
        run(&mut s, "mail.deletePermanently", json!({"id": copy}));
        assert_eq!(unpushed(&s, a), 1);
        // A mailbox whose UIDs can't be trusted (no UIDVALIDITY known) is not touched.
        s.mailbox.folder_mut(inbox).unwrap().uid_validity = 0;
        run(&mut s, "mail.markRead", json!({"id": three}));
        run(&mut s, "mail.deletePermanently", json!({"id": three}));
        assert_eq!((unpushed(&s, a), request(&s, a).ops.len()), (0, 0));
        // An account that isn't on a server has nothing to push, and none to ask.
        let local_account = s.mailbox.accounts[0].id;
        assert_eq!(unpushed(&s, local_account), 0);
        assert!(cmd::account::sync_request(&s, local_account).is_err() && cmd::account::sync_request(&s, 424242).is_err());
        assert_eq!(unpushed(&s, 424242), 0);
    }

    #[test]
    fn rules_on_new_mail_reach_the_server() {
        let (mut s, a, inbox, archive) = synced();
        s.mailbox.rules.push(mailcraft_model::Rule {
            id: 9_001,
            name: "Reports".into(),
            enabled: true,
            conditions: vec![mailcraft_model::Condition::SubjectContains("report".into())],
            actions: vec![mailcraft_model::Action::MarkRead, mailcraft_model::Action::MoveTo(archive)],
            stop: false,
        });
        // The same UID twice in one answer is one message.
        let arrived = SyncResult {
            account: a,
            folders: vec![news(
                Some(inbox),
                "INBOX",
                FolderKind::Inbox,
                7,
                vec![from_server(4, "Weekly report"), from_server(4, "Weekly report"), from_server(5, "Hello")],
                vec![],
            )],
            ..Default::default()
        };
        assert_eq!(apply(&mut s, arrived)["new"], 2);
        let report = titled(&s, a, "Weekly report");
        assert_eq!(state(&s, report), (archive, 0, true, FlagStatus::None));
        // What the rule did here is done on the server by the next run.
        assert_eq!(
            request(&s, a).ops,
            [
                flags("INBOX", 4, Some(true), None, None),
                RemoteOp::Move { message: report, from: "INBOX".into(), uid: 4, to: "Archive".into(), to_folder: archive, create: vec![] },
            ]
        );
        assert_eq!(known(&request(&s, a), inbox), vec![1, 2, 3, 4, 5]);

        // The same on the very first run, when the Inbox only starts to follow its mailbox.
        let mut s = session();
        let a = run(&mut s, "account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}}))["id"].as_u64().unwrap();
        let (inbox, archive) = (folder(&s, a, FolderKind::Inbox), folder(&s, a, FolderKind::Archive));
        run(
            &mut s,
            "rules.add",
            json!({"name": "Reports", "conditions": [{"type": "subjectContains", "value": "report"}], "actions": [{"type": "moveTo", "value": archive}]}),
        );
        let first = SyncResult {
            account: a,
            folders: vec![
                news(None, "INBOX", FolderKind::Inbox, 7, vec![from_server(1, "Weekly report"), from_server(2, "Hello")], vec![]),
                news(None, "Archive", FolderKind::Archive, 8, vec![], vec![]),
            ],
            ..Default::default()
        };
        assert_eq!(apply(&mut s, first)["new"], 2);
        let report = titled(&s, a, "Weekly report");
        assert_eq!((state(&s, report).0, s.mailbox.count(inbox)), (archive, 1));
        assert_eq!(
            request(&s, a).ops,
            [RemoteOp::Move { message: report, from: "INBOX".into(), uid: 1, to: "Archive".into(), to_folder: archive, create: vec![] }]
        );
    }

    #[test]
    fn a_backlog_goes_up_in_batches_and_is_counted_fast() {
        let (mut s, a, inbox, archive) = synced();
        let many: Vec<Message> = (10..5_010).map(|uid| from_server(uid, &format!("Bulk {uid}"))).collect();
        apply(&mut s, SyncResult { account: a, folders: vec![news(Some(inbox), "INBOX", FolderKind::Inbox, 7, many, vec![])], ..Default::default() });
        assert_eq!((s.mailbox.count(inbox), unpushed(&s, a)), (5_003, 0));
        // Everything read at once, and a few hundred filed.
        run(&mut s, "mail.markAllRead", json!({"folder": inbox}));
        let filed: Vec<Id> = s.mailbox.messages_in(inbox).filter(|m| m.uid >= 4_700).map(|m| m.id).collect();
        run(&mut s, "mail.move", json!({"ids": filed, "folder": archive}));
        // Each filed message has two things to say: that it was read, and where it went.
        assert_eq!(unpushed(&s, a), 5_002 + 310);
        let req = request(&s, a);
        assert_eq!(req.ops.len(), 2_000);
        // Asked every few seconds by the app: it must not be felt.
        let started = std::time::Instant::now();
        for _ in 0..20 {
            assert_eq!(unpushed(&s, a), 5_312);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
        // The moves go first, each after the flag that goes with it: a long run of flag changes
        // doesn't hold them up.
        let moves = req.ops.iter().take(620).filter(|op| matches!(op, RemoteOp::Move { .. })).count();
        assert_eq!((moves, req.ops.iter().filter(|op| matches!(op, RemoteOp::Move { .. })).count()), (310, 310));
        // What a run got through leaves the count, the rest goes with the next. Here the flags
        // went through and the moves didn't.
        let through: Vec<FlagsPushed> = req
            .ops
            .iter()
            .filter_map(|op| match op {
                RemoteOp::Flags { folder, uid, read, .. } => Some(pushed(folder, *uid, *read, None)),
                _ => None,
            })
            .collect();
        assert_eq!(through.len(), 1_690);
        apply(&mut s, SyncResult { account: a, flagged: through, ..Default::default() });
        assert_eq!((unpushed(&s, a), request(&s, a).ops.len()), (3_622, 2_000));
    }

    #[test]
    fn answers_that_make_no_sense_change_nothing_they_should_not() {
        let (mut s, a, inbox, archive) = synced();
        let two = titled(&s, a, "Two");
        run(&mut s, "mail.archive", json!({"id": two}));
        let before = s.mailbox.clone();
        // Moves and deletions nobody asked for, flags for messages that aren't there, folders
        // that aren't the account's, a UID that is taken, a mailbox of another life.
        let nonsense = SyncResult {
            account: a,
            flagged: vec![pushed("INBOX", 99, Some(true), None), pushed("Nowhere", 1, Some(true), Some(true)), pushed("", 0, None, None)],
            moved: vec![
                moved(424242, "INBOX", 77, archive, 5, 8),
                moved(two, "Nowhere", 2, archive, 5, 8),
                moved(two, "INBOX", 2, 424242, 5, 8),
                moved(0, "", 0, 0, 0, 0),
            ],
            deleted: vec![Deleted { from: "INBOX".into(), uid: 77 }, Deleted { from: "Nowhere".into(), uid: 1 }, Deleted::default()],
            created: vec![
                Created { folder: 424242, remote: "X".into() },
                Created { folder: kind(&s, FolderKind::Inbox), remote: "Y".into() },
                Created::default(),
            ],
            ..Default::default()
        };
        apply(&mut s, nonsense);
        // The one entry that named the waiting move finished it, into a folder that isn't one:
        // the message stays where its owner put it, off the server. Nothing else moved.
        assert!(s.mailbox.pending.is_empty());
        let m = s.mailbox.message(two).unwrap();
        assert_eq!((m.folder, m.uid, m.synced), (archive, 0, None));
        assert_eq!((s.mailbox.messages.len(), &s.mailbox.folders, s.mailbox.count(inbox)), (before.messages.len(), &before.folders, 2));
        assert_eq!(unpushed(&s, a), 0);

        // A new UID that another message already has, or of another life of the mailbox, is not taken.
        for (new_uid, uid_validity) in [(1, 8), (5, 9), (5, 0)] {
            let (mut s, a, _, archive) = synced();
            let two = titled(&s, a, "Two");
            run(&mut s, "mail.archive", json!({"id": two}));
            apply(&mut s, SyncResult { account: a, moved: vec![moved(two, "INBOX", 2, archive, new_uid, uid_validity)], ..Default::default() });
            assert_eq!((state(&s, two).1, s.mailbox.pending.len()), (0, 0), "{new_uid} {uid_validity}");
        }
        // The news of a mailbox worked out against a folder that is gone, or that follows
        // another mailbox, is left for the next run to read from the start.
        let (mut s, a, inbox, _) = synced();
        let stale = SyncResult {
            account: a,
            folders: vec![
                news(Some(424242), "Lists", FolderKind::Custom, 3, vec![from_server(1, "Stale")], vec![]),
                news(Some(inbox), "Other", FolderKind::Inbox, 7, vec![from_server(8, "Misfiled")], vec![has(1, false, true)]),
                news(None, "", FolderKind::Custom, 3, vec![from_server(1, "Nameless")], vec![]),
            ],
            ..Default::default()
        };
        let folders = s.mailbox.folders.clone();
        assert_eq!(apply(&mut s, stale)["new"], 0);
        assert_eq!((&s.mailbox.folders, s.mailbox.count(inbox)), (&folders, 3));
        // A server that names no UIDVALIDITY doesn't take the known one away.
        apply(
            &mut s,
            SyncResult { account: a, folders: vec![news(Some(inbox), "INBOX", FolderKind::Inbox, 0, vec![], vec![])], ..Default::default() },
        );
        assert_eq!(s.mailbox.folder(inbox).unwrap().uid_validity, 7);
        // An answer for another address is not this account's, whatever id it carries (the
        // mailbox was replaced while the run was under way).
        let strangers = SyncResult {
            account: a,
            email: "someone@elsewhere.example".into(),
            folders: vec![news(Some(inbox), "INBOX", FolderKind::Inbox, 7, vec![from_server(9, "Not ours")], vec![has(1, false, true)])],
            ..Default::default()
        };
        let before = s.mailbox.clone();
        assert!(cmd::account::apply_sync(&mut s, strangers).is_err());
        assert!(s.mailbox == before);
        apply(&mut s, SyncResult { account: a, email: " AVERY@mail.example".into(), ..Default::default() });
    }

    // ---- an answer that comes in pieces ----
    //
    // As `mailcraft_net::sync_with` hands it over: what was sent, what was pushed, then each
    // mailbox's new messages newest first, a batch to a piece, and only the last batch says the
    // mailbox is read (its tests check that it does).

    use cmd::account::SyncTotals;

    /// Take a piece of a run's answer in, as the app does.
    fn take(s: &mut Session, totals: &mut SyncTotals, piece: SyncResult) -> Value {
        let applied = cmd::account::apply_sync_part(s, totals, piece).unwrap();
        assert!(s.mailbox.clone().repair().is_empty(), "{:?}", s.mailbox.clone().repair());
        applied
    }

    /// A piece that says the mailbox is read up to `uid_seen`, whatever it brings.
    fn batch(id: Option<Id>, remote: &str, kind: FolderKind, uids: &[u32], uid_seen: u32) -> FolderDelta {
        let new = uids.iter().map(|uid| from_server(*uid, &format!("Mail {uid}"))).collect();
        FolderDelta { uid_seen, ..news(id, remote, kind, 7, new, vec![]) }
    }

    #[test]
    fn an_answer_in_pieces_is_taken_in_as_it_comes() {
        let mut s = session();
        let servers = json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}});
        let a = run(&mut s, "account.add", servers)["id"].as_u64().unwrap();
        let (inbox, sent_items) = (folder(&s, a, FolderKind::Inbox), folder(&s, a, FolderKind::Sent));
        run(&mut s, "mail.new", json!({"to": "maya@lumen.example", "subject": "Queued", "account": a}));
        let queued = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
        let piece = |progress: &str| SyncResult { account: a, partial: true, progress: progress.into(), ..Default::default() };
        let mut totals = SyncTotals::default();

        // A line on how far the run has got is all the status bar says, and nothing else changes:
        // no list is made anew for it, no unsaved change claimed.
        let rev = s.rev();
        let applied = take(&mut s, &mut totals, piece("Connecting to the mail server"));
        assert_eq!((s.sync_status.as_str(), s.rev(), applied["partial"].as_bool()), ("Connecting to the mail server", rev, Some(true)));
        // A message that went leaves the Outbox at once, and its copy is expected on the server.
        let expected = Some(RemoteFlags { read: true, ..Default::default() });
        let applied = take(&mut s, &mut totals, SyncResult { sent: vec![queued], filed: vec![queued], ..piece("Sent 1 of 1") });
        let m = s.mailbox.message(queued).unwrap();
        assert_eq!((applied["sent"].as_u64(), m.folder, m.uid, m.synced, s.can_undo()), (Some(1), sent_items, 0, expected, false));
        assert!(s.rev() > rev);
        // A copy that didn't get there after all is no longer expected; one that is said to be
        // there later than the message went is, all the same.
        let applied = take(&mut s, &mut totals, SyncResult { unfiled: vec![queued, 424242, 0], ..piece("Keeping a copy of what was sent") });
        assert_eq!((applied["sent"].as_u64(), s.mailbox.message(queued).unwrap().synced), (Some(0), None));
        take(&mut s, &mut totals, SyncResult { filed: vec![queued], ..piece("") });
        assert_eq!(s.mailbox.message(queued).unwrap().synced, expected);
        // Saying it again, or of a message that is not waiting in Sent Items, changes nothing.
        let other = s.mailbox.messages.values().find(|m| m.account != a).map(|m| m.id).unwrap();
        let before = s.mailbox.clone();
        take(&mut s, &mut totals, SyncResult { filed: vec![queued, 424242, 0], sent: vec![queued], ..piece("") });
        take(&mut s, &mut totals, SyncResult { filed: vec![other], ..piece("") });
        take(&mut s, &mut totals, SyncResult { unfiled: vec![other], ..piece("") });
        assert!(s.mailbox == before && s.sync_status == "Keeping a copy of what was sent");

        // The newest mail of the Inbox shows with the first piece that has any...
        let first = SyncResult { folders: vec![batch(None, "INBOX", FolderKind::Inbox, &[5, 6], 0)], ..piece("Inbox: 2 of 6") };
        let applied = take(&mut s, &mut totals, first.clone());
        assert_eq!((applied["new"].as_u64(), s.mailbox.count(inbox), s.sync_status.as_str()), (Some(2), 2, "Inbox: 2 of 6"));
        // ...and the mailbox is not said to be read: a run that ended here would ask for the
        // rest again, and not for what came.
        let f = s.mailbox.folder(inbox).unwrap();
        assert_eq!((f.remote.as_str(), f.uid_validity, f.uid_seen, known(&request(&s, a), inbox)), ("INBOX", 7, 0, vec![5, 6]));
        take(&mut s, &mut totals, SyncResult { folders: vec![batch(Some(inbox), "INBOX", FolderKind::Inbox, &[3, 4], 0)], ..piece("Inbox: 4 of 6") });
        assert_eq!((s.mailbox.count(inbox), s.mailbox.folder(inbox).unwrap().uid_seen, totals.new, totals.finished), (4, 0, 4, false));

        // The last piece: the rest of the Inbox, and the copy of what was sent, which is the
        // message that is here already.
        let copy = Message { message_id: s.mailbox.message(queued).unwrap().message_id.clone(), read: true, ..from_server(1, "Queued") };
        let last = SyncResult {
            account: a,
            folders: vec![batch(Some(inbox), "INBOX", FolderKind::Inbox, &[1, 2], 6), news(None, "Sent", FolderKind::Sent, 9, vec![copy], vec![])],
            ..Default::default()
        };
        let applied = take(&mut s, &mut totals, last);
        assert_eq!((applied["new"].as_u64(), applied["matched"].as_u64(), applied["partial"].as_bool()), (Some(2), Some(1), Some(false)));
        assert_eq!((s.mailbox.count(inbox), s.mailbox.folder(inbox).unwrap().uid_seen, s.mailbox.count(sent_items)), (6, 6, 1));
        assert_eq!(s.mailbox.message(queued).map(|m| m.uid), Some(1));
        // The status bar, and whoever asks, hear how the whole run went, not its last piece.
        assert_eq!(
            (applied["summary"].as_str(), applied["total"]["new"].as_u64(), applied["total"]["sent"].as_u64()),
            (Some("6 new messages, 1 sent"), Some(6), Some(1))
        );
        assert!(s.sync_status.starts_with("6 new messages, 1 sent — "), "{}", s.sync_status);
        assert_eq!(totals, SyncTotals { new: 6, sent: 1, matched: 1, finished: true, ..Default::default() });
        assert_eq!(totals.summary(), "6 new messages, 1 sent");

        // A piece that comes a second time brings nothing a second time, and takes nothing back.
        let again = take(&mut s, &mut SyncTotals::default(), first);
        assert_eq!((again["new"].as_u64(), s.mailbox.count(inbox), s.mailbox.folder(inbox).unwrap().uid_seen), (Some(0), 6, 6));
    }

    #[test]
    fn the_end_of_a_run_says_how_all_of_it_went() {
        let (mut s, a, inbox, _) = synced();
        let piece = |progress: &str| SyncResult { account: a, partial: true, progress: progress.into(), ..Default::default() };
        let more = |uids: &[u32], uid_seen: u32| vec![batch(Some(inbox), "INBOX", FolderKind::Inbox, uids, uid_seen)];
        // A problem told early is what the run is remembered for, whatever came after it.
        let mut totals = SyncTotals::default();
        take(&mut s, &mut totals, SyncResult { errors: vec!["Couldn't open \"Clutter\": no".into()], ..piece("Checking Inbox (1 of 3)") });
        take(&mut s, &mut totals, SyncResult { folders: more(&[5, 6], 3), errors: vec!["Another".into()], ..piece("Inbox: 2 of 3") });
        assert_eq!(s.sync_status, "Inbox: 2 of 3");
        let applied = take(&mut s, &mut totals, SyncResult { account: a, folders: more(&[4], 6), ..Default::default() });
        assert!(s.sync_status.starts_with("Couldn't open \"Clutter\": no — "), "{}", s.sync_status);
        assert_eq!(
            (applied["summary"].as_str(), applied["errors"].as_array().map(Vec::len), totals.errors.len(), totals.new),
            (Some("Couldn't open \"Clutter\": no"), Some(0), 2, 3)
        );
        assert_eq!(applied["total"]["errors"], json!(["Couldn't open \"Clutter\": no", "Another"]));

        // A run that was called off says so, and what it had brought by then.
        let mut totals = SyncTotals::default();
        take(&mut s, &mut totals, SyncResult { folders: more(&[8, 9], 6), ..piece("Inbox: 2 of 5") });
        let applied = take(&mut s, &mut totals, SyncResult { account: a, stopped: true, folders: more(&[], 6), ..Default::default() });
        assert!(s.sync_status.starts_with("Stopped (2 new messages) — "), "{}", s.sync_status);
        assert_eq!((applied["stopped"].as_bool(), totals.stopped, totals.finished, totals.new), (Some(true), true, true, 2));
        // The mailbox is read up to below what didn't come: the next run asks for it.
        assert_eq!((s.mailbox.folder(inbox).unwrap().uid_seen, known(&request(&s, a), inbox)), (6, vec![1, 2, 3, 4, 5, 6, 8, 9]));
        let rev = s.rev();
        apply(&mut s, SyncResult { account: a, stopped: true, ..Default::default() });
        assert!(s.sync_status.starts_with("Stopped — ") && s.rev() == rev, "{}", s.sync_status);
        apply(&mut s, SyncResult { account: a, ..Default::default() });
        assert!(s.sync_status.starts_with("Up to date — ") && s.rev() == rev, "{}", s.sync_status);

        // Taken in on its own, or by the command, a piece still only says how far the run has got.
        apply(&mut s, piece("Checking Archive (2 of 3)"));
        assert_eq!(s.sync_status, "Checking Archive (2 of 3)");
        let applied = run(&mut s, "sync.apply", json!({"result": {"account": a, "partial": true, "progress": "Archive: 50 of 80"}}));
        assert_eq!(
            (applied["partial"].as_bool(), applied["progress"].as_str(), s.sync_status.as_str()),
            (Some(true), Some("Archive: 50 of 80"), "Archive: 50 of 80")
        );
        // A piece without a line leaves the one that is there.
        apply(&mut s, piece(""));
        assert_eq!(s.sync_status, "Archive: 50 of 80");
        // An answer from before there were pieces reads as a whole one.
        let old: SyncResult = serde_json::from_value(json!({"account": a, "sent": [], "errors": []})).unwrap();
        assert!(!old.partial && !old.stopped && old.progress.is_empty());
    }

    #[test]
    fn wherever_the_pieces_stop_the_next_run_brings_the_rest() {
        // A mailbox of nine messages, three to a piece.
        let server: Vec<u32> = (1..=9).collect();
        for taken in 0..=3 {
            let mut s = session();
            let a =
                run(&mut s, "account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}}))["id"].as_u64().unwrap();
            let inbox = folder(&s, a, FolderKind::Inbox);
            let pieces = [(&[7, 8, 9], 0, true), (&[4, 5, 6], 0, true), (&[1, 2, 3], 9, false)];
            let mut totals = SyncTotals::default();
            for (uids, uid_seen, partial) in pieces.into_iter().take(taken) {
                let folders = vec![batch(None, "INBOX", FolderKind::Inbox, uids, uid_seen)];
                take(&mut s, &mut totals, SyncResult { account: a, partial, folders, ..Default::default() });
            }
            // Then nobody is there for the others. The next run is asked for what is above the
            // count of how far the mailbox was read and not known: all that is missing, and
            // nothing that came.
            let asked = request(&s, a);
            let state = asked.folders.iter().find(|f| f.id == inbox).unwrap();
            let wanted: Vec<u32> = server.iter().copied().filter(|uid| *uid > state.uid_seen && !state.known_uids.contains(uid)).collect();
            assert_eq!(wanted.len(), 9 - 3 * taken, "{taken}");
            let brought =
                apply(&mut s, SyncResult { account: a, folders: vec![batch(None, "INBOX", FolderKind::Inbox, &wanted, 9)], ..Default::default() });
            assert_eq!(brought["new"], 9 - 3 * taken, "{taken}");
            let mut uids: Vec<u32> = s.mailbox.messages_in(inbox).map(|m| m.uid).collect();
            uids.sort_unstable();
            assert_eq!((uids, s.mailbox.folder(inbox).unwrap().uid_seen), (server.clone(), 9), "{taken}");
        }
    }

    /// A server that numbers a mailbox anew (a new UIDVALIDITY) takes away the UIDs, not what
    /// was done here meanwhile: each change is made again on the message's new copy, found by
    /// its Message-ID, and what can't be found that way is said.
    #[test]
    fn what_was_done_here_outlives_a_mailbox_the_server_numbers_anew() {
        let (mut s, a, inbox, archive) = synced();
        let nameless = Message { message_id: String::new(), ..from_server(4, "Four") };
        apply(
            &mut s,
            SyncResult { account: a, folders: vec![news(Some(inbox), "INBOX", FolderKind::Inbox, 7, vec![nameless], vec![])], ..Default::default() },
        );
        let (one, two, three, four) = (titled(&s, a, "One"), titled(&s, a, "Two"), titled(&s, a, "Three"), titled(&s, a, "Four"));
        // Before the server hears of it: Two read, Three filed, One deleted for good, and Four,
        // which has no Message-ID, read.
        run(&mut s, "mail.markRead", json!({"id": two}));
        run(&mut s, "mail.move", json!({"id": three, "folder": archive}));
        run(&mut s, "mail.deletePermanently", json!({"id": one}));
        run(&mut s, "mail.markRead", json!({"id": four}));
        assert_eq!(unpushed(&s, a), 4);

        // The server's INBOX comes back numbered anew, every message on it under another UID.
        let anew = vec![
            from_server(11, "One"),
            from_server(12, "Two"),
            from_server(13, "Three"),
            Message { message_id: String::new(), ..from_server(14, "Four") },
        ];
        let r = apply(
            &mut s,
            SyncResult { account: a, folders: vec![news(Some(inbox), "INBOX", FolderKind::Inbox, 70, anew, vec![])], ..Default::default() },
        );
        let note = r["errors"][0].as_str().unwrap();
        assert!(note.contains("anew") && note.contains("3 change(s)") && note.contains("1 change(s)"), "{note}");

        // Two is the message it was, read, under its new UID; the server is told it was read.
        assert_eq!(state(&s, two), (inbox, 12, true, FlagStatus::None));
        // Three stays filed; One stays deleted. Neither comes back.
        assert_eq!(state(&s, three), (archive, 0, false, FlagStatus::None));
        assert!(s.mailbox.message(one).is_none() && s.mailbox.messages_in(inbox).all(|m| m.subject != "One" && m.subject != "Three"));
        // Four's change couldn't be kept: it is as the server has it.
        let four = titled(&s, a, "Four");
        assert_eq!(state(&s, four), (inbox, 14, false, FlagStatus::None));
        let ops = request(&s, a).ops;
        assert!(ops.contains(&flags("INBOX", 12, Some(true), None, None)), "{ops:?}");
        assert!(
            ops.contains(&RemoteOp::Move { message: three, from: "INBOX".into(), uid: 13, to: "Archive".into(), to_folder: archive, create: vec![] }),
            "{ops:?}"
        );
        assert!(ops.contains(&RemoteOp::Delete { from: "INBOX".into(), uid: 11 }), "{ops:?}");
        assert_eq!((ops.len(), unpushed(&s, a)), (3, 3));
    }
}

#[test]
fn files_round_trip() {
    let mut s = session();
    let saved = run(&mut s, "file.save", json!({}));
    assert_eq!(saved["name"], "mailbox.mailcraft");
    let before = s.mailbox.clone();
    run(&mut s, "file.new", json!({}));
    assert_eq!((s.mailbox.messages.len(), s.can_undo(), s.dirty), (0, false, false));
    run(&mut s, "file.open", json!({"name": "mailbox.mailcraft", "data": saved["data"]}));
    assert!(s.mailbox == before);
    assert!(s.run("file.open", &json!({"name": "x.mailcraft", "data": "bm9wZQ=="})).is_err());
    assert!(s.mailbox == before);

    // A message out as .eml and back in.
    let id = s.rows()[1].id;
    let eml = run(&mut s, "file.exportMessage", json!({"id": id}));
    assert_eq!(eml["name"], "Homepage mockups, round three.eml");
    let archive = kind(&s, FolderKind::Archive);
    let r = run(&mut s, "file.import", json!({"name": "in.eml", "data": eml["data"], "folder": archive}));
    let copy = s.mailbox.message(r["ids"][0].as_u64().unwrap()).unwrap();
    let orig = s.mailbox.message(id).unwrap();
    assert_eq!(
        (copy.folder, &copy.subject, &copy.from, &copy.to, &copy.body.text, copy.attachments.len()),
        (archive, &orig.subject, &orig.from, &orig.to, &orig.body.text, 1)
    );
    assert_eq!(copy.attachments[0].data, orig.attachments[0].data);

    // A folder as .mbox, the calendar as .ics, the contacts as .vcf.
    let mbox = run(&mut s, "file.exportFolder", json!({}));
    assert_eq!((mbox["name"].as_str(), mbox["messages"].as_u64()), (Some("Inbox.mbox"), Some(13)));
    let ics = run(&mut s, "file.exportCalendar", json!({}));
    let vcf = run(&mut s, "file.exportContacts", json!({}));
    let (events, tasks, contacts) = (s.mailbox.events.len(), s.mailbox.tasks.len(), s.mailbox.contacts.len());
    assert_eq!(
        (ics["events"].as_u64(), ics["tasks"].as_u64(), vcf["contacts"].as_u64()),
        (Some(events as u64), Some(tasks as u64), Some(contacts as u64))
    );
    run(&mut s, "file.new", json!({}));
    assert_eq!(run(&mut s, "file.import", json!({"name": "all.mbox", "data": mbox["data"]}))["messages"], 13);
    let r = run(&mut s, "file.import", json!({"name": "cal.ics", "data": ics["data"]}));
    assert_eq!((r["events"].as_u64(), r["tasks"].as_u64()), (Some(events as u64), Some(tasks as u64)));
    assert_eq!(run(&mut s, "file.import", json!({"name": "people.vcf", "data": vcf["data"]}))["contacts"], contacts);
    // The same calendar again updates rather than doubles.
    assert_eq!(run(&mut s, "file.import", json!({"name": "cal.ics", "data": ics["data"]}))["updated"], events);
    assert_eq!(s.mailbox.events.len(), events);
    let standup = s.mailbox.events.values().find(|e| e.title == "Team stand-up").unwrap();
    assert_eq!(standup.occurrences(Date::new(2026, 10, 12).unwrap(), Date::new(2026, 10, 18).unwrap()).len(), 5);
    assert!(s.run("file.import", &json!({"name": "junk.bin", "data": "AAEC"})).is_err());
}

#[test]
fn options_validate() {
    let mut s = session();
    let p = run(
        &mut s,
        "file.options",
        json!({"twelveHour": false, "weekStart": "monday", "workDays": ["tue", "MO"], "defaultReminder": null, "previewLines": 9}),
    );
    assert_eq!((p["twelveHour"].as_bool(), p["weekStart"].as_str(), p["previewLines"].as_u64()), (Some(false), Some("Mon"), Some(3)));
    assert_eq!(s.prefs.work_days, [mailcraft_time::Weekday::Mon, mailcraft_time::Weekday::Tue]);
    assert!(s.run("file.options", &json!({"workStart": 18, "workEnd": 9})).is_err());
    assert!(s.run("file.options", &json!({"weekStart": "someday"})).is_err());
    assert_eq!(s.prefs.work_start, 8);
    assert_eq!(session::parse_enum::<CalView>("WorkWeek"), Some(CalView::WorkWeek));
    assert_eq!(session::enum_code(&CalView::WorkWeek), "workWeek");
}

#[test]
fn parity_has_a_floor_that_only_rises() {
    let s = session();
    let p = catalog::parity(&s.registry);
    let percent = p["percent"].as_f64().unwrap();
    assert!(percent >= 60.0, "catalog parity fell to {percent}%");
    // Every catalog id is spelled like a command id.
    for f in catalog::features() {
        assert!(f.id.contains('.') && !f.id.contains(' '), "{}", f.id);
    }
    assert!(catalog::parity_markdown(&s.registry).contains("## Missing"));
}

/// Never-crash: every command, with junk for every parameter it might read, returns `Ok` or `Err`.
#[test]
fn hostile_params_never_panic() {
    let junk = [
        json!(null),
        json!({}),
        json!([]),
        json!("text"),
        json!(-1),
        json!({"id": -1, "ids": [-1, 0, 1.5, "x", null, 18446744073709551615u64], "folder": 0, "index": 1e300, "value": {"deep": [[[]]]}}),
        json!({"id": 18446744073709551615u64, "folder": 18446744073709551615u64, "parent": "x", "account": 1e19, "calendar": -5}),
        json!({"id": 1, "name": "", "title": "", "category": "", "query": "\"\"\"", "by": "nothing", "value": "nothing", "response": "maybe"}),
        json!({"name": "\u{0}\r\n/\\..", "title": "x".repeat(100_000), "category": "\u{202e}", "to": "a@b,;;<<>>\"", "subject": "\r\nBcc: x@example.org", "body": "\u{0}"}),
        json!({"start": "9999-12-31T23:59", "end": "0001-01-01", "date": "9999-12-31", "due": "0000-00-00", "from": "9999-01-01", "to": "0001-01-01", "occurrence": "x", "minutes": 1e300, "reminder": -1}),
        json!({"title": "t", "start": "2026-10-14T10:00", "recurrence": {"freq": "daily", "interval": 4294967295u32, "count": 4294967295u32, "byDay": ["xx", 5], "nth": 99, "until": "0001-01-01"}}),
        json!({"data": "!!!!", "name": "../../etc/passwd", "path": "\u{0}", "mime": "\r\n"}),
        json!({"result": {"account": 1, "folders": [{"id": 1, "remote": "\u{0}", "new": [{"uid": 1}, {"uid": 1}], "flags": [{"uid": 4294967295u32}], "removed": [0, 1]}], "sent": [0, 1, 2]}}),
        json!({"result": {
            "account": 1, "sent": [1, 2, 3], "filed": [1, 3, 18446744073709551615u64],
            "flagged": [{"folder": "", "uid": 4294967295u32, "read": true}, {"folder": "INBOX", "uid": 7, "flagged": false}],
            "moved": [{"message": 1, "from": "\u{0}", "uid": 1, "toFolder": 18446744073709551615u64, "newUid": 4294967295u32, "uidValidity": 1}, {"message": 0, "from": "INBOX", "uid": 7, "toFolder": 2, "newUid": 7, "uidValidity": 4294967295u32}],
            "deleted": [{"from": "INBOX", "uid": 0}, {"from": "INBOX", "uid": 7}],
            "created": [{"folder": 1, "remote": "\r\n"}, {"folder": 2, "remote": "INBOX"}, {"folder": 18446744073709551615u64}],
            "folders": [
                {"remote": "INBOX", "kind": "inbox", "uidValidity": 4294967295u32, "uidSeen": 4294967295u32, "new": [{"uid": 7, "messageId": "a", "synced": "junk"}, {"uid": 7}, {"uid": 0, "synced": {"uid": 9}}], "flags": [{"uid": 7, "read": true}, {"uid": 0}], "removed": [7, 7, 0]},
                {"remote": "INBOX", "kind": "inbox", "uidValidity": 1, "new": [{"uid": 4294967295u32, "messageId": "a"}], "removed": [4294967295u32]},
                {"id": 2, "remote": "", "new": [{}]},
            ],
            "errors": ["\u{0}", ""],
        }}),
        json!({"conditions": [{"type": "x"}, 5, null], "actions": [{"type": "moveTo", "value": "x"}], "name": "r"}),
        json!({"email": "a@b.example", "incoming": {"host": "", "port": 1e9, "security": 5}, "outgoing": 7}),
        json!({"emails": [1, null, {"value": 2}], "phones": "x", "address": {"street": 5}, "birthday": 5, "workDays": [], "weekStart": 1}),
        json!({"account": 1, "address": "a@example.org/../../me\r\n?$top=1#x"}),
        json!({"account": 2, "address": "x".repeat(100_000)}),
        json!({"id": 2, "address": "\u{0}@example.org", "email": "\u{202e}@example.org"}),
        json!({"account": 1, "download": {"account": 1, "calendar": "\u{0}", "from": 1e300, "events": [{"id": 5}, null], "contacts": {"id": "c"}}}),
        json!({"account": 1, "download": {
            "account": 1, "complete": true, "from": i64::MIN, "to": i64::MAX,
            "events": [
                {"id": "e-1", "title": "\u{0}\r\n", "start": i64::MAX, "end": i64::MIN, "allDay": true, "kind": "occurrence", "series": "\u{202e}", "attendees": [{"address": {"email": "\r\n"}}]},
                {"id": "e-1", "start": i64::MIN, "end": i64::MAX, "reminderMinutes": 4294967295u32}, {"id": "e 2"}, {"kind": "seriesMaster"}, {},
            ],
            "contacts": [{"id": "c-1", "displayName": "\u{0}", "emails": [{"value": "a@b.example\r\nBcc: x@example.org"}], "birthday": {"year": 0, "month": 13, "day": 32}}, {"id": "c-1"}, {}],
        }}),
    ];
    let mut s = session();
    // Shared mailboxes switched on, under a Microsoft account, so that their commands get past
    // the door (#70).
    let _ = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"}));
    // And one open, so that the folder pane's commands (Favorites, counts) meet its folders.
    let _ = s.run("account.addSharedMailbox", &json!({"address": "support@example.org"}));
    let ids: Vec<&'static str> = s.registry.all().iter().map(|c| c.id).collect();
    for round in 0..2 {
        for id in &ids {
            for j in &junk {
                // File paths in junk stay away from the disk: only the in-memory forms are tried.
                if j.get("path").is_some() && id.starts_with("file.") {
                    continue;
                }
                let _ = s.run(id, j);
            }
            // State that makes more commands reachable on the second pass.
            if round == 1 {
                let _ = s.run("file.new", &json!({"sample": true}));
                let first = s.rows().first().map(|r| r.id).unwrap_or(0);
                let _ = s.run("mail.select", &json!({"id": first}));
                let _ = s.run("mail.new", &json!({}));
                let e = s.mailbox.events.keys().next().copied().unwrap_or(0);
                let _ = s.run("calendar.select", &json!({"id": e}));
                let c = s.mailbox.contacts.keys().next().copied().unwrap_or(0);
                let _ = s.run("people.select", &json!({"id": c}));
                let t = s.mailbox.tasks.keys().next().copied().unwrap_or(0);
                let _ = s.run("tasks.select", &json!({"id": t}));
            }
        }
        // Whatever happened, the mailbox is still consistent and the list still builds.
        let _ = s.rows();
        assert!(s.mailbox.clone().repair().is_empty(), "round {round}: {:?}", s.mailbox.clone().repair());
    }
}

#[test]
fn a_panicking_command_is_contained() {
    fn boom(s: &mut Session, _: &Value) -> CmdResult {
        // Change something, then fail the hard way.
        let ids: Vec<Id> = s.mailbox.messages.keys().copied().collect();
        for id in ids {
            let _ = s.mailbox.remove_message(id);
        }
        panic!("boom");
    }
    let mut s = session();
    let mut specs = s.registry.all().to_vec();
    specs.push(CommandSpec::new("test.boom", "Boom", "Test", boom));
    s.registry = std::sync::Arc::new(Registry::new(specs));
    let before = s.mailbox.clone();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = s.run("test.boom", &json!({}));
    std::panic::set_hook(hook);
    assert!(matches!(r, Err(CmdError::Failed(_) | CmdError::Said(_))));
    assert!(s.mailbox == before && !s.can_undo());
    // The session still works.
    assert!(s.run("mail.next", &json!({})).is_ok());
}

/// The server's folders are all here, in its tree, as soon as a run has listed them: before any
/// of their mail has come. Mail that comes later lands in them, a folder made flat by an earlier
/// run finds its place, and a nonsense list does no harm.
#[test]
fn the_servers_folder_tree_is_here_before_its_mail() {
    use mailcraft_model::sync::{FolderDelta, ListedFolder, SyncResult};
    use mailcraft_model::{Address, Message};

    let mut s = session();
    let a = run(&mut s, "account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}}))["id"].as_u64().unwrap();
    let listed = |remote: &str, kind: FolderKind, parent: &str, name: &str| ListedFolder {
        remote: remote.into(),
        kind,
        parent: parent.into(),
        name: name.into(),
    };
    let tree = vec![
        // Children before their parents, as a server may list them.
        listed("INBOX/Projects/Atlas", FolderKind::Custom, "INBOX/Projects", "Atlas"),
        listed("INBOX", FolderKind::Inbox, "", "Inbox"),
        listed("Sent Items", FolderKind::Sent, "", "Sent Items"),
        listed("INBOX/Projects", FolderKind::Custom, "INBOX", "Projects"),
        listed("Receipts", FolderKind::Custom, "", "Receipts"),
        listed("Receipts/2026", FolderKind::Custom, "Receipts", "2026"),
    ];
    let before = s.mailbox.folders.iter().filter(|f| f.account == a).count();
    cmd::account::apply_sync(
        &mut s,
        SyncResult { account: a, listed: tree.clone(), partial: true, progress: "Found 6 folders".into(), ..Default::default() },
    )
    .unwrap();
    let find = |s: &Session, remote: &str| s.mailbox.folders.iter().find(|f| f.account == a && f.remote == remote).cloned().unwrap();
    let (inbox, sent) = (find(&s, "INBOX"), find(&s, "Sent Items"));
    // The standard folders follow their mailboxes; nothing is made twice.
    assert_eq!((inbox.kind, sent.kind), (FolderKind::Inbox, FolderKind::Sent));
    assert_eq!(Some(inbox.id), s.mailbox.folder_of_kind(a, FolderKind::Inbox));
    let (projects, atlas, receipts, year) =
        (find(&s, "INBOX/Projects"), find(&s, "INBOX/Projects/Atlas"), find(&s, "Receipts"), find(&s, "Receipts/2026"));
    assert_eq!((projects.name.as_str(), projects.parent), ("Projects", Some(inbox.id)));
    assert_eq!((atlas.name.as_str(), atlas.parent), ("Atlas", Some(projects.id)));
    assert_eq!((receipts.parent, year.parent, year.name.as_str()), (None, Some(receipts.id), "2026"));
    assert_eq!(s.mailbox.folders.iter().filter(|f| f.account == a).count(), before + 4);
    assert_eq!(s.sync_status, "Found 6 folders");

    // Said again (every run says it): nothing new, nothing moved.
    let folders = s.mailbox.folders.clone();
    cmd::account::apply_sync(&mut s, SyncResult { account: a, listed: tree.clone(), partial: true, ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.folders, folders);

    // Mail for a folder deep in the tree lands in it.
    let mail = Message { uid: 1, subject: "Deep".into(), from: Address::new("", "x@example.org"), date: 100, ..Default::default() };
    let delta = FolderDelta {
        id: None,
        remote: "INBOX/Projects/Atlas".into(),
        kind: FolderKind::Custom,
        uid_validity: 3,
        uid_seen: 1,
        new: vec![mail],
        ..Default::default()
    };
    cmd::account::apply_sync(&mut s, SyncResult { account: a, folders: vec![delta], ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.count(atlas.id), 1);

    // A folder an earlier run made at the top, before the tree was known, finds its place.
    let flat = s.mailbox.add_folder(a, None, "Travel").unwrap();
    s.mailbox.folder_mut(flat).unwrap().remote = "Receipts/Travel".into();
    let more = vec![listed("Receipts", FolderKind::Custom, "", "Receipts"), listed("Receipts/Travel", FolderKind::Custom, "Receipts", "Travel")];
    cmd::account::apply_sync(&mut s, SyncResult { account: a, listed: more, ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.folder(flat).unwrap().parent, Some(receipts.id));

    // A folder named like an address keeps its whole name, and one an earlier version cut down
    // to what follows the last dot gets it back; a folder its owner renamed is left alone.
    let cut = s.mailbox.add_folder(a, None, "org").unwrap();
    s.mailbox.folder_mut(cut).unwrap().remote = "ada@example.org".into();
    let renamed = s.mailbox.add_folder(a, None, "My own name").unwrap();
    s.mailbox.folder_mut(renamed).unwrap().remote = "lists@example.net".into();
    let addresses = vec![
        listed("ada@example.org", FolderKind::Custom, "", "ada@example.org"),
        listed("lists@example.net", FolderKind::Custom, "", "lists@example.net"),
        listed("new@example.com", FolderKind::Custom, "", "new@example.com"),
    ];
    cmd::account::apply_sync(&mut s, SyncResult { account: a, listed: addresses, ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.folder(cut).unwrap().name, "ada@example.org");
    assert_eq!(s.mailbox.folder(renamed).unwrap().name, "My own name");
    assert_eq!(find(&s, "new@example.com").name, "new@example.com");
    // And without a list (an older host), the name is not cut at its dots either.
    let late = FolderDelta { id: None, remote: "late@example.org".into(), kind: FolderKind::Custom, uid_validity: 1, ..Default::default() };
    cmd::account::apply_sync(&mut s, SyncResult { account: a, folders: vec![late], ..Default::default() }).unwrap();
    assert_eq!(find(&s, "late@example.org").name, "late@example.org");

    // Nonsense: a folder inside itself, a ring, no name, a parent nobody listed, thousands of them.
    let mut junk = vec![
        listed("Loop", FolderKind::Custom, "Loop", "Loop"),
        listed("A", FolderKind::Custom, "B", "A"),
        listed("B", FolderKind::Custom, "A", "B"),
        listed("", FolderKind::Custom, "", ""),
        listed("Orphan/Child", FolderKind::Custom, "Orphan", ""),
        listed("Receipts", FolderKind::Inbox, "Receipts/2026", "Receipts"),
    ];
    junk.extend((0..5000).map(|i| listed(&format!("Bulk/{i}"), FolderKind::Custom, "Bulk", &"x".repeat(i % 300))));
    cmd::account::apply_sync(&mut s, SyncResult { account: a, listed: junk, ..Default::default() }).unwrap();
    assert!(s.mailbox.folders.len() < 2100);
    // No folder ended up inside itself, and the mailbox still holds together.
    for f in &s.mailbox.folders {
        assert!(!s.mailbox.subtree(f.id).iter().skip(1).any(|id| *id == f.id), "{}", f.name);
    }
    let mut repaired = s.mailbox.clone();
    repaired.repair();
    assert_eq!(repaired.folders.len(), s.mailbox.folders.len());
}

#[test]
fn a_windows_1250_contact_file_keeps_its_letters() {
    // #185: a vCard 2.1 file with raw 8-bit values in Windows-1250 lost đ, ć, č and Š, because the
    // bytes were read as UTF-8 before the CHARSET of each line was looked at.
    let mut s = session();
    let file: &[u8] = b"BEGIN:VCARD\r\nVERSION:2.1\r\nN;CHARSET=WINDOWS-1250:Gvo\xf0\xe6i\xe8;Ivana\r\nFN;CHARSET=WINDOWS-1250:Ivana Gvo\xf0\xe6i\xe8\r\nORG;CHARSET=WINDOWS-1250:\x8akola \xe8e\x9a\xe6e\r\nEMAIL;INTERNET:ivana@example.org\r\nEND:VCARD\r\n";
    let read = crate::io::import(&mut s, "ivana.vcf", file, None).unwrap();
    assert_eq!(read["contacts"], 1);
    let c = s.mailbox.contact(s.sel.contact.unwrap()).unwrap();
    assert_eq!((c.display_name.as_str(), c.last.as_str(), c.company.as_str()), ("Ivana Gvođćič", "Gvođćič", "Škola češće"));
}

// ---- C1 PR-a: a mailbox in mail is saved, never opened; questions name servers ----

#[test]
fn a_mailcraft_attachment_is_never_opened_from_a_message() {
    use crate::opening::{Verdict, verdict};
    for name in ["backup.mailcraft", "Backup.MAILCRAFT", "x/../y.mailcraft", "backup.mailcraft."] {
        assert_eq!(verdict(name), Verdict::Blocked, "{name}");
    }
}

#[test]
fn the_question_for_account_add_names_the_servers() {
    let s = session();
    let d = crate::agent::describe(
        &s,
        "account.add",
        &json!({"email": "me@example.org", "incoming": {"host": "imap.evil.example", "port": 993}, "outgoing": {"host": "smtp.evil.example", "port": 465}}),
    );
    assert!(d.contains("me@example.org") && d.contains("imap imap.evil.example:993") && d.contains("smtp smtp.evil.example:465"), "{d}");
}

#[test]
fn the_question_for_account_edit_names_the_new_host_only_when_it_changes() {
    let mut s = session();
    let id = run(
        &mut s,
        "account.add",
        json!({"email": "me@example.org", "incoming": {"host": "imap.example.org", "port": 993}, "outgoing": {"host": "smtp.example.org", "port": 465}}),
    )["id"]
        .as_u64()
        .unwrap();
    let moved = crate::agent::describe(&s, "account.edit", &json!({"id": id, "incoming": {"host": "imap.evil.example", "port": 993}}));
    assert!(moved.contains("imap imap.example.org:993") && moved.contains("imap imap.evil.example:993"), "{moved}");
    let same = crate::agent::describe(&s, "account.edit", &json!({"id": id, "displayName": "Me"}));
    assert!(!same.contains("servers change"), "{same}");
}

#[test]
fn the_question_for_file_open_reads_the_servers_from_the_file() {
    let mut s = Session::empty();
    run(&mut s, "account.add", json!({"email": "owner@example.org", "incoming": {"host": "imap.evil.example", "port": 993}}));
    let dir = std::env::temp_dir().join(format!("mailcraft-c1a-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("crafted.mailcraft");
    run(&mut s, "file.saveAs", json!({"path": path.to_string_lossy()}));
    let asker = Session::empty();
    let d = crate::agent::describe(&asker, "file.open", &json!({"path": path.to_string_lossy()}));
    assert!(d.contains("owner@example.org") && d.contains("imap.evil.example:993"), "{d}");
    let missing = crate::agent::describe(&asker, "file.open", &json!({"path": dir.join("none.mailcraft").to_string_lossy()}));
    assert!(missing.contains("can't be read"), "{missing}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_mbox_import_stops_after_the_most_it_takes() {
    let mut s = session();
    let one = b"From a@example.org Tue Jul  1 08:52:37 2003\nSubject: x\n\nx\n\n";
    let file = one.repeat(crate::io::MAX_IMPORT_MESSAGES + 50);
    let r = crate::io::import(&mut s, "big.mbox", &file, None).unwrap();
    assert_eq!(r["messages"], crate::io::MAX_IMPORT_MESSAGES);
    assert_eq!(r["stoppedAfter"], crate::io::MAX_IMPORT_MESSAGES);
    // A file within the limit says nothing of it.
    let r = crate::io::import(&mut s, "small.mbox", &one.repeat(3), None).unwrap();
    assert_eq!(r["messages"], 3);
    assert!(r["stoppedAfter"].is_null());
}

#[test]
fn a_reply_quotes_only_the_first_names_of_a_long_list() {
    let mut s = session();
    let inbox = kind(&s, FolderKind::Inbox);
    let to = (0..5000).map(|i| mailcraft_model::Address::new("", format!("u{i}@example.org"))).collect();
    let id = s.mailbox.add_message(mailcraft_model::Message { folder: inbox, subject: "many".into(), to, ..Default::default() }).unwrap();
    run(&mut s, "mail.reply", json!({"id": id}));
    let c = s.compose.as_ref().unwrap();
    let body = c.quote.as_ref().map_or_else(|| c.body.plain(), |q| q.text.clone());
    assert!(body.contains("(+4900)") && !body.contains("u4999@"), "{}", body.len());
}
