// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;

use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Window = egui_kittest::Harness<'static, MailApp>;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app
}

/// A run starts on Home, and File opens on Info every time, as in Outlook: where the owner
/// last was is neither kept in the preferences nor while File is closed (#119).
#[test]
fn the_ribbon_opens_on_home_and_file_on_info() {
    let mut a = app();
    a.execute("ui.tab", json!({"tab": "View"})).unwrap();
    a.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    let saved: crate::UiState = serde_json::from_value(serde_json::to_value(a.prefs()).unwrap()).unwrap();
    let mut b = app();
    b.apply_prefs(saved);
    assert_eq!((b.ui.tab.as_str(), b.ui.backstage_page.as_str(), b.ui.backstage), ("Home", "info", false));
    // Within a run: a tab stays chosen, File's page doesn't once File is closed.
    b.execute("ui.tab", json!({"tab": "View"})).unwrap();
    assert_eq!(b.ui.tab, "View");
    b.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    assert_eq!(b.ui.backstage_page, "options");
    // Another page while it is open is kept.
    b.execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    assert_eq!(b.ui.backstage_page, "signatures");
    for open in [json!({"value": true}), json!({})] {
        b.execute("ui.backstage", json!({"value": false})).unwrap();
        assert_eq!(b.execute("ui.backstage", open).unwrap()["page"], "info");
    }
    b.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    b.execute("ui.tab", json!({"tab": "Home"})).unwrap();
    b.execute("ui.tab", json!({"tab": "File"})).unwrap();
    assert_eq!((b.ui.backstage, b.ui.backstage_page.as_str()), (true, "info"));
}

