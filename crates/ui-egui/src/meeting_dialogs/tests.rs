// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Propose New Time, Forward and View All Proposals through real frames, and the buttons of an
//! appointment's window that go with them, against the stand-in for the account's server (which
//! notes every piece of mail a real one would send). Invented people at `example.org`.

use std::sync::{Arc, Mutex};

use egui::Key;
use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use mailcraft_model::remote::{RemoteEvent, RemoteProposal};
use mailcraft_model::{Address, Attendee, Id, Importance};
use serde_json::{Value, json};

use super::*;
use crate::Services;
use crate::calendar_sync::testing::{Server, event, microsoft_account, serve};
use crate::dialogs::Dialog;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const HOUR: i64 = 3_600;

type Harness = egui_kittest::Harness<'static, MailApp>;

fn invite(name: &str, email: &str) -> Attendee {
    Attendee { address: Address::new(name, email), ..Default::default() }
}

/// Bo's meeting tomorrow at noon, to which the owner is invited.
fn board() -> RemoteEvent {
    RemoteEvent {
        organizer: Some(Address::new("Bo Marsh", "bo@example.org")),
        attendees: vec![invite("Avery", "avery@mail.example"), invite("Cy", "cy@example.org")],
        is_organizer: Some(false),
        ..event("g-1", "Board", NOW + 20 * HOUR)
    }
}

/// A meeting the owner organizes, the day after tomorrow, with Bo and Cy invited.
fn planning() -> RemoteEvent {
    RemoteEvent {
        organizer: Some(Address::new("Avery", "avery@mail.example")),
        attendees: vec![invite("Bo Marsh", "bo@example.org"), invite("Cy", "cy@example.org")],
        ..event("m-1", "Planning", NOW + 44 * HOUR)
    }
}

/// The sample mailbox with an account whose server (the stand-in) has `events`, brought down,
/// in the whole window.
fn served(events: Vec<RemoteEvent>) -> (Harness, Arc<Mutex<Server>>) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let server = serve(&mut app);
    microsoft_account(&mut app);
    server.lock().unwrap().events = events;
    app.auto_sync = true;
    let mut harness = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(8);
    // (The sample has reminders that are due: their window would lie over what is clicked.)
    let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    harness.run_steps(3);
    (harness, server)
}

fn click(harness: &mut Harness, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(3);
}

fn press(harness: &mut Harness, key: Key) {
    harness.key_press(key);
    harness.run_steps(3);
}

fn type_text(harness: &mut Harness, text: &str) {
    harness.event(egui::Event::Text(text.into()));
    harness.run_steps(3);
}

fn says(harness: &Harness, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

/// Click the last thing that says `label`.
fn click_label(harness: &mut Harness, label: &str) {
    let rect = harness.query_all_by_label_contains(label).last().unwrap_or_else(|| panic!("nothing says {label}")).rect();
    click(harness, rect.center());
}

/// Click a button of the dialog by what the hit map calls it.
fn hit(harness: &mut Harness, kind: &str, id: u64) {
    let rect = harness.state().hit.rect(kind, id).unwrap_or_else(|| panic!("no {kind} to click"));
    click(harness, rect.center());
}

/// Click a button of the bar of the window numbered `window`.
fn bar(harness: &mut Harness, window: u64, key: &str) {
    let found = harness.state().hit.rect("windowBar", crate::forms::kit::bar::hit_id(window, key));
    click(harness, found.unwrap_or_else(|| panic!("no {key} in the bar of window {window}")).center());
}

fn dialog(harness: &Harness) -> Option<&'static str> {
    harness.state().dialog.as_ref().map(|d| d.name())
}

fn named(harness: &Harness, title: &str) -> mailcraft_model::Event {
    let found = harness.state().session.mailbox.events.values().find(|e| e.title == title);
    found.map(|e| (**e).clone()).unwrap_or_else(|| panic!("no event called {title}"))
}

fn id_of(harness: &Harness, title: &str) -> Id {
    named(harness, title).id
}

fn mail(server: &Arc<Mutex<Server>>) -> Vec<String> {
    server.lock().unwrap().mail.clone()
}

