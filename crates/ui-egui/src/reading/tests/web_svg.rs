// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #565: an SVG picture on the web, once the owner asks for the message's pictures, is shown as
//! the others are; nothing the drawing itself names is asked for, and a damaged one says why.

use serde_json::json;

use super::{Host, app_with, png, point_at, says, select, shown, stand_in_at, web_message, web_pictures, window};

#[test]
fn an_svg_on_the_web_is_shown_once_asked_for_and_nothing_it_names_is() {
    let pictures = web_pictures("drawings.example", 3);
    let (mut app, ids) = app_with(vec![web_message(0, "a@drawings.example", "Drawings", &pictures)]);
    let host = Host::answering(&mut app, |url| match url.rsplit('/').next().unwrap_or("") {
        "0.png" => Ok(b"<svg xmlns='http://www.w3.org/2000/svg' width='40' height='30'>\
                        <rect width='40' height='30' fill='url(https://drawings.example/paint.svg#p) #2e8b57'/></svg>"
            .to_vec()),
        "1.png" => Ok(b"<svg xmlns='http://www.w3.org/2000/svg' width='40' height='30'><rect></svg>".to_vec()),
        _ => Ok(png(40, 30)),
    });
    let id = ids[0];
    let mut harness = window(app, (1280.0, 900.0));
    select(&mut harness, id);
    harness.state_mut().execute("ui.downloadPictures", json!({})).unwrap();
    harness.run_steps(12);
    // The drawing and the PNG are shown; only the message's own three addresses were asked for.
    assert_eq!((host.asked(), shown(&harness, id)), (pictures.clone(), (2, 1, 1)));
    harness.run_steps(20);
    assert_eq!(host.asked(), pictures);
    let at = stand_in_at(&harness, id, "/1.png");
    point_at(&mut harness, at);
    assert!(says(&harness, "This picture couldn't be downloaded: it is a drawing (SVG) that is damaged or can't be shown."));
}
