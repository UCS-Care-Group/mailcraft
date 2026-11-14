// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The pictures bar says why "Always download pictures from" can't be used when the server
//! didn't vouch for the sender (#1347). Addresses are invented.

use serde_json::json;

use super::{Host, app_with, png, says, select, web_message, window};

const ALWAYS: &str = "Always download pictures from news@shop.example";

fn pictures() -> Vec<String> {
    vec!["https://a.shop.example/p/0.png".to_string(), "https://a.shop.example/p/1.png".to_string()]
}

#[test]
fn a_remembered_sender_or_domain_gets_no_greyed_link() {
    for (senders, domains) in [(vec!["news@shop.example"], vec![]), (vec![], vec!["shop.example"])] {
        let mut unvouched = web_message(0, "news@shop.example", "Not vouched", &pictures());
        unvouched.headers.clear();
        let (mut app, ids) = app_with(vec![unvouched]);
        app.ui.picture_senders = senders.into_iter().map(String::from).collect();
        app.ui.picture_domains = domains.into_iter().map(String::from).collect();
        let _host = Host::answering(&mut app, |_| Ok(png(40, 30)));
        let mut harness = window(app, (1280.0, 800.0));
        select(&mut harness, ids[0]);
        // The bar says it is asked first, and offers nothing to remember.
        assert!(says(&harness, "You always download this sender's pictures"));
        assert!(!says(&harness, "Always download pictures from") && !says(&harness, "Always from @"));
        assert!(harness.state().hit.rect("alwaysDownloadPictures", ids[0]).is_none());
    }
}

#[test]
fn an_unvouched_sender_is_shown_greyed_out_without_the_domain() {
    let mut unvouched = web_message(0, "news@shop.example", "Not vouched", &pictures());
    unvouched.headers.clear();
    let vouched = web_message(1, "news@shop.example", "Vouched", &pictures());
    let junk = web_message(2, "news@shop.example", "Junk", &pictures());
    let mut junk_unvouched = web_message(3, "news@shop.example", "Junk, not vouched", &pictures());
    junk_unvouched.headers.clear();
    let (mut app, ids) = app_with(vec![unvouched, vouched, junk, junk_unvouched]);
    app.execute("mail.junk", json!({"ids": [ids[2], ids[3]]})).unwrap();
    let _host = Host::answering(&mut app, |_| Ok(png(40, 30)));
    let mut harness = window(app, (1280.0, 800.0));

    // Not vouched for: the option is there, can't be clicked, and the domain's is not there.
    select(&mut harness, ids[0]);
    assert!(says(&harness, "Pictures not downloaded."));
    assert!(says(&harness, ALWAYS));
    assert!(!says(&harness, "Always from @shop.example"));
    assert!(harness.state().hit.rect("alwaysDownloadPictures", ids[0]).is_none());
    assert!(harness.state().hit.rect("alwaysDownloadPicturesDomain", ids[0]).is_none());
    assert!(harness.state().hit.rect("downloadPictures", ids[0]).is_some());
    assert!(harness.state().ui.picture_senders.is_empty());

    // Vouched for: both, and both can be used.
    select(&mut harness, ids[1]);
    assert!(says(&harness, ALWAYS) && says(&harness, "Always from @shop.example"));
    assert!(harness.state().hit.rect("alwaysDownloadPictures", ids[1]).is_some());
    assert!(harness.state().hit.rect("alwaysDownloadPicturesDomain", ids[1]).is_some());

    // Junk E-mail, vouched for or not: neither.
    for id in [ids[2], ids[3]] {
        select(&mut harness, id);
        assert!(!says(&harness, "Always download pictures from") && !says(&harness, "Always from @"));
        assert!(harness.state().hit.rect("alwaysDownloadPictures", id).is_none());
        assert!(harness.state().hit.rect("downloadPictures", id).is_some());
    }
}