fn windows(harness: &mut Harness) -> Value {
    let ctx = harness.ctx.clone();
    crate::control::inspect(harness.state_mut(), &ctx)["windows"].clone()
}

fn status(harness: &Harness) -> String {
    harness.state().status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
}

#[test]
fn another_time_is_proposed_from_its_dialog_and_from_nowhere_else() {
    for theme in ["light", "dark"] {
        let (mut harness, server) = served(vec![board(), planning()]);
        harness.state_mut().execute("ui.theme", json!({"value": theme})).unwrap();
        let board = id_of(&harness, "Board");
        harness.state_mut().execute("calendar.select", json!({"id": board})).unwrap();
        // The command the menus and the ribbon run lacks a time: its dialog asks for one.
        let pending = harness.state_mut().run("calendar.propose", json!({"id": board})).unwrap();
        harness.run_steps(4);
        assert_eq!((pending["pending"].as_str(), dialog(&harness)), (Some("proposeTime"), Some("proposeTime")), "{theme}");
        // It says whose meeting it is, when it is now, and who either button writes to.
        assert!(says(&harness, "Bo Marsh organizes \"Board\"."), "{theme}");
        assert!(says(&harness, "It is now on Thursday, October 15, 2026 12:00 PM – 1:00 PM."));
        assert!(says(&harness, "Either button writes to Bo Marsh: your answer, and the time you propose."));
        // No key sends: Enter is nobody's here, and Escape cancels.
        press(&mut harness, Key::Enter);
        harness.run_steps(8);
        assert_eq!((dialog(&harness), mail(&server).len()), (Some("proposeTime"), 0), "{theme}");
        press(&mut harness, Key::Escape);
        harness.run_steps(8);
        assert!(dialog(&harness).is_none() && mail(&server).is_empty() && harness.state().session.mailbox.event_changes.is_empty());

        // Another time, an hour later: an end that isn't after its start is said, and can't be sent.
        harness.state_mut().run("calendar.propose", json!({"id": board})).unwrap();
        harness.run_steps(3);
        let Some(Dialog::Meeting(MeetingDialog::Propose(propose))) = &mut harness.state_mut().dialog else { panic!("not the dialog") };
        (propose.start_time, propose.end_time) = ("3:00 PM".into(), "2:00 PM".into());
        harness.run_steps(3);
        assert!(says(&harness, "The proposed end has to be after the proposed start."));
        hit(&mut harness, "proposeTentative", 0);
        harness.run_steps(8);
        assert_eq!((dialog(&harness), mail(&server).len()), (Some("proposeTime"), 0));
        let Some(Dialog::Meeting(MeetingDialog::Propose(propose))) = &mut harness.state_mut().dialog else { panic!("not the dialog") };
        (propose.end_time, propose.note) = ("4:00 PM".into(), "Can you make it later?".into());
        harness.run_steps(3);
        assert!(!says(&harness, "The proposed end has to be after"));
        // Pressed: the answer and the proposal go to the organizer, and the dialog is gone.
        hit(&mut harness, "proposeTentative", 0);
        harness.run_steps(8);
        assert!(dialog(&harness).is_none(), "{theme}");
        let (start, end) = (NOW + 23 * HOUR, NOW + 24 * HOUR);
        assert_eq!(
            mail(&server),
            ["response:Board:Tentative:Can you make it later?".to_string(), format!("proposal:Board:{start}-{end}")],
            "{theme}"
        );
        assert_eq!(status(&harness), "Another time was proposed to Bo Marsh");
        // The meeting is where its organizer has it, tentatively.
        assert_eq!(named(&harness, "Board").start.hour, 12);

        // Decline and Propose: out of the calendar, as any meeting that is declined.
        harness.state_mut().run("calendar.propose", json!({"id": board})).unwrap();
        harness.run_steps(3);
        hit(&mut harness, "proposeDecline", 0);
        harness.run_steps(8);
        assert!(harness.state().session.mailbox.event(board).is_none());
        assert_eq!(mail(&server).len(), 4);
        assert!(mail(&server)[2].starts_with("response:Board:Declined:") && mail(&server)[3].starts_with("proposal:Board:"), "{:?}", mail(&server));

        // A meeting of the owner's own has nobody to propose to: the dialog says so, and neither
        // button does anything.
        let planning = id_of(&harness, "Planning");
        harness.state_mut().execute("calendar.select", json!({"id": planning})).unwrap();
        harness.state_mut().run("calendar.propose", json!({"id": planning})).unwrap();
        harness.run_steps(3);
        assert!(says(&harness, "You organize this meeting, so there is nobody to propose another time to."));
        hit(&mut harness, "proposeTentative", 0);
        hit(&mut harness, "proposeDecline", 0);
        harness.run_steps(8);
        assert_eq!((dialog(&harness), mail(&server).len()), (Some("proposeTime"), 4));
        hit(&mut harness, "dialogCancel", 0);
        assert!(dialog(&harness).is_none());
    }
}

