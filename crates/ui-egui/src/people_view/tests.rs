// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People: the list, the card and the small pieces they are made of.

use mailcraft_model::{Labeled, Message};

use super::*;

fn contact(first: &str, last: &str) -> Contact {
    Contact { first: first.into(), last: last.into(), ..Default::default() }
}

fn day(y: i32, m: u8, d: u8) -> Date {
    Date::new(y, m, d).unwrap()
}

#[test]
fn groups_come_from_the_name_the_list_sorts_by() {
    assert_eq!(group_of(true, "Chen, Maya"), Group::Favorites);
    assert_eq!(group_of(false, "Chen, Maya"), Group::Letter('C'));
    assert_eq!(group_of(false, "  okafor"), Group::Letter('O'));
    assert_eq!(group_of(false, "élan"), Group::Letter('É'));
    // A letter whose capital is two letters (ß → SS) still gives one.
    assert_eq!(group_of(false, "ßtraße"), Group::Letter('S'));
    for odd in ["", "   ", "42 Wallaby Way", "@home", "\u{0}", "(Acme)"] {
        assert_eq!(group_of(false, odd), Group::Other, "{odd:?}");
    }
    assert_eq!(group_label(Group::Favorites, &[]), "Favorites");
    assert_eq!(group_label(Group::Letter('C'), &[]), "C");
    assert_eq!(group_label(Group::Other, &[]), "#");
}

#[test]
fn rows_are_offset_by_the_headings_above_them() {
    let l =
        Layout::new(ROW_H, [(1, Group::Favorites), (2, Group::Favorites), (3, Group::Letter('A')), (4, Group::Letter('B')), (5, Group::Letter('B'))]);
    let tops: Vec<f32> = l.rows.iter().map(|r| r.top).collect();
    assert_eq!(tops, [GROUP_H, GROUP_H + ROW_H, 2.0 * GROUP_H + 2.0 * ROW_H, 3.0 * GROUP_H + 3.0 * ROW_H, 3.0 * GROUP_H + 4.0 * ROW_H]);
    assert_eq!(l.height, 3.0 * GROUP_H + 5.0 * ROW_H);
    let headings: Vec<Option<Group>> = l.rows.iter().map(|r| r.heading).collect();
    assert_eq!(headings, [Some(Group::Favorites), None, Some(Group::Letter('A')), Some(Group::Letter('B')), None]);
    let empty = Layout::<Group>::new(ROW_H, []);
    assert_eq!((empty.rows.len(), empty.height), (0, 0.0));
    assert!(empty.visible(0.0, 900.0).is_empty());
    assert_eq!(empty.neighbour(Some(7), 1), None);
}

#[test]
fn only_the_rows_in_view_are_picked() {
    let l = Layout::new(ROW_H, (1..=50_000u64).map(|id| (id, Group::Letter('A'))));
    assert_eq!(l.height, GROUP_H + 50_000.0 * ROW_H);
    let ids = |min: f32, max: f32| l.visible(min, max).iter().map(|r| r.id).collect::<Vec<_>>();
    // The top of the list: the heading and the rows a 200 px window reaches.
    assert_eq!(ids(0.0, 200.0), [1, 2, 3, 4]);
    // Far down: a screenful, not the whole list.
    let far = l.visible(1_000_000.0, 1_000_900.0);
    assert!((17..=20).contains(&far.len()), "{}", far.len());
    assert!(far.iter().all(|r| r.top + ROW_H >= 1_000_000.0 && r.top - GROUP_H <= 1_000_900.0));
    // Windows off either end, upside down or not numbers at all are empty, never a panic.
    assert!(ids(l.height + 100.0, l.height + 900.0).is_empty());
    assert!(ids(-900.0, -100.0).is_empty());
    assert!(ids(500.0, 100.0).is_empty());
    let _ = ids(f32::NAN, f32::NAN);
    let _ = ids(f32::NEG_INFINITY, f32::INFINITY);
}