fn window(app: MailApp, width: f32) -> Window {
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(width, 700.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(4);
    // The sample has reminders due, and their window would lie over the ribbon.
    let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    harness.run_steps(3);
    harness
}

fn click(harness: &mut Window, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(3);
}

/// What brings up every tab of every module, and the tab a message being written has.
fn every_tab() -> Vec<(String, Vec<(&'static str, Value)>)> {
    let mut out = Vec::new();
    for module in ["view.mail", "view.calendar", "view.people", "view.tasks"] {
        for tab in ["Home", "Send / Receive", "Folder", "View", "Help"] {
            out.push((format!("{module} › {tab}"), vec![(module, json!({})), ("ui.tab", json!({"tab": tab}))]));
        }
    }
    for tab in ["Message", "Insert", "Format Text", "Review"] {
        out.push((
            tab.to_string(),
            vec![("view.mail", json!({})), ("mail.first", json!({})), ("mail.reply", json!({})), ("ui.tab", json!({"tab": tab}))],
        ));
    }
    out
}

/// Every label, tooltip name and group title of every tab is in the template the catalogues
/// are made from (`cargo xtask i18n`), as written, a large button's line break and all: a
/// control the extraction missed would stay English in every language.
#[test]
fn every_ribbon_text_is_in_the_template() {
    let template = mailcraft_i18n::po::parse(include_str!("../../../i18n/po/messages.pot"));
    let known: std::collections::BTreeSet<String> = template.entries.iter().filter(|e| e.context.is_none()).map(|e| e.id.clone()).collect();
    let mut a = app();
    let mut missing = std::collections::BTreeSet::new();
    for (_, commands) in every_tab() {
        for (command, params) in commands {
            a.run(command, params).unwrap();
        }
        let tab = describe(&a);
        for group in &tab.groups {
            let texts = std::iter::once(group.title).chain(group.items.iter().flat_map(|i| [i.label, i.tip]));
            missing.extend(texts.filter(|s| !s.is_empty() && !known.contains(*s)));
        }
    }

    assert!(missing.is_empty(), "not in messages.pot (run `cargo xtask i18n`): {missing:?}");
}

/// One description feeds both layouts: every control has a key of its own, an icon that is
/// drawn and a command the program knows, and a place in the classic groups and in the row.
#[test]
fn every_control_has_its_place_in_both_layouts() {
    let mut a = app();
    let mut seen = 0;
    for (name, commands) in every_tab() {
        for (command, params) in commands {
            a.run(command, params).unwrap();
        }
        let tab = describe(&a);
        let items: Vec<&Item> = tab.items().collect();
        assert!(!items.is_empty(), "{name}");
        for item in &items {
            let key = item.key;
            assert_eq!(items.iter().filter(|other| other.key == key).count(), 1, "{name}: two controls are called {key}");
            assert!(icons::NAMES.contains(&item.icon), "{name}: {key} has no drawing for \"{}\"", item.icon);
            assert!(!item.label.is_empty() && item.rank >= 1, "{name}: {key}");
            if let Some((command, _)) = &item.command {
                let known = a.session.registry.get(command).is_some() || crate::UI_COMMANDS.iter().any(|c| c.0 == *command);
                assert!(known, "{name}: {key} runs {command}, which is no command");
            }
            match item.size {
                // Only there to gather others in the simplified row: nothing of its own to do.
                Size::Hidden => {
                    assert!(item.command.is_none() && item.menu.is_none(), "{name}: {key}");
                    assert!(items.iter().any(|other| other.under == Some(key)), "{name}: nothing is under {key}");
                }
                Size::Big | Size::Small if item.pending => {
                    assert!(item.command.is_none() && item.menu.is_none(), "{name}: {key} is pending and does nothing")
                }
                Size::Big | Size::Small => assert!(item.command.is_some() != item.menu.is_some(), "{name}: {key} runs a command or opens a menu"),
            }
            if let Some(host) = item.under {
                let host = items.iter().find(|other| other.key == host).unwrap_or_else(|| panic!("{name}: {key} is under {host}, which isn't there"));
                assert!(host.under.is_none(), "{name}: {key} is under {}, which is under something itself", host.key);
            }
        }
        // The simplified row has each control it offers once, as a button or under one.
        let buttons = row(&tab);
        for item in items {
            let shown = buttons.iter().flat_map(|b| std::iter::once(b.item).chain(b.drops.iter().copied())).filter(|shown| shown.key == item.key);
            assert_eq!(shown.count(), usize::from(!item.classic_only), "{name}: {} in the simplified row", item.key);
        }
        assert!(buttons.windows(2).all(|pair| pair[0].item.section <= pair[1].item.section), "{name}: the row is in its stretches' order");
        assert!(buttons.iter().all(|b| b.item.command.is_some() || b.has_drop() || b.item.pending), "{name}: a button does something");
        // The classic layout has all but what is only there for the row.
        assert!(tab.groups.iter().all(|group| !group.title.is_empty() && !group.items.is_empty()), "{name}");
        seen += 1;
    }
    assert_eq!(seen, 24);
}

/// Mail › Home in the order the simplified row has it, with its split buttons.
#[test]
fn the_mail_row_is_in_order() {
    let a = app();
    let tab = describe(&a);
    let buttons = row(&tab);
    let keys: Vec<&str> = buttons.iter().map(|b| b.item.key).collect();
    assert_eq!(
        keys.iter().take(12).copied().collect::<Vec<_>>(),
        [
            "mail.new",
            "mail.delete",
            "mail.archive",
            "moveMessages",
            "mail.reply",
            "mail.replyAll",
            "mail.forward",
            "mail.toggleRead",
            "categorize",
            "followUp",
            "rules",
            "filter"
        ]
    );
    let sections: Vec<u8> = buttons.iter().take(12).map(|b| b.item.section).collect();
    assert_eq!(sections, [0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4]);
    let button = |key: &str| buttons.iter().find(|b| b.item.key == key).unwrap();
    // New Email and Delete run their command and open a menu; Move only opens one; Archive only runs.
    assert!(button("mail.new").is_split() && button("mail.new").drops.iter().map(|d| d.key).eq(["newItems"]));
    assert!(button("mail.delete").is_split() && button("mail.delete").drops.iter().map(|d| d.key).eq(["junk", "mail.deletePermanently"]));
    assert!(!button("moveMessages").is_split() && button("moveMessages").has_drop());
    assert!(!button("mail.archive").has_drop());
    assert_eq!(button("mail.replyAll").item.name(), "Reply All");
    // Calendar › Home: the new items, then today and the views.
    let mut a = app();
    a.execute("view.calendar", json!({})).unwrap();
    let tab = describe(&a);
    let buttons = row(&tab);
    let keys: Vec<&str> = buttons.iter().map(|b| b.item.key).take(8).collect();
    assert_eq!(keys, ["calendar.new", "calendar.newMeeting", "view.today", "view.next7Days", "view.day", "view.workWeek", "view.week", "view.month"]);
    assert_eq!(buttons.iter().take(8).map(|b| b.item.section).collect::<Vec<_>>(), [0, 0, 1, 1, 2, 2, 2, 2]);
    // New Appointment has the other new items under its ▾; the four views keep their
    // names as long as there is room (they aren't told from one another by their icons
    // alone), and the one in use is marked.
    let button = |key: &str| buttons.iter().find(|b| b.item.key == key).unwrap();
    assert!(button("calendar.new").is_split() && !button("calendar.newMeeting").has_drop());
    for view in ["view.day", "view.workWeek", "view.week", "view.month"] {
        assert!(!button(view).item.plain && button(view).item.checked == (view == "view.week"), "{view}");
    }
    assert_eq!((button("view.workWeek").item.name(), button("view.next7Days").item.name()), ("Work Week".to_string(), "Next 7 Days".to_string()));
    assert!(button("view.day").item.rank < button("view.today").item.rank && button("view.today").item.rank < button("view.next7Days").item.rank);
    // Back, Forward and Go to Date are under Today's ▾ (the calendar's own header has them too).
    assert!(button("view.today").is_split() && button("view.today").drops.iter().map(|d| d.key).eq(["view.previous", "view.next", "view.goToDate"]));
    // What is done with an appointment is in the row while one is selected, and the
    // answers to a meeting while it is somebody else's.
    let has = |a: &MailApp, key: &str| describe(a).items().any(|item| item.key == key);
    assert!(!has(&a, "calendar.edit") && !has(&a, "calendar.delete") && !has(&a, "calendar.accept") && has(&a, "file.exportCalendar"));
    let event = |a: &MailApp, title: &str| a.session.mailbox.events.values().find(|e| e.title.starts_with(title)).map(|e| e.id).unwrap();
    a.execute("calendar.select", json!({"id": event(&a, "Lunch with Dev")})).unwrap();
    assert!(has(&a, "calendar.edit") && has(&a, "calendar.delete") && has(&a, "showAs") && has(&a, "categorize") && !has(&a, "calendar.decline"));
    a.execute("calendar.select", json!({"id": event(&a, "Q4 roadmap")})).unwrap();
    assert!(has(&a, "calendar.edit") && has(&a, "calendar.accept") && has(&a, "calendar.tentative") && has(&a, "calendar.decline"));
    let tab = describe(&a);
    let sections: Vec<(&str, u8)> = row(&tab).iter().map(|b| (b.item.key, b.item.section)).filter(|(_, section)| *section >= 3).collect();
    assert_eq!(sections.first().copied(), Some(("calendar.edit", 3)));
    assert_eq!(sections.last().copied(), Some(("file.exportCalendar", 5)));
    a.execute("calendar.select", json!({"id": null})).unwrap();
    // The View tab has the calendar's time scale while the calendar is in front.
    a.execute("ui.tab", json!({"tab": "View"})).unwrap();
    assert!(describe(&a).items().any(|item| item.key == "timeScale" && item.menu == Some(Menu::TimeScale)));
    a.execute("view.mail", json!({})).unwrap();
    assert!(!describe(&a).items().any(|item| item.key == "timeScale"));
    a.execute("ui.tab", json!({"tab": "Home"})).unwrap();
    // Tasks › Home gathers the five due dates under one button.
    a.execute("view.tasks", json!({})).unwrap();
    let tab = describe(&a);
    let buttons = row(&tab);
    let follow_up = buttons.iter().find(|b| b.item.key == "followUp").unwrap();
    assert_eq!(follow_up.drops.len(), 5);
    assert!(follow_up.item.command.is_none() && !follow_up.is_split());
}

/// Calendar › View has the four views, the one in use marked, in place of the messages'
/// sorting and preview, which mean nothing there; Mail › View keeps those and has no views.
#[test]
fn the_calendar_view_tab_has_its_views() {
    let mail_only = ["view.conversations", "preview", "arrangeBy", "view.reverseSort"];
    let views = ["view.day", "view.workWeek", "view.week", "view.month"];
    let checked_view =
        |a: &MailApp| -> Vec<&str> { describe(a).items().filter(|item| views.contains(&item.key) && item.checked).map(|item| item.key).collect() };
    let mut a = app();
    a.execute("view.calendar", json!({})).unwrap();
    a.execute("ui.tab", json!({"tab": "View"})).unwrap();
    let tab = describe(&a);
    assert_eq!(tab.groups.iter().map(|g| g.title).collect::<Vec<_>>(), ["Layout", "Arrangement", "Calendar", "Window"]);
    let arrangement: Vec<(&str, bool, u8, u8)> = tab.groups[1].items.iter().map(|item| (item.key, item.checked, item.section, item.rank)).collect();
    assert_eq!(arrangement, [("view.day", false, 1, 2), ("view.workWeek", false, 1, 2), ("view.week", true, 1, 2), ("view.month", false, 1, 2)]);
    // The calendar's own group is as it was.
    let calendar: Vec<&str> = tab.groups[2].items.iter().map(|item| item.key).collect();
    assert_eq!(calendar, ["timeScale", "view.overlay", "view.sideBySide"]);
    assert!(!tab.items().any(|item| mail_only.contains(&item.key)));
    // Each view, pressed, is the one marked.
    for view in views {
        a.execute(view, json!({})).unwrap();
        assert_eq!(checked_view(&a), [view]);
    }
    a.execute("view.mail", json!({})).unwrap();
    let tab = describe(&a);
    assert!(!tab.items().any(|item| views.contains(&item.key)));
    assert!(mail_only.iter().all(|key| tab.items().any(|item| item.key == *key)));
}

/// Labels go before buttons, the plain before the others, the least important first; the
/// first-rank button keeps its label until all the rest is behind `…`.
#[test]
fn the_row_gives_way_in_order() {
    let item = |key: &'static str, section: u8, rank: u8, plain: bool| {
        let item = Item::big("mail", key, key, json!({})).at(section, rank);
        if plain { item.plain() } else { item }
    };
    let tab = Tab::of(vec![g("All", vec![item("a", 0, 1, false), item("b", 1, 1, true), item("c", 1, 3, false), item("d", 2, 5, false)])]);
    let buttons = row(&tab);
    let width = |_: usize, labelled: bool| if labelled { 100.0 } else { 40.0 };
    let at = |available: f32| plan(&buttons, &width, available);
    let all = |labels: [bool; 4]| labels.iter().copied().enumerate().collect::<Vec<_>>();
    // 100 + rule + 100 + gap + 100 + rule + 100.
    let full = 400.0 + 2.0 * RULE_GAP + BUTTON_GAP;
    assert_eq!(at(full), Plan { shown: all([true, true, true, true]), more: vec![] });
    assert_eq!(at(10_000.0).shown, at(full).shown);
    // The plain one's label first, then the others' from the least important.
    assert_eq!(at(full - 1.0).shown, all([true, false, true, true]));
    assert_eq!(at(full - 61.0).shown, all([true, false, true, false]));
    assert_eq!(at(full - 121.0).shown, all([true, false, false, false]));
    // Then buttons go behind `…`, the least important first; the first one keeps its label.
    assert_eq!(at(full - 181.0), Plan { shown: vec![(0, true), (1, false), (2, false)], more: vec![3] });
    let three = 100.0 + RULE_GAP + 40.0 + BUTTON_GAP + 40.0 + BUTTON_GAP + MORE_W;
    assert_eq!(at(three - 1.0), Plan { shown: vec![(0, true), (1, false)], more: vec![2, 3] });
    // Then the first-rank label, and last the first-rank buttons, from the end of the row.
    let two = 100.0 + RULE_GAP + 40.0 + BUTTON_GAP + MORE_W;
    assert_eq!(at(two - 1.0), Plan { shown: vec![(0, false), (1, false)], more: vec![2, 3] });
    assert_eq!(at(two - 61.0), Plan { shown: vec![(0, false)], more: vec![1, 2, 3] });
    assert_eq!(at(40.0), Plan { shown: vec![], more: vec![0, 1, 2, 3] });
    assert_eq!(at(-5.0).more.len(), 4);
    assert_eq!(plan(&[], &width, 100.0), Plan::default());

    // At any width: every button is in the row or behind `…`, the row fits, and a narrower
    // window never shows a button or a label that a wider one doesn't.
    let mut wider: Option<Plan> = None;
    for available in (0..=450).rev().map(|w| w as f32) {
        let p = at(available);
        let mut places: Vec<usize> = p.shown.iter().map(|(i, _)| *i).chain(p.more.iter().copied()).collect();
        places.sort_unstable();
        assert_eq!(places, [0, 1, 2, 3], "{available}");
        let state: Vec<(bool, bool)> =
            (0..4).map(|i| p.shown.iter().find(|(place, _)| *place == i).map_or((false, false), |(_, labelled)| (true, *labelled))).collect();
        assert!(p.shown.is_empty() || row_width(&buttons, &state, &width) <= available, "{available}: {p:?}");
        if let Some(wider) = &wider {
            for (i, labelled) in &p.shown {
                let before = wider.shown.iter().find(|(place, _)| place == i);
                assert!(before.is_some_and(|(_, was)| *was || !*labelled), "{available}: {p:?} after {wider:?}");
            }
        }
        wider = Some(p);
    }
}

/// What is on screen of a tab's row: checks that it is all inside the band, in order and not
/// over one another, and that what isn't on screen is behind `…`. Returns the keys behind `…`.
fn check_row(harness: &Window, name: &str) -> Vec<&'static str> {
    let app = harness.state();
    let tab = describe(app);
    let buttons = row(&tab);
    let band = app.hit.rect("ribbonBand", 0).unwrap_or_else(|| panic!("{name}: no band"));
    let more = behind_more(&harness.ctx);
    let inside = |r: Rect| r.min.x >= band.min.x - 0.5 && r.max.x <= band.max.x + 0.5 && r.min.y >= band.min.y - 0.5 && r.max.y <= band.max.y + 0.5;
    let mut end = band.min.x;
    for b in &buttons {
        let key = b.item.key;
        let Some(r) = app.hit.rect("ribbon", hit_id(key)) else {
            assert!(more.contains(&key), "{name}: {key} is neither in the row nor behind …");
            continue;
        };
        assert!(!more.contains(&key), "{name}: {key} is in the row and behind …");
        assert!(inside(r), "{name}: {key} at {r:?} is outside the band {band:?}");
        assert!(r.min.x >= end - 0.5 && r.width() >= ICON, "{name}: {key} at {r:?} is over the button before it (which ends at {end})");
        end = r.max.x;
        // Its `▾` half is beside it when it has one of its own.
        match app.hit.rect("ribbonMenu", hit_id(key)) {
            Some(drop) => {
                assert!(b.is_split() && inside(drop) && (drop.min.x - end).abs() < 0.5, "{name}: {key} ▾ at {drop:?}");
                end = drop.max.x;
            }
            None => assert!(!b.is_split(), "{name}: {key} has no ▾"),
        }
    }
    let mode = app.hit.rect("ribbonMode", 0).unwrap_or_else(|| panic!("{name}: no layout button"));
    assert!(inside(mode) && mode.min.x >= end - 0.5, "{name}: the layout button at {mode:?}");
    match app.hit.rect("ribbonMore", 0) {
        Some(r) => assert!(!more.is_empty() && inside(r) && r.min.x >= end - 0.5 && r.max.x <= mode.min.x + 0.5, "{name}: … at {r:?}"),
        None => assert!(more.is_empty(), "{name}: {more:?} are behind a … that isn't there"),
    }
    more
}