#[test]
fn a_meeting_is_forwarded_from_its_dialogs_send_button_alone() {
    let (mut harness, server) = served(vec![board(), event("e-1", "Budget review", NOW - 3 * HOUR)]);
    let budget = id_of(&harness, "Budget review");
    harness.state_mut().execute("calendar.select", json!({"id": budget})).unwrap();
    let pending = harness.state_mut().run("calendar.forward", json!({"id": budget})).unwrap();
    harness.run_steps(4);
    assert_eq!((pending["pending"].as_str(), dialog(&harness)), (Some("forwardMeeting"), Some("forwardMeeting")));
    assert!(says(&harness, "Budget review") && says(&harness, "Name who it is to go to. Nobody is written to until you press Send."));
    assert!(says(&harness, "The account's server forwards it."));
    // What isn't an address is said, and keeps Send from sending to the others too. (The line
    // of who it goes to has the keyboard when the dialog opens.)
    type_text(&mut harness, "dana@example.org; not an address");
    assert!(says(&harness, "Send writes to: dana@example.org."));
    assert!(says(&harness, "Not an e-mail address: not an address"));
    hit(&mut harness, "forwardSend", 0);
    harness.run_steps(8);
    assert_eq!((dialog(&harness), mail(&server).len()), (Some("forwardMeeting"), 0));
    // Nobody named: Send does nothing, and neither does a key.
    let Some(Dialog::Meeting(MeetingDialog::Forward(forward))) = &mut harness.state_mut().dialog else { panic!("not the dialog") };
    forward.to.clear();
    harness.run_steps(3);
    hit(&mut harness, "forwardSend", 0);
    press(&mut harness, Key::Enter);
    harness.run_steps(8);
    assert_eq!((dialog(&harness), mail(&server).len()), (Some("forwardMeeting"), 0));
    let Some(Dialog::Meeting(MeetingDialog::Forward(forward))) = &mut harness.state_mut().dialog else { panic!("not the dialog") };
    (forward.to, forward.note) = ("Dana Swope <dana@example.org>; eli@example.org".into(), "For you two.".into());
    harness.run_steps(3);
    assert!(says(&harness, "Send writes to: dana@example.org, eli@example.org."));
    // Escape: nothing went anywhere.
    press(&mut harness, Key::Escape);
    harness.run_steps(8);
    assert!(dialog(&harness).is_none() && mail(&server).is_empty() && harness.state().session.mailbox.event_changes.is_empty());

    // Send, pressed: the server forwards it to those two, with the note.
    harness.state_mut().run("calendar.forward", json!({"id": budget, "to": "Dana Swope <dana@example.org>; eli@example.org"})).unwrap_err();
    harness.state_mut().run("calendar.forward", json!({"id": budget})).unwrap();
    harness.run_steps(3);
    let Some(Dialog::Meeting(MeetingDialog::Forward(forward))) = &mut harness.state_mut().dialog else { panic!("not the dialog") };
    (forward.to, forward.note) = ("Dana Swope <dana@example.org>; eli@example.org".into(), "For you two.".into());
    harness.run_steps(3);
    hit(&mut harness, "forwardSend", 0);
    harness.run_steps(8);
    assert!(dialog(&harness).is_none());
    assert_eq!(mail(&server), ["forward:Budget review:2:For you two."]);
    assert_eq!(status(&harness), "Forwarded to 2 people");
    assert!(harness.state().session.mailbox.event_changes.is_empty(), "the server did it");
    assert_eq!(named(&harness, "Budget review").id, budget, "and the item is as it was");

    // Somebody else's meeting: the dialog says that its organizer hears of the forward.
    let board = id_of(&harness, "Board");
    harness.state_mut().execute("calendar.select", json!({"id": board})).unwrap();
    harness.state_mut().run("calendar.forward", json!({"id": board})).unwrap();
    harness.run_steps(3);
    assert!(says(&harness, "The account's server forwards it, and tells the meeting's organizer that you did."));
    hit(&mut harness, "dialogCancel", 0);
    // An item kept on this computer goes as mail from the Outbox, which the dialog says too.
    let review = harness.state().session.mailbox.events.values().find(|e| e.title.starts_with("Design review")).map(|e| e.id).unwrap();
    harness.state_mut().execute("calendar.select", json!({"id": review})).unwrap();
    harness.state_mut().run("calendar.forward", json!({"id": review})).unwrap();
    harness.run_steps(3);
    assert!(says(&harness, "It goes as mail from the Outbox at the next Send/Receive, with the item attached as an iCalendar file."));
    hit(&mut harness, "dialogCancel", 0);
    assert_eq!(mail(&server).len(), 1);
}