#[test]
fn the_keyboard_moves_through_the_rows_and_stops_at_the_ends() {
    let l = Layout::new(ROW_H, [(10, Group::Favorites), (20, Group::Letter('A')), (30, Group::Letter('B'))]);
    assert_eq!(l.neighbour(Some(10), 1), Some(20));
    assert_eq!(l.neighbour(Some(20), -1), Some(10));
    assert_eq!(l.neighbour(Some(10), -1), Some(10));
    assert_eq!(l.neighbour(Some(30), 1), Some(30));
    assert_eq!(l.neighbour(Some(10), i64::MAX), Some(30));
    assert_eq!(l.neighbour(Some(30), i64::MIN), Some(10));
    // Nothing selected, or a selection the search has filtered out.
    assert_eq!(l.neighbour(None, 1), Some(10));
    assert_eq!(l.neighbour(None, -1), Some(30));
    assert_eq!(l.neighbour(Some(99), 1), Some(10));
}

#[test]
fn rows_say_who_and_where() {
    let mut c = contact("Maya", "Chen");
    c.company = "Lumen Studio".into();
    c.emails = vec![Labeled::new("work", ""), Labeled::new("home", "maya@lumen.example")];
    assert_eq!(shown_name(&c), "Maya Chen");
    assert_eq!(subtitle(&c), "Lumen Studio · maya@lumen.example");
    assert_eq!(avatar_address(&c), Address::new("Maya Chen", "maya@lumen.example"));
    c.title = "Design Lead".into();
    c.department = " ".into();
    assert_eq!(role_line(&c), "Design Lead · Lumen Studio");
    // A company, or an address, standing in for the name isn't said twice.
    let org = Contact { company: "Harbor Books".into(), emails: vec![Labeled::new("work", "hello@harborbooks.example")], ..Default::default() };
    assert_eq!((shown_name(&org).as_str(), subtitle(&org).as_str()), ("Harbor Books", "hello@harborbooks.example"));
    assert_eq!(role_line(&org), "");
    assert_eq!(role_line(&Contact { department: "Orders".into(), ..org.clone() }), "Orders");
    let bare = Contact { emails: vec![Labeled::new("", "grace.hopper@example.org")], ..Default::default() };
    assert_eq!((shown_name(&bare).as_str(), subtitle(&bare).as_str()), ("grace.hopper@example.org", ""));
    assert_eq!(avatar_address(&bare).initials(), "GH");
    // Writing to them: the name with the address, or the address alone when that is the name.
    assert_eq!(recipient(&c, " maya@home.example "), "Maya Chen <maya@home.example>");
    assert_eq!(recipient(&bare, "grace.hopper@example.org"), "grace.hopper@example.org");
    assert_eq!(recipient(&org, "orders@harborbooks.example"), "Harbor Books <orders@harborbooks.example>");
    assert_eq!(recipient(&Contact::default(), ""), "");
}

#[test]
fn hostile_names_stay_on_one_line() {
    let empty = Contact::default();
    assert_eq!(shown_name(&empty), "(no name)");
    assert_eq!((subtitle(&empty).as_str(), role_line(&empty).as_str()), ("", ""));
    assert_eq!(avatar_address(&empty), Address::default());
    assert_eq!(avatar_address(&empty).initials(), "");
    let odd = Contact { display_name: "Line\none\ttwo\u{0}".into(), company: "A\r\nB".into(), ..Default::default() };
    assert_eq!(shown_name(&odd), "Line one two");
    assert_eq!(subtitle(&odd), "A B");
    assert_eq!(clip_chars("héllo wörld", 5), "héllo…");
    assert_eq!(clip_chars("héllo", 5), "héllo");
    assert_eq!(clip_chars("", 0), "");
    assert_eq!(clip_chars("abc", 0), "…");
    assert_eq!(clip_chars(&"é".repeat(100_000), 3), "ééé…");
    // However long the text in a file is, a line of the list lays out a bounded piece of it.
    assert_eq!(one_line(&"x".repeat(1_000_000), 10), format!("{}…", "x".repeat(9)));
    assert_eq!(one_line("  a\r\nb\u{7}  ", 100), "a b");
    assert_eq!(one_line("\n\n", 100), "");
    let long = Contact { display_name: "n".repeat(100_000), company: "c".repeat(100_000), title: "t".repeat(100_000), ..Default::default() };
    assert!(shown_name(&long).chars().count() <= 200 && subtitle(&long).chars().count() <= 300 && role_line(&long).chars().count() <= 400);
}

