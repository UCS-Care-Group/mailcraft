// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sender images: who is looked up, what comes of it, and the switch.

use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_model::{Contact, Labeled, Mailbox};

use super::*;
use crate::Services;

const NOW: i64 = 1_791_993_600;

fn png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 30, 30, 255])).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// A window with sender images on, whose host answers each address with what `answer` makes of
/// it and notes it; and the mail in its list: from a contact, from somebody the owner wrote to,
/// from a stranger, and from the contact again in Junk E-mail.
fn window(answer: impl Fn(&str) -> Result<Vec<u8>, String> + 'static) -> (MailApp, Arc<Mutex<Vec<String>>>, Vec<Message>) {
    let mut mailbox = Mailbox::with_local_account("Avery Quinn", "avery@lumen-works.example");
    // An account on a mail server, which vouches for every sender (a remembered domain needs it).
    mailbox.accounts[0].kind = mailcraft_model::AccountKind::Imap;
    mailbox.accounts[0].incoming =
        Some(mailcraft_model::Server { host: "imap.example.org".into(), port: 993, security: mailcraft_model::Security::Tls });
    let account = mailbox.accounts[0].id;
    let [inbox, sent, junk] = [FolderKind::Inbox, FolderKind::Sent, FolderKind::Junk].map(|kind| mailbox.folder_of_kind(account, kind).unwrap());
    mailbox.add_contact(Contact { display_name: "Ada".into(), emails: vec![Labeled::new("work", " Ada@Example.org ")], ..Default::default() });
    let mail = |folder, from: &str, to: &str| Message {
        folder,
        from: Address::new("", from),
        to: vec![Address::new("", to)],
        date: NOW,
        // On the server (mail from a file is never vouched for).
        uid: 1,
        headers: vec![("Authentication-Results".into(), format!("mx.example.org; dmarc=pass header.from={}", from.rsplit('@').next().unwrap_or("")))],
        ..Default::default()
    };
    mailbox.add_message(mail(sent, "avery@lumen-works.example", "Grace@Shop.example")).unwrap();
    let ids: Vec<Id> = [
        mail(inbox, "ada@example.org", "avery@lumen-works.example"),
        mail(inbox, "grace@shop.example", "avery@lumen-works.example"),
        mail(inbox, "stranger@spam.example", "avery@lumen-works.example"),
        mail(junk, "ada@example.org", "avery@lumen-works.example"),
    ]
    .into_iter()
    .map(|m| mailbox.add_message(m).unwrap())
    .collect();
    let shown = ids.iter().map(|id| (*mailbox.messages[id]).clone()).collect();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let noted = asked.clone();
    let services = Services {
        // A host that could download a message's pictures (it is never asked to here).
        pictures: Some(Box::new(|_| mpsc::channel().1)),
        sender_images: Some(Box::new(move |address: String, _| {
            let (tx, rx) = mpsc::channel();
            tx.send(answer(&address)).unwrap();
            noted.lock().unwrap().push(address);
            rx
        })),
        ..Default::default()
    };
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), services);
    app.ui.sender_images = true;
    (app, asked, shown)
}

/// One frame of the list: which of `shown` have a picture.
fn frame(app: &mut MailApp, ctx: &egui::Context, shown: &[Message]) -> Vec<bool> {
    let mut got = Vec::new();
    ctx.run_ui(egui::RawInput::default(), |ui| got = shown.iter().map(|m| picture(app, ui.ctx(), m, &m.from).is_some()).collect())
        .drop_without_applying_deltas();
    got
}

#[test]
fn only_known_senders_are_looked_up_and_never_for_junk() {
    let (mut app, asked, shown) = window(|_| Ok(png()));
    let ctx = egui::Context::default();
    for _ in 0..4 {
        frame(&mut app, &ctx, &shown);
    }
    assert_eq!(*asked.lock().unwrap(), ["ada@example.org", "grace@shop.example"]);
    // The contact and the one written to have their pictures; the stranger and Junk keep initials.
    assert_eq!(frame(&mut app, &ctx, &shown), [true, true, false, false]);
}

