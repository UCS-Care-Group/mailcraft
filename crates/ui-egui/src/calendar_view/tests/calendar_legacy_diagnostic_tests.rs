// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

/// DIAGNOSTIC — not an acceptance test. `Alt+Down` on a clicked occurrence of a series asks
/// nothing: that is where `a_repeating_item_is_asked_about_however_the_grid_changes_it` first
/// fails (calendar_view.rs, historical line 6375). This replays the same input — the same click, the same chord —
/// and prints one line saying what the window held before the chord, what the chord did, and
/// what the very same `calendar.move` does when it is asked directly. That tells apart the
/// three candidates: the key never reached the grid, the command refused the move, or the
/// command ran without a chooser. Temporary; remove once the cause is fixed.
#[test]
fn diagnose_which_link_of_alt_down_asks_nothing() {
    use egui::Key;
    let mut rig = Rig::sample();
    rig.run("view.week", json!({}));
    let monday = d(2026, 10, 12);
    let series = rig.app.session.mailbox.events.values().find(|e| e.title == "Team stand-up").cloned().expect("the sample's stand-up");
    assert!(series.recurrence.is_some(), "fixture: the stand-up repeats");
    let series_id = series.id;
    // Everything the window holds about this item, read from the window itself.
    let look = |rig: &Rig| {
        let now = rig.app.session.mailbox.event(series_id).cloned();
        let ran = match &rig.app.ran.2 {
            Ok(v) => json!({ "ok": v.clone() }),
            Err(e) => json!({ "error": e.clone() }),
        };
        json!({
            "dialog": rig.dialog(),
            "status": rig.app.status_msg.as_ref().map(|(said, _)| said.clone()).unwrap_or_default(),
            "ran": [rig.app.ran.0, rig.app.ran.1.clone(), ran],
            "focus": [rig.app.session.sel.event, rig.app.session.sel.event_day.map(|day| day.iso())],
            "set": rig.app.session.sel.events.iter().map(|item| (item.id, item.day.map(|day| day.iso()))).collect::<Vec<_>>(),
            "series": now
                .as_ref()
                .map(|e| (iso_minute(e.start), iso_minute(e.end), e.exceptions.iter().map(|day| day.iso()).collect::<Vec<_>>())),
            "own": rig.all("Team stand-up").into_iter().filter(|e| !e.3).map(|e| (iso_minute(e.1), iso_minute(e.2))).collect::<Vec<_>>(),
        })
    };
    let height = rig.at("Team stand-up").center().y;
    let spot = pos2(rig.day("allDay", monday).expect("Monday is drawn").center().x - 20.0, height);
    rig.click(spot, 1);
    assert_eq!(rig.selected(), (Some(series_id), Some(monday)), "the click selects Monday's occurrence");
    // What `has_keyboard` asks of the window (line 410), in the state the chord's own frame
    // starts from: a modifier changes neither the focus nor an open popup.
    let ctx = rig.ctx.clone();
    let before = json!({
        "has_keyboard": has_keyboard(&rig.app, &ctx),
        "backstage": rig.app.ui.backstage,
        "focused": format!("{:?}", ctx.memory(|m| m.focused())),
        "popup_open": egui::Popup::is_any_open(&ctx),
        "active_is_main": crate::keys::active(&ctx) == crate::keys::main_layer(),
        "keys_had": ctx.data(|d| d.get_temp::<bool>(egui::Id::unique(KEYS_ID))).unwrap_or(false),
        "look": look(&rig),
    });
    rig.chord(ALT, Key::ArrowDown);
    let first = look(&rig);
    // A chord that did nothing at all is sent once more: a one-frame accident is then told
    // apart from a key that is never the grid's while that occurrence is selected.
    let quiet = rig.dialog().is_none() && rig.app.ran.1 != "calendar.move";
    if quiet {
        rig.chord(ALT, Key::ArrowDown);
    }
    let again = quiet.then(|| look(&rig));
    // And the command itself, with the very parameters `nudge_event` builds for this chord
    // (lines 3161–3165): the chooser's own code, with no key in front of it.
    let slot = Scale::of(time_scale(&rig.app.session)).slot;
    let params = json!({ "id": series_id, "start": iso_minute(series.start.add_minutes(slot)) });
    let direct = match rig.app.run("calendar.move", params.clone()) {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "error": e }),
    };
    rig.frames(2);
    let after_direct = look(&rig);
    let seen = json!({
        "before_chord": before,
        "after_chord": first,
        "after_second_chord": again,
        "move_params": params,
        "move_itself": direct,
        "after_move_itself": after_direct,
    });
    println!("ALT-DOWN-DIAG {seen}");
    assert!(first["dialog"] == json!("confirm"), "the chooser is asked on Alt+Down itself: {seen}");
}