#[test]
fn labels_read_like_words() {
    assert_eq!(label_text("work"), "Work");
    assert_eq!(label_text(" MOBILE "), "Mobile");
    assert_eq!(label_text("assistant"), "Assistant");
    assert_eq!(label_text("élan"), "Élan");
    assert_eq!(label_text(""), "");
    assert_eq!(label_text("a\nb"), "Ab");
    assert_eq!(label_text(&"x".repeat(500)).chars().count(), 40);
}

#[test]
fn only_plain_web_addresses_open() {
    assert_eq!(web_target("https://harborworks.example/a?b=1").as_deref(), Some("https://harborworks.example/a?b=1"));
    assert_eq!(web_target(" HTTP://example.org ").as_deref(), Some("HTTP://example.org"));
    assert_eq!(web_target("harborworks.example/team").as_deref(), Some("https://harborworks.example/team"));
    for bad in [
        "",
        "   ",
        "localhost",
        "javascript:alert(1)",
        "javascript://example.org/%0aalert(1)",
        "file:///C:/Windows/system32/calc.exe",
        "data:text/html,<script>1</script>",
        "mailto:a@example.org",
        "example.org:8080",
        "https://",
        "https://[::1",
        "https://[zz]/",
        "https://exa mple.org",
        "https://example.org/\u{0}",
        "ms-settings://x",
    ] {
        assert_eq!(web_target(bad), None, "{bad:?}");
    }
}

#[test]
fn birthdays_read_well_and_survive_bad_dates() {
    assert_eq!(birthday_text(day(1988, 3, 14)), "March 14, 1988");
    assert_eq!(birthday_text(day(9999, 12, 31)), "December 31, 9999");
    // A date a damaged file could carry: the fields are public.
    assert_eq!(birthday_text(Date { year: -5, month: 0, day: 0 }), "-005-00-00");
    assert_eq!(birthday_text(Date { year: i32::MAX, month: 13, day: 255 }), format!("{}-13-255", i32::MAX));

    let today = day(2026, 10, 14);
    assert_eq!(next_birthday(day(1988, 3, 14), today), Some((day(2027, 3, 14), 39)));
    assert_eq!(next_birthday(day(1988, 12, 1), today), Some((day(2026, 12, 1), 38)));
    assert_eq!(next_birthday(day(1988, 10, 14), today), Some((today, 38)));
    // February 29th falls on the 28th outside leap years.
    assert_eq!(next_birthday(day(2000, 2, 29), today), Some((day(2027, 2, 28), 27)));
    assert_eq!(next_birthday(day(2000, 2, 29), day(2028, 1, 1)), Some((day(2028, 2, 29), 28)));
    // No believable age: born this year, in the future, or a card without a real year.
    assert_eq!(next_birthday(day(2026, 12, 1), today), None);
    assert_eq!(next_birthday(day(9999, 12, 31), today), None);
    assert_eq!(next_birthday(day(1, 1, 1), today), None);
    // Clocks and dates at the edges of what a date can hold.
    assert_eq!(next_birthday(day(9990, 1, 1), day(9999, 12, 31)), None);
    assert_eq!(next_birthday(day(9990, 12, 31), day(9999, 12, 31)), Some((day(9999, 12, 31), 9)));
    assert_eq!(next_birthday(Date { year: 1990, month: 0, day: 0 }, today), None);
    assert_eq!(next_birthday(Date { year: i32::MIN, month: 200, day: 200 }, Date { year: i32::MAX, month: 12, day: 31 }), None);
}