#[test]
fn what_is_no_picture_keeps_the_initials_and_is_not_asked_again() {
    let (mut app, asked, shown) = window(|address| if address.starts_with("ada") { Ok(b"<svg/>".to_vec()) } else { Err("404".into()) });
    let ctx = egui::Context::default();
    for _ in 0..6 {
        assert_eq!(frame(&mut app, &ctx, &shown), [false; 4]);
    }
    assert_eq!(asked.lock().unwrap().len(), 2);
}

#[test]
fn off_asks_nobody_and_turning_it_off_forgets() {
    let (mut app, asked, shown) = window(|_| Ok(png()));
    let ctx = egui::Context::default();
    app.run("ui.senderImages", json!({"value": false})).unwrap();
    for _ in 0..4 {
        assert_eq!(frame(&mut app, &ctx, &shown), [false; 4]);
    }
    assert!(asked.lock().unwrap().is_empty());
    assert_eq!(app.run("ui.senderImages", json!({})).unwrap(), json!({"value": true}));
    for _ in 0..4 {
        frame(&mut app, &ctx, &shown);
    }
    assert_eq!(asked.lock().unwrap().len(), 2);
    // What the host was handed lets go at once, so a look-up in flight asks nothing more.
    let token = Arc::downgrade(&app.sender_images.alive);
    app.run("ui.senderImages", json!({"value": false})).unwrap();
    assert!(app.sender_images.looks.is_empty());
    assert_eq!(token.strong_count(), 0);
}

#[test]
fn waiting_for_a_free_place_does_not_repaint_at_once() {
    // A host that never answers, and more senders on screen than it is asked for at a time.
    let pending = Arc::new(Mutex::new(Vec::new()));
    let (mut app, _, mut shown) = window(|_| Ok(png()));
    let held = pending.clone();
    app.services.sender_images = Some(Box::new(move |_, _| {
        let (tx, rx) = mpsc::channel();
        held.lock().unwrap().push(tx);
        rx
    }));
    app.ui.download_pictures = true;
    let stranger = shown[2].clone();
    shown.extend((0..AT_A_TIME + 2).map(|n| Message { from: Address::new("", format!("s{n}@spam.example")), ..stranger.clone() }));
    let ctx = egui::Context::default();
    for _ in 0..4 {
        frame(&mut app, &ctx, &shown);
    }
    assert_eq!(pending.lock().unwrap().len(), AT_A_TIME);
    assert!(!ctx.requested_repaint_last_pass());
}

#[test]
fn a_draft_to_a_stranger_is_not_looked_up() {
    let (mut app, asked, shown) = window(|_| Ok(png()));
    app.ui.download_pictures = true;
    let account = app.session.mailbox.accounts[0].id;
    let drafts = app.session.mailbox.folder_of_kind(account, FolderKind::Drafts).unwrap();
    let draft = Message { folder: drafts, from: shown[0].to[0].clone(), to: vec![Address::new("", "stranger@spam.example")], ..shown[0].clone() };
    let ctx = egui::Context::default();
    for _ in 0..4 {
        ctx.run_ui(egui::RawInput::default(), |ui| {
            picture(&mut app, ui.ctx(), &draft, &draft.to[0]);
        })
        .drop_without_applying_deltas();
    }
    assert!(asked.lock().unwrap().is_empty());
}

#[test]
fn clearing_the_pictures_stops_what_is_in_flight() {
    let (mut app, _, shown) = window(|_| Ok(png()));
    let ctx = egui::Context::default();
    frame(&mut app, &ctx, &shown);
    let token = Arc::downgrade(&app.sender_images.alive);
    app.run("ui.clearPictureCache", json!({})).unwrap();
    assert_eq!(token.strong_count(), 0);
}