#[test]
fn a_proposal_is_taken_up_in_the_meetings_window_and_sent_as_any_update() {
    let proposed = |who: &str, email: &str, hours: i64| RemoteProposal {
        from: Address::new(who, email),
        start: NOW + hours * HOUR,
        end: NOW + (hours + 1) * HOUR,
    };
    let meeting = RemoteEvent { proposals: vec![proposed("Bo Marsh", "bo@example.org", 47), proposed("Cy", "cy@example.org", 68)], ..planning() };
    let (mut harness, server) = served(vec![meeting]);
    let planning = named(&harness, "Planning");
    assert_eq!(planning.proposals.len(), 2);
    let pending = harness.state_mut().run("calendar.proposals", json!({"id": planning.id})).unwrap();
    harness.run_steps(4);
    assert_eq!((pending["pending"].as_str(), dialog(&harness)), (Some("proposals"), Some("proposals")));
    assert!(says(&harness, "Planning") && says(&harness, "It is now on Friday, October 16, 2026 12:00 PM – 1:00 PM."));
    assert!(says(&harness, "Bo Marsh: Friday, October 16, 2026 3:00 PM – 4:00 PM"));
    assert!(says(&harness, "Cy: Saturday, October 17, 2026 12:00 PM – 1:00 PM"));
    assert!(says(&harness, "Nothing is sent until you send the update there."));
    // Accept Bo's: the meeting opens with that time in its fields. Nothing has changed yet, and
    // nobody was written to.
    hit(&mut harness, "proposalAccept", 0);
    harness.run_steps(4);
    assert!(dialog(&harness).is_none());
    let open = windows(&mut harness);
    assert_eq!((&open[0]["event"], &open[0]["dirty"], &open[0]["meeting"]), (&json!(planning.id), &json!(true), &json!(true)));
    assert_eq!(named(&harness, "Planning").start, planning.start);
    assert!(mail(&server).is_empty() && harness.state().session.mailbox.event_changes.is_empty());
    // Send Update asks, as every update does; a no leaves it all as it was.
    let window = open[0]["id"].as_u64().unwrap();
    bar(&mut harness, window, "send");
    assert_eq!(dialog(&harness), Some("confirm"));
    assert!(says(&harness, "Change This Meeting?") && mail(&server).is_empty());
    press(&mut harness, Key::Escape);
    harness.run_steps(4);
    assert!(mail(&server).is_empty() && named(&harness, "Planning").start == planning.start);
    // A yes sends the update, and the meeting is at the proposed time.
    bar(&mut harness, window, "send");
    click_label(&mut harness, "Send Update");
    harness.run_steps(8);
    assert_eq!(mail(&server), ["update:Planning:2"]);
    let now = named(&harness, "Planning");
    assert_eq!((now.start.date.day, now.start.hour, now.end.hour), (16, 15, 16));

    // Scripts and agents get the list as data, and open nothing.
    let listed = harness.state_mut().execute("calendar.proposals", json!({"id": planning.id})).unwrap();
    assert_eq!(listed["proposals"].as_array().map(Vec::len), Some(2));
    assert!(dialog(&harness).is_none());
}