#[test]
fn recent_mail_is_the_newest_from_or_to_any_address() {
    let mut mb = Mailbox::with_local_account("Me", "me@example.org");
    let inbox = mb.inbox().unwrap();
    let mut add = |from: &str, to: &str, cc: &str, date: i64| {
        let list = |s: &str| if s.is_empty() { Vec::new() } else { vec![Address::new("", s)] };
        mb.add_message(Message { folder: inbox, from: Address::new("", from), to: list(to), cc: list(cc), date, ..Default::default() }).unwrap()
    };
    let a = add("dev@patel.example", "me@example.org", "", 100);
    let b = add("me@example.org", "DEV@Patel.example", "", 300);
    let c = add("someone@example.org", "me@example.org", " dev@home.example ", 200);
    add("someone@example.org", "me@example.org", "", 400);
    // A message without a sender or recipients, for the contact without an address below.
    add("", "", "", 500);
    let mut dev = contact("Dev", "Patel");
    dev.emails = vec![Labeled::new("work", " dev@patel.example"), Labeled::new("home", "dev@home.example"), Labeled::new("old", "  ")];
    assert_eq!(recent_mail(&mb, &dev, 8), [b, c, a]);
    assert_eq!(recent_mail(&mb, &dev, 2), [b, c]);
    assert!(recent_mail(&mb, &dev, 0).is_empty());
    // No address, no mail: an empty address must not match the message nobody sent.
    assert!(recent_mail(&mb, &contact("No", "Mail"), 8).is_empty());
    assert!(recent_mail(&mb, &Contact::default(), 8).is_empty());
}

#[test]
fn the_contact_forms_delete_asks_first() {
    // #186: Delete in a contact's form gives way to the question, and only its answer deletes.
    let mut w = testing::window("view.people");
    let id = people::sorted(&w.state().session, "")[0];
    w.state_mut().run("people.select", serde_json::json!({"id": id})).unwrap();
    w.run_steps(2);
    testing::key(&mut w, egui::Key::Enter);
    assert_eq!(testing::dialog(&w), Some("editContact"));
    use egui_kittest::kittest::Queryable;
    let press = |w: &mut egui_kittest::Harness<'_, MailApp>| {
        // (A window that has just opened finds its place first.)
        w.run_steps(5);
        let pos = w.query_all_by_label("Delete").last().expect("a Delete button").rect().center();
        testing::click(w, pos, egui::PointerButton::Primary, 1);
    };
    press(&mut w);
    assert_eq!(testing::dialog(&w), Some("confirm"));
    assert!(w.state().session.mailbox.contact(id).is_some());
    press(&mut w);
    assert_eq!(testing::dialog(&w), None);
    assert!(w.state().session.mailbox.contact(id).is_none());
}

#[test]
fn clicks_and_keys_work_the_list() {
    use egui::{Key, PointerButton};
    let mut w = testing::window("view.people");
    let ids = people::sorted(&w.state().session, "");
    let at = |w: &egui_kittest::Harness<'_, MailApp>, id: Id| w.state().hit.rect("contact", id).unwrap().center();
    let selected = |w: &egui_kittest::Harness<'_, MailApp>| w.state().session.sel.contact;
    assert!(ids.len() >= 4 && selected(&w).is_none());

    // A click selects; Up and Down walk the list in its order and stop at its ends.
    let pos = at(&w, ids[1]);
    testing::click(&mut w, pos, PointerButton::Primary, 1);
    assert_eq!(selected(&w), Some(ids[1]));
    testing::key(&mut w, Key::ArrowUp);
    testing::key(&mut w, Key::ArrowUp);
    assert_eq!(selected(&w), Some(ids[0]));
    testing::key(&mut w, Key::ArrowDown);
    testing::key(&mut w, Key::ArrowDown);
    assert_eq!(selected(&w), Some(ids[2]));

    // Enter opens the contact; while its form is up, the keys are the form's.
    testing::key(&mut w, Key::Enter);
    assert_eq!(testing::dialog(&w), Some("editContact"));
    testing::key(&mut w, Key::ArrowDown);
    assert_eq!(selected(&w), Some(ids[2]));
    w.state_mut().dialog = None;
    w.step();

    // A double click selects the row and opens it.
    let pos = at(&w, ids[3]);
    testing::click(&mut w, pos, PointerButton::Primary, 2);
    assert_eq!((selected(&w), testing::dialog(&w)), (Some(ids[3]), Some("editContact")));
    w.state_mut().dialog = None;
    w.step();

    // A right click selects the row under the pointer and opens its menu, which keeps the keys.
    let pos = at(&w, ids[0]);
    testing::click(&mut w, pos, PointerButton::Secondary, 1);
    assert_eq!(selected(&w), Some(ids[0]));
    let ctx = w.state().ctx.clone().unwrap();
    assert!(egui::Popup::is_any_open(&ctx));
    testing::key(&mut w, Key::ArrowDown);
    assert_eq!(selected(&w), Some(ids[0]));
}