#[test]
fn a_stranger_is_looked_up_when_their_mails_pictures_come_anyway() {
    let all_mail = |app: &mut MailApp| app.ui.download_pictures = true;
    let their_domain = |app: &mut MailApp| app.ui.picture_domains = vec!["spam.example".into()];
    for say_so in [all_mail, their_domain] {
        let (mut app, asked, shown) = window(|_| Ok(png()));
        say_so(&mut app);
        let ctx = egui::Context::default();
        for _ in 0..4 {
            frame(&mut app, &ctx, &shown);
        }
        assert_eq!(*asked.lock().unwrap(), ["ada@example.org", "grace@shop.example", "stranger@spam.example"]);
        // Still nothing for Junk E-mail.
        assert_eq!(frame(&mut app, &ctx, &shown), [true, true, true, false]);
    }
    // A remembered domain holds only for mail the server vouches for: a `From:` is anybody's.
    let (mut app, asked, mut shown) = window(|_| Ok(png()));
    their_domain(&mut app);
    shown[2].headers.clear();
    let ctx = egui::Context::default();
    for _ in 0..4 {
        frame(&mut app, &ctx, &shown);
    }
    assert_eq!(*asked.lock().unwrap(), ["ada@example.org", "grace@shop.example"]);
}

#[test]
fn a_known_sender_has_no_picture_in_a_folder_under_junk() {
    let (mut app, asked, mut shown) = window(|_| Ok(png()));
    let account = app.session.mailbox.accounts[0].id;
    let junk = app.session.mailbox.folder_of_kind(account, FolderKind::Junk).unwrap();
    shown[3].folder = app.session.mailbox.add_folder(account, Some(junk), "Later").unwrap();
    let ctx = egui::Context::default();
    for _ in 0..4 {
        assert!(!frame(&mut app, &ctx, &shown)[3]);
    }
    assert_eq!(*asked.lock().unwrap(), ["ada@example.org", "grace@shop.example"]);
}

#[test]
fn a_sites_icon_is_shown_for_a_sender_but_never_in_a_message() {
    let mut ico = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(16, 16, image::Rgba([0, 90, 200, 255])).write_to(&mut ico, image::ImageFormat::Ico).unwrap();
    let ico = ico.into_inner();
    assert!(images::probe(&ico).is_none() && images::decode(&ico, 64, 64).is_none());
    let (mut app, _, shown) = window(move |_| Ok(ico.clone()));
    let ctx = egui::Context::default();
    for _ in 0..4 {
        frame(&mut app, &ctx, &shown);
    }
    assert_eq!(frame(&mut app, &ctx, &shown), [true, true, false, false]);
}

#[test]
fn a_senders_picture_needs_the_servers_word() {
    // Anybody can write a contact's address in `From:`: with no word from the server, or a
    // failed one, the contact keeps their initials and nobody is asked.
    for said in [None, Some("mx.example.org; dmarc=fail header.from=example.org")] {
        let (mut app, asked, mut shown) = window(|_| Ok(png()));
        shown[0].headers = said.map(|value| vec![("Authentication-Results".to_string(), value.to_string())]).unwrap_or_default();
        let ctx = egui::Context::default();
        for _ in 0..4 {
            frame(&mut app, &ctx, &shown);
        }
        assert_eq!(*asked.lock().unwrap(), ["grace@shop.example"], "{said:?}");
        assert_eq!(frame(&mut app, &ctx, &shown), [false, true, false, false], "{said:?}");
    }
}

#[test]
fn who_sent_mail_went_to_needs_no_word_from_the_server() {
    let (mut app, asked, _) = window(|_| Ok(png()));
    let account = app.session.mailbox.accounts[0].id;
    let sent = app.session.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    let mine = Message {
        folder: sent,
        from: Address::new("", "avery@lumen-works.example"),
        to: vec![Address::new("", "ada@example.org")],
        ..Default::default()
    };
    let ctx = egui::Context::default();
    let mut got = false;
    for _ in 0..4 {
        ctx.run_ui(egui::RawInput::default(), |ui| got = picture(&mut app, ui.ctx(), &mine, &mine.to[0]).is_some()).drop_without_applying_deltas();
    }
    assert!(got);
    assert_eq!(*asked.lock().unwrap(), ["ada@example.org"]);
}