/// Every tab of every module in one row at 1200, 1000 and 800 wide: nothing is cut off and
/// nothing scrolls; what doesn't fit is behind `…`.
#[test]
fn the_simplified_row_fits_at_every_width() {
    assert_eq!(crate::UiState::default().ribbon_mode, Mode::Simplified);
    for width in [1200.0, 1000.0, 800.0] {
        let mut harness = window(app(), width);
        let mut most = 0;
        for (name, commands) in every_tab() {
            for (command, params) in commands {
                harness.state_mut().run(command, params).unwrap();
            }
            harness.run_steps(3);
            let name = format!("{name} at {width}");
            let more = check_row(&harness, &name);
            most = most.max(more.len());
            // What a tab is for stays in the row, with its label (what is still to be built
            // gives way first).
            let first = row(&describe(harness.state())).iter().find(|b| !b.item.pending).map(|b| b.item.key).unwrap();
            assert!(harness.state().hit.rect("ribbon", hit_id(first)).is_some_and(|r| r.width() > 60.0), "{name}: {first}");
            // The band is one row high, and the panes start right under it.
            let band = harness.state().hit.rect("ribbonBand", 0).unwrap();
            assert!((band.height() - BAND_H).abs() < 0.5, "{name}: {band:?}");
        }
        // Mail › Home has the most to show: all of it at 1200, some of it behind … at 800.
        harness.state_mut().run("compose.discard", json!({})).ok();
        harness.state_mut().run("ui.tab", json!({"tab": "Home"})).unwrap();
        harness.run_steps(3);
        let more = check_row(&harness, "mail home");
        match width as u32 {
            1200 => assert!(more.is_empty(), "{more:?}"),
            800 => assert!(
                more.contains(&"mail.findRelated") && more.contains(&"mail.findFromSender") && !more.contains(&"mail.delete") && most >= more.len(),
                "{more:?}"
            ),
            _ => {}
        }
    }
}

