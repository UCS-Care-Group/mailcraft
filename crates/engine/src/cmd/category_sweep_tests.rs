// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A renamed category reaching the mail and the events that only the server has (#851): what the
//! runs are asked, what holds the old name's deletion in Outlook, every way a wait ends, undo, and
//! what an agent is asked.

use mailcraft_model::remote::{
    CategoriesDownload, CategoriesUploaded, CategoryChange, CategoryChangeKind, CategoryOutcome, CategoryRename, CategoryRenamed, CategoryResult,
    EventsUpload, EventsUploaded,
};
use mailcraft_model::sync::{GraphMailRequest, GraphMailResult};
use mailcraft_model::{Calendar, Category, Id, Mailbox, Origin, Rgb, Rights};
use serde_json::json;

use crate::Session;
use crate::agent::{Class, Verdict, check};

const NOW: i64 = 1_800_000_000;
const RED: Rgb = Rgb(0xD1, 0x3B, 0x3B);
const GREEN: Rgb = Rgb(0x2E, 0x8B, 0x57);

fn tag(name: &str, color: Rgb) -> Category {
    Category { name: name.into(), color }
}

/// A Microsoft account whose server lists "Red category" and "Holiday", and lets the list be changed.
fn rig() -> (Session, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.mailbox.categories.clear();
    let a = s.run("account.add", &json!({"email": "a@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    read(&mut s, a, vec![tag("Red category", RED), tag("Holiday", GREEN)]);
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": true})).unwrap();
    (s, a)
}

fn read(s: &mut Session, account: Id, categories: Vec<Category>) {
    let download = CategoriesDownload { account, email: "a@example.org".into(), categories, complete: true };
    s.run("sync.applyCategories", &json!({"download": download})).unwrap();
}

/// What the categories' job would tell the server now.
fn changes(s: &mut Session, account: Id) -> Vec<CategoryChange> {
    let upload = s.run("sync.categoryChanges", &json!({"account": account})).unwrap();
    serde_json::from_value(upload["changes"].clone()).unwrap()
}

fn change(kind: CategoryChangeKind, name: &str) -> CategoryChange {
    CategoryChange { kind, name: name.into(), color: GREEN }
}

/// The categories' job ran, and the server took all of it.
fn job(s: &mut Session, account: Id) -> Vec<CategoryChange> {
    let sent = changes(s, account);
    let outcomes = sent
        .iter()
        .map(|c| CategoryOutcome {
            kind: c.kind,
            name: c.name.clone(),
            color: c.color,
            result: CategoryResult::Done,
            error: String::new(),
            found: false,
        })
        .collect();
    let result = CategoriesUploaded { account, email: "a@example.org".into(), outcomes, ..Default::default() };
    s.run("sync.applyCategoryChanges", &json!({"result": result})).unwrap();
    sent
}

/// What the next mail run is asked to rename.
fn mail_asked(s: &mut Session, account: Id) -> Option<CategoryRename> {
    let request = s.run("sync.graphRequest", &json!({"account": account})).unwrap();
    serde_json::from_value::<GraphMailRequest>(request).unwrap().rename
}

/// What the next calendar job is asked to rename.
fn events_asked(s: &mut Session, account: Id) -> Option<CategoryRename> {
    let upload = s.run("sync.eventChanges", &json!({"account": account})).unwrap();
    serde_json::from_value::<EventsUpload>(upload).unwrap().rename
}

fn renamed(from: &str, to: &str) -> CategoryRenamed {
    CategoryRenamed { from: from.into(), to: to.into(), ..Default::default() }
}

/// A mail run came back with `came`; what it said.
fn mail_came(s: &mut Session, account: Id, came: CategoryRenamed) -> serde_json::Value {
    let result = GraphMailResult { account, email: "a@example.org".into(), renamed: Some(came), ..Default::default() };
    s.run("sync.applyGraph", &json!({"result": result})).unwrap()
}

/// A calendar job came back with `came`; what it said.
fn events_came(s: &mut Session, account: Id, came: CategoryRenamed) -> serde_json::Value {
    let result = EventsUploaded { account, email: "a@example.org".into(), renamed: Some(came), ..Default::default() };
    s.run("sync.applyEventChanges", &json!({"result": result})).unwrap()
}

fn done(from: &str, to: &str) -> CategoryRenamed {
    CategoryRenamed { done: true, ..renamed(from, to) }
}

fn rename(s: &mut Session, account: Id, from: &str, to: &str) {
    s.run("category.rename", &json!({"account": account, "from": from, "to": to})).unwrap();
}

#[test]
fn the_old_name_goes_in_outlook_only_once_the_servers_mail_and_events_have_the_new_one() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    // Until the server has the new name nothing there is given it: it would show as unknown.
    assert_eq!((mail_asked(&mut s, a), events_asked(&mut s, a)), (None, None));
    assert_eq!(job(&mut s, a), [change(CategoryChangeKind::Create, "Holiday 2026")]);
    let asked = CategoryRename { from: "Holiday".into(), to: "Holiday 2026".into(), calendars: Vec::new() };
    assert_eq!(mail_asked(&mut s, a), Some(asked.clone()));
    assert_eq!(events_asked(&mut s, a), Some(asked.clone()));
    assert!(changes(&mut s, a).is_empty(), "deleted while mail and events there may have it");
    // A run that did a part of it: asked again, and the old name still waits.
    mail_came(&mut s, a, CategoryRenamed { changed: 2_000, ..renamed("Holiday", "Holiday 2026") });
    assert_eq!(mail_asked(&mut s, a), Some(asked.clone()));
    assert!(changes(&mut s, a).is_empty());
    // The mail is done: the events still hold it.
    mail_came(&mut s, a, done("Holiday", "Holiday 2026"));
    assert_eq!(mail_asked(&mut s, a), None);
    assert!(changes(&mut s, a).is_empty(), "deleted while events there may have it");
    events_came(&mut s, a, done("Holiday", "Holiday 2026"));
    assert_eq!(events_asked(&mut s, a), None);
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Holiday")]);
    assert!(s.mailbox.category_sweeps.is_empty(), "{:?}", s.mailbox.category_sweeps);
}

#[test]
fn what_waits_is_kept_in_the_file() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    mail_came(&mut s, a, done("Holiday", "Holiday 2026"));
    let back: Mailbox = serde_json::from_str(&serde_json::to_string(&s.mailbox).unwrap()).unwrap();
    assert_eq!(back.category_sweeps, s.mailbox.category_sweeps);
    assert!(back.category_sweeps[&a][0].mail_done && !back.category_sweeps[&a][0].events_done);
    assert!(back.sweep_holds(a, "Holiday"));
}

#[test]
fn a_files_rename_with_spaces_at_the_ends_of_its_names_still_takes_its_answer() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    let mut file = serde_json::to_value(&s.mailbox).unwrap();
    let sweep = &mut file["categorySweeps"][a.to_string()][0];
    (sweep["from"], sweep["to"]) = (json!("  Holiday\t"), json!(" Holiday 2026 "));
    file["categorySweeps"][a.to_string()].as_array_mut().unwrap().push(json!({"from": " Red category ", "to": "red category"}));
    s.mailbox = serde_json::from_value(file).unwrap();
    assert_eq!(s.mailbox.category_sweeps[&a].len(), 1, "a rename to the same name is no rename");
    // The answer names it as it was asked for, and ends it: it isn't asked for ever.
    assert_eq!(mail_asked(&mut s, a), Some(CategoryRename { from: "Holiday".into(), to: "Holiday 2026".into(), calendars: Vec::new() }));
    mail_came(&mut s, a, done("Holiday", "Holiday 2026"));
    events_came(&mut s, a, done("Holiday", "Holiday 2026"));
    assert!(s.mailbox.category_sweeps.is_empty());
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Holiday")]);
}

