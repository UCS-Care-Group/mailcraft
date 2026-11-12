// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encrypted messages in the window (#30 part 6): what starts opening one, what an agent can
//! and can't do about one on screen, and what an opened one may fetch. A stand-in host plays the
//! key holders; the messages are invented and their cryptography is the crates' own tests.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_engine::decrypt::{DecryptJob, Failure, Plain};
use mailcraft_model::{Id, Mailbox};
use serde_json::json;
use zeroize::Zeroizing;

use super::remote::{Access, access};
use crate::control::{ControlRequest, Outcome};
use crate::decrypting::{DWELL_MS, DecryptHost};
use crate::{MailApp, Services};

/// 2026-11-09.
const NOW: i64 = 1_794_182_400;
const SECRET: &str = "the vault code is 4417";

/// A host that opens nothing by itself: it counts the jobs it is given and keeps their senders,
/// so that a test answers them when it likes.
#[derive(Default)]
struct Stand {
    opened: AtomicUsize,
    answers: Mutex<Vec<(Id, Sender<Result<Plain, Failure>>)>>,
}

impl DecryptHost for Stand {
    fn open(&self, job: DecryptJob) -> Receiver<Result<Plain, Failure>> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        self.answers.lock().unwrap().push((job.id, tx));
        rx
    }
}

fn smime(subject: &str) -> Vec<u8> {
    format!(
        "From: alice@smime.example\r\nTo: bob@smime.example\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\nContent-Type: application/pkcs7-mime; smime-type=enveloped-data; name=smime.p7m\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=smime.p7m\r\n\r\nMA8GCSqGSIb3DQEHA6ACMAA=\r\n"
    )
    .into_bytes()
}

fn plain(authenticated: bool) -> Plain {
    let html = format!(
        "Content-Type: text/html; charset=utf-8\r\n\r\n<p>{SECRET}</p><img src=\"https://tracker.example/p.png\" width=\"40\" height=\"40\">\r\n"
    );
    Plain { data: Zeroizing::new(html.into_bytes()), inline: false, signatures: Vec::new(), authenticated }
}

fn app_with(count: usize) -> (MailApp, Vec<Id>, Arc<Stand>) {
    let mut mb = Mailbox::with_local_account("Bob", "bob@smime.example");
    let inbox = mb.inbox().unwrap();
    let mut ids = Vec::new();
    for i in 0..count {
        let mut m = mailcraft_mime::parse_fetched(&smime(&format!("Vault {i}")));
        m.folder = inbox;
        ids.push(mb.add_message(m).unwrap());
    }
    let mut s = Session::at(mb, NOW, 0);
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    let host = Arc::new(Stand::default());
    // (Pictures can be downloaded in this build: the answer is never asked for.)
    let services = Services { decrypting: Some(host.clone()), pictures: Some(Box::new(|_| channel().1)), ..Services::default() };
    let mut app = MailApp::new(s, services);
    app.autosave = false;
    app.auto_sync = false;
    (app, ids, host)
}

fn answer_all(app: &mut MailApp, host: &Stand, authenticated: bool) {
    let waiting: Vec<_> = host.answers.lock().unwrap().drain(..).collect();
    for (_, tx) in waiting {
        tx.send(Ok(plain(authenticated))).unwrap();
    }
    app.tick_decrypting(&egui::Context::default());
}

/// The request `method` as an agent allowed nothing in advance sends it.
fn agent(app: &mut MailApp, method: &str, params: serde_json::Value) -> serde_json::Value {
    let (req, _rx) = ControlRequest::new(method, params);
    match crate::control::handle(app, &egui::Context::default(), &req.from_agent(Vec::new())) {
        Outcome::Done(v) => v,
        _ => json!({"ok": false, "error": "asked"}),
    }
}

/// The owner selects message `id` with their own input (a click or an arrow key on its row), and
/// the window takes a frame.
fn owner_selects(app: &mut MailApp, id: Id) {
    app.session.run("mail.select", &json!({"id": id})).unwrap();
    frame(app);
}

