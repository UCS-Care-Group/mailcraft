// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The title row is compact (#1158): the search box, the account's picture and the buttons fit a
//! row of 36 points, and the tabs and the ribbon start right under it, so the message list and
//! the reading pane keep the height.

use serde_json::json;

use crate::people_view::testing;

#[test]
fn the_title_row_is_compact_and_the_tabs_follow_it() {
    for theme in ["light", "dark"] {
        let mut h = testing::window("view.mail");
        let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
        h.state_mut().execute("ui.theme", json!({"value": theme})).unwrap();
        h.run_steps(4);
        if let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") {
            h.render().unwrap().save(std::path::Path::new(&dir).join(format!("title-row-{theme}.png"))).unwrap();
        }
        assert!(h.state().search_box_shown, "the search box is in the row");
        // The numbers are the ones the row is meant to have (36 points, a box of 28), not the
        // constants, so that a row made taller again fails here.
        let search = h.state().hit.rect("searchBox", 0).expect("the search box");
        assert!((search.height() - 28.0).abs() < 0.5, "{theme}: the box is 28 points high, {search:?}");
        let row_bottom = search.center().y + 18.0;
        // Under the row come the tabs (30 points) and then the ribbon's card, 13 points further
        // down: 43 in all. A taller row (48 points, as before) would put New Email 49 down.
        let new_mail = h.state().hit.rect("ribbon", crate::ribbon::hit_id("mail.new")).expect("New Email in the ribbon");
        let below = new_mail.min.y - row_bottom;
        assert!((40.0..46.0).contains(&below), "{theme}: New Email {below} points under the row");
    }
}