/// The buttons of the row do what they say: the main part runs the command, a `▾` opens its
/// menu, and what is behind `…` runs from there.
#[test]
fn the_row_runs_its_commands_and_so_does_what_is_behind_more() {
    let mut harness = window(app(), 800.0);
    harness.state_mut().run("mail.next", json!({})).unwrap();
    harness.run_steps(3);
    let focus = harness.state().session.sel.focus().unwrap();
    let message = |h: &Window| h.state().session.mailbox.message(focus).cloned().unwrap();
    let at = |h: &Window, kind: &str, key: &str| h.state().hit.rect(kind, hit_id(key)).unwrap_or_else(|| panic!("no {kind} {key}")).center();

    // The main part: Unread/Read on the message in view.
    let read = message(&harness).read;
    let pos = at(&harness, "ribbon", "mail.toggleRead");
    click(&mut harness, pos);
    assert_eq!(message(&harness).read, !read);

    // Behind `…`: Pin, Related and From Sender are there, and run.
    let more = check_row(&harness, "mail home at 800");
    assert!(more.contains(&"mail.pin") && more.contains(&"mail.findRelated") && more.contains(&"mail.findFromSender"), "{more:?}");
    assert!(harness.query_by_label("Pin").is_none(), "the menu is shut");
    let dots = harness.state().hit.rect("ribbonMore", 0).unwrap().center();
    click(&mut harness, dots);
    for key in &more {
        let item = describe(harness.state()).items().find(|item| item.key == *key).cloned().unwrap();
        assert!(harness.query_all_by_label_contains(&item.name()).next().is_some(), "{key} isn't in the menu behind …");
    }
    let pinned = message(&harness).pinned;
    harness.get_by_label("Pin").click();
    harness.run_steps(3);
    assert_eq!(message(&harness).pinned, !pinned);
    assert!(harness.query_by_label("Pin").is_none(), "the menu shuts when something is chosen");
    click(&mut harness, dots);
    harness.get_by_label("Related").click();
    harness.run_steps(3);
    assert!(harness.state().session.view.search.starts_with("subject:"), "{:?}", harness.state().session.view.search);
    harness.state_mut().execute("view.clearSearch", json!({})).unwrap();
    harness.state_mut().execute("mail.select", json!({"id": focus})).unwrap();
    harness.run_steps(3);
    let sender = mailcraft_model::search::keyword_value(message(&harness).from.email.trim());
    click(&mut harness, dots);
    harness.get_by_label("From Sender").click();
    harness.run_steps(3);
    assert_eq!(harness.state().session.view.search, format!("from:{sender}"));
    harness.state_mut().execute("view.clearSearch", json!({})).unwrap();
    harness.state_mut().execute("mail.select", json!({"id": focus})).unwrap();
    harness.run_steps(3);

    // A `▾`: New Email's lists the other new things; Delete's, what else gets rid of a message.
    let pos = at(&harness, "ribbonMenu", "mail.new");
    click(&mut harness, pos);
    assert!(harness.state().session.compose.is_none(), "the ▾ doesn't start a message");
    harness.get_by_label_contains("Folder…").click();
    harness.run_steps(3);
    assert_eq!(harness.state().dialog.as_ref().map(|d| d.name()), Some("folder"));
    harness.state_mut().execute("ui.closeDialog", json!({})).unwrap();
    harness.run_steps(2);
    let junk = harness.state().session.mailbox.folders.iter().find(|f| f.kind == FolderKind::Junk).map(|f| f.id).unwrap();
    assert_ne!(message(&harness).folder, junk);
    let pos = at(&harness, "ribbonMenu", "mail.delete");
    click(&mut harness, pos);
    // Its Junk is a menu of its own (#504): Move to Junk is in it.
    harness.get_by_role_and_label(egui::accesskit::Role::Button, "Junk").click();
    harness.run_steps(3);
    harness.get_by_label_contains("Move to Junk").click();
    harness.run_steps(3);
    assert_eq!(message(&harness).folder, junk);

    // And the main part of that button: New Email starts a message, in the reading pane, as a
    // reply does (#286).
    let pos = at(&harness, "ribbon", "mail.new");
    click(&mut harness, pos);
    let compose = harness.state().session.compose_id().expect("a message");
    assert!(harness.state().windows.of_compose(compose).is_none());
    assert_eq!(crate::windows::compose_window::pane_compose(harness.state()), Some(compose));
    assert_eq!(harness.state().ui.tab, "Message");
}