/// A frame: input is taken in (none injected), and the window looks at what was chosen.
fn frame(app: &mut MailApp) {
    app.raw_input_hook(&mut egui::RawInput::default());
    app.track_choice();
}

/// Frames in which nothing happens.
fn idle(app: &mut MailApp, frames: usize) {
    for _ in 0..frames {
        frame(app);
    }
}

fn stays_closed(app: &mut MailApp, host: &Stand, id: Id) {
    let ctx = egui::Context::default();
    for step in 0..20 {
        assert!(app.opened_view_at(&ctx, id, 1_000.0 + f64::from(step) * 100.0).is_none());
    }
    assert_eq!(host.opened.load(Ordering::SeqCst), 0, "a key holder was asked for a message the owner did not choose");
}

#[test]
fn a_message_an_agent_selected_is_not_opened_until_the_owner_says_so_and_then_it_is_not_in_a_screenshot() {
    let (mut app, ids, host) = app_with(1);
    let ctx = egui::Context::default();
    let id = ids[0];
    // `mail.select` needs nothing from the owner. The window draws the message and, for any other
    // reason, would open it: no key holder is asked.
    let answer = agent(&mut app, "engine.execute", json!({"command": "mail.select", "params": {"id": id}}));
    assert_eq!(answer["ok"], true, "{answer}");
    idle(&mut app, 8);
    stays_closed(&mut app, &host, id);
    assert!(app.decryption_needs_owner(id));
    // The owner's Open: it begins once it has stayed.
    app.owner_opens(id);
    assert!(app.opened_view_at(&ctx, id, 5_000.0).is_none());
    assert!(app.opened_view_at(&ctx, id, 5_000.0 + DWELL_MS + 1.0).is_none());
    assert_eq!(host.opened.load(Ordering::SeqCst), 1);
    answer_all(&mut app, &host, true);
    assert!(app.opened_view_at(&ctx, id, 6_000.0).is_some_and(|m| m.body.text.contains(SECRET)));
    // It is on screen (selected): no screenshot for an agent, and nothing says why but the words.
    assert!(app.decrypted_on_screen());
    let shot = agent(&mut app, "ui.screenshot", json!({}));
    assert_eq!(shot["ok"], false, "{shot}");
    assert!(shot["error"].as_str().unwrap().contains("Encrypted message"), "{shot}");
    // Once it is let go of it is off screen: the screenshot is for the agent again.
    app.session.forget_decryption(id);
    assert!(!app.decrypted_on_screen());
}

#[test]
fn a_window_an_agent_opens_through_engine_execute_never_opens_the_message() {
    let (mut app, ids, host) = app_with(1);
    let id = ids[0];
    // The way the MCP bridge sends every command: nested, with params of its own.
    let answer = agent(&mut app, "engine.execute", json!({"command": "ui.openWindow", "params": {"message": id}}));
    assert_eq!(answer["ok"], true, "{answer}");
    assert!(app.windows.of_message(id).is_some(), "the window did not open: this test shows nothing");
    idle(&mut app, 8);
    stays_closed(&mut app, &host, id);
    assert!(app.decryption_needs_owner(id));
    // The same through `ui.openWindow` itself.
    let (mut app, ids, host) = app_with(1);
    let answer = agent(&mut app, "ui.openWindow", json!({"message": ids[0]}));
    assert_eq!(answer["ok"], true, "{answer}");
    idle(&mut app, 8);
    stays_closed(&mut app, &host, ids[0]);
}