#[test]
fn an_answer_for_another_rename_or_another_account_ends_nothing() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    mail_came(&mut s, a, done("Holiday", "Trips"));
    mail_came(&mut s, a, done("Red category", "Holiday 2026"));
    assert!(mail_asked(&mut s, a).is_some());
    assert!(changes(&mut s, a).is_empty());
}

#[test]
fn what_keeps_the_old_name_in_outlook_is_said_and_the_name_is_not_deleted_there() {
    // Each way a part can end with something left: said, and once both parts ended the old name
    // is a category of the list again, never deleted in Outlook.
    let h = || renamed("Holiday", "Holiday 2026");
    let cases: [(CategoryRenamed, &str); 6] = [
        (CategoryRenamed { changed: 5, skipped: 2, ..done("Holiday", "Holiday 2026") }, "to 2 of the events that have \\\"Holiday\\\""),
        (CategoryRenamed { denied: true, ..h() }, "doesn't let MailCraft see which events have the category \\\"Holiday\\\""),
        (CategoryRenamed { refused: true, ..h() }, "wouldn't say which events have the category \\\"Holiday\\\""),
        (CategoryRenamed { changed: 4, lists_refused: 1, ..done("Holiday", "Holiday 2026") }, "in 1 of your calendars"),
        (CategoryRenamed { passed: 1, ..done("Holiday", "Holiday 2026") }, "left 1 of the events"),
        (CategoryRenamed { skipped: 2_000, more: true, ..done("Holiday", "Holiday 2026") }, "more of the events may keep \\\"Holiday\\\""),
    ];
    for (came, line) in cases {
        let (mut s, a) = rig();
        rename(&mut s, a, "Holiday", "Holiday 2026");
        job(&mut s, a);
        // The events end with something left while the mail still waits: nothing is deleted.
        let said = events_came(&mut s, a, came.clone());
        assert!(said.to_string().contains(line), "{said}");
        assert!(!said.to_string().contains("back on the list"), "{said}");
        assert_eq!(events_asked(&mut s, a), None, "not asked again");
        assert!(mail_asked(&mut s, a).is_some() && changes(&mut s, a).is_empty());
        // The mail is done: the old name stays, here and there.
        let said = mail_came(&mut s, a, done("Holiday", "Holiday 2026"));
        assert!(said.to_string().contains("The category \\\"Holiday\\\" isn't deleted, and is back on the list."), "{said}");
        assert!(s.mailbox.category_of(a, "Holiday").is_some() && s.mailbox.category_of(a, "Holiday 2026").is_some());
        assert!(changes(&mut s, a).is_empty(), "{came:?}");
        assert!(s.mailbox.category_sweeps.is_empty());
    }
    // Nothing left: the old name goes, and nothing is said.
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    events_came(&mut s, a, CategoryRenamed { changed: 3, ..done("Holiday", "Holiday 2026") });
    let said = mail_came(&mut s, a, CategoryRenamed { changed: 5, ..done("Holiday", "Holiday 2026") });
    assert!(!said.to_string().contains("Holiday"), "{said}");
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Holiday")]);
}