/// The layout is chosen at the right end of the band, in View and by command; it is kept with
/// the settings, and settings that say nothing of it (or nonsense) mean the simplified one.
#[test]
fn the_layout_is_chosen_and_kept() {
    let mut harness = window(app(), 1200.0);
    assert_eq!(harness.state().ui.ribbon_mode, Mode::Simplified);
    let simple = harness.state().hit.rect("ribbonBand", 0).unwrap();
    let button = harness.state().hit.rect("ribbonMode", 0).unwrap().center();
    click(&mut harness, button);
    assert!(harness.query_all_by_label_contains("Simplified Ribbon").next().is_some());
    harness.get_by_label_contains("Classic Ribbon").click();
    harness.run_steps(3);
    assert_eq!(harness.state().ui.ribbon_mode, Mode::Classic);
    let classic = harness.state().hit.rect("ribbonBand", 0).unwrap();
    assert!(classic.height() > simple.height() + 30.0, "{simple:?} -> {classic:?}");

    // Kept with the settings.
    let saved = serde_json::to_value(harness.state().prefs()).unwrap();
    assert_eq!(saved["ribbonMode"], "classic");
    let restored: crate::UiState = serde_json::from_value(saved).unwrap();
    assert_eq!(restored.ribbon_mode, Mode::Classic);
    let old: crate::UiState = serde_json::from_value(json!({"tab": "Home"})).unwrap();
    assert_eq!(old.ribbon_mode, Mode::Simplified);
    let odd: crate::UiState = serde_json::from_value(json!({"ribbonMode": "sideways", "tab": "View"})).unwrap();
    assert_eq!((odd.ribbon_mode, odd.tab.as_str()), (Mode::Simplified, "View"));

    // By command, and from the button again.
    let a = harness.state_mut();
    assert!(a.enabled("ui.ribbonMode"));
    assert!(a.execute("ui.ribbonMode", json!({"value": "sideways"})).is_err() && a.execute("ui.ribbonMode", json!({})).is_err());
    assert_eq!(a.ui.ribbon_mode, Mode::Classic);
    assert_eq!(a.execute("ui.ribbonMode", json!({"value": "Simplified"})).unwrap()["value"], "simplified");
    assert_eq!(a.execute("ui.ribbonMode", json!({"value": "classic"})).unwrap()["value"], "classic");
    harness.run_steps(3);
    let button = harness.state().hit.rect("ribbonMode", 0).unwrap().center();
    click(&mut harness, button);
    harness.get_by_label_contains("Simplified Ribbon").click();
    harness.run_steps(3);
    assert_eq!(harness.state().ui.ribbon_mode, Mode::Simplified);
    // View lists it too.
    harness.state_mut().run("ui.tab", json!({"tab": "View"})).unwrap();
    assert!(describe(harness.state()).items().any(|item| item.menu == Some(Menu::RibbonLayout)));
}