#[test]
fn a_click_or_key_the_control_channel_injected_is_not_the_owners_choice() {
    let (mut app, ids, host) = app_with(2);
    // A click on a row, as `ui.click` injects it: events queued, and the frame takes them in.
    app.synthetic.push(egui::Event::PointerMoved(egui::pos2(10.0, 10.0)));
    app.raw_input_hook(&mut egui::RawInput::default());
    owner_selects(&mut app, ids[0]);
    // The arrow key, as `ui.key` injects it, a frame later.
    app.synthetic.push(egui::Event::PointerMoved(egui::pos2(10.0, 10.0)));
    app.raw_input_hook(&mut egui::RawInput::default());
    owner_selects(&mut app, ids[1]);
    idle(&mut app, 8);
    stays_closed(&mut app, &host, ids[0]);
    assert!(app.decryption_needs_owner(ids[0]) && app.decryption_needs_owner(ids[1]));
    // The owner's own click, after the channel has been quiet, is theirs.
    idle(&mut app, 8);
    owner_selects(&mut app, ids[0]);
    assert!(!app.decryption_needs_owner(ids[0]));
    let ctx = egui::Context::default();
    app.opened_view_at(&ctx, ids[0], 9_000.0);
    app.opened_view_at(&ctx, ids[0], 9_000.0 + DWELL_MS + 1.0);
    assert_eq!(host.opened.load(Ordering::SeqCst), 1);
}

#[test]
fn a_message_the_agent_picked_first_is_opened_by_the_owners_own_click() {
    let (mut app, ids, host) = app_with(1);
    let ctx = egui::Context::default();
    agent(&mut app, "engine.execute", json!({"command": "mail.select", "params": {"id": ids[0]}}));
    idle(&mut app, 8);
    // The owner clicks Open (the button's click: not driven).
    app.owner_opens(ids[0]);
    app.opened_view_at(&ctx, ids[0], 2_000.0);
    app.opened_view_at(&ctx, ids[0], 2_000.0 + DWELL_MS + 1.0);
    assert_eq!(host.opened.load(Ordering::SeqCst), 1);
}

#[test]
fn a_screenshot_asked_for_before_a_message_opened_is_refused_when_it_would_be_taken() {
    let (mut app, ids, host) = app_with(1);
    let ctx = egui::Context::default();
    let id = ids[0];
    // The agent asks for a screenshot while nothing is open: it is queued, to be taken in a few frames.
    let (tx, rx_requests) = channel();
    app.control_rx = Some(rx_requests);
    let (req, reply) = ControlRequest::new("ui.screenshot", json!({}));
    tx.send(req.from_agent(Vec::new())).unwrap();
    app.drain_control(&ctx);
    assert_eq!(app.pending_shots.len(), 1, "the screenshot was not queued");
    // Before it is taken the owner selects the message and it opens, in the pane.
    idle(&mut app, 8);
    owner_selects(&mut app, id);
    app.opened_view_at(&ctx, id, 1_000.0);
    app.opened_view_at(&ctx, id, 1_000.0 + DWELL_MS + 1.0);
    answer_all(&mut app, &host, true);
    assert!(app.opened_view_at(&ctx, id, 2_000.0).is_some());
    // The frames go by: the picture is not taken, and the agent is told why.
    for q in &mut app.queued_shots {
        q.1 = 0.0;
    }
    for _ in 0..4 {
        app.issue_screenshots(&ctx);
    }
    let answer = reply.try_recv().expect("the agent was not answered");
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(answer["error"].as_str().unwrap().contains("Encrypted message"), "{answer}");
    assert!(app.pending_shots.is_empty());
    // A frame that was taken before the refusal is not handed over either.
    let (tx, reply) = channel();
    app.pending_shots.push((99, None, true, tx, f64::MAX));
    app.agent_shots.push((99, 0));
    app.shot_images.lock().unwrap().push((99, Arc::new(egui::ColorImage::example())));
    app.collect_screenshots();
    assert_eq!(reply.try_recv().expect("no answer")["ok"], false);
}