#[test]
fn an_undo_after_the_old_name_came_back_still_gives_the_servers_items_the_old_name() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    events_came(&mut s, a, done("Holiday", "Holiday 2026"));
    mail_came(&mut s, a, CategoryRenamed { changed: 5, skipped: 2, ..done("Holiday", "Holiday 2026") });
    assert!(s.mailbox.category_of(a, "Holiday").is_some() && s.mailbox.category_sweeps.is_empty());
    // The five that were given the new name get the old one again, before the new one is deleted.
    assert_eq!(s.undo_renames_on_server(false), [(a, "Holiday 2026".to_string(), "Holiday".to_string())]);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "Holiday").is_some() && s.mailbox.category_of(a, "Holiday 2026").is_none());
    assert_eq!(mail_asked(&mut s, a), Some(CategoryRename { from: "Holiday 2026".into(), to: "Holiday".into(), calendars: Vec::new() }));
    assert!(changes(&mut s, a).is_empty(), "the new name's deletion waits for them");
    mail_came(&mut s, a, done("Holiday 2026", "Holiday"));
    events_came(&mut s, a, done("Holiday 2026", "Holiday"));
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Holiday 2026")]);
}

#[test]
fn an_undo_of_a_step_that_made_two_names_renames_nothing_on_the_server() {
    // One step puts two names on the list; the server gives one of them another colour since. The
    // undo takes the other off alone: that is no rename, and the server's items are left alone.
    let (mut s, a) = rig();
    let calendar = s.mailbox.alloc_id();
    let origin = Origin { account: a, ..Default::default() };
    s.mailbox.calendars.push(Calendar { id: calendar, name: "Calendar".into(), visible: true, origin, rights: Rights::Edit, ..Default::default() });
    s.run("calendar.new", &json!({"title": "Pack", "calendar": calendar, "categories": ["Projects", "Trips"]})).unwrap();
    assert!(s.mailbox.category_of(a, "Projects").is_some() && s.mailbox.category_of(a, "Trips").is_some());
    assert_eq!(job(&mut s, a).len(), 2);
    let projects = s.mailbox.category_of(a, "Projects").unwrap().clone();
    let other = if s.mailbox.category_of(a, "Trips").unwrap().color == RED { GREEN } else { RED };
    read(&mut s, a, vec![tag("Red category", RED), tag("Holiday", GREEN), projects, tag("Trips", other)]);
    assert!(s.undo_renames_on_server(false).is_empty());
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.category_of(a, "Projects").is_none() && s.mailbox.category_of(a, "Trips").is_some(), "the case this is about");
    assert_eq!((mail_asked(&mut s, a), events_asked(&mut s, a)), (None, None));
    assert!(s.mailbox.category_sweeps.is_empty(), "{:?}", s.mailbox.category_sweeps);
}