/// #123: there is room between the tabs and the ribbon in both layouts, and in the classic one
/// between the lowest line of a large button's label and the group's name under it.
#[test]
fn the_ribbon_has_room_under_the_tabs_and_over_the_group_names() {
    let mut harness = window(app(), 2400.0);
    for mode in ["classic", "simplified"] {
        harness.state_mut().execute("ui.ribbonMode", json!({"value": mode})).unwrap();
        for (name, commands) in every_tab() {
            for (command, params) in commands {
                harness.state_mut().run(command, params).unwrap();
            }
            harness.run_steps(3);
            let name = format!("{name} ({mode})");
            let app = harness.state();
            let band = app.hit.rect("ribbonBand", 0).unwrap();
            let strip = app.hit.rect("ribbonTab", hit_id("Home")).unwrap();
            assert!(band.min.y - strip.max.y >= 4.0, "{name}: tabs end at {}, the ribbon starts at {}", strip.max.y, band.min.y);
            if mode != "classic" {
                continue;
            }
            let mut checked = 0;
            for group in &describe(app).groups {
                let Some(title) = app.hit.rect("ribbonGroup", hit_id(group.title)) else { continue };
                assert!(title.max.y <= band.max.y - 2.0, "{name}: {} at {title:?} in {band:?}", group.title);
                for item in group.items.iter().filter(|item| item.size == Size::Big) {
                    let label = app.hit.rect("ribbonLabel", hit_id(item.key)).unwrap_or_else(|| panic!("{name}: {} has no label", item.key));
                    let gap = title.min.y - label.max.y;
                    // 2 pt, not 4: Windows draws the label half a point taller than Linux does.
                    assert!(gap >= 2.0, "{name}: {}'s label ends {gap} over {}", item.key, group.title);
                    checked += 1;
                }
            }
            assert!(checked > 0 || name.contains("Help"), "{name}: no large buttons were measured");
        }
    }
}