#[test]
fn the_window_bar_sets_importance_response_options_and_teams() {
    for theme in ["light", "dark"] {
        let (mut harness, server) = served(vec![event("e-1", "Budget review", NOW - 3 * HOUR)]);
        harness.state_mut().execute("ui.theme", json!({"value": theme})).unwrap();
        let new = json!({"new": "meeting", "start": "2026-10-16T10:00", "attendees": "Maya Okafor <maya@lumen.example>"});
        let window = harness.state_mut().execute("ui.openWindow", new.clone()).unwrap()["window"].as_u64().unwrap();
        harness.run_steps(3);
        type_text(&mut harness, "Launch");
        let state = |harness: &mut Harness| windows(harness)[0].clone();
        assert_eq!((&state(&mut harness)["importance"], &state(&mut harness)["online"]), (&json!("normal"), &json!(false)), "{theme}");
        // High Importance on; Low takes its place; pressed again it is off; High once more.
        // (Low Importance is the first of the row to go behind `…` when the row is full, as it
        // is for a new meeting: it works from there.)
        let low = |harness: &mut Harness| {
            if harness.state().hit.rect("windowBar", crate::forms::kit::bar::hit_id(window, "importanceLow")).is_some() {
                bar(harness, window, "importanceLow");
            } else {
                bar(harness, window, "more");
                click_label(harness, "Low Importance");
            }
        };
        bar(&mut harness, window, "importanceHigh");
        assert_eq!(state(&mut harness)["importance"], "high");
        low(&mut harness);
        assert_eq!(state(&mut harness)["importance"], "low");
        low(&mut harness);
        assert_eq!(state(&mut harness)["importance"], "normal");
        bar(&mut harness, window, "importanceHigh");
        // Teams Meeting on: the form says where the link will come from.
        bar(&mut harness, window, "teams");
        assert_eq!(state(&mut harness)["online"], true, "{theme}");
        assert!(says(&harness, "The account's server makes it when the meeting is sent"), "{theme}");
        // Response Options: no proposals of another time, please. (It too is behind `…` when
        // the row is full, and opens from there.)
        if harness.state().hit.rect("windowBar", crate::forms::kit::bar::hit_id(window, "responseOptions")).is_some() {
            bar(&mut harness, window, "responseOptions");
        } else {
            bar(&mut harness, window, "more");
            click_label(&mut harness, "Response Options");
        }
        assert!(says(&harness, "Request Responses") && says(&harness, "Allow New Time Proposals"));
        click_label(&mut harness, "Allow New Time Proposals");
        // Send: made on the server as it says, the invitation goes, and the link comes back.
        bar(&mut harness, window, "send");
        harness.run_steps(10);
        assert!(harness.state().windows.is_empty() && dialog(&harness).is_none(), "{theme}");
        assert_eq!(mail(&server), ["invitation:Launch:1"]);
        let there = server.lock().unwrap().events.iter().find(|e| e.title == "Launch").cloned().unwrap();
        assert_eq!((there.importance, there.options.request_responses, there.options.allow_proposals), (Importance::High, true, false));
        let launch = named(&harness, "Launch");
        let link = format!("https://teams.example.org/l/meetup-join/{}", there.id);
        assert_eq!((launch.importance, launch.join_link(), launch.response_options.allow_proposals), (Importance::High, Some(link.as_str()), false));

        // Opened again: the link to join by is shown as it is, with Join and Copy Link; the
        // meeting stays a Teams meeting, which the button says instead of doing anything.
        let window = harness.state_mut().execute("ui.openWindow", json!({"event": launch.id})).unwrap()["window"].as_u64().unwrap();
        harness.run_steps(4);
        assert!(says(&harness, &link) && says(&harness, "Join") && says(&harness, "Copy Link"), "{theme}");
        assert_eq!(state(&mut harness)["joinUrl"], link.as_str());
        bar(&mut harness, window, "teams");
        assert_eq!((&state(&mut harness)["online"], &state(&mut harness)["dirty"]), (&json!(true), &json!(false)));
        // Join opens that link, in the system's browser, and nothing else. (The frame in which
        // the button is let go is the one that asks the system for it.)
        let join = harness.query_all_by_label("Join").last().expect("a Join button").rect().center();
        harness.event(egui::Event::PointerMoved(join));
        harness.step();
        let mut opened: Vec<String> = Vec::new();
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton { pos: join, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            harness.step();
            opened.extend(harness.output().platform_output.commands.iter().filter_map(|command| match command {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
                _ => None,
            }));
        }
        harness.run_steps(2);
        assert_eq!(opened.as_slice(), std::slice::from_ref(&link), "{theme}");
        harness.state_mut().execute("ui.closeWindow", json!({"all": true})).unwrap();
        harness.run_steps(3);

        // An account that can't make Teams meetings: the meeting is made without, and that is said.
        server.lock().unwrap().no_teams = true;
        let window = harness.state_mut().execute("ui.openWindow", new.clone()).unwrap()["window"].as_u64().unwrap();
        harness.run_steps(3);
        type_text(&mut harness, "Retro");
        bar(&mut harness, window, "teams");
        bar(&mut harness, window, "send");
        harness.run_steps(10);
        let retro = named(&harness, "Retro");
        assert!(retro.online.is_none() && !retro.origin.id.is_empty(), "{theme}: {retro:?}");
        assert_eq!(mail(&server).last().map(String::as_str), Some("invitation:Retro:1"));
        assert!(
            status(&harness).starts_with("\"Retro\" couldn't be made a Teams meeting: the account's server wouldn't have it"),
            "{}",
            status(&harness)
        );

        // In a calendar on this computer there is no server to make one: the button says why,
        // and nothing is switched on.
        let local = harness.state().session.mailbox.calendars.iter().find(|c| c.origin.is_local()).map(|c| c.id).unwrap();
        let mut here = new.clone();
        here["calendar"] = json!(local);
        let window = harness.state_mut().execute("ui.openWindow", here).unwrap()["window"].as_u64().unwrap();
        harness.run_steps(3);
        bar(&mut harness, window, "teams");
        assert_eq!(state(&mut harness)["online"], false);
        assert!(says(&harness, "A Teams meeting is made by an account's server, and this calendar is kept on this computer only."), "{theme}");
        harness.state_mut().execute("ui.closeWindow", json!({"all": true})).unwrap();

        // The link of a meeting is followed only when it is an https address: what a file may
        // hold in its place is no link, and Join says so instead of opening anything.
        let id = launch.id;
        if let Ok(event) = harness.state_mut().session.mailbox.event_mut(id) {
            event.online = Some(mailcraft_model::OnlineMeeting { join_url: "javascript:alert(1)".into(), ..Default::default() });
        }
        assert!(harness.state_mut().execute("ui.joinMeeting", json!({"event": id})).is_err());
    }
}