#[test]
fn a_rename_on_its_way_shows_how_far_it_has_got_and_what_it_came_to() {
    use mailcraft_model::sync::RenameProgress;
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    s.sync_progress.begin(&[a]);
    let renaming = Some(RenameProgress { done: 340, total: 1_200 });
    let piece = GraphMailResult { account: a, email: "a@example.org".into(), partial: true, renaming, ..Default::default() };
    s.run("sync.applyGraph", &json!({"result": piece})).unwrap();
    assert_eq!(s.sync_status, "Renaming a category on the server: 340 of 1200 messages");
    let row = s.run("sync.progress", &json!({})).unwrap()["accounts"][0].clone();
    assert_eq!((row["phase"].as_str(), row["done"].as_u64(), row["total"].as_u64()), (Some("renaming"), Some(340), Some(1_200)), "{row}");
    // A run that isn't the end of it says nothing of it yet.
    mail_came(&mut s, a, CategoryRenamed { changed: 700, ..renamed("Holiday", "Holiday 2026") });
    assert!(!s.sync_status.contains("renamed"), "{}", s.sync_status);
    // The run's end says what it came to, with what the runs before it reached, and the
    // calendar's job says it of the events.
    mail_came(&mut s, a, CategoryRenamed { changed: 500, ..done("Holiday", "Holiday 2026") });
    assert!(s.sync_status.starts_with("The category was renamed on 1200 messages on the server"), "{}", s.sync_status);
    let mut uploaded = EventsUploaded {
        account: a,
        email: "a@example.org".into(),
        renamed: Some(CategoryRenamed { changed: 2_000, ..renamed("Holiday", "Holiday 2026") }),
        ..Default::default()
    };
    let mut taken = crate::sync_driver::CalendarTaken::default();
    s.take_in_calendar(a, Some(&uploaded), None, &mut taken);
    assert!(!format!("{:?}", taken.status).contains("renamed"), "{:?}", taken.status);
    uploaded.renamed = Some(CategoryRenamed { changed: 1, ..done("Holiday", "Holiday 2026") });
    let mut taken = crate::sync_driver::CalendarTaken::default();
    s.take_in_calendar(a, Some(&uploaded), None, &mut taken);
    assert_eq!(taken.status.len(), 1, "{:?}", taken.status);
    assert!(format!("{:?}", taken.status).contains("The category was renamed on 2001 events on the server"), "{:?}", taken.status);
    // A later rename starts its count anew.
    rename(&mut s, a, "Holiday 2026", "Trips");
    job(&mut s, a);
    mail_came(&mut s, a, CategoryRenamed { changed: 3, ..done("Holiday 2026", "Trips") });
    assert!(s.sync_status.starts_with("The category was renamed on 3 messages on the server"), "{}", s.sync_status);
}

#[test]
fn only_so_many_renames_wait_on_an_account_and_the_oldest_is_let_go() {
    let most = mailcraft_model::remote::MAX_CATEGORY_SWEEPS;
    let (mut s, a) = rig();
    let listed: Vec<Category> = (0..=most).map(|n| tag(&format!("Old {n}"), GREEN)).collect();
    read(&mut s, a, listed);
    for n in 0..=most {
        rename(&mut s, a, &format!("Old {n}"), &format!("New {n}"));
    }
    assert_eq!(s.mailbox.category_sweeps[&a].len(), most);
    assert!(!s.mailbox.sweep_holds(a, "Old 0") && s.mailbox.sweep_holds(a, "Old 1"));
}

#[test]
fn more_own_calendars_than_a_rename_names_is_said_and_the_old_name_stays() {
    let (mut s, a) = rig();
    for n in 0..=mailcraft_model::remote::MAX_RENAMED_CALENDARS {
        let own = s.mailbox.alloc_id();
        let origin = Origin { account: a, id: format!("cal-{n}"), ..Default::default() };
        let owner = Some(mailcraft_model::CalendarOwner { address: "a@example.org".into(), ..Default::default() });
        s.mailbox.calendars.push(Calendar {
            id: own,
            name: format!("Calendar {n}"),
            visible: true,
            origin,
            rights: Rights::Edit,
            owner,
            ..Default::default()
        });
    }
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    mail_came(&mut s, a, done("Holiday", "Holiday 2026"));
    assert_eq!(events_asked(&mut s, a).unwrap().calendars.len(), mailcraft_model::remote::MAX_RENAMED_CALENDARS);
    let said = events_came(&mut s, a, CategoryRenamed { changed: 3, ..done("Holiday", "Holiday 2026") });
    assert!(said.to_string().contains("the events in 1 more keep the category \\\"Holiday\\\""), "{said}");
    assert!(s.mailbox.category_of(a, "Holiday").is_some() && changes(&mut s, a).is_empty());
}