/// Every control is in its group. Large buttons and columns use the standard layout, while
/// Message also has formatting and tag rows.
#[test]
fn the_classic_layout_sets_every_control_in_its_group() {
    let mut harness = window(app(), 2400.0);
    harness.state_mut().execute("ui.ribbonMode", json!({"value": "classic"})).unwrap();
    for (name, commands) in every_tab() {
        for (command, params) in commands {
            harness.state_mut().run(command, params).unwrap();
        }
        harness.run_steps(3);
        let app = harness.state();
        let tab = describe(app);
        let band = app.hit.rect("ribbonBand", 0).unwrap();
        assert!((band.height() - (CONTENT_H + LABEL_H + 10.0)).abs() < 0.5, "{name}: {band:?}");
        assert!(app.hit.rect("ribbonMore", 0).is_none() && app.hit.rect("ribbonMode", 0).is_some(), "{name}");
        let mut end = band.min.x;
        for group in &tab.groups {
            if !group.classic_rows.is_empty() {
                let mut drawn = std::collections::BTreeSet::new();
                let mut bottom = band.min.y;
                let mut right = end;
                for row in fit::packed_rows(group) {
                    let mut next = end;
                    let mut row_bottom = bottom;
                    for item in row {
                        assert!(drawn.insert(item.key), "{name}: {} appears twice", item.key);
                        let r = app.hit.rect("ribbon", hit_id(item.key)).unwrap_or_else(|| panic!("{name}: {} isn't drawn", item.key));
                        assert!((r.height() - 22.0).abs() < 0.5 && r.min.x >= next - 0.5 && r.min.y >= bottom - 0.5, "{name}: {} at {r:?}", item.key);
                        assert!(r.max.x <= band.max.x && r.max.y < band.max.y, "{name}: {} at {r:?}", item.key);
                        next = r.max.x;
                        row_bottom = row_bottom.max(r.max.y);
                    }
                    right = right.max(next);
                    bottom = row_bottom;
                }
                assert_eq!(drawn.len(), fit::shown(group).len(), "{name}: {} dropped a control", group.title);
                end = right;
                continue;
            }
            let mut column: Vec<Rect> = Vec::new();
            for item in &group.items {
                let at = app.hit.rect("ribbon", hit_id(item.key));
                match item.size {
                    Size::Hidden => assert!(at.is_none(), "{name}: {} is drawn", item.key),
                    Size::Big => {
                        let r = at.unwrap_or_else(|| panic!("{name}: {} isn't drawn", item.key));
                        assert!((r.height() - CONTENT_H).abs() < 0.5 && r.width() >= 46.0, "{name}: {} at {r:?}", item.key);
                        assert!(r.min.x >= end - 0.5 && r.max.x <= band.max.x, "{name}: {} at {r:?} after {end}", item.key);
                        end = r.max.x;
                        column.clear();
                    }
                    Size::Small => {
                        let r = at.unwrap_or_else(|| panic!("{name}: {} isn't drawn", item.key));
                        assert!((r.height() - 22.0).abs() < 0.5, "{name}: {} at {r:?}", item.key);
                        if column.len() == 3 {
                            column.clear();
                        }
                        match column.last() {
                            // Under the one before it, in the same column.
                            Some(above) => {
                                assert!((r.min.x - above.min.x).abs() < 0.5 && r.min.y >= above.max.y - 0.5, "{name}: {} at {r:?}", item.key)
                            }
                            // A new column, after what came before.
                            None => {
                                assert!(r.min.x >= end - 0.5, "{name}: {} at {r:?} after {end}", item.key);
                            }
                        }
                        column.push(r);
                        end = end.max(r.max.x);
                    }
                }
            }
        }
    }
    // Mail › Home, group by group, as the classic ribbon has always had it.
    let a = app();
    let tab = describe(&a);
    let groups: Vec<(&str, Vec<&str>)> =
        tab.groups.iter().map(|g| (g.title, g.items.iter().filter(|i| i.size != Size::Hidden).map(|i| i.label).collect())).collect();
    assert_eq!(
        groups,
        [
            ("New", vec!["New\nEmail", "New\nItems"]),
            ("Delete", vec!["Delete", "Archive", "Junk", "Delete Permanently"]),
            ("Respond", vec!["Reply", "Reply\nAll", "Forward", "Forward as Attachment", "Reply and Delete", "View Source", "Meeting", "Add Sender"]),
            ("Move", vec!["Move", "Rules", "Resend Message"]),
            ("Tags", vec!["Unread/\nRead", "Categorize", "Follow\nUp", "Pin"]),
            ("Find", vec!["Filter\nEmail", "Related", "From Sender"]),
            ("Send/Receive", vec!["Send/Receive\nAll Folders"]),
        ]
    );
}