#[test]
fn who_is_known_follows_the_contacts_and_sent_items() {
    let (mut app, _, _) = window(|_| Ok(png()));
    assert!(is_known(&mut app, "ada@example.org") && is_known(&mut app, "grace@shop.example"));
    let account = app.session.mailbox.accounts[0].id;
    let [inbox, sent, outbox] =
        [FolderKind::Inbox, FolderKind::Sent, FolderKind::Outbox].map(|kind| app.session.mailbox.folder_of_kind(account, kind).unwrap());
    let to_grace = *app.session.mailbox.messages_in(sent).next().map(|m| &m.id).unwrap();
    let from_ada = *app.session.mailbox.messages_in(inbox).next().map(|m| &m.id).unwrap();
    let change = |app: &mut MailApp, f: &dyn Fn(&mut Mailbox)| {
        f(&mut app.session.mailbox);
        app.session.mailbox_changed();
    };
    // Mail on its way: who it is to is known as it is queued, and stays so once it went.
    let queued = Message { folder: outbox, to: vec![Address::new("", " Lin@New.example ")], ..Default::default() };
    change(&mut app, &|mb| drop(mb.add_message(queued.clone())));
    assert!(is_known(&mut app, "lin@new.example"));
    let queued = *app.session.mailbox.messages_in(outbox).next().map(|m| &m.id).unwrap();
    change(&mut app, &|mb| mb.move_message(queued, sent).unwrap());
    assert!(is_known(&mut app, "lin@new.example"));
    // A contact added, and its address changed.
    let kim = Contact { emails: vec![Labeled::new("home", "kim@new.example")], ..Default::default() };
    change(&mut app, &|mb| {
        mb.add_contact(kim.clone());
    });
    assert!(is_known(&mut app, "kim@new.example"));
    let kim = *app.session.mailbox.contacts.iter().find(|(_, c)| c.emails.iter().any(|e| e.value == "kim@new.example")).unwrap().0;
    change(&mut app, &|mb| mb.contact_mut(kim).unwrap().emails = vec![Labeled::new("home", "kim@other.example")]);
    assert!(!is_known(&mut app, "kim@new.example") && is_known(&mut app, "kim@other.example"));
    // Mail in the Inbox deleted for good is not what made anybody known; the one sent to Grace is.
    change(&mut app, &|mb| drop(mb.remove_message(from_ada).unwrap()));
    assert!(is_known(&mut app, "grace@shop.example"));
    change(&mut app, &|mb| drop(mb.remove_message(to_grace).unwrap()));
    assert!(!is_known(&mut app, "grace@shop.example") && is_known(&mut app, "lin@new.example"));
    // A contact removed.
    let ada = *app.session.mailbox.contacts.iter().find(|(_, c)| c.display_name == "Ada").unwrap().0;
    change(&mut app, &|mb| mb.remove_contact(ada).unwrap());
    assert!(!is_known(&mut app, "ada@example.org"));
}

#[test]
fn a_picture_too_large_to_decode_on_the_window_keeps_the_initials() {
    // Small as a file (one colour), large once decoded.
    let mut big = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(MAX_SIDE + 1, 8, image::Rgba([200, 30, 30, 255])).write_to(&mut big, image::ImageFormat::Png).unwrap();
    let big = big.into_inner();
    let (mut app, _, shown) = window(move |_| Ok(big.clone()));
    let ctx = egui::Context::default();
    for _ in 0..4 {
        assert_eq!(frame(&mut app, &ctx, &shown), [false; 4]);
    }
}