#[test]
fn nothing_is_opened_for_a_message_that_only_went_by_and_only_one_at_a_time() {
    let (mut app, ids, host) = app_with(20);
    let ctx = egui::Context::default();
    for id in &ids {
        app.owner_picks(*id);
    }
    // Arrowing down the folder: each is on screen for a moment.
    for (i, id) in ids.iter().enumerate() {
        assert!(app.opened_view_at(&ctx, *id, 1_000.0 + 20.0 * i as f64).is_none());
    }
    assert_eq!(host.opened.load(Ordering::SeqCst), 0);
    // All of them stayed (twenty windows, say): one is begun, the rest wait for it.
    for id in &ids {
        app.opened_view_at(&ctx, *id, 10_000.0);
    }
    for id in &ids {
        app.opened_view_at(&ctx, *id, 10_000.0 + DWELL_MS + 10.0);
    }
    assert_eq!(host.opened.load(Ordering::SeqCst), 1, "more than one decryption at a time");
    // The one that was begun is done: the next is.
    answer_all(&mut app, &host, true);
    for id in &ids {
        app.opened_view_at(&ctx, *id, 10_500.0);
    }
    assert_eq!(host.opened.load(Ordering::SeqCst), 2);
}

#[test]
fn what_an_opened_message_may_fetch_is_nothing_unless_it_can_tell_it_was_not_changed_and_the_owner_asks() {
    let (mut app, ids, host) = app_with(2);
    let ctx = egui::Context::default();
    // Every switch the owner has: all mail.
    app.ui.download_pictures = true;
    for (id, authenticated) in [(ids[0], false), (ids[1], true)] {
        owner_selects(&mut app, id);
        app.opened_view_at(&ctx, id, 1_000.0);
        app.opened_view_at(&ctx, id, 1_000.0 + DWELL_MS + 1.0);
        answer_all(&mut app, &host, authenticated);
    }
    let view = |app: &MailApp, id| app.session.decrypted_message(id).unwrap();
    // AES-CBC or 3DES: never, not even by the per-message Download pictures.
    let cbc = view(&app, ids[0]);
    assert_eq!(access(&app, &cbc), Access::Blocked);
    assert!(app.run("ui.downloadPictures", json!({"message": ids[0]})).is_err());
    assert_eq!(access(&app, &cbc), Access::Blocked);
    // AES-GCM or OpenPGP: not because of the switch for all mail, only the owner's click for it.
    let gcm = view(&app, ids[1]);
    assert_eq!(access(&app, &gcm), Access::Blocked);
    assert!(app.run("ui.downloadPictures", json!({"message": ids[1]})).is_ok());
    assert_eq!(access(&app, &gcm), Access::Allowed);
    // (The message the window keeps its stored copy of is judged by its number: the same.)
    let stored = app.session.mailbox.messages.get(&ids[0]).unwrap().clone();
    assert_eq!(access(&app, &stored), Access::Blocked);
}

#[test]
fn the_layout_kept_of_a_message_goes_when_the_message_does() {
    let (mut app, ids, host) = app_with(1);
    let ctx = egui::Context::default();
    app.owner_picks(ids[0]);
    app.opened_view_at(&ctx, ids[0], 1_000.0);
    app.opened_view_at(&ctx, ids[0], 1_000.0 + DWELL_MS + 1.0);
    answer_all(&mut app, &host, true);
    let view = app.session.decrypted_message(ids[0]).unwrap();
    // The window laid it out.
    let place = super::Place::Pane;
    let doc = super::view::doc_shown(&ctx, &view, place, false, mailcraft_html::Env::for_pane(600.0, false));
    assert!(!doc.blocks.is_empty());
    app.forget_unopened_layouts(&ctx);
    // The engine lets it go (Try Again, a mailbox opened): the layout is dropped with it.
    app.session.forget_decryption(ids[0]);
    app.forget_unopened_layouts(&ctx);
    let store = super::view::entries_for_tests(&ctx);
    assert_eq!(store, 0, "the text of a message that was let go of is still in the layout cache");
}