#[test]
fn the_search_narrows_the_list_and_the_keys_stay_inside_it() {
    let mut w = testing::window("view.people");
    let all = people::sorted(&w.state().session, "");
    w.state_mut().execute("people.select", json!({"id": all[0]})).unwrap();
    w.state_mut().execute("people.search", json!({"query": "harbor"})).unwrap();
    w.run_steps(2);
    let found = people::sorted(&w.state().session, "harbor");
    assert!(!found.is_empty() && found.len() < all.len() && !found.contains(&all[0]));
    // Only what matches is drawn.
    for id in &all {
        assert_eq!(w.state().hit.rect("contact", *id).is_some(), found.contains(id), "{id}");
    }
    // The selection is outside the list: Down starts at its top.
    testing::key(&mut w, egui::Key::ArrowDown);
    assert_eq!(w.state().session.sel.contact, found.first().copied());
    // Nothing matches: nothing is drawn, and the keys do nothing.
    w.state_mut().execute("people.search", json!({"query": "zzzz"})).unwrap();
    w.run_steps(2);
    assert!(all.iter().all(|id| w.state().hit.rect("contact", *id).is_none()));
    testing::key(&mut w, egui::Key::ArrowDown);
    assert_eq!(w.state().session.sel.contact, found.first().copied());
}

#[test]
fn the_list_is_sorted_again_only_when_it_changes() {
    let mut w = testing::window("view.people");
    let ctx = w.state().ctx.clone().unwrap();
    let first = list_layout(w.state(), &ctx);
    let last = first.rows.last().unwrap().id;
    assert_eq!(first.rows.len(), w.state().session.mailbox.contacts.len());
    // More frames, a selection, a change somewhere else in the mailbox's view: the same list.
    w.state_mut().execute("people.select", json!({"id": last})).unwrap();
    w.run_steps(3);
    assert!(Arc::ptr_eq(&first, &list_layout(w.state(), &ctx)));
    // A contact changes: it moves up under Favorites.
    w.state_mut().execute("people.favorite", json!({"id": last})).unwrap();
    let starred = list_layout(w.state(), &ctx);
    assert!(!Arc::ptr_eq(&first, &starred));
    assert!(starred.rows.iter().take_while(|r| r.id != last).all(|r| r.heading != Some(Group::Letter('B'))));
    assert_ne!(starred.rows.last().map(|r| r.id), Some(last));
    // Undo puts the list back; a search narrows it; clearing the search brings it back.
    w.state_mut().execute("edit.undo", json!({})).unwrap();
    assert_eq!(*list_layout(w.state(), &ctx), *first);
    w.state_mut().execute("people.search", json!({"query": "harbor"})).unwrap();
    assert!(list_layout(w.state(), &ctx).rows.len() < first.rows.len());
    w.state_mut().execute("people.search", json!({"query": ""})).unwrap();
    assert_eq!(*list_layout(w.state(), &ctx), *first);
    // Recent mail is looked up again only when the mailbox changes, too.
    let c = w.state().session.mailbox.contact(last).cloned().unwrap();
    let recent = recent_for(w.state(), &ctx, &c);
    assert!(Arc::ptr_eq(&recent, &recent_for(w.state(), &ctx, &c)));
    assert_eq!(*recent, recent_mail(&w.state().session.mailbox, &c, RECENT));
}