fn right_click(harness: &mut Harness, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(3);
}

#[test]
fn the_menu_of_an_item_offers_what_can_be_done_with_a_meeting() {
    use mailcraft_model::remote::{CalendarsDownload, RemoteCalendar};
    let link = "https://teams.example.org/l/meetup-join/g-1";
    let online = mailcraft_model::OnlineMeeting { join_url: link.into(), ..Default::default() };
    let (mut harness, server) = served(vec![RemoteEvent { online: Some(online), ..board() }, event("e-1", "Physio", NOW + 18 * HOUR)]);
    // The account's server has a second calendar.
    let account = harness.state().session.mailbox.accounts.iter().find(|a| a.email == "avery@mail.example").map(|a| a.id).unwrap();
    let list = CalendarsDownload {
        account,
        email: "avery@mail.example".into(),
        complete: true,
        calendars: vec![
            RemoteCalendar { id: "c-main".into(), name: "Calendar".into(), main: true, ..Default::default() },
            RemoteCalendar { id: "c-side".into(), name: "Side".into(), ..Default::default() },
        ],
        ..Default::default()
    };
    mailcraft_engine::cmd::remote::apply_calendars(&mut harness.state_mut().session, &list).unwrap();
    harness.state_mut().execute("view.calendar", json!({})).unwrap();
    harness.state_mut().execute("view.week", json!({})).unwrap();
    harness.run_steps(8);
    let at = |harness: &Harness, title: &str| {
        harness.state().hit.rect("event", id_of(harness, title)).unwrap_or_else(|| panic!("{title} isn't drawn")).center()
    };

    // Somebody else's meeting, held online: answer it, propose another time, join, forward.
    let board = id_of(&harness, "Board");
    let spot = at(&harness, "Board");
    right_click(&mut harness, spot);
    for label in
        ["Accept", "Tentative", "Decline", "Propose New Time…", "Join Teams Meeting", "Forward…", "Importance", "Show As", "Move to Calendar"]
    {
        assert!(says(&harness, label), "the menu has no {label}");
    }
    // Forward opens its dialog, on that meeting; nothing is sent by opening it.
    click_label(&mut harness, "Forward…");
    harness.run_steps(3);
    assert_eq!(dialog(&harness), Some("forwardMeeting"));
    assert!(says(&harness, "Board") && mail(&server).is_empty());
    press(&mut harness, Key::Escape);
    // Propose New Time opens its own.
    let spot = at(&harness, "Board");
    right_click(&mut harness, spot);
    click_label(&mut harness, "Propose New Time…");
    harness.run_steps(3);
    assert_eq!(dialog(&harness), Some("proposeTime"));
    press(&mut harness, Key::Escape);
    harness.run_steps(3);
    assert!(mail(&server).is_empty());
    // How much it matters to the owner is the owner's own business: set at once, nobody is asked
    // and nobody is told but the server, which keeps it with the owner's copy.
    let spot = at(&harness, "Board");
    right_click(&mut harness, spot);
    click_label(&mut harness, "Importance");
    harness.run_steps(3);
    click_label(&mut harness, "High");
    harness.run_steps(10);
    assert_eq!((named(&harness, "Board").importance, dialog(&harness)), (Importance::High, None));
    assert!(mail(&server).is_empty());
    assert_eq!(server.lock().unwrap().events.iter().find(|e| e.id == "g-1").map(|e| e.importance), Some(Importance::High));
    let _ = board;

    // An appointment of the owner's own: no answers and nothing to join; it can go into the
    // account's other calendar from here.
    let spot = at(&harness, "Physio");
    right_click(&mut harness, spot);
    assert!(says(&harness, "Forward…") && says(&harness, "Move to Calendar"));
    assert!(!says(&harness, "Propose New Time…") && !says(&harness, "Join Teams Meeting"));
    click_label(&mut harness, "Move to Calendar");
    harness.run_steps(3);
    click_label(&mut harness, "Side");
    harness.run_steps(12);
    let side = harness.state().session.mailbox.calendars.iter().find(|c| c.origin.id == "c-side").map(|c| c.id).unwrap();
    assert_eq!(named(&harness, "Physio").calendar, side);
    // The server makes it in the other calendar at once. The one in the calendar it was in goes
    // when the copy has been seen there, which the next looks at the calendar bring: "Side"
    // isn't ticked, and is asked for all the same while something in it is yet to be seen.
    let placed = |server: &Arc<Mutex<Server>>| {
        let there = server.lock().unwrap();
        let mut all: Vec<Option<String>> = there.events.iter().filter(|e| e.title == "Physio").map(|e| there.placed.get(&e.id).cloned()).collect();
        all.sort();
        all
    };
    assert_eq!(placed(&server), [None, Some("c-side".to_string())]);
    let mut at = crate::now_ms();
    for _ in 0..3 {
        at += 31_000.0;
        harness.state_mut().calendar_tick_at(true, at);
        harness.state_mut().poll_calendar();
    }
    assert_eq!(placed(&server), [Some("c-side".to_string())]);
    assert_eq!(named(&harness, "Physio").calendar, side);
    let mailbox = &harness.state().session.mailbox;
    assert!(mailbox.event_changes.is_empty() && mailbox.event_checks.is_empty());
}