#[test]
fn a_second_rename_before_the_first_is_done_gives_everything_the_last_name() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    mail_came(&mut s, a, done("Holiday", "Holiday 2026"));
    // The events there still have "Holiday", the mail "Holiday 2026": both are to end as "Trips".
    rename(&mut s, a, "Holiday 2026", "Trips");
    job(&mut s, a);
    let ask = |from: &str| CategoryRename { from: from.into(), to: "Trips".into(), calendars: Vec::new() };
    assert_eq!(events_asked(&mut s, a), Some(ask("Holiday")));
    assert_eq!(mail_asked(&mut s, a), Some(ask("Holiday 2026")));
    // Neither old name goes while anything there may have it.
    assert!(changes(&mut s, a).is_empty(), "{:?}", changes(&mut s, a));
    events_came(&mut s, a, done("Holiday", "Trips"));
    assert_eq!(events_asked(&mut s, a), Some(ask("Holiday 2026")));
    events_came(&mut s, a, done("Holiday 2026", "Trips"));
    mail_came(&mut s, a, done("Holiday 2026", "Trips"));
    let mut gone: Vec<String> = changes(&mut s, a).into_iter().filter(|c| c.kind == CategoryChangeKind::Delete).map(|c| c.name).collect();
    gone.sort();
    assert_eq!(gone, ["Holiday", "Holiday 2026"]);
}

#[test]
fn an_undo_gives_what_the_server_renamed_the_old_name_again() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    // Part-way: some mail there has the new name, the rest the old one.
    mail_came(&mut s, a, CategoryRenamed { changed: 2_000, ..renamed("Holiday", "Holiday 2026") });
    s.run("edit.undo", &json!({})).unwrap();
    // The way forward is ended, and the way back is asked for.
    let back = CategoryRename { from: "Holiday 2026".into(), to: "Holiday".into(), calendars: Vec::new() };
    assert_eq!(mail_asked(&mut s, a), Some(back.clone()));
    assert_eq!(events_asked(&mut s, a), Some(back));
    // The new name isn't deleted there while mail there has it.
    assert!(changes(&mut s, a).iter().all(|c| c.kind != CategoryChangeKind::Delete), "{:?}", changes(&mut s, a));
    mail_came(&mut s, a, done("Holiday 2026", "Holiday"));
    events_came(&mut s, a, done("Holiday 2026", "Holiday"));
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Holiday 2026")]);
    // A redo is the rename again.
    s.run("edit.redo", &json!({})).unwrap();
    job(&mut s, a);
    assert_eq!(mail_asked(&mut s, a), Some(CategoryRename { from: "Holiday".into(), to: "Holiday 2026".into(), calendars: Vec::new() }));
}

#[test]
fn an_undo_before_the_server_had_the_new_name_asks_it_for_nothing() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!((mail_asked(&mut s, a), events_asked(&mut s, a)), (None, None));
    assert!(changes(&mut s, a).is_empty());
    assert!(!s.mailbox.sweep_holds(a, "Holiday") && !s.mailbox.sweep_holds(a, "Holiday 2026"));
}

#[test]
fn nothing_is_renamed_for_a_name_that_left_the_list_or_came_back_to_it() {
    // The new name deleted here meanwhile: the items there keep what they have.
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    s.run("category.remove", &json!({"account": a, "name": "Holiday 2026"})).unwrap();
    assert_eq!((mail_asked(&mut s, a), events_asked(&mut s, a)), (None, None));
    assert!(!s.mailbox.sweep_holds(a, "Holiday"));

    // The old name made again as a category of its own: what has it keeps it.
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    s.run("category.set", &json!({"account": a, "name": "Holiday", "color": "#2E8B57"})).unwrap();
    assert_eq!((mail_asked(&mut s, a), events_asked(&mut s, a)), (None, None));

    // Other letters only is no rename of what the server has under the name.
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "HOLIDAY");
    assert!(s.mailbox.category_sweeps.is_empty());

    // A category kept on this computer only has nothing on the server.
    let (mut s, a) = rig();
    s.mailbox.account_categories.get_mut(&a).unwrap().push(tag("Here", RED));
    s.mailbox.local_categories.entry(a).or_default().push("Here".into());
    rename(&mut s, a, "Here", "There");
    assert!(s.mailbox.category_sweeps.is_empty());
}

#[test]
fn only_the_accounts_own_calendars_that_may_be_changed_are_named() {
    let (mut s, a) = rig();
    let mut calendar = |name: &str, id: &str, rights: Rights, owner: &str| {
        let own = s.mailbox.alloc_id();
        let origin = Origin { account: a, id: id.into(), ..Default::default() };
        let owner = (!owner.is_empty()).then(|| mailcraft_model::CalendarOwner { address: owner.into(), ..Default::default() });
        s.mailbox.calendars.push(Calendar { id: own, name: name.into(), visible: true, origin, rights, owner, ..Default::default() });
    };
    calendar("Calendar", "", Rights::Edit, "a@example.org");
    calendar("Projects", "cal-projects", Rights::Edit, "a@example.org");
    calendar("Rota", "cal-rota", Rights::Read, "a@example.org");
    calendar("A colleague's", "cal-theirs", Rights::Edit, "colleague@example.org");
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    assert_eq!(events_asked(&mut s, a).unwrap().calendars, ["cal-projects"]);
}