/// The folder menu with a search box, drawn on its own: the app, and the folder chosen.
type Picker = egui_kittest::Harness<'static, (MailApp, Option<Id>)>;

fn picker(app: MailApp) -> Picker {
    let mut h = egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(400.0, 500.0)).build_ui_state(
        |ui, (app, picked): &mut (MailApp, Option<Id>)| {
            folder_menu(ui, app, |_, id| *picked = Some(id));
        },
        (app, None),
    );
    h.run_steps(3);
    h
}

fn type_into(h: &mut Picker, text: &str) {
    h.event(egui::Event::Text(text.to_string()));
    h.run_steps(3);
}

fn press(h: &mut Picker, key: egui::Key) {
    h.key_press(key);
    h.run_steps(3);
}

fn folder_row(name: &str) -> FolderRow {
    (1, name.to_string(), name.to_string(), 0)
}

/// Typing narrows the list to the folders whose name holds it, any part, whatever the case
/// and accents; nothing typed shows all (#74).
#[test]
fn folder_search_matches_any_part_ignoring_case_and_accents() {
    let list = [folder_row("Inbox"), folder_row("Café Receipts"), folder_row("Projects"), folder_row("Archive 2026")];
    assert_eq!(folder_matches(&list, ""), [0, 1, 2, 3]);
    assert_eq!(folder_matches(&list, "   "), [0, 1, 2, 3]);
    assert_eq!(folder_matches(&list, "BOX"), [0]);
    assert_eq!(folder_matches(&list, "cafe"), [1]);
    assert_eq!(folder_matches(&list, "é rec"), [1]);
    assert_eq!(folder_matches(&list, "202"), [3]);
    assert!(folder_matches(&list, "zzz").is_empty());
}

/// Hostile or enormous text asks for nothing and never panics.
#[test]
fn folder_search_survives_hostile_text() {
    let list = [folder_row("Inbox"), folder_row("Ünï")];
    let long = "é".repeat(100_000);
    for q in [long.as_str(), "\u{0}\u{7f}\u{202e}", "\u{1F600}\u{200d}\u{1F600}", "%s{}[]\\", "\n\r\t", "ß", "\u{301}"] {
        let _ = folder_matches(&list, q);
    }
    assert!(folder_matches(&list, &long).is_empty());
    assert!(folder_matches(&[], "x").is_empty());
    // Through the window as well: typed, then keys, with nothing to choose.
    let mut h = picker(app());
    type_into(&mut h, &"\u{1F600}x".repeat(500));
    for key in [egui::Key::ArrowDown, egui::Key::ArrowUp, egui::Key::Enter, egui::Key::Escape] {
        press(&mut h, key);
    }
    assert_eq!(h.state().1, None);
}

/// The box has the keyboard when the menu opens: typing filters, Enter chooses.
#[test]
fn folder_search_is_typed_into_and_chosen_by_keyboard() {
    let a = app();
    let sent = {
        let mb = &a.session.mailbox;
        let account = mb.default_account().map(|x| x.id).unwrap();
        mb.folder_of_kind(account, mailcraft_model::FolderKind::Sent).unwrap()
    };
    let mut h = picker(a);
    assert!(h.query_by_label_contains("Drafts").is_some());
    type_into(&mut h, "SENT");
    assert!(h.query_by_label_contains("Drafts").is_none());
    press(&mut h, egui::Key::Enter);
    assert_eq!(h.state().1, Some(sent));
}

/// Nothing matching says so, and Enter chooses nothing.
#[test]
fn folder_search_with_no_match_says_so() {
    let mut h = picker(app());
    type_into(&mut h, "qqqqzz");
    assert!(h.query_by_label_contains("No folders match").is_some());
    press(&mut h, egui::Key::Enter);
    assert_eq!(h.state().1, None);
}

/// Down moves to the next match, and Escape clears the search (so the full list is back).
#[test]
fn folder_search_arrows_move_and_escape_clears() {
    let mut probe = picker(app());
    type_into(&mut probe, "e");
    press(&mut probe, egui::Key::Enter);
    let first = probe.state().1;
    let mut h = picker(app());
    type_into(&mut h, "e");
    press(&mut h, egui::Key::ArrowDown);
    press(&mut h, egui::Key::Enter);
    assert!(h.state().1.is_some());
    assert_ne!(h.state().1, first);
    let mut h = picker(app());
    type_into(&mut h, "sent");
    assert!(h.query_by_label_contains("Drafts").is_none());
    press(&mut h, egui::Key::Escape);
    assert!(h.query_by_label_contains("Drafts").is_some());
}