#[test]
fn undoing_a_send_takes_back_who_it_went_to() {
    let (mut app, _, _) = window(|_| Ok(png()));
    // A local account files what it sends in Sent Items at once.
    app.session.mailbox.accounts[0].kind = mailcraft_model::AccountKind::Local;
    app.session.mailbox_changed();
    app.run("mail.new", json!({"to": "lin@new.example", "subject": "Hello"})).unwrap();
    app.run("compose.saveDraft", json!({})).unwrap();
    assert!(!is_known(&mut app, "lin@new.example"));
    // Sent, the draft goes and the message comes: as many messages as before.
    app.run("compose.send", json!({})).unwrap();
    assert!(is_known(&mut app, "lin@new.example"));
    assert!(app.session.undo());
    assert!(!is_known(&mut app, "lin@new.example"));
    assert!(app.session.redo());
    assert!(is_known(&mut app, "lin@new.example"));
}

/// `cargo test -p mailcraft-ui-egui --release -- --ignored --nocapture known_senders_of_200000`
#[test]
#[ignore = "builds 200,000 messages and prints how long keeping up with them takes; run by hand"]
fn known_senders_of_200000_messages_during_a_send_receive() {
    use std::time::{Duration, Instant};
    let (mut app, _, _) = window(|_| Ok(png()));
    let account = app.session.mailbox.accounts[0].id;
    let [inbox, sent] = [FolderKind::Inbox, FolderKind::Sent].map(|kind| app.session.mailbox.folder_of_kind(account, kind).unwrap());
    // One message in ten is mail the owner sent.
    let message = |i: usize| {
        let (folder, from, to) = if i.is_multiple_of(10) {
            (sent, "avery@lumen-works.example".to_string(), format!("r{}@shop.example", i % 20_000))
        } else {
            (inbox, format!("s{}@spam.example", i % 5_000), "avery@lumen-works.example".to_string())
        };
        Message { folder, from: Address::new("", from), to: vec![Address::new("", to)], date: NOW, ..Default::default() }
    };
    for i in 0..200_000 {
        app.session.mailbox.add_message(message(i)).unwrap();
    }
    app.session.mailbox_changed();
    let start = Instant::now();
    assert!(is_known(&mut app, "r10@shop.example"));
    eprintln!("gathered from {} messages in {:?}", app.session.mailbox.messages.len(), start.elapsed());
    // A Send/Receive: 400 changes, each a page of 50 messages coming down with a message marked
    // read, and every tenth one a message the server no longer has.
    let mut took = [Vec::new(), Vec::new()];
    for page in 0..400 {
        let mb = &mut app.session.mailbox;
        for i in 0..50 {
            mb.add_message(message(200_000 + page * 50 + i)).unwrap();
        }
        let older = *mb.messages.keys().nth(page * 7).unwrap();
        mb.message_mut(older).unwrap().read = true;
        let removes = page % 10 == 1;
        if removes {
            let gone = *mb.messages_in(inbox).nth(page).map(|m| &m.id).unwrap();
            mb.remove_message(gone).unwrap();
        }
        app.session.mailbox_changed();
        let one = Instant::now();
        is_known(&mut app, "nobody@spam.example");
        took[usize::from(removes)].push(one.elapsed());
    }
    for (what, took) in ["without", "with"].iter().zip(&took) {
        let all: Duration = took.iter().sum();
        eprintln!(
            "{} changes {what} a message removed: {:?} each, the slowest {:?}",
            took.len(),
            all / took.len() as u32,
            took.iter().max().unwrap()
        );
    }
    assert!(is_known(&mut app, &format!("r{}@shop.example", (200_000 + 399 * 50) % 20_000)));
    // A sent message deleted for good: the mail is gathered again.
    let sent_one = *app.session.mailbox.messages_in(sent).next().map(|m| &m.id).unwrap();
    app.session.mailbox.remove_message(sent_one).unwrap();
    app.session.mailbox_changed();
    let start = Instant::now();
    is_known(&mut app, "nobody@spam.example");
    eprintln!("a sent message deleted for good: gathered again in {:?}", start.elapsed());
}