#[test]
fn a_shared_mailbox_is_asked_for_nothing_and_another_accounts_list_is_left_alone() {
    let (mut s, a) = rig();
    let shared = s.run("account.addSharedMailbox", &json!({"account": a, "address": "team@example.org"})).unwrap()["id"].as_u64().unwrap();
    let b = s.run("account.add", &json!({"email": "b@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    assert!(mail_asked(&mut s, a).is_some());
    assert_eq!((mail_asked(&mut s, shared), events_asked(&mut s, shared)), (None, None));
    assert_eq!((mail_asked(&mut s, b), events_asked(&mut s, b)), (None, None));
    // Removing the account ends what waited for it.
    s.run("account.remove", &json!({"id": a})).unwrap();
    assert!(s.mailbox.category_sweeps.is_empty());
}

#[test]
fn without_the_calendars_permission_the_events_wait_for_it_and_the_old_name_stays() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    mail_came(&mut s, a, done("Holiday", "Holiday 2026"));
    assert!(changes(&mut s, a).is_empty());
    // The owner hasn't let the calendar be read: said once, and the old name isn't deleted.
    s.driver.calendar.consent_needed.insert(a);
    let look = s.calendar_due(&facts(), 1_000.0);
    assert_eq!(look.said.len(), 1, "{:?}", look.said);
    assert!(look.said[0].1.contains("events in Outlook keep the category \"Holiday\""), "{:?}", look.said);
    assert!(look.plans.is_empty() && s.calendar_due(&facts(), 2_000.0).said.is_empty());
    // (Nor does an answer that says so end it.)
    let consent = crate::sync_driver::CalendarCame { consent_needed: true, ..Default::default() };
    s.calendar_answered(a, false, 2_000.0, 2_500.0, &consent);
    assert!(changes(&mut s, a).is_empty() && s.mailbox.category_of(a, "Holiday").is_none());
    // The permission comes: the events are asked for, and then the old name goes.
    s.driver.calendar.consent_needed.remove(&a);
    assert!(events_asked(&mut s, a).is_some());
    events_came(&mut s, a, done("Holiday", "Holiday 2026"));
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Holiday")]);
}

fn facts() -> crate::sync_driver::CalendarFacts {
    crate::sync_driver::CalendarFacts { focused: true, backstage: false, reachable: true, busy: Default::default() }
}

#[test]
fn the_calendars_job_carries_a_rename_by_itself_after_a_quiet_moment() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    // Seen waiting now; sent once it has waited out the quiet time, with nothing else to send.
    s.calendar_due(&facts(), 1_000.0);
    let look = s.calendar_due(&facts(), 1_000.0 + 3_600_000.0);
    let plan = look.plans.iter().find(|plan| plan.account == a).expect("a job for the account");
    let upload = plan.changes.as_ref().expect("it carries the rename up");
    assert!(plan.push && upload.ops.is_empty());
    assert_eq!(upload.rename.as_ref().map(|r| (r.from.as_str(), r.to.as_str())), Some(("Holiday", "Holiday 2026")));
}

#[test]
fn an_agents_question_says_that_outlooks_mail_and_events_get_the_new_name() {
    let (s, a) = rig();
    match check(&s, "category.rename", &json!({"account": a, "from": "Holiday", "to": "Holiday 2026"}), &[]) {
        Verdict::Ask(Class::Send, text) => {
            assert!(
                text.contains("the mail and events there that have the old one get it") && text.contains("the old one is deleted there"),
                "{text}"
            )
        }
        other => panic!("{other:?}"),
    }
    // Other letters alone leave the mail and events there as they are: nothing is promised of them.
    match check(&s, "category.rename", &json!({"account": a, "from": "Holiday", "to": " HOLI\u{7}DAY "}), &[]) {
        Verdict::Ask(_, text) => assert!(text.contains("the new spelling is made there") && !text.contains("mail and events"), "{text}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_rename_that_waits_for_the_servers_events_doesnt_keep_the_days_round_a_meeting_from_being_looked_at() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    assert!(events_asked(&mut s, a).is_some());
    let day = mailcraft_time::Date { year: 2026, month: 11, day: 19 };
    assert!(s.calendar_around(a, day, false).is_ok());
}

#[test]
fn items_the_server_keeps_answering_not_now_for_are_given_up_after_a_few_runs_and_said() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    mail_came(&mut s, a, done("Holiday", "Holiday 2026"));
    let stuck = || CategoryRenamed { stuck: 3, ..renamed("Holiday", "Holiday 2026") };
    let tries = mailcraft_model::remote::MAX_RENAME_TRIES;
    // A run that changed something starts the count again.
    for _ in 1..tries {
        events_came(&mut s, a, stuck());
    }
    events_came(&mut s, a, CategoryRenamed { changed: 1, ..stuck() });
    for _ in 1..tries {
        let said = events_came(&mut s, a, stuck());
        assert!(!said.to_string().contains("Holiday"), "{said}");
        assert!(events_asked(&mut s, a).is_some() && changes(&mut s, a).is_empty());
    }
    // A lost connection or a busy server (nothing answered) is no such run, and doesn't start
    // the count again either.
    events_came(&mut s, a, renamed("Holiday", "Holiday 2026"));
    assert!(events_asked(&mut s, a).is_some() && changes(&mut s, a).is_empty());
    let said = events_came(&mut s, a, stuck());
    assert!(said.to_string().contains("to 3 of the events that have \\\"Holiday\\\""), "{said}");
    assert!(said.to_string().contains("\\\"Holiday\\\" isn't deleted, and is back on the list"), "{said}");
    assert_eq!(events_asked(&mut s, a), None);
    assert!(changes(&mut s, a).is_empty() && s.mailbox.category_of(a, "Holiday").is_some());
}

#[test]
fn a_rename_that_got_nowhere_is_a_failed_job_and_waits_longer() {
    use crate::sync_driver::{CalendarCame, Pacing};
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    let answer = |s: &mut Session, came: CategoryRenamed, at: f64| {
        let uploaded = EventsUploaded { account: a, email: "a@example.org".into(), renamed: Some(came), ..Default::default() };
        // (The days asked for came down with it: the job itself worked.)
        let events =
            mailcraft_model::remote::EventsDownload { account: a, email: "a@example.org".into(), from: NOW, to: NOW + 86_400, ..Default::default() };
        let taken = s.calendar_answered(a, true, at, at, &CalendarCame { uploaded: Some(&uploaded), events: Some(&events), ..Default::default() });
        assert!(taken.taken && taken.errors.is_empty(), "{:?}", taken.errors);
    };
    // Nothing changed and not done: a failure, as an upload nobody answered is.
    answer(&mut s, CategoryRenamed { stuck: 2, ..renamed("Holiday", "Holiday 2026") }, 1_000.0);
    assert!(!s.driver.calendar.book.patient(&Pacing::CALENDAR, a, 1_001.0));
    // A part done: it worked, and the rest goes on without a longer wait.
    answer(&mut s, CategoryRenamed { changed: 2_000, ..renamed("Holiday", "Holiday 2026") }, 2_000.0);
    assert!(s.driver.calendar.book.patient(&Pacing::CALENDAR, a, 2_001.0));
}

#[test]
fn an_agents_undo_of_a_rename_that_reached_outlook_says_what_it_does_to_the_mail_and_events_there() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    match check(&s, "edit.undo", &json!({}), &[]) {
        Verdict::Ask(_, text) => assert!(
            text.contains(
                "gives the mail and events in Outlook of \u{201c}a@example.org\u{201d} that have the category \u{201c}Holiday 2026\u{201d} the name \u{201c}Holiday\u{201d}"
            ),
            "{text}"
        ),
        other => panic!("an agent's undo renames mail and events in Outlook unasked: {other:?}"),
    }
    // The list may not be changed there just now: the rename back waits and goes when it may
    // again, so it is asked for all the same.
    s.run("sync.categoryWrites", &json!({"account": a, "allowed": false})).unwrap();
    assert!(!s.undo_renames_on_server(false).is_empty());
    assert!(matches!(check(&s, "edit.undo", &json!({}), &[Class::Delete]), Verdict::Ask(..)));
    // One undone before Outlook had the new name does nothing there.
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    assert!(s.undo_renames_on_server(false).is_empty());
    match check(&s, "edit.undo", &json!({}), &[]) {
        Verdict::Ask(_, text) => assert!(!text.contains("mail and events in Outlook"), "{text}"),
        Verdict::Run => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_server_that_keeps_failing_at_the_question_is_given_up_and_said() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    events_came(&mut s, a, done("Holiday", "Holiday 2026"));
    let failed = || CategoryRenamed { failed: true, ..renamed("Holiday", "Holiday 2026") };
    for _ in 1..mailcraft_model::remote::MAX_RENAME_TRIES {
        let said = mail_came(&mut s, a, failed());
        assert!(!said.to_string().contains("Holiday"), "{said}");
        assert!(mail_asked(&mut s, a).is_some() && changes(&mut s, a).is_empty());
    }
    // What it could name the last time is said with it.
    let said = mail_came(&mut s, a, CategoryRenamed { stuck: 2, ..failed() });
    assert!(said.to_string().contains("to 2 of the messages that have"), "{said}");
    assert!(said.to_string().contains("kept failing to say which messages have the category \\\"Holiday\\\""), "{said}");
    assert_eq!(mail_asked(&mut s, a), None);
    assert!(changes(&mut s, a).is_empty() && s.mailbox.category_of(a, "Holiday").is_some());
}

#[test]
fn an_agents_undo_that_renames_outlooks_mail_and_events_is_a_send() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    // Outlook has both names while the rename is under way: the undo deletes one there and
    // has the mail and events given the other, which a yes to deleting alone doesn't cover.
    assert_eq!(s.undo_renames_on_server(false), [(a, "Holiday 2026".to_string(), "Holiday".to_string())]);
    for allowed in [vec![], vec![Class::Delete]] {
        assert!(matches!(check(&s, "edit.undo", &json!({}), &allowed), Verdict::Ask(..)), "{allowed:?}");
    }
    assert_eq!(s.undo_reverts(false), Some(Class::Send));
    assert!(matches!(check(&s, "edit.undo", &json!({}), &[Class::Send, Class::Delete]), Verdict::Run));
    // The same for the redo of it.
    s.run("edit.undo", &json!({})).unwrap();
    job(&mut s, a);
    assert!(!s.undo_renames_on_server(true).is_empty());
    assert!(matches!(check(&s, "edit.redo", &json!({}), &[Class::Delete]), Verdict::Ask(..)));
}

#[test]
fn a_second_rename_taken_back_leaves_the_first_one_waiting() {
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    rename(&mut s, a, "Holiday 2026", "Trips");
    s.run("edit.undo", &json!({})).unwrap();
    // The list has "Holiday 2026" again: it is made in Outlook, and "Holiday" stays there until
    // the mail and events that have it are given the new name.
    assert_eq!(job(&mut s, a), [change(CategoryChangeKind::Create, "Holiday 2026")]);
    assert_eq!(mail_asked(&mut s, a), Some(CategoryRename { from: "Holiday".into(), to: "Holiday 2026".into(), calendars: Vec::new() }));
    assert!(changes(&mut s, a).is_empty(), "the old name is deleted before the server's items have the new one");
    mail_came(&mut s, a, done("Holiday", "Holiday 2026"));
    events_came(&mut s, a, done("Holiday", "Holiday 2026"));
    assert_eq!(changes(&mut s, a), [change(CategoryChangeKind::Delete, "Holiday")]);
    // Both taken back and made again: everything ends on the last name.
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    rename(&mut s, a, "Holiday 2026", "Trips");
    for step in ["edit.undo", "edit.undo", "edit.redo", "edit.redo"] {
        s.run(step, &json!({})).unwrap();
    }
    assert_eq!(job(&mut s, a), [change(CategoryChangeKind::Create, "Trips")]);
    assert_eq!(mail_asked(&mut s, a), Some(CategoryRename { from: "Holiday".into(), to: "Trips".into(), calendars: Vec::new() }));
    assert!(changes(&mut s, a).is_empty());
}

#[test]
fn an_undo_that_starts_a_stopped_rename_again_is_a_send_and_what_was_passed_over_is_said() {
    // The old name made again stops the rename; taking that back starts it again.
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    s.run("category.set", &json!({"account": a, "name": "Holiday", "color": "#2E8B57"})).unwrap();
    assert_eq!(mail_asked(&mut s, a), None);
    assert!(!s.undo_renames_on_server(false).is_empty());
    assert_eq!(s.undo_reverts(false), Some(Class::Send));
    assert!(matches!(check(&s, "edit.undo", &json!({}), &[Class::Delete]), Verdict::Ask(..)));
    s.run("edit.undo", &json!({})).unwrap();
    assert!(mail_asked(&mut s, a).is_some());

    // Items left as they are because their list can't be written back whole are said as that.
    let (mut s, a) = rig();
    rename(&mut s, a, "Holiday", "Holiday 2026");
    job(&mut s, a);
    let said = mail_came(&mut s, a, CategoryRenamed { passed: 2, ..done("Holiday", "Holiday 2026") }).to_string();
    assert!(said.contains("MailCraft left 2 of the messages that have the category \\\"Holiday\\\" as they are in Outlook"), "{said}");
    assert!(!said.contains("wouldn't"), "{said}");
}