#[test]
fn a_very_long_list_paints_only_what_shows() {
    let mut mb = Mailbox::with_local_account("Me", "me@example.org");
    for i in 0..50_000u32 {
        mb.add_contact(Contact {
            first: format!("Person {i:05}"),
            last: format!("Family {:03}", i % 700),
            emails: vec![Labeled::new("work", format!("p{i}@example.org"))],
            favorite: i % 5000 == 0,
            ..Default::default()
        });
    }
    let mut w = testing::window_on(mb, "view.people");
    let ids = people::sorted(&w.state().session, "");
    assert_eq!(ids.len(), 50_000);
    let drawn = |w: &egui_kittest::Harness<'_, MailApp>| ids.iter().filter(|id| w.state().hit.rect("contact", **id).is_some()).count();
    assert!((5..40).contains(&drawn(&w)), "{}", drawn(&w));
    assert!(w.state().hit.rect("contact", ids[0]).is_some());
    // The far end of the list: selecting it scrolls it into view, and still only a screenful
    // is painted.
    let last = ids[49_999];
    w.state_mut().execute("people.select", json!({"id": last})).unwrap();
    w.run_steps(40);
    assert!(w.state().hit.rect("contact", last).is_some());
    assert!(w.state().hit.rect("contact", ids[0]).is_none());
    assert!((5..40).contains(&drawn(&w)), "{}", drawn(&w));
    // And back, a row at a time from the keyboard.
    testing::key(&mut w, egui::Key::ArrowUp);
    assert_eq!(w.state().session.sel.contact, Some(ids[49_998]));
}

// ---- a name as it is shown (#454): what shows nothing is gone, what is part of a word or an
// emoji stays, each line cut at its own length ----

fn named(name: &str) -> Contact {
    Contact { display_name: name.into(), ..Default::default() }
}

#[test]
fn a_contacts_name_loses_what_shows_nothing() {
    // A right-to-left override, a zero-width space, a Hangul filler and tags.
    assert_eq!(shown_name(&named("Maya\u{202e} \u{3164}Chen\u{200b}\u{e0041}\u{e0042}")), "Maya Chen");
    // Spaces, tabs and line breaks run together into one space.
    assert_eq!(shown_name(&named("  Maya \t\r\n  Chen  ")), "Maya Chen");
    // An emoji made with a joiner, and one variation selector after a heart, stay.
    assert_eq!(shown_name(&named("Ada 👩\u{200d}💻")), "Ada 👩\u{200d}💻");
    assert_eq!(shown_name(&named("Ada ❤\u{fe0f}\u{fe0f}\u{fe0f}")), "Ada ❤\u{fe0f}");
    // A joiner that joins nothing goes.
    assert_eq!(shown_name(&named("\u{200d}Ada\u{200d} Chen")), "Ada Chen");
}

#[test]
fn a_name_of_nothing_that_shows_is_no_name() {
    for name in ["\u{3164}\u{200b}\u{2066}", "\u{2800}\u{feff}\u{e0041}", "\u{fe0f}\u{200d}"] {
        assert_eq!(shown_name(&named(name)), "(no name)", "{name:?}");
    }
}

#[test]
fn each_line_is_cut_at_its_own_length_with_the_ellipsis_inside() {
    assert_eq!(shown_name(&named(&"n".repeat(500))), format!("{}…", "n".repeat(199)));
    assert_eq!(shown_name(&named(&"n".repeat(200))), "n".repeat(200));
    let long = Contact { company: "c".repeat(500), title: "t".repeat(500), ..named("Ada") };
    assert_eq!(subtitle(&long), format!("{}…", "c".repeat(299)));
    assert_eq!(role_line(&long).chars().count(), 400);
    assert!(role_line(&long).ends_with('…'));
}

#[test]
fn the_lines_under_the_name_are_cleaned_too() {
    let c = Contact {
        company: "Harbor\u{200b} Books\u{202e}".into(),
        title: "Buyer\u{e0041}".into(),
        emails: vec![Labeled::new("work", "orders@harborbooks.example\u{feff}")],
        ..named("Maya Chen")
    };
    assert_eq!(subtitle(&c), "Harbor Books · orders@harborbooks.example");
    assert_eq!(role_line(&c), "Buyer · Harbor Books");
}
